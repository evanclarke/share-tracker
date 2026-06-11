//! Atomic Sell + parcel-allocation creation.
//!
//! A Sell and the purchase parcels it consumes are created together in one
//! transaction so that an under- (or over-) allocated Sell can never be
//! persisted. This is the only write path for Sell trades and their
//! allocations — the standalone `parcel_allocations` write routes are disabled
//! (see `parcel_allocation::router`) so a partial state cannot be reintroduced
//! after the fact.
//!
//! `PUT /sells/{id}` is an upsert: it replaces the Sell trade row and *all* of
//! its parcel allocations with the submitted set.

use crate::entities::trade::{self, TradeType};
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::put,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::{Row, SqlitePool};

#[derive(Debug, Deserialize)]
pub struct AllocationInput {
    pub purchase_trade_id: i64,
    pub quantity_allocated: Decimal,
}

#[derive(Debug, Deserialize)]
pub struct SellBody {
    pub date: NaiveDate,
    #[serde(default)]
    pub settlement_date: Option<NaiveDate>,
    pub listing_id: i64,
    pub average_price: Decimal,
    pub quantity: Decimal,
    pub currency: String,
    /// GST-inclusive when `brokerage_includes_gst` is set (the server splits
    /// it; any supplied `gst_on_brokerage` is ignored), ex-GST otherwise —
    /// same contract as `trade::TradeBody`.
    pub brokerage: Decimal,
    #[serde(default)]
    pub gst_on_brokerage: Decimal,
    #[serde(default)]
    pub brokerage_includes_gst: bool,
    pub brokerage_currency: String,
    pub fx_rate: Decimal,
    #[serde(default)]
    pub contract_note_ref: Option<String>,
    /// Optional statement cross-check (net proceeds — quantity × price minus
    /// brokerage and GST); see `trade::Trade::statement_total`.
    #[serde(default)]
    pub statement_total: Option<Decimal>,
    /// The holding account the Sell happens in: its allocations may only
    /// consume parcels held in the same account. Defaults to the seeded
    /// default account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
    #[serde(default)]
    pub allocations: Vec<AllocationInput>,
}

#[derive(Debug)]
pub enum SellError {
    Db(sqlx::Error),
    /// Allocated quantities do not sum exactly to the sell quantity.
    AllocationMismatch,
    /// A referenced purchase trade does not exist.
    PurchaseParcelMissing,
    /// A referenced purchase trade is not a Buy or DRP.
    PurchaseTradeNotBuyOrDrp,
    /// Allocating these parcels would exceed a purchase parcel's quantity.
    PurchaseQuantityExceeded,
    /// The existing trade is a buy-back participation Sell
    /// (`buyback_action_id` set): its figures derive from the buy-back's
    /// terms and it carries a linked dividend-component income row, so
    /// free-form edits are rejected. Delete it (`DELETE /sells/:id`, which
    /// also removes the income row) and re-participate instead (see
    /// `entities::buyback_participation`).
    BuyBackSell,
    /// The existing trade is a scrip-for-scrip exchange closing Sell
    /// (`scrip_action_id` set): it consumes exactly the parcels the exchange
    /// substituted, so free-form edits are rejected. Delete it
    /// (`DELETE /sells/:id`, which also removes the replacement Buys) and
    /// re-exchange instead (see `entities::scrip_exchange`).
    ScripExchangeSell,
    /// The existing trade is a demerger closing Sell (`demerger_action_id`
    /// set): it consumes exactly the parcels the demerger apportioned, so
    /// free-form edits are rejected. Delete it (`DELETE /sells/:id`, which
    /// also removes the head and demerged-entity Buys) and re-demerge
    /// instead (see `entities::demerger`).
    DemergerSell,
    /// The existing trade is a holding-account transfer-out Sell
    /// (`transfer_id` set): it consumes exactly the parcels the transfer
    /// moved, so free-form edits (and `DELETE /sells`) are rejected. Delete
    /// the transfer (`DELETE /transfers/:id`, which also removes the
    /// transfer-in Buys) and re-transfer instead (see `entities::transfer`).
    TransferSell,
    /// An allocation consumes a parcel held in a different holding account
    /// from the Sell's: a sale can only dispose of units its account actually
    /// holds. Move the parcel first (see `entities::transfer`) or fix the
    /// Sell's holding account. (Scrip-for-scrip exchange and demerger closing
    /// Sells are exempt — they mechanically close the *whole* holding across
    /// every account, and their replacement Buys stay in each consumed
    /// parcel's account.)
    PurchaseInDifferentAccount,
    /// The existing trade is a worthless-shares recognise closing Sell
    /// (`worthless_action_id` set): it consumes exactly the parcels the
    /// recognise operation closed at nil proceeds, so free-form edits are
    /// rejected. Delete it (`DELETE /sells/:id`, which restores the holding)
    /// and re-recognise instead (see `entities::worthless`).
    WorthlessSell,
    /// A supplied statement total failed the cross-check (see
    /// `trade::check_statement_total`): for a Sell it must equal the net
    /// proceeds, quantity × price − brokerage − GST.
    StatementTotal(trade::StatementTotalError),
}

impl From<sqlx::Error> for SellError {
    fn from(e: sqlx::Error) -> Self {
        SellError::Db(e)
    }
}

impl From<SellError> for ApiError {
    fn from(e: SellError) -> Self {
        match e {
            SellError::AllocationMismatch => {
                ApiError::unprocessable("the parcel allocations do not sum to the sell quantity")
            }
            SellError::PurchaseParcelMissing => {
                ApiError::unprocessable("an allocated purchase parcel does not exist")
            }
            SellError::PurchaseTradeNotBuyOrDrp => {
                ApiError::unprocessable("an allocated parcel is not a Buy or DRP trade")
            }
            SellError::PurchaseQuantityExceeded => ApiError::unprocessable(
                "the allocations exceed a purchase parcel's available quantity",
            ),
            SellError::BuyBackSell => ApiError::unprocessable(
                "this Sell is a buy-back participation and cannot be edited — \
                 delete it and re-participate instead",
            ),
            SellError::ScripExchangeSell => ApiError::unprocessable(
                "this Sell is a scrip-for-scrip exchange closing Sell and cannot be edited — \
                 delete it and re-exchange instead",
            ),
            SellError::DemergerSell => ApiError::unprocessable(
                "this Sell is a demerger closing Sell and cannot be edited — \
                 delete it and re-demerge instead",
            ),
            SellError::TransferSell => ApiError::unprocessable(
                "this Sell is a holding-account transfer-out and cannot be edited — \
                 delete the transfer and re-transfer instead",
            ),
            SellError::WorthlessSell => ApiError::unprocessable(
                "this Sell is a worthless-shares recognise closing Sell and cannot be edited — \
                 delete it and re-recognise instead",
            ),
            SellError::PurchaseInDifferentAccount => ApiError::unprocessable(
                "an allocated parcel is held in a different holding account from the Sell — \
                 transfer it first or fix the Sell's account",
            ),
            // The cross-check rejection says what the Sell nets to, so a typo
            // is findable without re-deriving the figure by hand.
            SellError::StatementTotal(detail) => {
                ApiError::unprocessable(trade::statement_total_detail(&detail))
            }
            SellError::Db(err) => err.into(),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/sells/{id}", put(upsert).delete(delete))
}

/// Outcome of a delete request, so the handler can map to the right status.
#[derive(Debug, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    NotFound,
    /// The id refers to a trade that is not a Sell — deletion is refused so a
    /// Buy/DRP parcel can't be removed through the sells endpoint.
    NotASell,
    /// The Sell is a scrip-for-scrip exchange or demerger closing Sell whose
    /// replacement Buys are themselves consumed by later allocations or AMIT
    /// adjustments — deleting the group would orphan those dependants. Remove
    /// them first (mapped to 422).
    ReplacementReferenced,
    /// The Sell is a holding-account transfer-out Sell: its group (and the
    /// transfer record itself) is deleted as a whole via
    /// `DELETE /transfers/:id`, not here (mapped to 422).
    TransferSell,
}

/// The columns [`db_delete_sell`] inspects to classify the target row before
/// deleting it. Mapped by column name via `FromRow`.
#[derive(sqlx::FromRow)]
struct SellDeleteRow {
    trade_type: TradeType,
    scrip_action_id: Option<i64>,
    demerger_action_id: Option<i64>,
    transfer_id: Option<i64>,
}

/// The provenance links a trade may carry that make it an immutable
/// action-derived Sell. Read by [`db_upsert_sell`] and mapped by column name
/// via `FromRow`.
#[derive(sqlx::FromRow)]
struct SellProvenance {
    buyback_action_id: Option<i64>,
    scrip_action_id: Option<i64>,
    demerger_action_id: Option<i64>,
    transfer_id: Option<i64>,
    worthless_action_id: Option<i64>,
}

/// Whether the trade `id` is a holding-account transfer's network-fee disposal
/// Sell (linked from `transfers.fee_sale_trade_id`). Such a Sell is part of its
/// transfer group: it is created and removed only via the transfer, never
/// edited or deleted on its own here.
async fn is_transfer_fee_sale(
    tx: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM transfers WHERE fee_sale_trade_id = ?)")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
}

/// Delete a Sell trade and all of its parcel allocations in one transaction,
/// freeing the purchase parcels those allocations consumed. A buy-back
/// participation Sell takes its linked dividend-component income row
/// (`income.buyback_trade_id`) with it, so the capital and dividend sides of
/// the participation are always created and removed together. A
/// scrip-for-scrip exchange or demerger closing Sell takes its group's
/// replacement Buys (`trades.scrip_action_id` / `trades.demerger_action_id`)
/// with it — the group substitutes the same parcels, so it only ever exists
/// as a whole — unless a replacement Buy is itself consumed by later
/// allocations or AMIT adjustments (refused; remove those dependants first).
pub async fn db_delete_sell(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let row: Option<SellDeleteRow> = sqlx::query_as(
        "SELECT trade_type, scrip_action_id, demerger_action_id, transfer_id \
         FROM trades WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let (scrip_action, demerger_action) = match row {
        None => return Ok(DeleteOutcome::NotFound),
        Some(r) if r.trade_type != TradeType::Sell => return Ok(DeleteOutcome::NotASell),
        // A transfer-out Sell only ever goes with its whole group, via
        // DELETE /transfers/:id (which also removes the transfer record).
        Some(r) if r.transfer_id.is_some() => return Ok(DeleteOutcome::TransferSell),
        Some(r) => (r.scrip_action_id, r.demerger_action_id),
    };
    // A transfer's network-fee disposal Sell is likewise part of its transfer
    // group (linked via transfers.fee_sale_trade_id, not transfer_id, so it
    // stays in the gains reports) — it too is removed via DELETE /transfers.
    if is_transfer_fee_sale(&mut tx, id).await? {
        return Ok(DeleteOutcome::TransferSell);
    }

    let group = scrip_action
        .map(|a| ("scrip_action_id", a))
        .or(demerger_action.map(|a| ("demerger_action_id", a)));
    if let Some((column, action_id)) = group {
        // The replacement Buys go with the closing Sell — but not while
        // anything still draws on them.
        let replacement_referenced: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS(\
                 SELECT 1 FROM trades t \
                 WHERE t.{column} = ?1 AND t.id <> ?2 \
                   AND (EXISTS(SELECT 1 FROM parcel_allocations \
                               WHERE purchase_trade_id = t.id OR sale_trade_id = t.id) \
                     OR EXISTS(SELECT 1 FROM amit_adjustments WHERE trade_id = t.id) \
                     OR EXISTS(SELECT 1 FROM income \
                               WHERE reinvestment_trade_id = t.id OR buyback_trade_id = t.id)))",
        ))
        .bind(action_id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if replacement_referenced {
            return Ok(DeleteOutcome::ReplacementReferenced);
        }
        sqlx::query(&format!(
            "DELETE FROM trades WHERE {column} = ? AND id <> ?"
        ))
        .bind(action_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("DELETE FROM income WHERE buyback_trade_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM parcel_allocations WHERE sale_trade_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM trades WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

/// Create or replace a Sell trade together with its full set of parcel
/// allocations, atomically. Returns an error (mapped to 422) unless the
/// allocations sum exactly to the sell quantity and every parcel is a valid,
/// not-over-allocated Buy/DRP. A buy-back participation Sell is immutable
/// here — delete it via `DELETE /sells/:id` and re-participate instead.
pub async fn db_upsert_sell(pool: &SqlitePool, id: i64, body: &SellBody) -> Result<(), SellError> {
    // Reference data (exchanges/listings) is not touched here, so resolving the
    // settlement date outside the write transaction is a consistent read.
    let settlement_date = match body.settlement_date {
        Some(d) => d,
        None => trade::auto_settlement_date(pool, id, body.listing_id, body.date).await?,
    };

    let mut tx = pool.begin().await?;

    // A buy-back participation, scrip-for-scrip exchange, or demerger Sell is
    // immutable here: its figures derive from its action's terms (and it
    // carries linked rows — a dividend income row, or the group's replacement
    // Buys). The upsert below never sets any provenance column, so a normal
    // Sell can't become one either.
    let existing: Option<SellProvenance> = sqlx::query_as(
        "SELECT buyback_action_id, scrip_action_id, demerger_action_id, transfer_id, \
                worthless_action_id \
         FROM trades WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(p) = existing {
        if p.buyback_action_id.is_some() {
            return Err(SellError::BuyBackSell);
        }
        if p.scrip_action_id.is_some() {
            return Err(SellError::ScripExchangeSell);
        }
        if p.demerger_action_id.is_some() {
            return Err(SellError::DemergerSell);
        }
        if p.transfer_id.is_some() {
            return Err(SellError::TransferSell);
        }
        if p.worthless_action_id.is_some() {
            return Err(SellError::WorthlessSell);
        }
    }
    // A transfer's network-fee disposal Sell carries its provenance on the
    // transfer (transfers.fee_sale_trade_id), not on the trade row, so it is
    // guarded separately — it is immutable here, like the transfer-out Sell.
    if is_transfer_fee_sale(&mut tx, id).await? {
        return Err(SellError::TransferSell);
    }

    upsert_sell_in_tx(
        &mut tx,
        id,
        body,
        settlement_date,
        None,
        None,
        None,
        None,
        None,
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// The transactional core of a Sell upsert: write the Sell trade row and
/// replace its full allocation set, validating every write-time invariant,
/// on the caller's transaction. Shared by `db_upsert_sell`, the buy-back
/// participation (`entities::buyback_participation`) — which writes the Sell
/// and the dividend-component income row in one transaction and stamps the
/// trade with its `buyback_action_id` provenance — the scrip-for-scrip
/// exchange (`entities::scrip_exchange`), which stamps its closing Sell with
/// `scrip_action_id`, the demerger (`entities::demerger`), which stamps its
/// closing Sell with `demerger_action_id`, and the holding-account transfer
/// (`entities::transfer`), which stamps its transfer-out Sell with
/// `transfer_id`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn upsert_sell_in_tx(
    tx: &mut sqlx::SqliteConnection,
    id: i64,
    body: &SellBody,
    settlement_date: NaiveDate,
    buyback_action_id: Option<i64>,
    scrip_action_id: Option<i64>,
    demerger_action_id: Option<i64>,
    transfer_id: Option<i64>,
    worthless_action_id: Option<i64>,
) -> Result<(), SellError> {
    // Allocations must account for the whole sale — no more, no less.
    let allocated: Decimal = body.allocations.iter().map(|a| a.quantity_allocated).sum();
    if allocated != body.quantity {
        return Err(SellError::AllocationMismatch);
    }

    // A GST-inclusive brokerage entry is split here (the operations that
    // build a SellBody internally pass flag false, making this the identity),
    // and the statement total — when recorded — must reconcile with the net
    // proceeds before anything is written.
    let (brokerage, gst_on_brokerage) = trade::resolve_brokerage(
        body.brokerage_includes_gst,
        body.brokerage,
        body.gst_on_brokerage,
    );
    trade::check_statement_total(trade::StatementTotalCheck {
        statement_total: body.statement_total,
        trade_type: TradeType::Sell,
        quantity: body.quantity,
        average_price: body.average_price,
        brokerage,
        gst_on_brokerage,
        currency: &body.currency,
        brokerage_currency: &body.brokerage_currency,
    })
    .map_err(SellError::StatementTotal)?;

    // Upsert the Sell trade row.
    sqlx::query(
        "INSERT INTO trades \
         (id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
          currency, brokerage, gst_on_brokerage, brokerage_includes_gst, brokerage_currency, \
          fx_rate, contract_note_ref, statement_total, \
          buyback_action_id, scrip_action_id, demerger_action_id, holding_account_id, transfer_id, \
          worthless_action_id) \
         VALUES (?, 'Sell', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             trade_type             = 'Sell', \
             date                   = excluded.date, \
             settlement_date        = excluded.settlement_date, \
             listing_id             = excluded.listing_id, \
             average_price          = excluded.average_price, \
             quantity               = excluded.quantity, \
             currency               = excluded.currency, \
             brokerage              = excluded.brokerage, \
             gst_on_brokerage       = excluded.gst_on_brokerage, \
             brokerage_includes_gst = excluded.brokerage_includes_gst, \
             brokerage_currency     = excluded.brokerage_currency, \
             fx_rate                = excluded.fx_rate, \
             contract_note_ref      = excluded.contract_note_ref, \
             statement_total        = excluded.statement_total, \
             buyback_action_id      = excluded.buyback_action_id, \
             scrip_action_id        = excluded.scrip_action_id, \
             demerger_action_id     = excluded.demerger_action_id, \
             holding_account_id     = excluded.holding_account_id, \
             transfer_id            = excluded.transfer_id, \
             worthless_action_id    = excluded.worthless_action_id",
    )
    .bind(id)
    .bind(body.date)
    .bind(settlement_date)
    .bind(body.listing_id)
    .bind(body.average_price.to_string())
    .bind(body.quantity.to_string())
    .bind(&body.currency)
    .bind(brokerage.to_string())
    .bind(gst_on_brokerage.to_string())
    .bind(body.brokerage_includes_gst)
    .bind(&body.brokerage_currency)
    .bind(body.fx_rate.to_string())
    .bind(&body.contract_note_ref)
    .bind(body.statement_total.map(|d| d.to_string()))
    .bind(buyback_action_id)
    .bind(scrip_action_id)
    .bind(demerger_action_id)
    .bind(body.holding_account_id)
    .bind(transfer_id)
    .bind(worthless_action_id)
    .execute(&mut *tx)
    .await?;

    // Replace this sale's allocations wholesale.
    sqlx::query("DELETE FROM parcel_allocations WHERE sale_trade_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    for alloc in &body.allocations {
        // Purchase parcel must exist and be a Buy/DRP.
        let purchase_row = sqlx::query(
            "SELECT trade_type, date, listing_id, quantity, holding_account_id \
             FROM trades WHERE id = ?",
        )
        .bind(alloc.purchase_trade_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(purchase_row) = purchase_row else {
            return Err(SellError::PurchaseParcelMissing);
        };
        let purchase_type: TradeType = purchase_row.try_get("trade_type")?;
        if !matches!(purchase_type, TradeType::Buy | TradeType::DRP) {
            return Err(SellError::PurchaseTradeNotBuyOrDrp);
        }
        // A sale can only dispose of units its holding account actually
        // holds. The scrip-for-scrip / demerger / worthless-shares closing Sell
        // is exempt: it mechanically closes the whole holding across every
        // account (the rollover replacement Buys stay in each consumed parcel's
        // account; a worthless recognise has no replacements — the loss rows
        // identify the closing Sell's account, totals unchanged).
        let purchase_account: i64 = purchase_row.try_get("holding_account_id")?;
        if scrip_action_id.is_none()
            && demerger_action_id.is_none()
            && worthless_action_id.is_none()
            && purchase_account != body.holding_account_id
        {
            return Err(SellError::PurchaseInDifferentAccount);
        }
        let purchase_date: NaiveDate = purchase_row.try_get("date")?;
        let purchase_listing: i64 = purchase_row.try_get("listing_id")?;
        let purchase_qty: Decimal = purchase_row
            .try_get::<String, _>("quantity")?
            .parse()
            .map_err(|_| SellError::Db(sqlx::Error::Decode("invalid purchase quantity".into())))?;

        sqlx::query(
            "INSERT INTO parcel_allocations (sale_trade_id, purchase_trade_id, quantity_allocated) \
             VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(alloc.purchase_trade_id)
        .bind(alloc.quantity_allocated.to_string())
        .execute(&mut *tx)
        .await?;

        // After inserting, the total allocated against this parcel (across all
        // sales) must not exceed the parcel's quantity. The parcel's quantity
        // is in as-acquired units while each allocation is in its own sale
        // date's units, so allocations are re-based back across any share
        // splits/consolidations (TD 2000/10) before comparing — e.g. after a
        // 2-for-1 split a 100-share parcel covers a 200-share sale.
        let splits =
            crate::entities::corporate_action::db_splits_for_listing(&mut *tx, purchase_listing)
                .await?;
        let rows = sqlx::query(
            "SELECT pa.quantity_allocated, s.date AS sale_date \
             FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
             WHERE pa.purchase_trade_id = ?",
        )
        .bind(alloc.purchase_trade_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut total = Decimal::ZERO;
        for row in &rows {
            let qty: Decimal = row
                .try_get::<String, _>("quantity_allocated")?
                .parse()
                .map_err(|_| {
                    SellError::Db(sqlx::Error::Decode("invalid allocated quantity".into()))
                })?;
            let sale_date: NaiveDate = row.try_get("sale_date")?;
            total += crate::entities::corporate_action::as_acquired_quantity(
                qty,
                &splits,
                purchase_date,
                sale_date,
            );
        }
        if total > purchase_qty {
            return Err(SellError::PurchaseQuantityExceeded);
        }
    }

    Ok(())
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<SellBody>,
) -> Result<StatusCode, ApiError> {
    db_upsert_sell(&pool, id, &body).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match db_delete_sell(&pool, id).await? {
        DeleteOutcome::Deleted => Ok(StatusCode::NO_CONTENT),
        DeleteOutcome::NotFound => Err(ApiError::not_found("no sell with that id")),
        DeleteOutcome::NotASell => Err(ApiError::unprocessable(
            "that trade is not a Sell — a Buy/DRP parcel cannot be deleted through this endpoint",
        )),
        DeleteOutcome::ReplacementReferenced => Err(ApiError::unprocessable(
            "a replacement parcel created by this exchange/demerger is consumed by a later \
             sale or AMIT adjustment — remove those first",
        )),
        DeleteOutcome::TransferSell => Err(ApiError::unprocessable(
            "this Sell belongs to a holding-account transfer — delete the transfer instead",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, test_pool};
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        test_support::listing(id).insert(pool).await;
    }

    async fn insert_buy(pool: &SqlitePool, id: i64, listing_id: i64, qty: Decimal) {
        test_support::buy(id, listing_id)
            .qty(qty)
            .insert(pool)
            .await;
    }

    fn sell_body(qty: Decimal, allocations: Vec<AllocationInput>) -> SellBody {
        SellBody {
            brokerage_includes_gst: false,
            statement_total: None,
            holding_account_id: 1,
            date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
            settlement_date: None,
            listing_id: 1,
            average_price: Decimal::from(15),
            quantity: qty,
            currency: "AUD".to_string(),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            brokerage_currency: "AUD".to_string(),
            fx_rate: Decimal::ONE,
            contract_note_ref: None,
            allocations,
        }
    }

    async fn count_allocations(pool: &SqlitePool, sale_id: i64) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations WHERE sale_trade_id = ?")
            .bind(sale_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn trade_exists(pool: &SqlitePool, id: i64) -> bool {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
        n > 0
    }

    #[tokio::test]
    async fn db_fully_allocated_sell_is_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(100),
            }],
        );
        db_upsert_sell(&pool, 2, &body).await.unwrap();

        assert!(trade_exists(&pool, 2).await);
        assert_eq!(count_allocations(&pool, 2).await, 1);
    }

    #[tokio::test]
    async fn db_under_allocated_sell_is_rejected_and_rolled_back() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        // sell 100 but only allocate 60
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(60),
            }],
        );
        let err = db_upsert_sell(&pool, 2, &body).await.unwrap_err();
        assert!(matches!(err, SellError::AllocationMismatch));
        // nothing persisted
        assert!(!trade_exists(&pool, 2).await);
        assert_eq!(count_allocations(&pool, 2).await, 0);
    }

    #[tokio::test]
    async fn db_over_allocated_sell_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        // allocations sum to 120 but sell quantity is 100
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(120),
            }],
        );
        let err = db_upsert_sell(&pool, 2, &body).await.unwrap_err();
        assert!(matches!(err, SellError::AllocationMismatch));
    }

    #[tokio::test]
    async fn db_allocation_exceeding_parcel_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(50)).await; // only 50 available

        // sell 100, fully allocated against a 50-unit parcel -> exceeds parcel
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(100),
            }],
        );
        let err = db_upsert_sell(&pool, 2, &body).await.unwrap_err();
        assert!(matches!(err, SellError::PurchaseQuantityExceeded));
        assert!(!trade_exists(&pool, 2).await);
    }

    #[tokio::test]
    async fn db_allocation_against_non_buy_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        // trade 3 is itself a Sell (created via this endpoint)
        let prior = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(100),
            }],
        );
        db_upsert_sell(&pool, 3, &prior).await.unwrap();

        // now try to allocate a new sell against the Sell trade 3
        let body = sell_body(
            Decimal::from(10),
            vec![AllocationInput {
                purchase_trade_id: 3,
                quantity_allocated: Decimal::from(10),
            }],
        );
        let err = db_upsert_sell(&pool, 4, &body).await.unwrap_err();
        assert!(matches!(err, SellError::PurchaseTradeNotBuyOrDrp));
    }

    /// A Sell may only dispose of parcels its own holding account holds: an
    /// allocation against another account's parcel is rejected (and rolled
    /// back), while the same Sell entered in the parcel's account succeeds.
    #[tokio::test]
    async fn db_allocation_in_different_account_is_rejected() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
        )
        .await
        .unwrap();
        // The parcel sits in the employer-plan account.
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        // A default-account Sell can't consume it…
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(100),
            }],
        );
        let err = db_upsert_sell(&pool, 2, &body).await.unwrap_err();
        assert!(matches!(err, SellError::PurchaseInDifferentAccount));
        assert!(!trade_exists(&pool, 2).await);

        // …while the same Sell in the plan account goes through.
        let mut body = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(100),
            }],
        );
        body.holding_account_id = 2;
        db_upsert_sell(&pool, 2, &body).await.unwrap();
        assert!(trade_exists(&pool, 2).await);
    }

    #[tokio::test]
    async fn api_allocation_in_different_account_returns_422() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
        )
        .await
        .unwrap();
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        // holding_account_id omitted → defaults to the default account, which
        // doesn't hold the parcel.
        let body = serde_json::json!({
            "date": "2024-06-03",
            "listing_id": 1,
            "average_price": "15",
            "quantity": "100",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "100" } ]
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/sells/2")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    async fn apply_split(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate) {
        use crate::entities::corporate_action;
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id,
                date,
                kind: corporate_action::ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
    }

    /// After a 2-for-1 split (TD 2000/10) a 100-share parcel covers a 200-share
    /// post-split sale: the allocation is in sale-date units and is re-based
    /// back to as-acquired units for the capacity check.
    #[tokio::test]
    async fn db_post_split_sell_allocates_against_pre_split_parcel() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await; // 2024-01-01
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap()).await;

        // One more post-split unit than the parcel converts to is rejected
        // (and rolled back)…
        let body = sell_body(
            Decimal::from(201),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(201),
            }],
        );
        let err = db_upsert_sell(&pool, 2, &body).await.unwrap_err();
        assert!(matches!(err, SellError::PurchaseQuantityExceeded));
        assert!(!trade_exists(&pool, 2).await);

        // …while selling exactly the 200 post-split units succeeds.
        let body = sell_body(
            Decimal::from(200),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(200),
            }],
        );
        db_upsert_sell(&pool, 2, &body).await.unwrap();
        assert!(trade_exists(&pool, 2).await);
    }

    #[tokio::test]
    async fn db_upsert_replaces_previous_allocations() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        insert_buy(&pool, 2, 1, Decimal::from(100)).await;

        // first version: 100 from parcel 1
        let v1 = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(100),
            }],
        );
        db_upsert_sell(&pool, 3, &v1).await.unwrap();
        assert_eq!(count_allocations(&pool, 3).await, 1);

        // revised: split across two parcels — old allocation should be gone
        let v2 = sell_body(
            Decimal::from(100),
            vec![
                AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: Decimal::from(40),
                },
                AllocationInput {
                    purchase_trade_id: 2,
                    quantity_allocated: Decimal::from(60),
                },
            ],
        );
        db_upsert_sell(&pool, 3, &v2).await.unwrap();
        assert_eq!(count_allocations(&pool, 3).await, 2);
    }

    #[tokio::test]
    async fn api_fully_allocated_sell_returns_204() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        let body = serde_json::json!({
            "date": "2024-06-03",
            "listing_id": 1,
            "average_price": "15",
            "quantity": "100",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "100" } ]
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/sells/2")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn db_delete_removes_sell_and_allocations() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(100),
            }],
        );
        db_upsert_sell(&pool, 2, &body).await.unwrap();
        assert_eq!(count_allocations(&pool, 2).await, 1);

        let outcome = db_delete_sell(&pool, 2).await.unwrap();
        assert_eq!(outcome, DeleteOutcome::Deleted);
        assert!(!trade_exists(&pool, 2).await);
        assert_eq!(count_allocations(&pool, 2).await, 0);
        // the purchase parcel itself is untouched
        assert!(trade_exists(&pool, 1).await);
    }

    #[tokio::test]
    async fn db_delete_missing_sell_is_not_found() {
        let pool = test_pool().await;
        assert_eq!(
            db_delete_sell(&pool, 99).await.unwrap(),
            DeleteOutcome::NotFound
        );
    }

    #[tokio::test]
    async fn db_delete_non_sell_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        // trade 1 is a Buy — deleting it via the sells endpoint is refused
        assert_eq!(
            db_delete_sell(&pool, 1).await.unwrap(),
            DeleteOutcome::NotASell
        );
        assert!(trade_exists(&pool, 1).await);
    }

    #[tokio::test]
    async fn api_delete_sell_returns_204_then_404() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(100),
            }],
        );
        db_upsert_sell(&pool, 2, &body).await.unwrap();

        let app = router().with_state(pool.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sells/2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // second delete: already gone
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sells/2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// A Sell's statement total is the *net proceeds* — quantity × price −
    /// brokerage − GST — and a GST-inclusive brokerage entry is split on the
    /// Sell path too: 100 × 15 − 9.95 (incl.) = 1490.05. A mismatched total
    /// is rejected (422 with the computed figure) and nothing persisted.
    #[tokio::test]
    async fn db_sell_statement_total_checks_net_proceeds_and_gst_splits() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        let mut body = sell_body(
            Decimal::from(100),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(100),
            }],
        );
        body.brokerage = "9.95".parse().unwrap();
        body.brokerage_includes_gst = true;
        body.statement_total = Some("1490.05".parse().unwrap());
        db_upsert_sell(&pool, 2, &body).await.unwrap();

        let t = trade::db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(t.brokerage, "9.05".parse::<Decimal>().unwrap());
        assert_eq!(t.gst_on_brokerage, "0.90".parse::<Decimal>().unwrap());
        assert!(t.brokerage_includes_gst);
        assert_eq!(t.statement_total, Some("1490.05".parse().unwrap()));

        // The Buy-direction figure (proceeds *plus* costs) must not pass.
        body.statement_total = Some("1509.95".parse().unwrap());
        let err = db_upsert_sell(&pool, 3, &body).await.unwrap_err();
        assert!(matches!(
            err,
            SellError::StatementTotal(trade::StatementTotalError::TotalMismatch { .. })
        ));
        assert!(!trade_exists(&pool, 3).await);
    }

    /// The cent-rounding tolerance applies to a Sell's net proceeds too:
    /// 33 × 14.906273 − 9.95 = 481.957009, so a contract note printing
    /// 481.96 is accepted; a whole cent off (481.95 — what truncation
    /// would give) still rejects and persists nothing.
    #[tokio::test]
    async fn db_sell_statement_total_accepts_cent_rounded_net_proceeds() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        let mut body = sell_body(
            "33".parse().unwrap(),
            vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: "33".parse().unwrap(),
            }],
        );
        body.average_price = "14.906273".parse().unwrap();
        body.brokerage = "9.05".parse().unwrap();
        body.gst_on_brokerage = "0.90".parse().unwrap();
        body.statement_total = Some("481.96".parse().unwrap());
        db_upsert_sell(&pool, 2, &body).await.unwrap();
        assert_eq!(
            trade::db_get(&pool, 2)
                .await
                .unwrap()
                .unwrap()
                .statement_total,
            Some("481.96".parse().unwrap())
        );

        body.statement_total = Some("481.95".parse().unwrap());
        let err = db_upsert_sell(&pool, 3, &body).await.unwrap_err();
        assert!(matches!(
            err,
            SellError::StatementTotal(trade::StatementTotalError::TotalMismatch { .. })
        ));
        assert!(!trade_exists(&pool, 3).await);
    }

    #[tokio::test]
    async fn api_sell_statement_total_mismatch_returns_422_with_detail() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        let body = serde_json::json!({
            "date": "2024-06-03",
            "listing_id": 1,
            "average_price": "15",
            "quantity": "100",
            "currency": "AUD",
            "brokerage": "9.95",
            "brokerage_includes_gst": true,
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "statement_total": "1500",
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "100" } ]
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/sells/2")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes();
        let detail = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            detail.contains("1490.05"),
            "detail must carry the net proceeds: {detail}"
        );
    }

    #[tokio::test]
    async fn api_under_allocated_sell_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        let body = serde_json::json!({
            "date": "2024-06-03",
            "listing_id": 1,
            "average_price": "15",
            "quantity": "100",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "60" } ]
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/sells/2")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes();
        let detail = String::from_utf8(body.to_vec()).unwrap();
        // The rejection says why, not a bare "HTTP 422".
        assert!(
            detail.contains("sum to the sell quantity"),
            "detail: {detail}"
        );
    }
}
