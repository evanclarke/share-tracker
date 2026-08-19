//! Does an AMMA statement's *set* of per-parcel AMIT adjustments actually
//! reconcile to the statement?
//!
//! [`crate::entities::amit_adjustment`]'s write-time checks validate each row
//! in isolation — the parcel is a Buy/DRP, on the statement's listing and
//! holding account, and the quantity is within the parcel — and (since
//! migration 0022) that no parcel appears twice. Nothing at write time can
//! see the *set*: a missed parcel silently overstates the cost base of every
//! unit it covers, and an unnecessary one over-reduces it. Because CGT event
//! E10 floors the reduced cost base at nil and treats the excess as a capital
//! gain (`docs/ato/amit-cost-base-adjustments.md`), an over-reduction does
//! not merely understate a cost base — it can manufacture a gain that was
//! never made.
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
}

/// The statement's own figures the checks compare against.
struct StatementFacts {
    year_end: NaiveDate,
    units_held: Decimal,
    units_adjusted: Decimal,
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
    let splits = corporate_action::db_share_split_events(&mut *tx).await?;
    // Every sale allocation with its sale date — the "was this parcel already
    // gone before the year began?" input.
    let sold = open_parcels::db_units_sold(&mut tx, None).await?;
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
            holding_account_id: row.try_get("holding_account_id")?,
            units_held,
            units_adjusted,
            parcel_count: adjustments.len() as i64,
            problems,
        });
    }
    Ok(alerts)
}

/// Units of the adjusted parcels disposed of between `from` and `to`
/// inclusive, in `to`'s unit basis — the same basis `units_adjusted` is
/// re-based into, so the two are comparable.
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
    let parcels: std::collections::BTreeSet<i64> = adjustments.iter().map(|a| a.trade_id).collect();
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

    problems
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
    use crate::test_support::{self, ApiClient, allocate, dec, test_pool, ymd};
    use axum::http::StatusCode;

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
    /// it, so the statement legitimately covers it — never flagged.
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
        amma(&pool, 1, 1, ymd(2024, 6, 30), "509").await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("509")).await;

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
