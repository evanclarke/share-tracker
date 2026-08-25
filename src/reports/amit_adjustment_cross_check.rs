//! Does an AMMA statement's *set* of per-parcel AMIT adjustments actually
//! reconcile to the statement?
//!
//! [`crate::entities::amit_adjustment`]'s write-time checks validate each row
//! in isolation — the parcel is a Buy/DRP, on the statement's listing and
//! holding account (or a rollover replacement parcel holding units carried out
//! of it), and the quantity is within the parcel — and (since migration 0022)
//! that no parcel appears twice. Nothing at write time can
//! see the *set*: a missed parcel silently overstates the cost base of every
//! unit it covers, and an unnecessary one over-reduces it. Because CGT event
//! E10 floors the reduced cost base at nil and treats the excess as a capital
//! gain (`docs/ato/amit-cost-base-adjustments.md`), an over-reduction does
//! not merely understate a cost base — it can manufacture a gain that was
//! never made.
//!
//! One check here is not about the set at all: the statement's own
//! `units_held` against the units **actually open** at its year end. Every
//! other comparison reconciles the set to the statement, so a set that
//! matched when it was generated goes on matching forever — a Buy dated
//! before the year end, entered afterwards to correct a missed parcel, adds
//! units the stored set never saw while both of those terms stand still
//! (SCENARIOS Z-d). The parcels are the third term.
//!
//! This is the set-level check, and like the other cross-checks
//! ([`super::amit_cash_cross_check`], [`super::e4_cross_check`]) it is
//! advisory and non-blocking: an empty report means every statement's
//! adjustments reconcile. One row per flagged statement, carrying every
//! problem found on it.
//!
//! The [`super::tax_report`]'s completeness section reads it filtered to the
//! report's year — an AMIT adjustment gap distorts the disposal schedule's
//! cost base, which is that report's central figure.

use crate::domain::open_parcels;
use crate::domain::rollover;
use crate::domain::tax_year::tax_year_for;
use crate::entities::corporate_action::{self, SplitEvent};
use crate::infra::decimal::row_dec;
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// One AMMA statement whose per-parcel adjustment set does not reconcile,
/// with every problem found on it. A statement that reconciles is not
/// returned at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmitAdjustmentAlert {
    pub amma_statement_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    /// The financial year the statement attributes, identified by the
    /// calendar year of its 30 June end (`domain::tax_year`).
    pub tax_year: i32,
    pub holding_account_id: i64,
    /// The statement's own `units_held`, verbatim.
    pub units_held: Decimal,
    /// Σ of the adjustment quantities, **re-based into the statement year's
    /// unit basis** so it is comparable with `units_held`: an adjustment's
    /// quantity is in its parcel's as-acquired units, which a split between
    /// acquisition and the year end would otherwise make a false mismatch.
    pub units_adjusted: Decimal,
    /// The units of the statement's listing **actually open on its holding
    /// account** at `tax_year_end_date`, from the shared open-parcels loader
    /// ([`crate::domain::open_parcels::load`]) — the same read
    /// [generation](crate::entities::amit_adjustment_generation) derives its
    /// set from — in that date's unit basis, so it is comparable with
    /// `units_held` and `units_adjusted` term for term. This is what a stored
    /// set has no way to notice moving under it.
    pub units_open_at_year_end: Decimal,
    /// How many adjustment rows the statement has.
    pub parcel_count: i64,
    /// Every problem found, each a self-contained sentence naming what to fix.
    pub problems: Vec<String>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/amit_adjustment_cross_check", get(report))
}

/// One AMMA statement's adjustment rows, joined to the adjusted parcel.
struct AdjustmentRow {
    trade_id: i64,
    quantity: Decimal,
    trade_date: NaiveDate,
    trade_quantity: Decimal,
    /// True when the parcel is a **rollover replacement** (a transfer,
    /// scrip-for-scrip exchange or demerger created it). Such a parcel is dated
    /// the operation, which may be *after* the statement's year end while still
    /// holding units the statement covered — the reach-through the write path
    /// accepts (SCENARIOS N-06) — so the acquired-after-the-year-end check must
    /// not fire on it.
    rollover_replacement: bool,
    /// The parcels a rollover carried this one's units out of, through as many
    /// hops as it takes (`domain::rollover::source_ancestors` — the same walk
    /// the write path traces the row's acceptability by). Empty unless
    /// `rollover_replacement`.
    ///
    /// They are what the coverage band's disposal allowance is measured on for
    /// such a row: the units it covers left the statement's own account through
    /// the operation's closing Sell, which is recorded against the *source*
    /// parcel, so a replacement looked at on its own shows no disposal at all
    /// (SCENARIOS Z-g).
    rollover_sources: Vec<i64>,
}

/// The statement's own figures the checks compare against.
struct StatementFacts {
    year_end: NaiveDate,
    units_held: Decimal,
    units_adjusted: Decimal,
    units_open_at_year_end: Decimal,
    cost_base_adjustment: Decimal,
}

/// Flag every AMMA statement whose per-parcel adjustment set does not
/// reconcile to it. An empty report means every statement's set reconciles.
pub async fn db_amit_adjustment_alerts(
    pool: &SqlitePool,
) -> Result<Vec<AmitAdjustmentAlert>, sqlx::Error> {
    // Every input on one read transaction: a single consistent snapshot, so
    // an interleaved write can't pair a statement with a half-entered
    // adjustment set (which is exactly what this report would then flag).
    let mut tx = pool.begin().await?;
    let statement_rows = sqlx::query(
        "SELECT a.id, a.listing_id, l.ticker, a.tax_year_end_date, a.units_held, \
                a.cost_base_adjustment, a.holding_account_id \
         FROM amma_statements a JOIN listings l ON l.id = a.listing_id \
         ORDER BY l.ticker, a.tax_year_end_date, a.id",
    )
    .fetch_all(&mut *tx)
    .await?;
    let adjustment_rows = sqlx::query(
        "SELECT aa.amma_statement_id, aa.trade_id, aa.quantity, \
                t.date AS trade_date, t.quantity AS trade_quantity, \
                COALESCE(t.transfer_id, t.scrip_action_id, t.demerger_action_id) \
                    IS NOT NULL AS rollover_replacement \
         FROM amit_adjustments aa JOIN trades t ON t.id = aa.trade_id \
         ORDER BY aa.amma_statement_id, t.date, aa.id",
    )
    .fetch_all(&mut *tx)
    .await?;
    // The rollover chain behind each adjusted replacement parcel, on the same
    // read transaction as everything else.
    let mut rollover_sources: HashMap<i64, Vec<i64>> = HashMap::new();
    for row in &adjustment_rows {
        let trade_id: i64 = row.try_get("trade_id")?;
        if row.try_get("rollover_replacement")?
            && let std::collections::hash_map::Entry::Vacant(slot) =
                rollover_sources.entry(trade_id)
        {
            slot.insert(rollover::source_ancestors(&mut tx, trade_id).await?);
        }
    }
    let splits = corporate_action::db_share_split_events(&mut *tx).await?;
    // Every sale allocation with its sale date — the "was this parcel already
    // gone before the year began?" input. Every consumption, deliberately: the
    // coverage check's allowance band counts the units that *left the
    // statement's account* during its year, and a rollover's closing Sell is
    // exactly such a departure (SCENARIOS Z-g — a fund taken over mid-year
    // must not flag its honest rows as excess coverage forever).
    let sold =
        open_parcels::db_units_sold(&mut tx, None, open_parcels::Counted::AllConsumptions).await?;
    // What was *actually* held at each statement's year end, keyed by
    // (year end, listing, holding account). One `load` per distinct year end
    // — the same shared read [generation](crate::entities::amit_adjustment_generation)
    // derives its set from, so this report cannot disagree with it about what
    // was open — still on the report's own read transaction.
    let mut open_at: HashMap<(NaiveDate, i64, i64), Decimal> = HashMap::new();
    let mut year_ends: Vec<NaiveDate> = statement_rows
        .iter()
        .map(|r| r.try_get("tax_year_end_date"))
        .collect::<Result<_, _>>()?;
    year_ends.sort_unstable();
    year_ends.dedup();
    for year_end in year_ends {
        for parcel in open_parcels::load(&mut tx, Some(year_end)).await? {
            *open_at
                .entry((
                    year_end,
                    parcel.parcel.listing_id,
                    parcel.parcel.holding_account_id,
                ))
                .or_default() += parcel.remaining_as_of;
        }
    }
    tx.commit().await?;

    let mut by_statement: HashMap<i64, Vec<AdjustmentRow>> = HashMap::new();
    for row in &adjustment_rows {
        by_statement
            .entry(row.try_get("amma_statement_id")?)
            .or_default()
            .push(AdjustmentRow {
                trade_id: row.try_get("trade_id")?,
                quantity: row_dec(row, "quantity")?,
                trade_date: row.try_get("trade_date")?,
                trade_quantity: row_dec(row, "trade_quantity")?,
                rollover_replacement: row.try_get("rollover_replacement")?,
                rollover_sources: rollover_sources
                    .get(&row.try_get::<i64, _>("trade_id")?)
                    .cloned()
                    .unwrap_or_default(),
            });
    }

    let mut alerts = Vec::new();
    for row in &statement_rows {
        let amma_statement_id: i64 = row.try_get("id")?;
        let listing_id: i64 = row.try_get("listing_id")?;
        let year_end: NaiveDate = row.try_get("tax_year_end_date")?;
        let units_held = row_dec(row, "units_held")?;
        let cost_base_adjustment = row_dec(row, "cost_base_adjustment")?;
        let adjustments = by_statement.get(&amma_statement_id).map_or(&[][..], |v| v);
        let listing_splits = splits.get(&listing_id).map_or(&[][..], |v| v);

        let holding_account_id: i64 = row.try_get("holding_account_id")?;
        let units_open_at_year_end = open_at
            .get(&(year_end, listing_id, holding_account_id))
            .copied()
            .unwrap_or_default();

        let units_adjusted = adjustments
            .iter()
            .map(|a| {
                corporate_action::split_adjusted_quantity(
                    a.quantity,
                    listing_splits,
                    a.trade_date,
                    Some(year_end),
                )
            })
            .sum::<Decimal>();
        let problems = problems_for(
            adjustments,
            listing_splits,
            &sold,
            &StatementFacts {
                year_end,
                units_held,
                units_adjusted,
                units_open_at_year_end,
                cost_base_adjustment,
            },
        );
        if problems.is_empty() {
            continue;
        }
        alerts.push(AmitAdjustmentAlert {
            amma_statement_id,
            listing_id,
            ticker: row.try_get("ticker")?,
            tax_year: tax_year_for(year_end),
            holding_account_id,
            units_held,
            units_adjusted,
            units_open_at_year_end,
            parcel_count: adjustments.len() as i64,
            problems,
        });
    }
    Ok(alerts)
}

/// Units of the adjusted parcels — **and of the parcels a rollover carried
/// their units out of** — disposed of between `from` and `to` inclusive, in
/// `to`'s unit basis, the same basis `units_adjusted` is re-based into, so the
/// two are comparable.
///
/// The sources count because a row against a rollover **replacement** parcel
/// covers units that were held during the statement's year and then left the
/// statement's account through the operation's own closing Sell — recorded
/// against the source parcel, never against the replacement. Measured on the
/// replacement alone, a fund taken over mid-year (whose final statement states
/// nil units held) showed every honest row as excess coverage, flagging the one
/// entry the write path accepts for those units, forever (SCENARIOS Z-g).
///
/// Both bounds are inclusive, matching
/// [`crate::domain::cost_base::AmitReductionEvent`]'s own boundary: a sale on
/// the year end itself counts as disposed by it, since the statement's
/// year-end position no longer includes those units.
fn disposed_between(
    adjustments: &[AdjustmentRow],
    splits: &[SplitEvent],
    sold: &HashMap<i64, Vec<(NaiveDate, Decimal)>>,
    from: NaiveDate,
    to: NaiveDate,
) -> Decimal {
    // By parcel, not by row: a parcel adjusted twice (pre-0022 data, flagged
    // separately below) must not have its disposals counted twice into the
    // allowance, which would mask the very over-coverage it is.
    let parcels: std::collections::BTreeSet<i64> = adjustments
        .iter()
        .flat_map(|a| std::iter::once(a.trade_id).chain(a.rollover_sources.iter().copied()))
        .collect();
    parcels
        .iter()
        .flat_map(|trade_id| sold.get(trade_id).map_or(&[][..], |v| v))
        .filter(|(sale_date, _)| *sale_date >= from && *sale_date <= to)
        .map(|&(sale_date, qty)| {
            corporate_action::split_adjusted_quantity(qty, splits, sale_date, Some(to))
        })
        .sum()
}

/// Every problem on one statement's adjustment set, in severity order.
fn problems_for(
    adjustments: &[AdjustmentRow],
    splits: &[SplitEvent],
    sold: &HashMap<i64, Vec<(NaiveDate, Decimal)>>,
    facts: &StatementFacts,
) -> Vec<String> {
    let &StatementFacts {
        year_end,
        units_held,
        units_adjusted,
        cost_base_adjustment,
        ..
    } = facts;
    let mut problems = Vec::new();

    // Highest signal: the whole statement's cost-base effect is missing. A
    // statement whose per-unit figure is nil adjusts nothing, so having no
    // rows is correct for it and is not flagged.
    if adjustments.is_empty() {
        if !cost_base_adjustment.is_zero() {
            problems.push(format!(
                "no AMIT adjustments entered, so the statement's {cost_base_adjustment} per-unit \
                 cost base adjustment reaches no parcel — generate them from the statement, or \
                 enter one row per parcel held at year end"
            ));
        }
        problems.extend(units_held_problem(facts, false));
        return problems;
    }

    // Coverage: Σ of the adjusted quantities against the units the statement
    // says were held, both in the statement year's unit basis — with the
    // units disposed of *during* the statement's year allowed on top of the
    // units held at its end.
    //
    // A row may legitimately cover units sold during the year: s 104-107B
    // makes the adjustment just before the end of the income year **or just
    // before the time of a relevant CGT event** (LCR 2015/11 para 13), which
    // is why `AmitReductionEvent::reduction_for_units` spills a whole-parcel
    // row onto them. For a holding sold out or transferred away mid-year that
    // is the *only* kind of unit there is — the statement then states nil
    // units held, and every honest row would read as excess coverage
    // (SCENARIOS F-04, F-17, F-25). So the acceptable band is
    // `units_held ..= units_held + disposed during the year`: below it a
    // parcel is missing, above it one is duplicated or covered for too much.
    let year_start = NaiveDate::from_ymd_opt(year_end.year() - 1, 7, 1).expect("valid FY start");
    let disposed_in_year = disposed_between(adjustments, splits, sold, year_start, year_end);
    if units_adjusted < units_held {
        let difference = units_adjusted - units_held;
        problems.push(format!(
            "adjusted units {units_adjusted} do not match the statement's units held \
             {units_held} (difference {difference:+}) — a parcel is missing, duplicated, or \
             covered for the wrong quantity"
        ));
    } else if units_adjusted > units_held + disposed_in_year {
        let excess = units_adjusted - units_held - disposed_in_year;
        problems.push(format!(
            "adjusted units {units_adjusted} exceed the statement's units held {units_held} \
             plus the {disposed_in_year} unit(s) disposed of during the year (excess \
             {excess}) — a parcel is duplicated or covered for more units than it held"
        ));
    }

    // Duplicate parcels. Rejected at write time since migration 0022, so this
    // only ever fires on rows entered before it — kept because the report's
    // job is to describe the data as it is, not as the current writer would
    // allow it.
    let mut seen: HashMap<i64, i64> = HashMap::new();
    for a in adjustments {
        *seen.entry(a.trade_id).or_insert(0) += 1;
    }
    let mut duplicates: Vec<(i64, i64)> = seen.into_iter().filter(|&(_, n)| n > 1).collect();
    duplicates.sort_unstable();
    for (trade_id, count) in duplicates {
        problems.push(format!(
            "parcel (trade #{trade_id}) is adjusted {count} times on this statement, so its \
             cost base is reduced {count} times over"
        ));
    }

    // Parcels that cannot have been held in the statement's year. Only the
    // two unambiguous cases: acquired after the year ended, or already fully
    // sold before the year began. A parcel disposed of *during* the year was
    // genuinely held for part of it and is not flagged.
    for a in adjustments {
        // A rollover replacement parcel is exempt from the date test: it is
        // dated its operation, which routinely postdates the year end (an AMMA
        // statement arrives in spring, and a transfer in between is the ordinary
        // case), while the units it holds are the ones the statement covered.
        // The write path only accepts such a row when the units trace back to
        // the statement's own account, so the "cannot have been held" reasoning
        // does not apply — flagging it would have made the supported entry look
        // like an error forever (SCENARIOS N-06).
        if a.trade_date > year_end && !a.rollover_replacement {
            problems.push(format!(
                "parcel (trade #{}) was acquired {} — after the statement's year ended {} — so \
                 the statement cannot cover it",
                a.trade_id, a.trade_date, year_end
            ));
            continue;
        }
        if a.rollover_replacement && a.trade_date > year_end {
            continue;
        }
        let sold_before_year = corporate_action::sold_in_acquired_units(
            &sold
                .get(&a.trade_id)
                .map_or(&[][..], |v| v)
                .iter()
                .copied()
                .filter(|&(sale_date, _)| sale_date < year_start)
                .collect::<Vec<_>>(),
            splits,
            a.trade_date,
        );
        if sold_before_year >= a.trade_quantity {
            problems.push(format!(
                "parcel (trade #{}) was fully sold before {} — the start of the statement's \
                 year — so nothing of it was held during the year",
                a.trade_id, year_start
            ));
        }
    }

    problems.extend(units_held_problem(facts, true));
    problems
}

/// The statement's own `units_held` against the units **actually open** at its
/// `tax_year_end_date`, on its own listing and holding account.
///
/// The other checks all reconcile the adjustment *set* to the *statement*, so
/// a set that matched its statement when it was generated goes on matching it
/// forever — including after a Buy dated *before* the year end is entered to
/// correct a missed parcel, which adds a parcel the stored set never saw
/// (SCENARIOS Z-d). Nothing in that comparison can notice, because both of its
/// terms stood still; only the parcels moved. This is the third term.
///
/// Both figures are in the year end's unit basis — `units_held` is what the
/// registry stated for that date, and `open_parcels::load(.., Some(year_end))`
/// re-bases each parcel's remainder into it — so a split between an
/// acquisition and the year end cannot make a false mismatch, exactly as in
/// the coverage check.
///
/// There is no allowance band here, unlike the coverage check: both terms are
/// *year-end* positions, so units disposed of during the year are already out
/// of both. A holding sold out (or transferred, exchanged, demerged away)
/// during the year has nil open at the year end and a statement stating nil
/// units held, and reconciles.
fn units_held_problem(facts: &StatementFacts, has_adjustments: bool) -> Option<String> {
    let &StatementFacts {
        year_end,
        units_held,
        units_adjusted,
        units_open_at_year_end,
        ..
    } = facts;
    if units_open_at_year_end == units_held {
        return None;
    }
    let difference = units_open_at_year_end - units_held;
    // Which of the two figures moved. When the set still sums to the
    // statement's own figure, the statement and its set agree with each other
    // and it is the parcels that changed underneath them — the stale-set case,
    // whose fix is to regenerate. When the set already sums to what is open,
    // the set has been rebuilt and it is the statement's stated figure left
    // behind (or the statement is against the wrong holding account).
    let cause = if !has_adjustments {
        // With no rows at all there is no set to have gone stale, and the
        // zero sum agrees with nothing on purpose — say only what is known.
        "check the statement's units held and holding account against the parcels entered"
    } else if units_adjusted == units_held {
        "the adjustment set still sums to the statement's figure, so it is the parcels that \
         changed after the set was generated — re-generate the set from the statement (replace) \
         once they are right"
    } else if units_adjusted == units_open_at_year_end {
        "the adjustment set already covers the units that are open, so it is the statement's \
         stated figure that disagrees — check it against the registry's holding statement, and \
         that it is the right holding account"
    } else {
        "check the statement's units held and holding account against the parcels entered, then \
         re-generate its adjustment set"
    };
    Some(format!(
        "the statement states {units_held} unit(s) held at {year_end} but \
         {units_open_at_year_end} unit(s) are open on its holding account at that date \
         (difference {difference:+}) — {cause}"
    ))
}

async fn report(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<AmitAdjustmentAlert>>, ApiError> {
    db_amit_adjustment_alerts(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action::{ActionKind, CorporateAction};
    use crate::entities::holding_account::{self, HoldingAccount};
    use crate::entities::sell::AllocationInput;
    use crate::entities::transfer::{self, TransferBody};
    use crate::test_support::{self, ApiClient, allocate, dec, test_pool, ymd};
    use axum::http::StatusCode;

    /// The units-held comparison's own sentence (SCENARIOS Z-d) for one
    /// statement, if the report made one. Used by the legitimate shapes that
    /// carry an unrelated problem of their own: what they pin is that *this*
    /// comparison stayed quiet.
    fn units_held_problem_for(alerts: &[AmitAdjustmentAlert], statement_id: i64) -> Option<String> {
        alerts
            .iter()
            .find(|a| a.amma_statement_id == statement_id)
            .and_then(|a| {
                a.problems
                    .iter()
                    .find(|p| p.contains("unit(s) are open on its holding account"))
                    .cloned()
            })
    }

    /// A second holding account, for the transfer and wrong-account cases.
    async fn second_account(pool: &SqlitePool) {
        holding_account::db_upsert(
            pool,
            &HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
    }

    /// Move `qty` units of parcel `purchase_trade_id` from account 1 to
    /// account 2 on `date`, as transfer #1.
    async fn transfer_all(
        pool: &SqlitePool,
        purchase_trade_id: i64,
        date: NaiveDate,
        qty: Decimal,
    ) {
        transfer::db_transfer(
            pool,
            1,
            &TransferBody {
                listing_id: 1,
                date,
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id,
                    quantity_allocated: qty,
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();
    }

    async fn amit_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .amit(true)
            .insert(pool)
            .await;
    }

    /// An AMMA statement for `year_end` stating `units` held, with a non-zero
    /// per-unit cost base adjustment (the case the report is about).
    async fn amma(pool: &SqlitePool, id: i64, listing_id: i64, year_end: NaiveDate, units: &str) {
        test_support::amma(id, listing_id)
            .units(dec(units))
            .cost_base_adjustment(dec("0.05"))
            .with(|a| {
                a.tax_year_end_date = year_end;
                a.date_received = year_end + chrono::Duration::days(60);
            })
            .insert(pool)
            .await;
    }

    async fn split(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate, ratio: &str) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id,
                date,
                kind: ActionKind::ShareSplit {
                    split_new_units: dec(ratio),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
    }

    /// A statement whose parcels are all covered for exactly the units held
    /// reconciles: nothing is reported.
    #[tokio::test]
    async fn db_a_reconciling_set_is_not_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 2, 28))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(ymd(2024, 3, 1))
            .qty(dec("1302"))
            .insert(&pool)
            .await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "1811").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("509")).await;
        test_support::amit_adjustment(&pool, 2, 1, 2, dec("1302")).await;

        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());
    }

    /// The highest-signal case: a statement with a real per-unit figure and
    /// no adjustments at all.
    #[tokio::test]
    async fn db_a_statement_with_no_adjustments_is_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 2, 28))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "509").await;

        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.amma_statement_id, 1);
        assert_eq!(a.ticker, "HNDQ");
        assert_eq!(a.tax_year, 2024);
        assert_eq!(a.units_held, dec("509"));
        assert_eq!(a.units_adjusted, Decimal::ZERO);
        assert_eq!(a.parcel_count, 0);
        assert_eq!(a.problems.len(), 1);
        assert!(
            a.problems[0].contains("no AMIT adjustments entered"),
            "{:?}",
            a.problems
        );
    }

    /// A statement whose per-unit adjustment is nil adjusts nothing, so
    /// having no rows is correct for it.
    #[tokio::test]
    async fn db_a_zero_per_unit_statement_with_no_adjustments_is_not_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 2, 28))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        test_support::amma(1, 1)
            .units(dec("509"))
            .cost_base_adjustment(Decimal::ZERO)
            .with(|a| a.tax_year_end_date = ymd(2024, 6, 30))
            .insert(&pool)
            .await;

        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());
    }

    /// A missed parcel shows up as a coverage shortfall, with the signed
    /// difference.
    #[tokio::test]
    async fn db_coverage_shortfall_is_flagged_with_the_signed_difference() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 2, 28))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(ymd(2024, 3, 1))
            .qty(dec("1302"))
            .insert(&pool)
            .await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "1811").await;
        // Only the first parcel entered.
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("509")).await;

        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].units_adjusted, dec("509"));
        assert_eq!(alerts[0].parcel_count, 1);
        assert!(
            alerts[0].problems[0].contains("-1302"),
            "{:?}",
            alerts[0].problems
        );
    }

    /// A split between acquisition and the statement's year end must not
    /// false-positive the coverage check: the adjustment quantities are
    /// as-acquired units, `units_held` is the year's basis.
    #[tokio::test]
    async fn db_a_split_does_not_false_positive_the_coverage_check() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("500"))
            .insert(&pool)
            .await;
        split(&pool, 1, 1, ymd(2024, 1, 15), "2").await;
        // 500 as-acquired units are 1000 units at 30 June 2024.
        amma(&pool, 1, 1, ymd(2024, 6, 30), "1000").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("500")).await;

        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());

        // And the naive comparison it replaces would have flagged it: with
        // the statement stating the as-acquired 500 instead, the re-based sum
        // (1000) is the mismatch.
        sqlx::query("UPDATE amma_statements SET units_held = '500' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].units_adjusted, dec("1000"));
    }

    /// A parcel adjusted twice on one statement (rows predating migration
    /// 0022's UNIQUE index, so inserted below it here).
    #[tokio::test]
    async fn db_a_duplicated_parcel_is_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 2, 28))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "509").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("509")).await;
        // The write path and the UNIQUE index both refuse this now, so the
        // duplicate is planted directly — the report still has to describe
        // data entered before migration 0022.
        sqlx::query("DROP INDEX amit_adjustments_statement_trade")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO amit_adjustments (id, amma_statement_id, trade_id, quantity) \
             VALUES (2, 1, 1, '509')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].parcel_count, 2);
        assert!(
            alerts[0]
                .problems
                .iter()
                .any(|p| p.contains("adjusted 2 times")),
            "{:?}",
            alerts[0].problems
        );
    }

    /// A parcel acquired after the statement's year ended cannot be covered
    /// by it.
    #[tokio::test]
    async fn db_a_parcel_acquired_after_the_year_end_is_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 8, 1))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "509").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("509")).await;

        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert!(
            alerts[0]
                .problems
                .iter()
                .any(|p| p.contains("after the statement's year ended")),
            "{:?}",
            alerts[0].problems
        );
    }

    /// A parcel fully sold before the year began was never held during it.
    #[tokio::test]
    async fn db_a_parcel_sold_out_before_the_year_began_is_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2022, 2, 1))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2023, 2, 1)) // before 1 July 2023
            .qty(dec("509"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, dec("509")).await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "509").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("509")).await;

        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert!(
            alerts[0]
                .problems
                .iter()
                .any(|p| p.contains("fully sold before")),
            "{:?}",
            alerts[0].problems
        );
    }

    /// A parcel disposed of *during* the year was genuinely held for part of
    /// it, so the statement legitimately covers it — never flagged. The
    /// statement states **nil** units held, which is what a registry states at
    /// 30 June for a holding that closed in February; the units-held check
    /// (SCENARIOS Z-d) compares that against the nil open at the year end and
    /// agrees, while the coverage band still allows the row over the units
    /// disposed of during the year.
    #[tokio::test]
    async fn db_a_mid_year_disposal_is_not_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2022, 2, 1))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2024, 2, 1)) // inside FY2024
            .qty(dec("509"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, dec("509")).await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "0").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("509")).await;

        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());
    }

    // ---------------------------------------------------------------------
    // SCENARIOS Z-d: the statement's own `units_held` against the units
    // actually open at its year end. The first two must flag; everything
    // after them is a legitimate shape that must stay unflagged.
    // ---------------------------------------------------------------------

    /// The finding's own reproduction, end to end through the API: a year is
    /// entered, its statement's adjustment set is generated and reconciles —
    /// and then a missed Buy dated *before* the year end is discovered and
    /// entered. Σ still equals `units_held` (neither moved), so every
    /// set-versus-statement comparison goes on agreeing; only the parcels
    /// changed, and the report has to say so.
    #[tokio::test]
    async fn api_a_back_dated_parcel_entered_after_generation_is_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "VDHG").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("1000"))
            .insert(&pool)
            .await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "1000").await;

        let client = ApiClient::full(&pool);
        client
            .post(
                "/amma_statements/1/generate_adjustments",
                &serde_json::json!({}),
            )
            .await
            .expect_status(StatusCode::CREATED);
        let clean: Vec<AmitAdjustmentAlert> = client
            .get_json("/reports/amit_adjustment_cross_check")
            .await;
        assert_eq!(clean, vec![]);

        // The missed parcel, dated inside the statement's year.
        test_support::buy(2, 1)
            .date(ymd(2024, 3, 1))
            .qty(dec("300"))
            .insert(&pool)
            .await;

        let alerts: Vec<AmitAdjustmentAlert> = client
            .get_json("/reports/amit_adjustment_cross_check")
            .await;
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        // The two set-versus-statement terms are unmoved and still agree; the
        // parcels are the term that changed.
        assert_eq!(a.units_held, dec("1000"));
        assert_eq!(a.units_adjusted, dec("1000"));
        assert_eq!(a.units_open_at_year_end, dec("1300"));
        assert_eq!(a.parcel_count, 1);
        assert_eq!(a.problems.len(), 1);
        let problem = &a.problems[0];
        assert!(
            problem.contains("states 1000 unit(s) held at 2024-06-30"),
            "{problem}"
        );
        assert!(
            problem.contains("1300 unit(s) are open on its holding account"),
            "{problem}"
        );
        assert!(problem.contains("difference +300"), "{problem}");
        assert!(
            problem.contains("it is the parcels that changed after the set was generated"),
            "{problem}"
        );

        // Regenerating the set with `replace` — the documented repair — moves
        // the set onto the parcels, and the report then says the *other*
        // figure is the one left behind: the statement's own units held, which
        // the registry would have stated as 1300 if 1300 were really held.
        client
            .post(
                "/amma_statements/1/generate_adjustments",
                &serde_json::json!({ "replace": true }),
            )
            .await
            .expect_status(StatusCode::CREATED);
        let regenerated: Vec<AmitAdjustmentAlert> = client
            .get_json("/reports/amit_adjustment_cross_check")
            .await;
        assert_eq!(regenerated.len(), 1);
        assert_eq!(regenerated[0].units_adjusted, dec("1300"));
        let problem = units_held_problem_for(&regenerated, 1).expect("still flagged");
        assert!(
            problem.contains("it is the statement's stated figure that disagrees"),
            "{problem}"
        );

        // Correcting `units_held` to what was really held closes it out.
        sqlx::query("UPDATE amma_statements SET units_held = '1300' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let cleared: Vec<AmitAdjustmentAlert> = client
            .get_json("/reports/amit_adjustment_cross_check")
            .await;
        assert_eq!(cleared, vec![]);
    }

    /// The other thing the third term surfaces: a statement typed against the
    /// wrong holding account. The parcels are all in account 1, so account 2
    /// held nothing at the year end — and no adjustment row can even be
    /// entered, since a row may only touch its statement's own account.
    #[tokio::test]
    async fn db_a_statement_against_an_account_that_held_nothing_is_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("1000"))
            .insert(&pool)
            .await;
        test_support::amma(1, 1)
            .units(dec("1000"))
            .cost_base_adjustment(dec("0.05"))
            .with(|a| {
                a.tax_year_end_date = ymd(2024, 6, 30);
                a.date_received = ymd(2024, 8, 29);
                a.holding_account_id = 2;
            })
            .insert(&pool)
            .await;

        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].units_open_at_year_end, Decimal::ZERO);
        // The "no adjustments" problem, and then the units-held one.
        assert!(
            alerts[0].problems[0].contains("no AMIT adjustments entered"),
            "{:?}",
            alerts[0].problems
        );
        let problem = &alerts[0].problems[1];
        assert!(
            problem.contains("states 1000 unit(s) held at 2024-06-30"),
            "{problem}"
        );
        assert!(problem.contains("0 unit(s) are open"), "{problem}");
        assert!(problem.contains("difference -1000"), "{problem}");
        assert!(
            problem.contains("holding account against the parcels"),
            "{problem}"
        );
    }

    /// A **partial sale during the year**: the units open at the year end are
    /// legitimately fewer than the units the statement's per-unit figure
    /// covered, and the statement states the year-end figure. Both terms of
    /// the units-held comparison are year-end positions, so the sold units
    /// are already out of both and there is nothing to allow for.
    #[tokio::test]
    async fn db_a_partial_sale_during_the_year_is_not_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2022, 2, 1))
            .qty(dec("1000"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2024, 2, 1))
            .qty(dec("400"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, dec("400")).await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "600").await;
        // The whole parcel is covered — the fund attributed to the units sold
        // in February too (s 104-107B / LCR 2015/11 para 13).
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("1000")).await;

        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());
    }

    /// A **share split** between acquisition and the year end: `units_held` is
    /// in the statement year's basis and the parcel was transacted in another,
    /// so the comparison must be split-aware in both terms — the open-parcels
    /// loader re-bases the remainder into the year end's basis.
    #[tokio::test]
    async fn db_a_split_does_not_false_positive_the_units_held_check() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("500"))
            .insert(&pool)
            .await;
        split(&pool, 1, 1, ymd(2024, 1, 15), "2").await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "1000").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("500")).await;

        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());

        // The naive comparison this replaces: 500 as-acquired units against a
        // statement stating the year's 1000 would have read as 500 missing.
        sqlx::query("UPDATE amma_statements SET units_held = '1100' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        // Both figures are in the year end's basis, not the parcel's.
        assert_eq!(alerts[0].units_open_at_year_end, dec("1000"));
        assert!(
            alerts[0]
                .problems
                .iter()
                .any(|p| p.contains("1000 unit(s) are open on its holding account")),
            "{:?}",
            alerts[0].problems
        );
    }

    /// A **bonus issue** re-bases units the same way a split does, and must be
    /// just as invisible to the comparison.
    #[tokio::test]
    async fn db_a_bonus_issue_does_not_false_positive_the_units_held_check() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("1000"))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 1, 15),
                kind: ActionKind::BonusIssue {
                    bonus_units: Decimal::ONE,
                    bonus_held_units: dec("10"),
                },
            },
        )
        .await
        .unwrap();
        // 1 for every 10 held: 1000 as transacted are 1100 at the year end.
        amma(&pool, 1, 1, ymd(2024, 6, 30), "1100").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("1000")).await;

        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());
    }

    /// A **transfer after the year end** — the ordinary order of events, since
    /// the statement arrives months after 30 June. At the year end the source
    /// parcel was still open on the statement's account (the closing Sell is
    /// dated the transfer), so the statement reconciles; the adjustment row
    /// itself is written against the replacement parcel (SCENARIOS N-06).
    #[tokio::test]
    async fn db_a_transfer_after_the_year_end_is_not_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("1000"))
            .insert(&pool)
            .await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "1000").await;
        second_account(&pool).await;
        transfer_all(&pool, 1, ymd(2024, 8, 15), dec("1000")).await;

        crate::entities::amit_adjustment_generation::db_generate(
            &pool,
            1,
            &crate::entities::amit_adjustment_generation::GenerateBody::default(),
        )
        .await
        .unwrap();

        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());
    }

    /// A **transfer during the year**: the units left the statement's account
    /// before 30 June, so it states nil units held and nil is what is open —
    /// the units-held comparison agrees and stays quiet. The row against the
    /// replacement parcel reconciles too: its units were disposed of during the
    /// year by the transfer's own closing Sell, which the coverage band now
    /// follows the rollover chain to find (SCENARIOS Z-g — before that it was
    /// measured on the replacement alone, which shows no disposal at all, and
    /// the honest row read as excess coverage).
    #[tokio::test]
    async fn db_a_transfer_during_the_year_is_not_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("1000"))
            .insert(&pool)
            .await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "0").await;
        second_account(&pool).await;
        transfer_all(&pool, 1, ymd(2024, 3, 15), dec("1000")).await;

        let replacement: i64 = sqlx::query_scalar(
            "SELECT id FROM trades WHERE transfer_id = 1 AND trade_type = 'Buy'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        test_support::amit_adjustment(&pool, 1, 1, replacement, dec("1000")).await;

        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert!(alerts.is_empty(), "{alerts:?}");
    }

    /// A **scrip-for-scrip exchange during the year** takes the whole holding
    /// of the statement's listing, so nothing of it is open at the year end
    /// and the statement states nil — the same shape as a sold-out holding,
    /// and the units-held comparison agrees. (This statement carries no
    /// adjustment rows at all, so the "no adjustments entered" problem is what
    /// flags it — the entry that clears *that* is a row against the
    /// cross-listing replacement parcel, which the write path accepts and
    /// `db_a_mid_year_takeovers_replacement_row_is_not_flagged` pins. Either
    /// way it is not this comparison's business.)
    #[tokio::test]
    async fn db_a_scrip_exchange_during_the_year_is_not_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::listing(2).ticker("NEWCO").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("1000"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2024, 3, 15),
                kind: ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: Decimal::ONE,
                    scrip_old_units: Decimal::ONE,
                    scrip_cash_per_unit: None,
                    scrip_market_value: None,
                    scrip_cash_currency: None,
                },
            },
        )
        .await
        .unwrap();
        crate::entities::scrip_exchange::db_exchange(&pool, 10)
            .await
            .unwrap();
        amma(&pool, 1, 1, ymd(2024, 6, 30), "0").await;

        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts[0].units_held, Decimal::ZERO);
        assert_eq!(alerts[0].units_open_at_year_end, Decimal::ZERO);
        assert_eq!(units_held_problem_for(&alerts, 1), None);
    }

    /// SCENARIOS Z-g: an AMIT **taken over mid-year**, its final statement's
    /// reduction entered against the cross-listing replacement parcel — the
    /// entry the write path now accepts, and the one the refusals used to point
    /// at each other over. It must not then be flagged forever: the units the
    /// row covers *were* held during the statement's year, and left the
    /// statement's account through the exchange's own closing Sell, so the
    /// coverage band's disposal allowance has to follow the rollover chain back
    /// to the parcel that held them.
    #[tokio::test]
    async fn db_a_mid_year_takeovers_replacement_row_is_not_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::listing(2).ticker("NEWCO").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("1000"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2024, 3, 15),
                kind: ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: Decimal::ONE,
                    scrip_old_units: Decimal::ONE,
                    scrip_cash_per_unit: None,
                    scrip_market_value: None,
                    scrip_cash_currency: None,
                },
            },
        )
        .await
        .unwrap();
        let replacement = crate::entities::scrip_exchange::db_exchange(&pool, 10)
            .await
            .unwrap()
            .replacements[0]
            .id;
        // The final statement: nil units held at 30 June, arriving in spring.
        amma(&pool, 1, 1, ymd(2024, 6, 30), "0").await;
        test_support::amit_adjustment(&pool, 1, 1, replacement, dec("1000")).await;

        assert!(
            db_amit_adjustment_alerts(&pool).await.unwrap().is_empty(),
            "{:?}",
            db_amit_adjustment_alerts(&pool).await.unwrap()
        );
    }

    /// A **demerger during the year** substitutes the head parcel with a
    /// replacement of the *same* listing carrying the same units, so the
    /// statement's units held are open at the year end all along — the
    /// comparison must follow the substitution rather than see a holding that
    /// vanished.
    #[tokio::test]
    async fn db_a_demerger_during_the_year_is_not_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HEAD").await;
        test_support::listing(2).ticker("SPUN").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("1000"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2024, 3, 15),
                kind: ActionKind::Demerger {
                    demerger_listing_id: 2,
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: Decimal::ONE,
                    demerger_cost_base_pct: dec("30"),
                    demerger_close_date: None,
                    demerger_close_price: None,
                    demerger_close_sourced_from: None,
                    demerger_close_reason: None,
                },
            },
        )
        .await
        .unwrap();
        crate::entities::demerger::db_demerge(&pool, 10)
            .await
            .unwrap();
        amma(&pool, 1, 1, ymd(2024, 6, 30), "1000").await;
        let replacement: i64 = sqlx::query_scalar(
            "SELECT id FROM trades WHERE demerger_action_id = 10 AND trade_type = 'Buy' \
             AND listing_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        test_support::amit_adjustment(&pool, 1, 1, replacement, dec("1000")).await;

        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());
    }

    /// SCENARIOS F-04: the holding was sold out during the statement's year,
    /// so the statement states **nil** units held — and the fund still
    /// attributes for the year, adjusting the units it covered just before
    /// the sale (s 104-107B / LCR 2015/11 para 13). The hand-entered rows
    /// covering those disposed units are the correct entry and must
    /// reconcile: the units disposed of during the year are allowed on top of
    /// the units held at its end.
    #[tokio::test]
    async fn db_a_statement_covering_units_sold_during_the_year_reconciles() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2022, 2, 1))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2024, 2, 1)) // inside FY2024
            .qty(dec("509"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, dec("509")).await;
        // Nil units held at 30 June — the figure the registry states for a
        // holding that closed in February.
        amma(&pool, 1, 1, ymd(2024, 6, 30), "0").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("509")).await;

        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());
    }

    /// The allowance is exactly the units disposed of during the year, not a
    /// blank cheque: a row covering more than the parcel held over the year
    /// is still flagged, and the message says what the ceiling was made of.
    #[tokio::test]
    async fn db_coverage_beyond_the_units_disposed_of_in_the_year_is_flagged() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        // A 1000-unit parcel of which 509 were sold during FY2024 — so 491 of
        // it is still open at the year end — and a second parcel of 100 held
        // throughout. The statement states the 591 held at 30 June.
        test_support::buy(1, 1)
            .date(ymd(2022, 2, 1))
            .qty(dec("1000"))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(ymd(2022, 3, 1))
            .qty(dec("100"))
            .insert(&pool)
            .await;
        test_support::sell(3, 1)
            .date(ymd(2024, 2, 1))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 3, 1, dec("509")).await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "591").await;
        // Whole-parcel rows: 1100 covered against 591 held + 509 disposed of
        // during the year — the top of the band exactly, so nothing is
        // flagged.
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("1000")).await;
        test_support::amit_adjustment(&pool, 2, 1, 2, dec("100")).await;
        assert!(db_amit_adjustment_alerts(&pool).await.unwrap().is_empty());

        // Now state 100 fewer units held: the same rows cover 100 more than
        // the year can account for.
        amma(&pool, 1, 1, ymd(2024, 6, 30), "491").await;

        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let problem = &alerts[0].problems[0];
        assert!(
            problem.contains("exceed the statement's units held 491"),
            "{problem}"
        );
        assert!(
            problem.contains("509 unit(s) disposed of during the year"),
            "{problem}"
        );
        assert!(problem.contains("excess 100"), "{problem}");
    }

    /// A disposal *before* the statement's year buys no allowance: those
    /// units were not held during it at all.
    #[tokio::test]
    async fn db_a_disposal_before_the_year_does_not_widen_the_coverage_band() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2022, 2, 1))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(ymd(2022, 3, 1))
            .qty(dec("100"))
            .insert(&pool)
            .await;
        // Sold in FY2023, a year before the statement's.
        test_support::sell(3, 1)
            .date(ymd(2023, 2, 1))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 3, 1, dec("509")).await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "100").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("509")).await;
        test_support::amit_adjustment(&pool, 2, 1, 2, dec("100")).await;

        let alerts = db_amit_adjustment_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        // Both the excess coverage and the parcel that was already gone.
        assert!(
            alerts[0].problems[0].contains("exceed the statement's units held 100"),
            "{:?}",
            alerts[0].problems
        );
        assert!(
            alerts[0].problems[1].contains("was fully sold before"),
            "{:?}",
            alerts[0].problems
        );
    }

    #[tokio::test]
    async fn api_get_amit_adjustment_cross_check() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 2, 28))
            .qty(dec("509"))
            .insert(&pool)
            .await;
        amma(&pool, 1, 1, ymd(2024, 6, 30), "509").await;

        let resp = ApiClient::over(router().with_state(pool))
            .get("/reports/amit_adjustment_cross_check")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let alerts: Vec<AmitAdjustmentAlert> = resp.json();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].ticker, "HNDQ");
    }
}
