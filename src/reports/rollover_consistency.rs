//! Do the parcel-substituting operations' **stored** figures still match what
//! today's facts say they should be — and did each whole-holding operation
//! actually consume the whole holding?
//!
//! A transfer, a scrip-for-scrip exchange and a demerger each write their
//! replacement parcels' carried cost base (and quantity) as *stored* values,
//! computed from the source parcels, splits, AMIT adjustments and
//! return-of-capital payments as they stood when the operation ran
//! (`domain::rollover`). Nothing re-derives them afterwards: the source parcels
//! the reports still walk can move, while the frozen replacement figures cannot
//! (SCENARIOS N-06, N-07).
//!
//! `entities::corporate_action` now refuses a `ReturnOfCapital`/`ShareSplit`/
//! `BonusIssue` dated on or before an operation that has already run,
//! `entities::amit_adjustment` refuses an adjustment covering units one carried
//! away, and every parcel-creating write refuses a parcel dated on or before a
//! whole-holding operation (`domain::whole_holding`), so the loudest routes
//! into that state are closed at write time. This report is the answer for the
//! rest — a source parcel's price, brokerage
//! or quantity edited after the move, an AMMA statement's per-unit figure
//! corrected, or any state that predates those guards — and, like the other
//! cross-checks ([`super::amit_adjustment_cross_check`],
//! [`super::e4_cross_check`]), it is advisory and non-blocking: an **empty
//! report means every rollover's stored figures still reconcile**.
//!
//! One row per flagged operation, carrying every problem found. The comparison
//! is per **currency**, because a replacement parcel keeps its source parcel's
//! currency (a foreign-listed security can hold AUD parcels) and amounts in
//! different currencies are never netted.
//!
//! # What is compared, and what is deliberately not
//!
//! - **Cost base** — Σ of what the consumed units' cost base is *now*
//!   (`rollover::CostBaseInputs::carried_cost_base`, the operations' own call)
//!   against Σ of the replacement parcels' stored initial cost base. Exact for a
//!   transfer, for a demerger (its percentage only *splits* the carried total
//!   across the head and demerged parcels), and for a scrip-for-scrip exchange
//!   with no cash component.
//! - **Quantity** — for a **transfer** only, where the units move one for one:
//!   Σ of the closing Sell's allocations against Σ of the transfer-in
//!   quantities. A scrip exchange and a demerger apply their own ratios, so
//!   there is nothing to compare without re-deriving them.
//! - A **partial-rollover scrip exchange** (one with a cash component) is
//!   reported as *not checked*, with the reason: its apportionment between the
//!   cash and scrip sides is the exchange's own, and re-deriving it here would
//!   be a second copy of the operation. Saying so beats a false mismatch on
//!   every such exchange, and beats silence.
//! - An **unconsumed parcel** — a different fault from a stale stored figure,
//!   and the reason a `WorthlessShares` recognise (which stores nothing and has
//!   no replacement parcels) is a `kind` here at all. The scrip-for-scrip
//!   exchange, the demerger and the recognise each consume *every* open parcel
//!   of their listing as a matter of law, so a parcel of that listing still
//!   open at the operation's date — one entered afterwards but dated on or
//!   before it — is units the operation could never reach (SCENARIOS V-d). The
//!   operation's own replacement parcels, dated the operation date, are
//!   excluded. A **transfer** is never checked this way: it moves a quantity
//!   the taxpayer chose, so a parcel left behind is a legitimate outcome.

use crate::domain::cost_base::ParcelRow;
use crate::domain::open_parcels;
use crate::domain::rollover::CostBaseInputs;
use crate::domain::whole_holding;
use crate::entities::corporate_action::as_acquired_quantity;
use crate::infra::decimal::row_dec;
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, SqlitePool};
use std::collections::HashMap;

/// Which operation a flagged group is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RolloverKind {
    Transfer,
    ScripForScrip,
    Demerger,
    /// A worthless-shares recognise. It stores no carried figures and creates
    /// no replacement parcels, so only the unconsumed-parcel check applies to
    /// it — but it is one of the three operations that consume the whole
    /// holding, and that check is the whole reason it is listed here.
    WorthlessShares,
}

impl RolloverKind {
    /// How the row names itself in a problem sentence.
    fn noun(self) -> &'static str {
        match self {
            RolloverKind::Transfer => "holding-account transfer",
            RolloverKind::ScripForScrip => "scrip-for-scrip exchange",
            RolloverKind::Demerger => "demerger",
            RolloverKind::WorthlessShares => "worthless-shares recognise",
        }
    }
}

impl From<whole_holding::Kind> for RolloverKind {
    fn from(kind: whole_holding::Kind) -> Self {
        match kind {
            whole_holding::Kind::ScripForScrip => RolloverKind::ScripForScrip,
            whole_holding::Kind::Demerger => RolloverKind::Demerger,
            whole_holding::Kind::WorthlessShares => RolloverKind::WorthlessShares,
        }
    }
}

/// One rollover whose stored figures no longer reconcile (or cannot be
/// checked), with every problem found on it. A rollover that reconciles is not
/// returned at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolloverAlert {
    pub kind: RolloverKind,
    /// The `transfers.id` / `corporate_actions.id` the group hangs off.
    pub group_id: i64,
    /// The operation date — the closing Sell's and the replacement parcels'.
    pub date: NaiveDate,
    /// The listing whose parcels were consumed (a demerger's demerged parcels
    /// are of another listing; they are counted, not listed here).
    pub listing_id: i64,
    pub ticker: String,
    /// The closing Sell, so the group is openable without a search.
    pub sell_trade_id: i64,
    /// The replacement parcels' trade ids, ascending.
    pub replacement_trade_ids: Vec<i64>,
    /// Every problem found, each a self-contained sentence naming what moved.
    pub problems: Vec<String>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/rollover_consistency", get(report))
}

/// One rollover group as read from `trades`.
struct Group {
    kind: RolloverKind,
    /// Set for the three operations that consume the *whole* holding, which is
    /// what decides whether the unconsumed-parcel check applies (a transfer
    /// moves a chosen quantity, so it never does). Read through
    /// `domain::whole_holding`, the one place that set is spelled out.
    whole_holding: Option<whole_holding::Kind>,
    group_id: i64,
    sell_trade_id: i64,
    date: NaiveDate,
    listing_id: i64,
    ticker: String,
    /// True for a scrip exchange carrying a cash component.
    partial_rollover: bool,
}

/// A replacement parcel's stored figures.
struct Replacement {
    id: i64,
    currency: String,
    quantity: Decimal,
    cost_base: Decimal,
}

/// Flag every rollover whose stored carried figures no longer match a
/// recomputation from today's facts. An empty report means they all do.
pub async fn db_rollover_alerts(pool: &SqlitePool) -> Result<Vec<RolloverAlert>, sqlx::Error> {
    // One read transaction: a consistent snapshot, so a write landing mid-read
    // cannot pair an operation with a half-updated source parcel — which is
    // exactly what this report would then flag.
    let mut tx = pool.begin().await?;
    let groups = db_groups(&mut tx).await?;
    let mut alerts = Vec::new();
    for group in &groups {
        let problems = problems_for(&mut tx, group).await?;
        if problems.problems.is_empty() {
            continue;
        }
        alerts.push(RolloverAlert {
            kind: group.kind,
            group_id: group.group_id,
            date: group.date,
            listing_id: group.listing_id,
            ticker: group.ticker.clone(),
            sell_trade_id: group.sell_trade_id,
            replacement_trade_ids: problems.replacement_trade_ids,
            problems: problems.problems,
        });
    }
    tx.commit().await?;
    Ok(alerts)
}

/// Every rollover group, oldest first — the four operations that close parcels
/// through a provenance-stamped Sell. The three whole-holding ones are named by
/// `domain::whole_holding`, so this report and the write-time guard read one
/// definition of that set; a transfer is the fourth, checked for its stored
/// figures but never for an unconsumed parcel.
async fn db_groups(conn: &mut sqlx::SqliteConnection) -> Result<Vec<Group>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT s.id AS sell_trade_id, s.date, s.listing_id, l.ticker, \
                s.transfer_id, {}, \
                ca.scrip_cash_per_unit \
         FROM trades s \
         JOIN listings l ON l.id = s.listing_id \
         LEFT JOIN corporate_actions ca ON ca.id = s.scrip_action_id \
         WHERE s.trade_type = 'Sell' \
           AND (s.transfer_id IS NOT NULL OR {}) \
         ORDER BY s.date, s.id",
        whole_holding::closing_sell_columns("s"),
        whole_holding::closing_sell_predicate("s"),
    )))
    .fetch_all(&mut *conn)
    .await?;
    rows.iter()
        .map(|row| {
            let (kind, whole, group_id) =
                if let Some(id) = row.try_get::<Option<i64>, _>("transfer_id")? {
                    (RolloverKind::Transfer, None, id)
                } else {
                    // The query's own predicate admitted the row, so one of the
                    // three columns is set; both come from `whole_holding`.
                    let (kind, id) = whole_holding::kind_of(row)?.ok_or_else(|| {
                        sqlx::Error::Decode(
                            "a rollover closing Sell carries no provenance column".into(),
                        )
                    })?;
                    (kind.into(), Some(kind), id)
                };
            let cash: Option<String> = row.try_get("scrip_cash_per_unit")?;
            Ok(Group {
                kind,
                whole_holding: whole,
                group_id,
                sell_trade_id: row.try_get("sell_trade_id")?,
                date: row.try_get("date")?,
                listing_id: row.try_get("listing_id")?,
                ticker: row.try_get("ticker")?,
                partial_rollover: cash.is_some(),
            })
        })
        .collect()
}

/// What [`problems_for`] found, with the replacement parcels it compared.
struct GroupProblems {
    replacement_trade_ids: Vec<i64>,
    problems: Vec<String>,
}

/// The stored-versus-recomputed comparison for one group.
async fn problems_for(
    conn: &mut sqlx::SqliteConnection,
    group: &Group,
) -> Result<GroupProblems, sqlx::Error> {
    let column = match group.kind {
        RolloverKind::Transfer => "transfer_id",
        RolloverKind::ScripForScrip => "scrip_action_id",
        RolloverKind::Demerger => "demerger_action_id",
        RolloverKind::WorthlessShares => "worthless_action_id",
    };
    // The replacement parcels' stored figures. The column name is one of the
    // three literals above, never user input.
    let replacement_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT id, currency, quantity, average_price, brokerage, gst_on_brokerage \
         FROM trades WHERE {column} = ? AND trade_type IN ('Buy', 'DRP') ORDER BY id"
    )))
    .bind(group.group_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut replacements = Vec::with_capacity(replacement_rows.len());
    for row in &replacement_rows {
        let quantity = row_dec(row, "quantity")?;
        replacements.push(Replacement {
            id: row.try_get("id")?,
            currency: row.try_get("currency")?,
            quantity,
            cost_base: row_dec(row, "average_price")? * quantity
                + row_dec(row, "brokerage")?
                + row_dec(row, "gst_on_brokerage")?,
        });
    }
    let replacement_trade_ids: Vec<i64> = replacements.iter().map(|r| r.id).collect();

    // Did the operation consume the whole holding it was supposed to? Asked
    // first, and asked of a partial-rollover exchange and a worthless-shares
    // recognise too: it depends on no stored figure and on no apportionment,
    // only on which parcels were open at the operation's date.
    let mut problems = unconsumed_problems(&mut *conn, group, &replacement_trade_ids).await?;

    if group.partial_rollover {
        problems.push(format!(
            "this {} carries a cash component, so how much of each parcel's cost base went to \
             the cash side (assessed then) rather than to the replacement parcels is the \
             exchange's own apportionment — the stored figures are not checked here, and a \
             fact dated on or before {} that changed a source parcel would go unnoticed",
            group.kind.noun(),
            group.date
        ));
        return Ok(GroupProblems {
            replacement_trade_ids,
            problems,
        });
    }
    // A recognise stores nothing and creates no replacement parcels — there is
    // no carried figure to compare, so the unconsumed-parcel check above is the
    // whole of it.
    if group.kind == RolloverKind::WorthlessShares {
        return Ok(GroupProblems {
            replacement_trade_ids,
            problems,
        });
    }

    // What the consumed units' cost base is *now*, through the operations' own
    // call, so this report cannot disagree with them about the pipeline.
    let inputs = CostBaseInputs::load(&mut *conn, group.listing_id).await?;
    let allocation_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT pa.quantity_allocated, {} \
         FROM parcel_allocations pa JOIN trades t ON t.id = pa.purchase_trade_id \
         WHERE pa.sale_trade_id = ? ORDER BY t.id",
        ParcelRow::columns_qualified("t")
    )))
    .bind(group.sell_trade_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut expected_cost_base: HashMap<String, Decimal> = HashMap::new();
    let mut expected_units = Decimal::ZERO;
    for row in &allocation_rows {
        let allocated = row_dec(row, "quantity_allocated")?;
        let parcel = ParcelRow::from_row(row)?;
        let as_acquired = as_acquired_quantity(allocated, &inputs.splits, parcel.date, group.date);
        *expected_cost_base
            .entry(parcel.currency.clone())
            .or_default() += inputs.carried_cost_base(&parcel, as_acquired, group.date)?;
        expected_units += allocated;
    }

    let mut stored_cost_base: HashMap<String, Decimal> = HashMap::new();
    let mut stored_units = Decimal::ZERO;
    for replacement in &replacements {
        *stored_cost_base
            .entry(replacement.currency.clone())
            .or_default() += replacement.cost_base;
        stored_units += replacement.quantity;
    }

    let mut currencies: Vec<&String> = expected_cost_base
        .keys()
        .chain(stored_cost_base.keys())
        .collect();
    currencies.sort_unstable();
    currencies.dedup();
    for currency in currencies {
        let expected = expected_cost_base
            .get(currency)
            .copied()
            .unwrap_or_default();
        let stored = stored_cost_base.get(currency).copied().unwrap_or_default();
        if expected != stored {
            problems.push(format!(
                "this {} carried {stored} {currency} of cost base away on {}, but the units it \
                 consumed are worth {expected} {currency} on today's facts (a difference of {}) — \
                 a source parcel, an AMIT adjustment, or a payment behind it has changed since. \
                 Delete the operation and run it again so the replacement parcels carry the \
                 current figures",
                group.kind.noun(),
                group.date,
                stored - expected,
            ));
        }
    }
    // Units move one for one only in a transfer; the other two apply a ratio.
    if group.kind == RolloverKind::Transfer && expected_units != stored_units {
        problems.push(format!(
            "this holding-account transfer moved {expected_units} unit(s) out of the source \
             account but its transfer-in parcels hold {stored_units} — the two sides no longer \
             agree, so the holding is over- or under-counted. Delete the transfer and re-enter it"
        ));
    }
    Ok(GroupProblems {
        replacement_trade_ids,
        problems,
    })
}

/// Every parcel of the group's listing still open at the operation's date that
/// the operation did not consume — one problem sentence each.
///
/// Only the three whole-holding operations are asked: each consumes *every*
/// open parcel of its listing as a matter of law, so an open parcel at its date
/// is a hole. A transfer moves a chosen quantity, so a parcel left behind is a
/// legitimate outcome and is never flagged.
///
/// "Open at the operation's date" is `domain::open_parcels::load(conn,
/// Some(date))` — the one implementation of that read, so this cannot disagree
/// with the portfolio and unrealised-gains views about what is still held. Two
/// exclusions, both by construction rather than by judgement: parcels of other
/// listings (a demerger's demerged parcels are of another security, and the
/// operation never claimed to consume them), and the group's **own**
/// replacement parcels, which are dated the operation date and are its output,
/// not its input.
async fn unconsumed_problems(
    conn: &mut sqlx::SqliteConnection,
    group: &Group,
    replacement_trade_ids: &[i64],
) -> Result<Vec<String>, sqlx::Error> {
    let Some(kind) = group.whole_holding else {
        return Ok(Vec::new());
    };
    let open = open_parcels::load(&mut *conn, Some(group.date)).await?;
    Ok(open
        .iter()
        .filter(|p| {
            p.parcel.listing_id == group.listing_id && !replacement_trade_ids.contains(&p.parcel.id)
        })
        .map(|p| {
            format!(
                "this {} consumed every parcel of {} open on {}, as it must — but trade #{} \
                 (acquired {}) still holds {} unit(s) it never consumed, so it was entered after \
                 the operation ran and dated on or before it: {}. Delete the operation, then run \
                 it again so it carries these units too",
                group.kind.noun(),
                group.ticker,
                group.date,
                p.parcel.id,
                p.parcel.date,
                p.remaining_as_of,
                kind.stranded_consequence(),
            )
        })
        .collect())
}

async fn report(State(pool): State<SqlitePool>) -> Result<Json<Vec<RolloverAlert>>, ApiError> {
    Ok(Json(db_rollover_alerts(&pool).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::holding_account::{self, HoldingAccount};
    use crate::entities::sell::AllocationInput;
    use crate::entities::transfer::{self, TransferBody};
    use crate::entities::{corporate_action as ca, scrip_exchange, trade};
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};

    async fn pool_with_transfer() -> (SqlitePool, i64) {
        let pool = test_pool().await;
        test_support::listing(1).ticker("AAA").insert(&pool).await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Broker".to_string(),
            },
        )
        .await
        .unwrap();
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(dec("100"))
            .price(dec("5"))
            .insert(&pool)
            .await;
        let group = transfer::db_transfer(
            &pool,
            1,
            &TransferBody {
                listing_id: 1,
                date: ymd(2023, 8, 1),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("100"),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();
        let replacement = group.transfer_ins[0].id;
        (pool, replacement)
    }

    /// A rollover nothing has disturbed reconciles, so the report is empty —
    /// which is what makes a non-empty one worth reading.
    #[tokio::test]
    async fn db_an_untouched_transfer_reconciles() {
        let (pool, _) = pool_with_transfer().await;
        assert_eq!(db_rollover_alerts(&pool).await.unwrap(), vec![]);
    }

    /// The state the write-time guards cannot see: the **source parcel** itself
    /// is edited after the move. Its cost base changes; the stored carried
    /// figure cannot, and the reports read the frozen one — so the difference is
    /// named, with the operation to redo (SCENARIOS N-06).
    #[tokio::test]
    async fn db_editing_the_source_parcel_after_the_move_is_flagged() {
        let (pool, replacement) = pool_with_transfer().await;
        // $5.00 → $6.00 a unit: the parcel is now worth $600, but the
        // transfer-in still carries $500.
        let mut parcel = trade::db_get(&pool, 1).await.unwrap().unwrap();
        parcel.average_price = dec("6");
        trade::db_upsert(&pool, &parcel).await.unwrap();

        let alerts = db_rollover_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let alert = &alerts[0];
        assert_eq!(alert.kind, RolloverKind::Transfer);
        assert_eq!(alert.group_id, 1);
        assert_eq!(alert.date, ymd(2023, 8, 1));
        assert_eq!(alert.ticker, "AAA");
        assert_eq!(alert.replacement_trade_ids, vec![replacement]);
        assert_eq!(alert.problems.len(), 1);
        let problem = &alert.problems[0];
        assert!(problem.contains("carried 500 AUD"), "{problem}");
        assert!(problem.contains("worth 600 AUD"), "{problem}");
        assert!(problem.contains("-100"), "{problem}");
        assert!(problem.contains("Delete the operation"), "{problem}");
    }

    /// A partial-rollover scrip exchange says it is not checked, rather than
    /// reporting a false mismatch for the cash-apportioned part.
    #[tokio::test]
    async fn db_a_scrip_exchange_with_cash_reports_that_it_is_not_checked() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("OLD").insert(&pool).await;
        test_support::listing(2).ticker("NEW").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        ca::db_upsert(
            &pool,
            &ca::CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2023, 7, 1),
                kind: ca::ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: Decimal::ONE,
                    scrip_old_units: Decimal::ONE,
                    scrip_cash_per_unit: Some(dec("2")),
                    scrip_market_value: Some(dec("18")),
                    scrip_cash_currency: Some("AUD".to_string()),
                },
            },
        )
        .await
        .unwrap();
        scrip_exchange::db_exchange(&pool, 10).await.unwrap();

        let alerts = db_rollover_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, RolloverKind::ScripForScrip);
        assert_eq!(alerts[0].group_id, 10);
        assert!(
            alerts[0].problems[0].contains("not checked here"),
            "{:?}",
            alerts[0].problems
        );
    }

    /// A cash-free exchange and a demerger are checked like a transfer: the
    /// percentage only splits the carried total, so the sum still has to match.
    #[tokio::test]
    async fn db_a_demerger_is_checked_on_its_carried_total() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("HEAD").insert(&pool).await;
        test_support::listing(2).ticker("SPUN").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        ca::db_upsert(
            &pool,
            &ca::CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2023, 7, 1),
                kind: ca::ActionKind::Demerger {
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
        assert_eq!(db_rollover_alerts(&pool).await.unwrap(), vec![]);

        // Move the source parcel's brokerage and the split of $1,000 no longer
        // adds up.
        let mut parcel = trade::db_get(&pool, 1).await.unwrap().unwrap();
        parcel.brokerage = dec("50");
        trade::db_upsert(&pool, &parcel).await.unwrap();
        let alerts = db_rollover_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, RolloverKind::Demerger);
        // Both replacement parcels are named — the head and the demerged one.
        assert_eq!(alerts[0].replacement_trade_ids.len(), 2);
    }

    #[tokio::test]
    async fn api_get_rollover_consistency() {
        let (pool, _) = pool_with_transfer().await;
        let client = ApiClient::over(router().with_state(pool.clone()));
        let alerts: Vec<RolloverAlert> = client.get_json("/reports/rollover_consistency").await;
        assert!(alerts.is_empty());

        let mut parcel = trade::db_get(&pool, 1).await.unwrap().unwrap();
        parcel.brokerage = dec("25");
        trade::db_upsert(&pool, &parcel).await.unwrap();
        let alerts: Vec<RolloverAlert> = client.get_json("/reports/rollover_consistency").await;
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].sell_trade_id, 2);
    }

    // ---- SCENARIOS V-d: the unconsumed-parcel problem ----

    /// An OLD → NEW exchange that has already run, plus a parcel of OLD dated
    /// before it that the exchange never consumed — the state a pre-guard build
    /// could reach, written the way that build wrote it.
    async fn pool_with_a_stranded_parcel() -> SqlitePool {
        let pool = test_pool().await;
        test_support::listing(1).ticker("OLD").insert(&pool).await;
        test_support::listing(2).ticker("NEW").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 2))
            .insert(&pool)
            .await;
        ca::db_upsert(
            &pool,
            &ca::CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2024, 6, 10),
                kind: ca::ActionKind::ScripForScrip {
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
        scrip_exchange::db_exchange(&pool, 10).await.unwrap();
        test_support::insert_parcel_bypassing_checks(&pool, 500, 1, ymd(2024, 2, 5), "50", "3")
            .await;
        pool
    }

    /// The report's answer to a database already in the state the write-time
    /// guard now refuses: the exchange consumed every parcel open on its date,
    /// so a parcel of the same listing still open then is units it could never
    /// reach — named with its trade id, its acquisition date, what it still
    /// holds, and the recovery (SCENARIOS V-d).
    #[tokio::test]
    async fn db_a_parcel_the_exchange_never_consumed_is_reported() {
        let pool = pool_with_a_stranded_parcel().await;
        let alerts = db_rollover_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let alert = &alerts[0];
        assert_eq!(alert.kind, RolloverKind::ScripForScrip);
        assert_eq!(alert.group_id, 10);
        assert_eq!(alert.date, ymd(2024, 6, 10));
        assert_eq!(alert.ticker, "OLD");
        assert_eq!(alert.problems.len(), 1);
        let problem = &alert.problems[0];
        assert!(problem.contains("scrip-for-scrip exchange"), "{problem}");
        assert!(problem.contains("trade #500"), "{problem}");
        assert!(problem.contains("acquired 2024-02-05"), "{problem}");
        assert!(problem.contains("50 unit(s)"), "{problem}");
        assert!(problem.contains("Delete the operation"), "{problem}");
    }

    /// A worthless-shares recognise is a `kind` of this report for exactly one
    /// reason: it stores no carried figures and creates no replacement parcels,
    /// so the only thing there is to check about it is whether it consumed the
    /// whole holding. Here it did not, and the row carries no replacements.
    #[tokio::test]
    async fn db_a_parcel_the_recognise_never_consumed_is_reported() {
        let pool = test_pool().await;
        test_support::recognised_worthless_listing(
            &pool,
            1,
            "DEAD",
            ymd(2024, 1, 2),
            90,
            ymd(2024, 6, 13),
        )
        .await;
        test_support::insert_parcel_bypassing_checks(&pool, 500, 1, ymd(2024, 3, 5), "40", "1")
            .await;

        let alerts = db_rollover_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, RolloverKind::WorthlessShares);
        assert_eq!(alerts[0].group_id, 90);
        assert!(alerts[0].replacement_trade_ids.is_empty());
        let problem = &alerts[0].problems[0];
        assert!(problem.contains("worthless-shares recognise"), "{problem}");
        assert!(problem.contains("already written off"), "{problem}");
    }

    /// A recognise that did consume the whole holding is not reported at all —
    /// which is what makes a row worth reading.
    #[tokio::test]
    async fn db_a_fully_consumed_operation_is_not_reported() {
        let pool = test_pool().await;
        test_support::recognised_worthless_listing(
            &pool,
            1,
            "DEAD",
            ymd(2024, 1, 2),
            90,
            ymd(2024, 6, 13),
        )
        .await;
        assert_eq!(db_rollover_alerts(&pool).await.unwrap(), vec![]);
    }

    /// A parcel dated **after** the operation is ordinary post-event activity —
    /// the head listing of a demerger keeps trading — and is never flagged. The
    /// demerger's own head replacement parcel, dated the demerger date itself,
    /// is likewise its output rather than an unconsumed input.
    #[tokio::test]
    async fn db_a_parcel_dated_after_the_operation_is_not_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("HEAD").insert(&pool).await;
        test_support::listing(2).ticker("SPIN").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 2))
            .insert(&pool)
            .await;
        ca::db_upsert(
            &pool,
            &ca::CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2024, 6, 11),
                kind: ca::ActionKind::Demerger {
                    demerger_listing_id: 2,
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: dec("5"),
                    demerger_cost_base_pct: dec("10"),
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
        test_support::buy(500, 1)
            .date(ymd(2024, 6, 12))
            .insert(&pool)
            .await;
        assert_eq!(db_rollover_alerts(&pool).await.unwrap(), vec![]);
    }

    /// A transfer moves a quantity the taxpayer chose, so units left behind are
    /// a legitimate outcome — never an unconsumed parcel. Only the three
    /// whole-holding operations are asked.
    #[tokio::test]
    async fn db_a_transfer_that_leaves_units_behind_is_not_flagged() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("AAA").insert(&pool).await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Broker".to_string(),
            },
        )
        .await
        .unwrap();
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(dec("100"))
            .price(dec("5"))
            .insert(&pool)
            .await;
        transfer::db_transfer(
            &pool,
            1,
            &TransferBody {
                listing_id: 1,
                date: ymd(2023, 8, 1),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("40"),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(db_rollover_alerts(&pool).await.unwrap(), vec![]);
    }
}
