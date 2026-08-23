use crate::domain::cost_base::AmitReductionEvent;
use crate::domain::rollover;
use crate::entities::corporate_action;
use crate::infra::decimal::{Money, parse_dec};
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AmitAdjustment {
    pub id: i64,
    pub amma_statement_id: i64,
    pub trade_id: i64,
    /// Units of the parcel covered by the statement's per-unit adjustment,
    /// expressed in the parcel's *as-acquired* units (the same basis as
    /// `trade.quantity`, which caps it). A share split/consolidation between
    /// the parcel's acquisition and the statement's year end is handled by
    /// [`reduction_for`], which re-bases this quantity into the statement
    /// year's basis before multiplying — enter the statement's per-unit figure
    /// exactly as the fund states it.
    ///
    /// It also decides *which* units of the parcel the reduction reaches once
    /// part of it has been sold: covering less than the whole parcel covers
    /// the units still held at the statement's year end first
    /// ([`crate::domain::cost_base::AmitReductionEvent::reduction_for_units`]).
    #[sqlx(try_from = "Money")]
    pub quantity: Decimal,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmitAdjustmentBody {
    pub amma_statement_id: i64,
    pub trade_id: i64,
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub quantity: Decimal,
}

impl CrudEntity for AmitAdjustment {
    type Key = i64;
    const TABLE: &'static str = "amit_adjustments";
    const COLUMNS: &'static str = "id, amma_statement_id, trade_id, quantity";
    const NOUN: &'static str = "AMIT adjustment";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/amit_adjustments",
            get(http::list_handler::<AmitAdjustment>),
        )
        .route(
            "/amit_adjustments/{id}",
            get(http::get_handler::<AmitAdjustment>)
                .put(upsert)
                .delete(http::delete_handler::<AmitAdjustment>),
        )
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<AmitAdjustment>, sqlx::Error> {
    http::crud_get(pool, id).await
}

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("AMIT adjustment write failed: {0}")]
    Db(#[from] sqlx::Error),
    #[error("the adjusted trade is not a Buy or DRP parcel")]
    TradeNotBuyOrDrp,
    #[error("the trade's listing differs from the AMMA statement's listing")]
    ListingMismatch,
    /// The trade sits in a different holding account from the AMMA
    /// statement's: a registry issues one statement per holder account, so a
    /// statement only ever adjusts its own account's parcels.
    #[error("the trade sits in a different holding account from the AMMA statement")]
    HoldingAccountMismatch,
    #[error("the adjusted quantity exceeds the trade's quantity")]
    QuantityExceedsTrade,
    /// Some or all of the parcel's units were carried into a **replacement
    /// parcel** by a transfer, scrip-for-scrip exchange or demerger
    /// (`domain::rollover`). Those operations compute the replacement's cost
    /// base when they run and store it as a fixed figure, so a reduction
    /// entered against the original afterwards reaches nothing at all: the
    /// original is fully consumed (no open-holdings report shows it) and its
    /// closing Sell is not a disposal (no realised gain nets it off), so the
    /// amount is silently lost (SCENARIOS F-17). Refused here so the state is
    /// unrepresentable rather than quietly wrong. Mapped to `422`.
    #[error(
        "{adjustable} of the parcel's units remain adjustable; the rest were carried into a \
         replacement parcel by a rollover"
    )]
    UnitsCarriedIntoReplacement {
        /// Units of the parcel a rollover has *not* carried away — the most
        /// an adjustment on it may cover (zero once the whole parcel went).
        adjustable: Decimal,
        /// The replacement parcels the units went into, ascending.
        replacements: Vec<i64>,
    },
    /// Another row already adjusts this parcel on this statement. Applying
    /// the statement's per-unit figure to the same parcel twice reduces its
    /// cost base twice, and CGT event E10's nil floor turns an over-reduction
    /// into a capital gain that was never made — so this is a data-model
    /// invariant, not an advisory cross-check. Also enforced by the
    /// `amit_adjustments_statement_trade` UNIQUE index (migration 0022).
    #[error("this parcel already has an adjustment on this AMMA statement")]
    DuplicateParcel,
}

/// [`db_upsert`] on a caller-supplied connection, so the AMMA statement's
/// `generate_adjustments` operation can write a whole generated set inside
/// one transaction and still go through the same per-row invariants (and the
/// same `row_history` audit trail) as a hand-entered row.
pub async fn db_upsert_on(
    conn: &mut sqlx::SqliteConnection,
    adj: &AmitAdjustment,
) -> Result<(), UpsertError> {
    db_write_on(
        conn,
        Some(adj.id),
        &AmitAdjustmentBody {
            amma_statement_id: adj.amma_statement_id,
            trade_id: adj.trade_id,
            quantity: adj.quantity,
        },
    )
    .await?;
    Ok(())
}

/// Write a *new* adjustment, letting the database assign its id, and answer
/// it. Same invariants and same audit trail as [`db_upsert_on`] — only the id
/// differs: the generator used to compute `SELECT MAX(id) + 1` and bind it,
/// which after a delete re-issues the freed id and hands the new row the
/// deleted one's `row_history` trail (SCENARIOS U-a). An `AUTOINCREMENT`
/// column only ever decides that when the INSERT leaves the id out.
pub async fn db_insert_on(
    conn: &mut sqlx::SqliteConnection,
    body: &AmitAdjustmentBody,
) -> Result<i64, UpsertError> {
    db_write_on(conn, None, body).await
}

/// The shared write core of both: `id` is `Some` on the client-supplied-id
/// upsert path (`PUT /amit_adjustments/{id}`) and `None` where the database
/// assigns it. Answers the id written.
async fn db_write_on(
    conn: &mut sqlx::SqliteConnection,
    id: Option<i64>,
    adj: &AmitAdjustmentBody,
) -> Result<i64, UpsertError> {
    use crate::entities::trade::TradeType;

    let trade_type: TradeType = sqlx::query_scalar("SELECT trade_type FROM trades WHERE id = ?")
        .bind(adj.trade_id)
        .fetch_one(&mut *conn)
        .await?;
    if !trade_type.is_acquisition() {
        return Err(UpsertError::TradeNotBuyOrDrp);
    }

    let (trade_listing_id, trade_account_id): (i64, i64) =
        sqlx::query_as("SELECT listing_id, holding_account_id FROM trades WHERE id = ?")
            .bind(adj.trade_id)
            .fetch_one(&mut *conn)
            .await?;
    let (amma_listing_id, amma_account_id): (i64, i64) =
        sqlx::query_as("SELECT listing_id, holding_account_id FROM amma_statements WHERE id = ?")
            .bind(adj.amma_statement_id)
            .fetch_one(&mut *conn)
            .await?;
    if trade_listing_id != amma_listing_id {
        return Err(UpsertError::ListingMismatch);
    }
    // A statement only ever adjusts parcels in its own holding account (the
    // registry issues one AMMA statement per holder account) — with one
    // exception, which is the whole answer to SCENARIOS N-06: where a rollover
    // has since carried the statement's units into a **replacement parcel**,
    // that parcel is where the reduction has to land, and it sits in whatever
    // account the operation moved them to. An AMMA statement for a year ended
    // 30 June arrives in August or September, so a transfer between the year end
    // and data entry is the ordinary case; refusing it here (while F-17's
    // `UnitsCarriedIntoReplacement` refuses the source parcel, correctly)
    // left the statement's reduction with nowhere to go at all.
    //
    // The chain is followed, not just one step, so a holding moved twice is
    // still reachable. The parcel's listing is already pinned to the
    // statement's above, which is what keeps a demerger's *demerged* parcel —
    // another listing, another trust — out of it.
    if trade_account_id != amma_account_id {
        let ancestors = rollover::source_ancestors(&mut *conn, adj.trade_id).await?;
        let mut reachable = false;
        for ancestor in ancestors {
            let account: Option<i64> =
                sqlx::query_scalar("SELECT holding_account_id FROM trades WHERE id = ?")
                    .bind(ancestor)
                    .fetch_optional(&mut *conn)
                    .await?;
            if account == Some(amma_account_id) {
                reachable = true;
                break;
            }
        }
        if !reachable {
            return Err(UpsertError::HoldingAccountMismatch);
        }
    }

    let trade_qty: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
        .bind(adj.trade_id)
        .fetch_one(&mut *conn)
        .await?;
    let trade_qty: Decimal = trade_qty
        .parse()
        .map_err(|_| UpsertError::Db(sqlx::Error::Decode("invalid trade quantity".into())))?;
    if adj.quantity > trade_qty {
        return Err(UpsertError::QuantityExceedsTrade);
    }

    // Units a rollover has already carried into a replacement parcel are
    // beyond this row's reach: the replacement's cost base was computed and
    // frozen when the operation ran, and the original's closing Sell is not a
    // disposal, so a reduction covering them would be silently lost
    // (SCENARIOS F-17). The three parcel-substituting operations are exactly
    // `domain::rollover`'s — a transfer, a scrip-for-scrip exchange and a
    // demerger — recognised by the provenance column their closing Sell and
    // their replacement Buys share. An ordinary Sell, a buy-back
    // participation and a worthless recognise are real disposals whose gain
    // the reduction does reach, so they are not counted here.
    let rollover_rows = sqlx::query(
        "SELECT pa.quantity_allocated, s.date, \
                COALESCE(s.transfer_id, s.scrip_action_id, s.demerger_action_id) AS group_id, \
                CASE WHEN s.transfer_id IS NOT NULL THEN 'transfer_id' \
                     WHEN s.scrip_action_id IS NOT NULL THEN 'scrip_action_id' \
                     ELSE 'demerger_action_id' END AS group_column \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE pa.purchase_trade_id = ? \
           AND (s.transfer_id IS NOT NULL OR s.scrip_action_id IS NOT NULL \
                OR s.demerger_action_id IS NOT NULL)",
    )
    .bind(adj.trade_id)
    .fetch_all(&mut *conn)
    .await?;
    if !rollover_rows.is_empty() {
        let trade_date: chrono::NaiveDate =
            sqlx::query_scalar("SELECT date FROM trades WHERE id = ?")
                .bind(adj.trade_id)
                .fetch_one(&mut *conn)
                .await?;
        let splits = corporate_action::db_splits_for_listing(&mut *conn, trade_listing_id).await?;
        let mut sales = Vec::with_capacity(rollover_rows.len());
        let mut groups = Vec::new();
        for row in &rollover_rows {
            let date: chrono::NaiveDate = row.try_get("date")?;
            let qty = parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?;
            sales.push((date, qty));
            groups.push((
                row.try_get::<String, _>("group_column")?,
                row.try_get::<i64, _>("group_id")?,
            ));
        }
        // Allocations are in their sale date's units; the parcel's own
        // quantity — and this row's — are as acquired.
        let carried = corporate_action::sold_in_acquired_units(&sales, &splits, trade_date);
        let adjustable = (trade_qty - carried).max(Decimal::ZERO);
        if adj.quantity > adjustable {
            groups.sort_unstable();
            groups.dedup();
            let mut replacements = Vec::new();
            for (column, group_id) in groups {
                // The replacement Buys of the same operation: same provenance
                // column and id, acquisition side. The column name comes from
                // the CASE above, never from user input.
                let ids: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    "SELECT id FROM trades \
                     WHERE {column} = ? AND trade_type IN ('Buy', 'DRP') ORDER BY id"
                )))
                .bind(group_id)
                .fetch_all(&mut *conn)
                .await?;
                replacements.extend(ids);
            }
            replacements.sort_unstable();
            replacements.dedup();
            return Err(UpsertError::UnitsCarriedIntoReplacement {
                adjustable,
                replacements,
            });
        }
    }

    // One adjustment per (statement, parcel): a second row for the same
    // parcel would apply the statement's per-unit figure to it twice. Checked
    // here so the rejection carries this module's own wording; the UNIQUE
    // index behind it (migration 0022) is the backstop for any other writer.
    // `IS NOT` rather than `!=` so a NULL id (the database-assigned insert)
    // excludes nothing: every existing row of the pair is a duplicate of a row
    // that does not exist yet.
    let duplicate: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM amit_adjustments \
         WHERE amma_statement_id = ? AND trade_id = ? AND id IS NOT ?",
    )
    .bind(adj.amma_statement_id)
    .bind(adj.trade_id)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    if duplicate.is_some() {
        return Err(UpsertError::DuplicateParcel);
    }

    // A NULL id is an omitted one: SQLite assigns the next id the
    // AUTOINCREMENT column has never issued.
    let result = sqlx::query(
        "INSERT INTO amit_adjustments (id, amma_statement_id, trade_id, quantity) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             amma_statement_id = excluded.amma_statement_id, \
             trade_id          = excluded.trade_id, \
             quantity          = excluded.quantity",
    )
    .bind(id)
    .bind(adj.amma_statement_id)
    .bind(adj.trade_id)
    .bind(Money(adj.quantity))
    .execute(&mut *conn)
    .await?;
    Ok(id.unwrap_or_else(|| result.last_insert_rowid()))
}

pub async fn db_upsert(pool: &SqlitePool, adj: &AmitAdjustment) -> Result<(), UpsertError> {
    let mut conn = pool.acquire().await?;
    db_upsert_on(&mut conn, adj).await
}

/// The cost-base reduction one AMMA statement applies to `quantity` units of
/// one parcel — *the* place the two figures are multiplied, so no caller can
/// pair them on mismatched unit bases. [`db_cost_base_reduction_events`] calls
/// it with a single unit, since it stores the per-unit figure and lets the
/// pipeline decide which units it reaches.
///
/// `quantity` is in the parcel's
/// *as-acquired* basis (the basis `trades.quantity` caps), while the
/// statement's per-unit `cost_base_adjustment` is stated per unit as the
/// statement's own tax year saw them. A share split/consolidation or bonus
/// issue between the parcel's acquisition and `tax_year_end_date` leaves those
/// two on different bases, so the quantity is re-based into the year-end basis
/// before multiplying (TD 2000/10: a split scales the unit count, never the
/// parcel's cost base). This is the AMIT counterpart of
/// [`corporate_action::RocEvent::per_unit_for`]'s re-basing of a
/// return-of-capital payment.
pub fn reduction_for(
    quantity: Decimal,
    cost_base_adjustment: Decimal,
    splits: &[corporate_action::SplitEvent],
    acquired: chrono::NaiveDate,
    tax_year_end_date: chrono::NaiveDate,
) -> Decimal {
    // The re-basing runs in whichever direction the two dates sit. Ordinarily
    // the parcel predates the year end and its units are scaled *forward* into
    // the year-end basis. A **rollover replacement** parcel can postdate it
    // (SCENARIOS N-06: an AMMA statement arrives after a transfer has moved the
    // units it covers, and the reduction belongs on the parcel now holding
    // them), and then the parcel's own basis is the later one — a split between
    // the year end and the operation date has already scaled it — so the
    // quantity is converted *back* to the year-end basis instead. Multiplying
    // by 1 in that case, which a one-directional window would do, would apply
    // the statement's per-unit figure to post-split units.
    let in_year_end_basis = if acquired <= tax_year_end_date {
        corporate_action::split_adjusted_quantity(
            quantity,
            splits,
            acquired,
            Some(tax_year_end_date),
        )
    } else {
        corporate_action::as_acquired_quantity(quantity, splits, tax_year_end_date, acquired)
    };
    in_year_end_basis * cost_base_adjustment
}

/// Every AMMA statement adjusting a purchase parcel, keyed by `trade_id` — the
/// [`AmitReductionEvent`] input the shared cost-base pipeline
/// (`domain::cost_base::adjusted_cost_base` / `adjustment_detail`) and the
/// net-capital-gain report's E10 walk all read. Sorted by `tax_year_end_date`
/// (then statement id) within each trade, matching the year order those walks'
/// running floor assumes.
///
/// `up_to` bounds the statements to years ending on or before it: an
/// adjustment arises at its statement's year end, so a report valued as at an
/// earlier date (a snapshot of a past day) must not include it. `None` = no
/// bound.
///
/// Each event carries the statement's reduction **per as-acquired unit** and
/// the units the adjustment row covers, rather than one pre-multiplied
/// whole-parcel total, so the pipeline can apply it to the units the row
/// actually covers (see [`AmitReductionEvent::per_unit_for`]) instead of
/// pooling it across the parcel. `disposed_by_year_end` — the units of the
/// parcel already sold when the statement's year ended — is what tells those
/// two groups apart, so the allocations are read here too
/// (`domain::open_parcels::db_units_sold`, the shared allocations read).
///
/// Takes the caller's own connection so the scrip-for-scrip exchange
/// (`entities::scrip_exchange`) and every report can run it inside their own
/// transaction.
pub async fn db_cost_base_reduction_events(
    conn: &mut sqlx::SqliteConnection,
    up_to: Option<chrono::NaiveDate>,
) -> Result<HashMap<i64, Vec<AmitReductionEvent>>, sqlx::Error> {
    let cutoff = crate::infra::date::as_of_or_open(up_to);
    let splits = corporate_action::db_share_split_events(&mut *conn).await?;
    let rows = sqlx::query(
        "SELECT aa.trade_id, a.id AS amma_statement_id, a.tax_year_end_date, aa.quantity, \
                a.cost_base_adjustment, t.listing_id, t.date AS trade_date \
         FROM amit_adjustments aa \
         JOIN amma_statements a ON a.id = aa.amma_statement_id \
         JOIN trades t ON t.id = aa.trade_id \
         WHERE a.tax_year_end_date <= ? \
         ORDER BY aa.trade_id, a.tax_year_end_date, a.id",
    )
    .bind(cutoff)
    .fetch_all(&mut *conn)
    .await?;

    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    // Sales after `up_to` are already excluded by the year-end filter above
    // (a statement's year end is never after the cutoff), so this read needs
    // no bound of its own.
    let sold = crate::domain::open_parcels::db_units_sold(&mut *conn, None).await?;

    let mut map: HashMap<i64, Vec<AmitReductionEvent>> = HashMap::new();
    for row in &rows {
        let trade_id: i64 = row.try_get("trade_id")?;
        let listing_id: i64 = row.try_get("listing_id")?;
        let amma_statement_id: i64 = row.try_get("amma_statement_id")?;
        let tax_year_end_date: chrono::NaiveDate = row.try_get("tax_year_end_date")?;
        let trade_date: chrono::NaiveDate = row.try_get("trade_date")?;
        let covered = parse_dec("quantity", row.try_get("quantity")?)?;
        let cba = parse_dec("cost_base_adjustment", row.try_get("cost_base_adjustment")?)?;
        let splits = splits.get(&listing_id).map_or(&[][..], |v| v);
        // Units of the parcel already disposed of when the statement's year
        // ended, re-based back to the parcel's as-acquired basis (an
        // allocation is in its own sale date's units).
        let disposed_by_year_end = sold.get(&trade_id).map_or(Decimal::ZERO, |sales| {
            sales
                .iter()
                .filter(|(sale_date, _)| *sale_date <= tax_year_end_date)
                .map(|&(sale_date, qty)| {
                    corporate_action::as_acquired_quantity(qty, splits, trade_date, sale_date)
                })
                .sum()
        });
        map.entry(trade_id).or_default().push(AmitReductionEvent {
            amma_statement_id,
            tax_year_end_date,
            per_unit: reduction_for(Decimal::ONE, cba, splits, trade_date, tax_year_end_date),
            covered,
            disposed_by_year_end,
        });
    }
    Ok(map)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<AmitAdjustmentBody>,
) -> Result<StatusCode, ApiError> {
    let adj = AmitAdjustment {
        id,
        amma_statement_id: body.amma_statement_id,
        trade_id: body.trade_id,
        quantity: body.quantity,
    };
    db_upsert(&pool, &adj).await?;
    Ok(StatusCode::NO_CONTENT)
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::TradeNotBuyOrDrp => {
                ApiError::unprocessable("the adjusted trade is not a Buy or DRP parcel")
            }
            UpsertError::ListingMismatch => ApiError::unprocessable(
                "the trade's listing differs from the AMMA statement's listing",
            ),
            UpsertError::HoldingAccountMismatch => ApiError::unprocessable(
                "the trade sits in a different holding account from the AMMA statement — \
                 a statement only adjusts its own account's parcels",
            ),
            UpsertError::QuantityExceedsTrade => {
                ApiError::unprocessable("the adjusted quantity exceeds the trade's quantity")
            }
            UpsertError::UnitsCarriedIntoReplacement {
                adjustable,
                replacements,
            } => {
                let list = replacements
                    .iter()
                    .map(|id| format!("#{id}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                ApiError::unprocessable(format!(
                    "a transfer, scrip-for-scrip exchange or demerger has carried this parcel's \
                     units into replacement parcel(s) {list}, which took their cost base with \
                     them — a reduction entered here would reach nothing, so at most {adjustable} \
                     unit(s) of it can still be adjusted. Enter the rest against the replacement \
                     parcel instead, where those units now are: that is accepted for a statement \
                     of the account they came from, and generating the statement's set does it for \
                     you"
                ))
            }
            UpsertError::DuplicateParcel => ApiError::unprocessable(
                "this parcel already has an adjustment on this AMMA statement — \
                 edit that row instead, or the statement's per-unit adjustment would \
                 reduce the parcel's cost base twice",
            ),
            UpsertError::Db(err) => err.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    /// The whole-parcel reduction each parcel's statements *state* — every
    /// event's covered units × its per-unit figure, summed. Which units of the
    /// parcel that amount then reaches is
    /// [`AmitReductionEvent::per_unit_for`]'s job, exercised in
    /// `domain::cost_base`; these tests pin the figure the rows are read as.
    async fn reductions(pool: &SqlitePool) -> HashMap<i64, Decimal> {
        events(pool)
            .await
            .into_iter()
            .map(|(trade_id, events)| {
                (trade_id, events.iter().map(|e| e.amount()).sum::<Decimal>())
            })
            .collect()
    }

    /// [`db_cost_base_reduction_events`] over a pool-acquired connection
    /// (reports call it on their own read transaction).
    async fn events(pool: &SqlitePool) -> HashMap<i64, Vec<AmitReductionEvent>> {
        let mut conn = pool.acquire().await.unwrap();
        db_cost_base_reduction_events(&mut conn, None)
            .await
            .unwrap()
    }

    async fn insert_test_listing(pool: &SqlitePool, id: i64, exchange_mic: &str, ticker: &str) {
        test_support::listing(id)
            .mic(exchange_mic)
            .ticker(ticker)
            .name(ticker)
            .amit(true)
            .insert(pool)
            .await;
    }

    async fn insert_buy_trade(pool: &SqlitePool, id: i64, listing_id: i64, quantity: Decimal) {
        test_support::buy(id, listing_id)
            .date(ymd(2024, 1, 16))
            .qty(quantity)
            .price(Decimal::from(100))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    async fn insert_sell_trade(pool: &SqlitePool, id: i64, listing_id: i64, quantity: Decimal) {
        test_support::sell(id, listing_id)
            .date(ymd(2024, 6, 3))
            .qty(quantity)
            .price(Decimal::from(120))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    async fn insert_amma(pool: &SqlitePool, id: i64, listing_id: i64, cost_base_adj: Decimal) {
        test_support::amma(id, listing_id)
            .cost_base_adjustment(cost_base_adj)
            .insert(pool)
            .await;
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let adj = AmitAdjustment {
            id: 1,
            amma_statement_id: 1,
            trade_id: 1,
            quantity: Decimal::from(100),
        };
        db_upsert(&pool, &adj).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amma_statement_id, 1);
        assert_eq!(got.trade_id, 1);
        assert_eq!(got.quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_sell_trade_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_sell_trade(&pool, 1, 1, Decimal::from(50)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let err = db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(50),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::TradeNotBuyOrDrp));
    }

    #[tokio::test]
    async fn db_listing_mismatch_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_test_listing(&pool, 2, "XASX", "VAS").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 2, "0.05".parse().unwrap()).await; // different listing

        let err = db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(50),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::ListingMismatch));
    }

    /// A statement only adjusts parcels in its own holding account: the same
    /// listing's parcel in another account is rejected (the registry issues
    /// one AMMA statement per holder account).
    #[tokio::test]
    async fn db_holding_account_mismatch_rejected() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
        )
        .await
        .unwrap();
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await; // default account

        let err = db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(50),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::HoldingAccountMismatch));

        // Re-pointing the statement at the parcel's account fixes it.
        sqlx::query("UPDATE amma_statements SET holding_account_id = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(50),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn db_quantity_exceeds_trade_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let err = db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(101),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::QuantityExceedsTrade));
    }

    /// One adjustment per (statement, parcel): a second row for the same
    /// parcel would apply the statement's per-unit figure to it twice, and
    /// E10's nil floor can turn that over-reduction into a capital gain that
    /// was never made. Re-saving the *same* row (same id) is still an update.
    #[tokio::test]
    async fn db_duplicate_parcel_on_one_statement_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        insert_amma(&pool, 2, 1, "0.03".parse().unwrap()).await;

        let first = AmitAdjustment {
            id: 1,
            amma_statement_id: 1,
            trade_id: 1,
            quantity: Decimal::from(100),
        };
        db_upsert(&pool, &first).await.unwrap();

        let err = db_upsert(
            &pool,
            &AmitAdjustment {
                id: 2,
                ..first.clone()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::DuplicateParcel));

        // Updating the existing row is not a duplicate of itself.
        db_upsert(
            &pool,
            &AmitAdjustment {
                quantity: Decimal::from(80),
                ..first.clone()
            },
        )
        .await
        .unwrap();
        // Nor is the same parcel on a *different* statement — each year's
        // statement adjusts every parcel it covers.
        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 2,
                amma_statement_id: 2,
                ..first
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn db_cost_base_reduction_calculation() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        // Two AMMA statements: $0.05/unit and $0.03/unit
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        insert_amma(&pool, 2, 1, "0.03".parse().unwrap()).await;

        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();
        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 2,
                amma_statement_id: 2,
                trade_id: 1,
                quantity: Decimal::from(80),
            },
        )
        .await
        .unwrap();

        let reductions = reductions(&pool).await;
        // 100 * 0.05 + 80 * 0.03 = 5.00 + 2.40 = 7.40
        assert_eq!(
            reductions.get(&1).copied(),
            Some("7.40".parse::<Decimal>().unwrap())
        );
    }

    /// SCENARIOS B-24: a share split between the parcel's acquisition and the
    /// statement's year end puts the two multiplicands on different unit
    /// bases. The stored `quantity` is as-acquired; the statement's per-unit
    /// figure is per unit *as the statement year saw them*, so the quantity is
    /// re-based to the year end before multiplying — 100 as-acquired units are
    /// 200 units at the year end, and the fund's 50c/unit is a $100 reduction,
    /// not $50.
    #[tokio::test]
    async fn db_cost_base_reduction_is_re_based_across_a_split() {
        use crate::entities::corporate_action::{ActionKind, CorporateAction};
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 1, 15),
                kind: ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        // FY2024 statement (year ended 30 June 2024, after the split).
        insert_amma(&pool, 1, 1, dec("0.50")).await;
        test_support::amit_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

        assert_eq!(reductions(&pool).await.get(&1).copied(), Some(dec("100")));

        // The itemised detail the E10 walk reads agrees, figure for figure —
        // and states it per as-acquired unit: $1 for each of the 100.
        let detail = events(&pool).await;
        assert_eq!(detail[&1].len(), 1);
        assert_eq!(detail[&1][0].per_unit, dec("1"));
        assert_eq!(detail[&1][0].covered, dec("100"));
        assert_eq!(detail[&1][0].amount(), dec("100"));

        // A parcel bought *after* the split is already on the year end's
        // basis, so nothing is re-based: 50 × $0.50 = $25.
        test_support::buy(2, 1)
            .date(ymd(2024, 3, 1))
            .qty(Decimal::from(50))
            .price(Decimal::from(5))
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 2, 1, 2, Decimal::from(50)).await;
        assert_eq!(reductions(&pool).await.get(&2).copied(), Some(dec("25")));
    }

    /// The cost base adjustment is driven solely by `cost_base_adjustment` (the per-unit
    /// AMIT cost base net amount), per ATO guidance (docs/ato/amit-cost-base-adjustments.md).
    /// `tax_deferred_amount` and `tax_free_amount` are informational-only and must NOT
    /// affect the reduction, even when large.
    #[tokio::test]
    async fn db_cost_base_reduction_ignores_tax_deferred_and_tax_free() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;

        // AMMA with cost_base_adjustment 0.05/unit but huge tax-deferred / tax-free lines.
        test_support::amma(1, 1)
            .cost_base_adjustment(dec("0.05"))
            .with(|a| {
                a.tax_deferred_amount = dec("999.99");
                a.tax_free_amount = dec("888.88");
            })
            .insert(&pool)
            .await;

        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();

        let reductions = reductions(&pool).await;
        // 100 * 0.05 = 5.00 — the tax-deferred/tax-free amounts are NOT added in.
        assert_eq!(
            reductions.get(&1).copied(),
            Some("5.00".parse::<Decimal>().unwrap())
        );
    }

    // API-level tests

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let body = serde_json::json!({
            "amma_statement_id": 1,
            "trade_id": 1,
            "quantity": "100"
        });
        let resp = client(&pool).put("/amit_adjustments/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amma_statement_id, 1);
        assert_eq!(got.trade_id, 1);
        assert_eq!(got.quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn api_sell_trade_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_sell_trade(&pool, 1, 1, Decimal::from(50)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let body = serde_json::json!({
            "amma_statement_id": 1,
            "trade_id": 1,
            "quantity": "50"
        });
        let resp = client(&pool).put("/amit_adjustments/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(detail.contains("not a Buy or DRP"), "detail: {detail}");
    }

    #[tokio::test]
    async fn api_listing_mismatch_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_test_listing(&pool, 2, "XASX", "VAS").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 2, "0.05".parse().unwrap()).await;

        let body = serde_json::json!({
            "amma_statement_id": 1,
            "trade_id": 1,
            "quantity": "50"
        });
        let resp = client(&pool).put("/amit_adjustments/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_duplicate_parcel_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let body = serde_json::json!({
            "amma_statement_id": 1,
            "trade_id": 1,
            "quantity": "100"
        });
        let c = client(&pool);
        assert_eq!(
            c.put("/amit_adjustments/1", &body).await.status,
            StatusCode::NO_CONTENT
        );
        let resp = c.put("/amit_adjustments/2", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(detail.contains("already has an adjustment"), "{detail}");
        assert!(db_get(&pool, 2).await.unwrap().is_none());
    }

    /// Move `quantity` units of parcel `trade_id` to holding account 2,
    /// returning the replacement parcel's id.
    async fn transfer_out(pool: &SqlitePool, trade_id: i64, quantity: Decimal) -> i64 {
        use crate::entities::holding_account::{self, HoldingAccount};
        use crate::entities::sell::AllocationInput;
        use crate::entities::transfer::{self, TransferBody};

        // Idempotent: several tests move a parcel into the same second account.
        holding_account::db_upsert(
            pool,
            &HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
        let group = transfer::db_transfer(
            pool,
            1,
            &TransferBody {
                listing_id: 1,
                date: ymd(2024, 5, 1),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: trade_id,
                    quantity_allocated: quantity,
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();
        group.transfer_ins[0].id
    }

    /// SCENARIOS F-17: a transfer carries the parcel's units — and their cost
    /// base — into a replacement parcel, computed and frozen when the
    /// transfer ran. An adjustment entered against the original afterwards
    /// would reach nothing at all, so it is refused rather than silently
    /// swallowed.
    #[tokio::test]
    async fn db_an_adjustment_on_a_parcel_a_rollover_closed_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VDHG").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        let replacement = transfer_out(&pool, 1, Decimal::from(100)).await;

        let err = db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap_err();
        match err {
            UpsertError::UnitsCarriedIntoReplacement {
                adjustable,
                replacements,
            } => {
                assert_eq!(adjustable, Decimal::ZERO);
                assert_eq!(replacements, vec![replacement]);
            }
            other => panic!("expected the rollover refusal, got {other:?}"),
        }
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// Only the units the rollover took are out of reach: a partial transfer
    /// leaves the rest adjustable, and the boundary is exact.
    #[tokio::test]
    async fn db_a_partial_rollover_leaves_the_units_it_did_not_take_adjustable() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VDHG").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        transfer_out(&pool, 1, Decimal::from(40)).await;

        let row = AmitAdjustment {
            id: 1,
            amma_statement_id: 1,
            trade_id: 1,
            quantity: Decimal::from(60),
        };
        db_upsert(&pool, &row).await.unwrap();

        let err = db_upsert(
            &pool,
            &AmitAdjustment {
                quantity: Decimal::from(61),
                ..row
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            UpsertError::UnitsCarriedIntoReplacement { adjustable, .. }
                if adjustable == Decimal::from(60)
        ));
    }

    /// The way out the refusal names, end to end: entering the adjustment
    /// *before* the transfer leaves the replacement parcel carrying the
    /// reduced cost base — which is why the refusal says to delete the
    /// operation, enter the row, and re-run it.
    #[tokio::test]
    async fn db_an_adjustment_entered_before_a_rollover_carries_into_the_replacement() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VDHG").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();
        let replacement = transfer_out(&pool, 1, Decimal::from(100)).await;

        // 100 × $100 + 9.95 + 0.995 = 10,010.945, less 100 × 5c.
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].trade_id, replacement);
        assert_eq!(
            parcels[0].remaining_cost_base,
            "10005.945".parse::<Decimal>().unwrap()
        );
    }

    /// An ordinary Sell is not a rollover: it is a real disposal, and a
    /// statement covering the units it sold reduces that sale's cost base
    /// (SCENARIOS F-04). The refusal must not reach it.
    #[tokio::test]
    async fn db_a_parcel_closed_by_an_ordinary_sell_stays_adjustable() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VDHG").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_sell_trade(&pool, 2, 1, Decimal::from(100)).await;
        crate::test_support::allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();
    }

    /// SCENARIOS N-06, the answer to the refusal below: the statement's units
    /// are now in the replacement parcel, in **another holding account**, and
    /// that is where its reduction belongs. Refusing it there (while F-17
    /// refuses the source parcel) left an AMMA statement for the year before a
    /// transfer recordable *nowhere* — the ordinary order of events, since a
    /// statement for a year ended 30 June arrives in spring. A parcel in an
    /// unrelated account is still refused: the rule is that the units trace back
    /// to the statement's account, not that any account will do.
    #[tokio::test]
    async fn db_a_replacement_parcel_in_another_account_is_adjustable() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VDHG").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        // The statement is account 1's; the transfer moves the units to 2.
        let replacement = transfer_out(&pool, 1, Decimal::from(100)).await;

        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: replacement,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();
        // And it reaches the units: $10,010.945 initial cost less $0.05 × 100.
        let parcels = crate::domain::open_parcels::load(&mut pool.acquire().await.unwrap(), None)
            .await
            .unwrap();
        let moved = parcels
            .iter()
            .find(|p| p.parcel.id == replacement)
            .expect("the replacement parcel is open");
        assert_eq!(moved.cost_base.amit_reduction, Decimal::from(5));

        // An unrelated parcel in account 2 — nothing to do with this statement's
        // account — is still refused.
        test_support::buy(50, 1)
            .date(ymd(2024, 2, 1))
            .qty(Decimal::from(10))
            .price(Decimal::from(100))
            .account(2)
            .insert(&pool)
            .await;
        let err = db_upsert(
            &pool,
            &AmitAdjustment {
                id: 2,
                amma_statement_id: 1,
                trade_id: 50,
                quantity: Decimal::from(10),
            },
        )
        .await
        .expect_err("a parcel that never held this account's units");
        assert!(
            matches!(err, UpsertError::HoldingAccountMismatch),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn api_rollover_replaced_parcel_returns_422_naming_the_replacement() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VDHG").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        let replacement = transfer_out(&pool, 1, Decimal::from(100)).await;

        let resp = client(&pool)
            .put(
                "/amit_adjustments/1",
                &serde_json::json!({
                    "amma_statement_id": 1,
                    "trade_id": 1,
                    "quantity": "100"
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(
            detail.contains(&format!("replacement parcel(s) #{replacement}")),
            "{detail}"
        );
        assert!(detail.contains("at most 0 unit(s)"), "{detail}");
        // The recovery the message names changed with SCENARIOS N-06: the row
        // now goes against the replacement parcel (which the account rule
        // accepts, since the units trace back to the statement's account), and
        // generating the statement's set does exactly that — rather than the old
        // advice to delete the operation and redo it.
        assert!(
            detail.contains("against the replacement parcel instead"),
            "{detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();

        let resp = client(&pool).get("/amit_adjustments").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<AmitAdjustment> = resp.json();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/amit_adjustments/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();

        let resp = client(&pool).delete("/amit_adjustments/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/amit_adjustments/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
