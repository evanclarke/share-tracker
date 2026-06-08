//! Atomic transfer of a holding between two holding accounts of the same
//! owner — e.g. moving vested RSU shares from the employer share-plan account
//! to the holder's own broker account (REQUIREMENTS "Holding accounts").
//!
//! A transfer is **not a CGT event**: the same beneficial owner holds the
//! shares before and after, so nothing is disposed of and no gain or loss
//! arises. `PUT /transfers/:id` records the transfer (date, listing, source
//! and destination accounts, per-parcel quantities) and executes it in one
//! transaction, mirroring the scrip-for-scrip mechanics:
//!
//! - a **transfer-out Sell** in the source account dated the transfer date —
//!   price 0, consuming the chosen quantity from each source parcel via
//!   parcel allocations, written through the shared `/sells` core so all its
//!   invariants (full allocation, parcel capacity, same-account) hold. It
//!   carries `transfer_id`, which excludes it from the realised-gains and
//!   net-capital-gain reports (no disposal happened) and from the franking
//!   at-risk stack (beneficial ownership is unchanged), and
//! - one **transfer-in Buy** per consumed parcel in the destination account,
//!   dated the transfer date, with the transferred quantity (in transfer-date
//!   units). The parcel's remaining reduced cost base (AMIT- and
//!   return-of-capital-adjusted, floored at nil), pro-rated for a partial
//!   transfer, is carried on the `brokerage` column with a zero price, and
//!   the parcel's acquisition date (chaining through any earlier rollover or
//!   transfer) is carried as `deemed_acquisition_date` — the 12-month
//!   discount clock and the AUD translation month of the cost base are
//!   unchanged by the move. The trade's `currency` and manual `fx_rate`
//!   fallback also carry over.
//!
//! The created trades form the transfer group (`trades.transfer_id`): each is
//! rejected by `PUT /sells`, `PUT`/`DELETE /trades`, and `DELETE /sells`;
//! `DELETE /transfers/:id` removes the whole group together with the transfer
//! record, restoring the pre-transfer holding (refused while a transfer-in
//! Buy is consumed by later allocations, AMIT adjustments, or income links).
//! A recorded transfer is immutable — delete it and re-transfer instead.

use crate::entities::corporate_action::{
    self, RocEvent, as_acquired_quantity, per_unit_reduction,
};
use crate::entities::sell::{self, AllocationInput, SellBody};
use crate::entities::trade::{self, Trade};
use crate::infra::decimal::parse_dec;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Transfer {
    pub id: i64,
    pub listing_id: i64,
    /// The date the holding moves: the transfer-out Sell and transfer-in
    /// Buys are dated on it.
    pub date: NaiveDate,
    pub from_account_id: i64,
    pub to_account_id: i64,
    /// The id of the network-fee disposal Sell, when the transfer incurred a
    /// fee paid in the crypto (NULL otherwise). Unlike the transfer-out Sell,
    /// this Sell is a real disposal and IS counted by the gains reports — it is
    /// linked here (not via `trades.transfer_id`) so it round-trips with the
    /// transfer yet stays visible to those reports.
    pub fee_sale_trade_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct TransferBody {
    pub listing_id: i64,
    pub date: NaiveDate,
    pub from_account_id: i64,
    pub to_account_id: i64,
    /// The parcels to move and how many units of each (in transfer-date
    /// units, like a Sell's allocations) — partial parcels allowed.
    pub allocations: Vec<AllocationInput>,
    /// Optional on-chain network fee paid in the transferred crypto: the source
    /// parcels (and unit counts) consumed to cover the fee. Empty/absent means
    /// no fee. These units are **disposed of** — a CGT event — not moved, so
    /// they are not transferred in; the disposal Sell flows through the gains
    /// reports. Must belong to the transfer's listing and the source account.
    #[serde(default)]
    pub fee_allocations: Vec<AllocationInput>,
    /// The fee crypto's market value per unit at the transfer date, in the
    /// listing's currency (AUD for an AUD-priced crypto) — the disposal's
    /// capital proceeds per unit. Required when `fee_allocations` is non-empty.
    #[serde(default)]
    pub fee_market_price: Option<Decimal>,
    /// The fee disposal's manual AUD fallback FX rate for a non-AUD-priced
    /// crypto listing (defaults to 1, the AUD case).
    #[serde(default)]
    pub fee_fx_rate: Option<Decimal>,
}

/// The two sides of an executed transfer: the transfer-out Sell in the source
/// account and the transfer-in Buys it re-created in the destination (one per
/// consumed parcel, in allocation order).
#[derive(Debug, Serialize)]
pub struct TransferGroup {
    pub transfer: Transfer,
    pub sell: Trade,
    pub transfer_ins: Vec<Trade>,
    /// The network-fee disposal Sell, when the transfer incurred a fee
    /// (`None` otherwise).
    pub fee_sale: Option<Trade>,
}

#[derive(Debug)]
pub enum TransferError {
    Db(sqlx::Error),
    /// Source and destination accounts are the same — nothing to move.
    SameAccount,
    /// A transfer is recorded and executed in one step and is immutable:
    /// delete it (`DELETE /transfers/:id`) and re-transfer instead.
    AlreadyExists,
    /// No allocations — a transfer must move at least one parcel.
    NothingToTransfer,
    /// A referenced parcel (moved or fee) does not belong to the transfer's
    /// listing.
    ListingMismatch,
    /// A network fee was specified (`fee_allocations` non-empty) without a
    /// positive per-unit market value for the fee crypto — the disposal needs
    /// its capital proceeds.
    FeeMarketPriceMissing,
    /// The Sell-side invariants failed: missing/over-allocated parcel, a
    /// non-Buy/DRP parcel, or a parcel outside the source account.
    Sell(sell::SellError),
}

impl From<sqlx::Error> for TransferError {
    fn from(e: sqlx::Error) -> Self {
        TransferError::Db(e)
    }
}

impl From<sell::SellError> for TransferError {
    fn from(e: sell::SellError) -> Self {
        match e {
            sell::SellError::Db(err) => TransferError::Db(err),
            other => TransferError::Sell(other),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/transfers", get(list))
        .route("/transfers/{id}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Transfer>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, date, from_account_id, to_account_id, fee_sale_trade_id \
         FROM transfers ORDER BY date, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Transfer>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, date, from_account_id, to_account_id, fee_sale_trade_id \
         FROM transfers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Record and execute a transfer, atomically: insert the transfer row, close
/// the chosen quantities in the source account with a price-0 transfer-out
/// Sell, and re-create them in the destination account carrying each parcel's
/// remaining reduced cost base and acquisition date.
pub async fn db_transfer(
    pool: &SqlitePool,
    id: i64,
    body: &TransferBody,
) -> Result<TransferGroup, TransferError> {
    if body.from_account_id == body.to_account_id {
        return Err(TransferError::SameAccount);
    }
    if body.allocations.is_empty() {
        return Err(TransferError::NothingToTransfer);
    }

    let mut tx = pool.begin().await?;

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM transfers WHERE id = ?)")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if exists {
        return Err(TransferError::AlreadyExists);
    }

    // A bad listing or account id fails the FK here (→ 422 via the shared map).
    sqlx::query(
        "INSERT INTO transfers (id, listing_id, date, from_account_id, to_account_id) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(body.listing_id)
    .bind(body.date)
    .bind(body.from_account_id)
    .bind(body.to_account_id)
    .execute(&mut *tx)
    .await?;

    // Cost-base inputs, shared with the open-parcels report and the
    // scrip-for-scrip exchange: splits re-base units, AMIT adjustments and
    // return-of-capital payments up to the transfer date reduce the carried
    // cost base.
    let splits = corporate_action::db_splits_for_listing(&mut *tx, body.listing_id).await?;
    let roc_rows = sqlx::query(
        "SELECT date, amount_per_unit, currency FROM corporate_actions \
         WHERE action_type = 'ReturnOfCapital' AND listing_id = ? ORDER BY date, id",
    )
    .bind(body.listing_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut roc_events = Vec::with_capacity(roc_rows.len());
    for row in &roc_rows {
        roc_events.push(RocEvent {
            date: row.try_get("date")?,
            amount_per_unit: parse_dec("amount_per_unit", row.try_get("amount_per_unit")?)?,
            currency: row.try_get("currency")?,
        });
    }
    let amit_reductions =
        crate::entities::amit_adjustment::db_cost_base_reductions(&mut *tx).await?;

    /// A transfer-in Buy to create: the moved units (transfer-date basis),
    /// their share of the parcel's remaining reduced cost base, and the
    /// carried acquisition date.
    struct TransferIn {
        quantity: Decimal,
        carried_cost_base: Decimal,
        currency: String,
        fx_rate: Decimal,
        deemed_acquisition_date: NaiveDate,
    }

    let mut transfer_ins: Vec<TransferIn> = Vec::with_capacity(body.allocations.len());
    for alloc in &body.allocations {
        let parcel = sqlx::query(
            "SELECT date, listing_id, quantity, average_price, brokerage, gst_on_brokerage, \
                    currency, fx_rate, deemed_acquisition_date \
             FROM trades WHERE id = ?",
        )
        .bind(alloc.purchase_trade_id)
        .fetch_optional(&mut *tx)
        .await?;
        // A missing parcel (or a non-Buy/DRP, over-allocation, or
        // wrong-account one) is also caught by the Sell core below; the
        // listing check is the one thing it does not enforce.
        let Some(parcel) = parcel else {
            return Err(TransferError::Sell(sell::SellError::PurchaseParcelMissing));
        };
        let parcel_listing: i64 = parcel.try_get("listing_id")?;
        if parcel_listing != body.listing_id {
            return Err(TransferError::ListingMismatch);
        }

        let date: NaiveDate = parcel.try_get("date")?;
        let qty = parse_dec("quantity", parcel.try_get("quantity")?)?;
        let price = parse_dec("average_price", parcel.try_get("average_price")?)?;
        let brok = parse_dec("brokerage", parcel.try_get("brokerage")?)?;
        let gst = parse_dec("gst_on_brokerage", parcel.try_get("gst_on_brokerage")?)?;
        let currency: String = parcel.try_get("currency")?;
        let fx_rate = parse_dec("fx_rate", parcel.try_get("fx_rate")?)?;
        let deemed: Option<NaiveDate> = parcel.try_get("deemed_acquisition_date")?;

        // The moved units' share of the parcel's remaining reduced cost base,
        // in the parcel's own currency: the open-parcels formula
        // (AMIT-reduced, then return-of-capital payments on the moved units,
        // both flooring at nil), pro-rated over the *as-acquired* moved units
        // so a partial transfer carries exactly its share.
        let moved_as_acquired =
            as_acquired_quantity(alloc.quantity_allocated, &splits, date, body.date);
        let initial_cost = price * qty + brok + gst;
        let amit = *amit_reductions.get(&alloc.purchase_trade_id).unwrap_or(&Decimal::ZERO);
        let net_cost = (initial_cost - amit).max(Decimal::ZERO);
        let roc_per_unit =
            per_unit_reduction(&roc_events, &splits, &currency, date, Some(body.date))?;
        let carried_cost_base = if qty > Decimal::ZERO {
            (net_cost * moved_as_acquired / qty - roc_per_unit * moved_as_acquired)
                .max(Decimal::ZERO)
        } else {
            Decimal::ZERO
        };

        transfer_ins.push(TransferIn {
            quantity: alloc.quantity_allocated,
            carried_cost_base,
            currency,
            fx_rate,
            // Chain through an earlier rollover/transfer: the discount clock
            // always runs from the first acquisition.
            deemed_acquisition_date: deemed.unwrap_or(date),
        });
    }

    // The transfer-out Sell: zero proceeds (nothing is disposed of; the
    // transfer_id keeps it out of every gains report), consuming the chosen
    // parcels in the source account. Settlement is the transfer date —
    // nothing market-settles.
    let listing_currency: String = sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
        .bind(body.listing_id)
        .fetch_one(&mut *tx)
        .await?;
    let sell_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM trades")
        .fetch_one(&mut *tx)
        .await?;
    let sell_body = SellBody {
        brokerage_includes_gst: false,
        statement_total: None,
        date: body.date,
        settlement_date: Some(body.date),
        listing_id: body.listing_id,
        average_price: Decimal::ZERO,
        quantity: body.allocations.iter().map(|a| a.quantity_allocated).sum(),
        currency: listing_currency.clone(),
        brokerage: Decimal::ZERO,
        gst_on_brokerage: Decimal::ZERO,
        brokerage_currency: listing_currency.clone(),
        fx_rate: Decimal::ONE,
        contract_note_ref: None,
        holding_account_id: body.from_account_id,
        allocations: body
            .allocations
            .iter()
            .map(|a| AllocationInput {
                purchase_trade_id: a.purchase_trade_id,
                quantity_allocated: a.quantity_allocated,
            })
            .collect(),
    };
    sell::upsert_sell_in_tx(&mut tx, sell_id, &sell_body, body.date, None, None, None, Some(id), None)
        .await?;

    // The transfer-in Buys: one per consumed parcel, dated the transfer date,
    // in the destination account, carrying the moved units' cost base (on the
    // brokerage column, price 0) and acquisition date.
    let mut transfer_in_ids = Vec::with_capacity(transfer_ins.len());
    for (i, t) in transfer_ins.iter().enumerate() {
        let buy_id = sell_id + 1 + i as i64;
        sqlx::query(
            "INSERT INTO trades \
             (id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
              currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, \
              holding_account_id, transfer_id, deemed_acquisition_date) \
             VALUES (?, 'Buy', ?, ?, ?, '0', ?, ?, ?, '0', ?, ?, ?, ?, ?)",
        )
        .bind(buy_id)
        .bind(body.date)
        .bind(body.date)
        .bind(body.listing_id)
        .bind(t.quantity.to_string())
        .bind(&t.currency)
        .bind(t.carried_cost_base.to_string())
        .bind(&t.currency)
        .bind(t.fx_rate.to_string())
        .bind(body.to_account_id)
        .bind(id)
        .bind(t.deemed_acquisition_date)
        .execute(&mut *tx)
        .await?;
        transfer_in_ids.push(buy_id);
    }

    // The optional network-fee disposal: the crypto consumed to cover the
    // on-chain fee is a CGT event (ATO: "if your crypto holding reduces during
    // a transfer to cover a network fee, the transaction fee is a disposal and
    // has capital gain consequences"). It is an ordinary Sell in the source
    // account at the fee crypto's market value — no `transfer_id`, so it is
    // counted by the gains reports like any disposal — linked to this transfer
    // via `transfers.fee_sale_trade_id` so the two are created and deleted
    // together. Its parcels' over-allocation is checked against the source
    // holding net of the transfer-out Sell above (both share the same tx).
    let mut fee_sale_id: Option<i64> = None;
    if !body.fee_allocations.is_empty() {
        let fee_market_price = match body.fee_market_price {
            Some(p) if p > Decimal::ZERO => p,
            _ => return Err(TransferError::FeeMarketPriceMissing),
        };
        // Fee parcels must be of the transfer's listing (the Sell core checks
        // the source-account and capacity invariants, but not the listing).
        for alloc in &body.fee_allocations {
            let parcel_listing: Option<i64> =
                sqlx::query_scalar("SELECT listing_id FROM trades WHERE id = ?")
                    .bind(alloc.purchase_trade_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            match parcel_listing {
                None => return Err(TransferError::Sell(sell::SellError::PurchaseParcelMissing)),
                Some(l) if l != body.listing_id => return Err(TransferError::ListingMismatch),
                Some(_) => {}
            }
        }

        let id_for_fee = sell_id + 1 + transfer_ins.len() as i64;
        let fee_body = SellBody {
            brokerage_includes_gst: false,
            statement_total: None,
            date: body.date,
            settlement_date: Some(body.date),
            listing_id: body.listing_id,
            average_price: fee_market_price,
            quantity: body.fee_allocations.iter().map(|a| a.quantity_allocated).sum(),
            currency: listing_currency.clone(),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            brokerage_currency: listing_currency,
            fx_rate: body.fee_fx_rate.unwrap_or(Decimal::ONE),
            contract_note_ref: None,
            holding_account_id: body.from_account_id,
            allocations: body
                .fee_allocations
                .iter()
                .map(|a| AllocationInput {
                    purchase_trade_id: a.purchase_trade_id,
                    quantity_allocated: a.quantity_allocated,
                })
                .collect(),
        };
        sell::upsert_sell_in_tx(&mut tx, id_for_fee, &fee_body, body.date, None, None, None, None, None)
            .await?;
        sqlx::query("UPDATE transfers SET fee_sale_trade_id = ? WHERE id = ?")
            .bind(id_for_fee)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        fee_sale_id = Some(id_for_fee);
    }

    tx.commit().await?;

    // Read the freshly created rows back so the response is exactly what was
    // stored.
    let transfer =
        db_get(pool, id).await?.ok_or_else(|| TransferError::Db(sqlx::Error::RowNotFound))?;
    let sell = trade::db_get(pool, sell_id)
        .await?
        .ok_or_else(|| TransferError::Db(sqlx::Error::RowNotFound))?;
    let mut created = Vec::with_capacity(transfer_in_ids.len());
    for tid in transfer_in_ids {
        created.push(
            trade::db_get(pool, tid)
                .await?
                .ok_or_else(|| TransferError::Db(sqlx::Error::RowNotFound))?,
        );
    }
    let fee_sale = match fee_sale_id {
        Some(fid) => Some(
            trade::db_get(pool, fid)
                .await?
                .ok_or_else(|| TransferError::Db(sqlx::Error::RowNotFound))?,
        ),
        None => None,
    };
    Ok(TransferGroup { transfer, sell, transfer_ins: created, fee_sale })
}

/// Outcome of a delete request, so the handler can map to the right status.
#[derive(Debug, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    NotFound,
    /// A transfer-in Buy is consumed by later allocations, AMIT adjustments,
    /// or an income link — deleting the group would orphan those dependants.
    /// Remove them first (mapped to 422).
    TransferInReferenced,
}

/// Delete a transfer and its whole trade group (the transfer-out Sell, its
/// allocations, every transfer-in Buy, and the network-fee disposal Sell if
/// any) in one transaction, restoring the pre-transfer holding.
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let fee_sale_id: Option<Option<i64>> =
        sqlx::query_scalar("SELECT fee_sale_trade_id FROM transfers WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(fee_sale_id) = fee_sale_id else {
        return Ok(DeleteOutcome::NotFound);
    };

    // The transfer-in Buys go with the group — but not while anything still
    // draws on them. (The fee Sell never gets drawn on — it is a disposal.)
    let referenced: bool = sqlx::query_scalar(
        "SELECT EXISTS(\
             SELECT 1 FROM trades t \
             WHERE t.transfer_id = ?1 AND t.trade_type <> 'Sell' \
               AND (EXISTS(SELECT 1 FROM parcel_allocations WHERE purchase_trade_id = t.id) \
                 OR EXISTS(SELECT 1 FROM amit_adjustments WHERE trade_id = t.id) \
                 OR EXISTS(SELECT 1 FROM income \
                           WHERE reinvestment_trade_id = t.id OR buyback_trade_id = t.id)))",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if referenced {
        return Ok(DeleteOutcome::TransferInReferenced);
    }

    sqlx::query(
        "DELETE FROM parcel_allocations WHERE sale_trade_id IN \
         (SELECT id FROM trades WHERE transfer_id = ?)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    // Break the transfer→fee-Sell link before deleting the trade rows so
    // neither foreign key (trades.transfer_id, transfers.fee_sale_trade_id)
    // is left dangling mid-delete (foreign_keys is on).
    if let Some(fee_id) = fee_sale_id {
        sqlx::query("UPDATE transfers SET fee_sale_trade_id = NULL WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM parcel_allocations WHERE sale_trade_id = ?")
            .bind(fee_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM trades WHERE transfer_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if let Some(fee_id) = fee_sale_id {
        sqlx::query("DELETE FROM trades WHERE id = ?")
            .bind(fee_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM transfers WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Transfer>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Transfer>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<TransferBody>,
) -> Result<(StatusCode, Json<TransferGroup>), (StatusCode, String)> {
    let unprocessable = |msg: &str| Err((StatusCode::UNPROCESSABLE_ENTITY, msg.to_string()));
    match db_transfer(&pool, id, &body).await {
        // 201 with the created group (like the scrip exchange): the client
        // needs the created trade ids, which a bare 204 would hide.
        Ok(group) => Ok((StatusCode::CREATED, Json(group))),
        Err(TransferError::SameAccount) => {
            unprocessable("the source and destination accounts are the same — nothing to move")
        }
        Err(TransferError::AlreadyExists) => unprocessable(
            "a transfer with that id already exists and is immutable — delete it to re-transfer",
        ),
        Err(TransferError::NothingToTransfer) => {
            unprocessable("a transfer must move at least one parcel")
        }
        Err(TransferError::ListingMismatch) => {
            unprocessable("a selected parcel does not belong to the transfer's listing")
        }
        Err(TransferError::FeeMarketPriceMissing) => unprocessable(
            "a network fee was specified without a positive per-unit market value for the fee \
             crypto — the disposal needs its capital proceeds",
        ),
        Err(TransferError::Sell(e)) => {
            tracing::warn!(error = ?e, "transfer rejected by a sell invariant");
            unprocessable(
                "the selected parcels are invalid: missing, over-allocated, not a Buy/DRP, \
                 or held in a different account from the source",
            )
        }
        Err(TransferError::Db(e)) => Err(crate::infra::http::write_error_body(&e)),
    }
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    match db_delete(&pool, id).await {
        Ok(DeleteOutcome::Deleted) => Ok(StatusCode::NO_CONTENT),
        Ok(DeleteOutcome::NotFound) => Err((StatusCode::NOT_FOUND, "no transfer with that id".to_string())),
        Ok(DeleteOutcome::TransferInReferenced) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "a transferred-in parcel is consumed by a later sale, AMIT adjustment, or income \
             link — remove those first"
                .to_string(),
        )),
        Err(e) => {
            tracing::error!(error = %e, "transfer delete failed");
            Err((StatusCode::INTERNAL_SERVER_ERROR, String::new()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::holding_account::{self, HoldingAccount};
    use crate::entities::trade::TradeType;
    use crate::entities::{listing, sell::SellError};
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        let pool = db::init(":memory:").await.unwrap();
        // Account 1 ('Default') is seeded; 2 is the employer plan the RSU
        // scenario transfers out of.
        holding_account::db_upsert(
            &pool,
            &HoldingAccount { id: 2, name: "ICE Employee Plan".to_string() },
        )
        .await
        .unwrap();
        pool
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: Some("XNYS".to_string()),
                ticker: ticker.to_string(),
                name: ticker.to_string(),
                isin: None,
                security_type: listing::SecurityType::Share,
                currency: "USD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    /// An RSU vest: a Buy at market value in the plan account (account 2).
    async fn insert_vest(pool: &SqlitePool, id: i64, date: NaiveDate, qty: &str, price: &str) {
        trade::db_upsert(
            pool,
            &Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                id,
                trade_type: TradeType::Buy,
                date,
                settlement_date: date,
                listing_id: 1,
                average_price: price.parse().unwrap(),
                quantity: qty.parse().unwrap(),
                currency: "USD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "USD".to_string(),
                fx_rate: dec("1.5"),
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
                holding_account_id: 2,
                transfer_id: None,
                ess_statement_id: None,
            },
        )
        .await
        .unwrap();
    }

    fn body(
        date: NaiveDate,
        from: i64,
        to: i64,
        allocations: Vec<(i64, &str)>,
    ) -> TransferBody {
        TransferBody {
            listing_id: 1,
            date,
            from_account_id: from,
            to_account_id: to,
            allocations: allocations
                .into_iter()
                .map(|(id, q)| AllocationInput {
                    purchase_trade_id: id,
                    quantity_allocated: q.parse().unwrap(),
                })
                .collect(),
            fee_allocations: Vec::new(),
            fee_market_price: None,
            fee_fx_rate: None,
        }
    }

    // DB-level tests

    /// The core move: vested plan shares land in the personal account as a
    /// price-0 Buy carrying the vest's cost base (on brokerage), currency,
    /// fx fallback, and acquisition date; the plan holding is closed by a
    /// zero-proceeds transfer-out Sell consuming the parcel.
    #[tokio::test]
    async fn transfer_moves_parcel_preserving_cost_base_and_acquisition_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await;

        let group = db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "100")]))
            .await
            .unwrap();

        assert_eq!(group.transfer.from_account_id, 2);
        assert_eq!(group.transfer.to_account_id, 1);

        // The transfer-out Sell: in the plan account, zero proceeds, the
        // whole parcel, marked with the transfer's provenance.
        assert_eq!(group.sell.trade_type, TradeType::Sell);
        assert_eq!(group.sell.holding_account_id, 2);
        assert_eq!(group.sell.quantity, dec("100"));
        assert_eq!(group.sell.average_price, Decimal::ZERO);
        assert_eq!(group.sell.transfer_id, Some(1));

        // The transfer-in Buy: in the personal account, carrying the vest's
        // $12,000 cost base, USD currency, fx fallback, and 2023 acquisition.
        assert_eq!(group.transfer_ins.len(), 1);
        let t = &group.transfer_ins[0];
        assert_eq!(t.trade_type, TradeType::Buy);
        assert_eq!(t.holding_account_id, 1);
        assert_eq!(t.date, d(2024, 6, 1));
        assert_eq!(t.quantity, dec("100"));
        assert_eq!(t.average_price, Decimal::ZERO);
        assert_eq!(t.brokerage, dec("12000"));
        assert_eq!(t.currency, "USD");
        assert_eq!(t.fx_rate, dec("1.5"));
        assert_eq!(t.deemed_acquisition_date, Some(d(2023, 3, 1)));
        assert_eq!(t.transfer_id, Some(1));
    }

    /// A crypto parcel transfers between holding accounts exactly like a
    /// share parcel (e.g. moving BTC between custody wallets): not a CGT
    /// event, satoshi-scale quantity preserved, the transfer-in carrying the
    /// parcel's cost base and acquisition date.
    #[tokio::test]
    async fn crypto_transfer_carries_cost_base_and_acquisition_date() {
        let pool = test_pool().await;
        listing::db_upsert(
            &pool,
            &listing::Listing {
                id: 1,
                exchange_mic: None,
                ticker: "BTC".to_string(),
                name: "Bitcoin".to_string(),
                isin: None,
                security_type: listing::SecurityType::Crypto,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
        // 0.12345678 BTC bought at A$60,000 in account 2.
        trade::db_upsert(
            &pool,
            &Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                id: 1,
                trade_type: TradeType::Buy,
                date: d(2023, 3, 1),
                settlement_date: d(2023, 3, 1), // crypto settles same-day
                listing_id: 1,
                average_price: dec("60000"),
                quantity: dec("0.12345678"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
                holding_account_id: 2,
                transfer_id: None,
                ess_statement_id: None,
            },
        )
        .await
        .unwrap();

        let group = db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "0.12345678")]))
            .await
            .unwrap();

        let t = &group.transfer_ins[0];
        assert_eq!(t.holding_account_id, 1);
        assert_eq!(t.quantity, dec("0.12345678"), "satoshi-scale quantity preserved");
        // The whole A$7,407.4068 cost base moves on the brokerage column...
        assert_eq!(t.average_price, Decimal::ZERO);
        assert_eq!(t.brokerage, dec("7407.40680000"));
        // ...with the original acquisition date driving the discount clock.
        assert_eq!(t.deemed_acquisition_date, Some(d(2023, 3, 1)));
    }

    /// Seed an AUD-priced BTC `Crypto` listing (id 1) and a single parcel
    /// (trade id 1) of `qty` units bought at A$`price`/unit on 2023-03-01 in
    /// account 2 — held long enough that a 2024-06-01 disposal is
    /// discount-eligible.
    async fn insert_crypto(pool: &SqlitePool, qty: &str, price: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id: 1,
                exchange_mic: None,
                ticker: "BTC".to_string(),
                name: "Bitcoin".to_string(),
                isin: None,
                security_type: listing::SecurityType::Crypto,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
        trade::db_upsert(
            pool,
            &Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                id: 1,
                trade_type: TradeType::Buy,
                date: d(2023, 3, 1),
                settlement_date: d(2023, 3, 1),
                listing_id: 1,
                average_price: dec(price),
                quantity: dec(qty),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
                holding_account_id: 2,
                transfer_id: None,
                ess_statement_id: None,
            },
        )
        .await
        .unwrap();
    }

    /// A transfer body carrying a network fee: `allocations` move, while
    /// `fee_allocations` are disposed of at `fee_price`/unit to cover the fee.
    fn fee_body(
        date: NaiveDate,
        from: i64,
        to: i64,
        allocations: Vec<(i64, &str)>,
        fee_allocations: Vec<(i64, &str)>,
        fee_price: &str,
    ) -> TransferBody {
        let map = |v: Vec<(i64, &str)>| {
            v.into_iter()
                .map(|(id, q)| AllocationInput {
                    purchase_trade_id: id,
                    quantity_allocated: q.parse().unwrap(),
                })
                .collect()
        };
        TransferBody {
            listing_id: 1,
            date,
            from_account_id: from,
            to_account_id: to,
            allocations: map(allocations),
            fee_allocations: map(fee_allocations),
            fee_market_price: Some(dec(fee_price)),
            fee_fx_rate: None,
        }
    }

    /// A crypto wallet transfer that burns a network fee: the moved units
    /// carry their cost base (not a CGT event), while the fee units are a
    /// real disposal — a Sell at market value that surfaces a capital gain in
    /// the realised-gains report (with the 12-month discount), and is linked
    /// back to the transfer.
    #[tokio::test]
    async fn network_fee_is_disposed_and_surfaces_in_realised_gains() {
        let pool = test_pool().await;
        insert_crypto(&pool, "1.0", "60000").await; // 1 BTC at A$60,000

        // Move 0.5 BTC to account 1; pay a 0.001 BTC network fee, the fee
        // crypto worth A$80,000/unit at the transfer date.
        let group = db_transfer(
            &pool,
            1,
            &fee_body(d(2024, 6, 1), 2, 1, vec![(1, "0.5")], vec![(1, "0.001")], "80000"),
        )
        .await
        .unwrap();

        // The move itself: 0.5 BTC lands in account 1 carrying A$30,000 cost
        // base, not a disposal.
        let moved = &group.transfer_ins[0];
        assert_eq!(moved.holding_account_id, 1);
        assert_eq!(moved.quantity, dec("0.5"));
        assert_eq!(moved.brokerage, dec("30000.0"));

        // The fee disposal: a Sell in the source account, no transfer_id (so
        // the gains reports count it), linked from the transfer.
        let fee = group.fee_sale.as_ref().expect("a fee Sell was created");
        assert_eq!(fee.trade_type, TradeType::Sell);
        assert_eq!(fee.holding_account_id, 2);
        assert_eq!(fee.quantity, dec("0.001"));
        assert_eq!(fee.average_price, dec("80000"));
        assert_eq!(fee.transfer_id, None);
        assert_eq!(group.transfer.fee_sale_trade_id, Some(fee.id));

        // The realised-gains report shows exactly the fee disposal: proceeds
        // 0.001 × 80,000 = A$80, cost base 0.001 × 60,000 = A$60, a A$20 gain,
        // fully discount-eligible (held > 12 months). The transfer-out Sell
        // (zero proceeds, transfer_id) is absent.
        let realised = crate::reports::realised_gains::db_realised_gains(&pool).await.unwrap();
        assert_eq!(realised.len(), 1, "only the fee is a disposal");
        let g = &realised[0];
        assert_eq!(g.sale_trade_id, fee.id);
        assert_eq!(g.proceeds, dec("80"));
        assert_eq!(g.cost_base, dec("60"));
        assert_eq!(g.capital_gain_loss, dec("20"));
        assert_eq!(g.discount_eligible_gain, dec("20"));
    }

    /// A fee disposal needs its capital proceeds: specifying fee parcels
    /// without a positive per-unit market value is rejected and nothing is
    /// persisted.
    #[tokio::test]
    async fn network_fee_requires_a_positive_market_price() {
        let pool = test_pool().await;
        insert_crypto(&pool, "1.0", "60000").await;

        let mut no_price = fee_body(d(2024, 6, 1), 2, 1, vec![(1, "0.5")], vec![(1, "0.001")], "0");
        no_price.fee_market_price = None;
        assert!(matches!(
            db_transfer(&pool, 1, &no_price).await,
            Err(TransferError::FeeMarketPriceMissing)
        ));

        // A zero price is just as invalid (the fee crypto has market value).
        let zero_price =
            fee_body(d(2024, 6, 1), 2, 1, vec![(1, "0.5")], vec![(1, "0.001")], "0");
        assert!(matches!(
            db_transfer(&pool, 1, &zero_price).await,
            Err(TransferError::FeeMarketPriceMissing)
        ));

        let transfers: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transfers").fetch_one(&pool).await.unwrap();
        assert_eq!(transfers, 0);
        let trades: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE id <> 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(trades, 0, "no transfer trades persisted");
    }

    /// The combined draw (moved units + fee units) can't exceed the source
    /// parcel — both Sells share the transfer's transaction, so the capacity
    /// check sees their sum.
    #[tokio::test]
    async fn moved_plus_fee_cannot_exceed_the_parcel() {
        let pool = test_pool().await;
        insert_crypto(&pool, "1.0", "60000").await;

        // 0.6 moved + 0.5 fee = 1.1 > 1.0 held.
        assert!(matches!(
            db_transfer(
                &pool,
                1,
                &fee_body(d(2024, 6, 1), 2, 1, vec![(1, "0.6")], vec![(1, "0.5")], "80000"),
            )
            .await,
            Err(TransferError::Sell(SellError::PurchaseQuantityExceeded))
        ));
        let transfers: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transfers").fetch_one(&pool).await.unwrap();
        assert_eq!(transfers, 0);
    }

    /// Deleting a fee'd transfer removes the fee disposal too (and its
    /// allocations), restoring the whole source parcel and clearing the gains
    /// report.
    #[tokio::test]
    async fn deleting_a_feed_transfer_removes_the_fee_disposal() {
        let pool = test_pool().await;
        insert_crypto(&pool, "1.0", "60000").await;
        let group = db_transfer(
            &pool,
            1,
            &fee_body(d(2024, 6, 1), 2, 1, vec![(1, "0.5")], vec![(1, "0.001")], "80000"),
        )
        .await
        .unwrap();
        let fee_id = group.fee_sale.unwrap().id;

        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Deleted);

        assert!(trade::db_get(&pool, fee_id).await.unwrap().is_none(), "fee Sell gone");
        let allocs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(allocs, 0, "fee + transfer-out allocations freed");
        let realised = crate::reports::realised_gains::db_realised_gains(&pool).await.unwrap();
        assert!(realised.is_empty());
        // The whole 1.0 BTC parcel is open again in account 2.
        let parcel = trade::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(parcel.quantity, dec("1.0"));
        assert_eq!(parcel.holding_account_id, 2);
    }

    /// The fee disposal Sell is part of the transfer group: it can't be
    /// edited or deleted on its own (PUT/DELETE /sells, PUT /trades) — undo
    /// it by deleting the transfer.
    #[tokio::test]
    async fn fee_disposal_sell_is_immutable_individually() {
        let pool = test_pool().await;
        insert_crypto(&pool, "1.0", "60000").await;
        let group = db_transfer(
            &pool,
            1,
            &fee_body(d(2024, 6, 1), 2, 1, vec![(1, "0.5")], vec![(1, "0.001")], "80000"),
        )
        .await
        .unwrap();
        let fee = group.fee_sale.unwrap();

        // PUT /sells on the fee Sell → rejected.
        let err = sell::db_upsert_sell(
            &pool,
            fee.id,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                date: d(2024, 6, 1),
                settlement_date: Some(d(2024, 6, 1)),
                listing_id: 1,
                average_price: dec("99"),
                quantity: dec("0.001"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                holding_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("0.001"),
                }],
            },
        )
        .await;
        assert!(matches!(err, Err(SellError::TransferSell)));

        // DELETE /sells on it → refused (goes via DELETE /transfers).
        assert_eq!(
            sell::db_delete_sell(&pool, fee.id).await.unwrap(),
            sell::DeleteOutcome::TransferSell
        );

        // PUT /trades on it → rejected.
        let mut edited = fee.clone();
        edited.average_price = dec("99");
        assert!(matches!(
            trade::db_upsert(&pool, &edited).await,
            Err(trade::UpsertError::TransferTrade)
        ));
    }

    /// A partial transfer splits the parcel: the moved units carry exactly
    /// their share of the cost base; the rest stays open in the source
    /// account.
    #[tokio::test]
    async fn partial_transfer_splits_the_parcel() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await;

        let group = db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "40")]))
            .await
            .unwrap();

        assert_eq!(group.sell.quantity, dec("40"));
        // 40% of the $12,000 vest cost base moves.
        assert_eq!(group.transfer_ins[0].quantity, dec("40"));
        assert_eq!(group.transfer_ins[0].brokerage, dec("4800"));

        // The remaining 60 units are still open in the plan account: a
        // plan-account Sell of 60 against the original parcel still fits.
        sell::db_upsert_sell(
            &pool,
            50,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                date: d(2024, 7, 1),
                settlement_date: Some(d(2024, 7, 1)),
                listing_id: 1,
                average_price: dec("130"),
                quantity: dec("60"),
                currency: "USD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "USD".to_string(),
                fx_rate: dec("1.5"),
                contract_note_ref: None,
                holding_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("60"),
                }],
            },
        )
        .await
        .unwrap();
    }

    /// A transfer is not a CGT event: nothing appears in the realised-gains
    /// (or, via it, net-capital-gain) report, and the franking at-risk stack
    /// is undisturbed.
    #[tokio::test]
    async fn transfer_is_absent_from_gains_reports() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await;

        db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "100")]))
            .await
            .unwrap();

        let realised =
            crate::reports::realised_gains::db_realised_gains(&pool).await.unwrap();
        assert!(realised.is_empty(), "an own-account transfer is not a disposal");
    }

    /// Deleting the transfer removes the whole group and the record,
    /// restoring the pre-transfer holding.
    #[tokio::test]
    async fn deleting_the_transfer_restores_the_pre_transfer_holding() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await;
        db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "100")]))
            .await
            .unwrap();

        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Deleted);

        let trades: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE transfer_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(trades, 0);
        let allocs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(allocs, 0);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
        // The original vest parcel is open in the plan account again.
        let vest = trade::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(vest.holding_account_id, 2);

        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::NotFound);
    }

    /// The delete is refused while a transfer-in Buy is consumed by a later
    /// sale — removing it would orphan that sale's allocation.
    #[tokio::test]
    async fn delete_is_refused_while_a_transfer_in_is_consumed() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await;
        let group = db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "100")]))
            .await
            .unwrap();

        sell::db_upsert_sell(
            &pool,
            50,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                date: d(2024, 8, 1),
                settlement_date: Some(d(2024, 8, 1)),
                listing_id: 1,
                average_price: dec("130"),
                quantity: dec("50"),
                currency: "USD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "USD".to_string(),
                fx_rate: dec("1.5"),
                contract_note_ref: None,
                holding_account_id: 1,
                allocations: vec![AllocationInput {
                    purchase_trade_id: group.transfer_ins[0].id,
                    quantity_allocated: dec("50"),
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::TransferInReferenced);

        // Removing the later sale unblocks the delete.
        assert_eq!(sell::db_delete_sell(&pool, 50).await.unwrap(), sell::DeleteOutcome::Deleted);
        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Deleted);
    }

    /// The transfer trades are immutable individually: PUT /sells,
    /// PUT/DELETE /trades, and DELETE /sells all reject them.
    #[tokio::test]
    async fn transfer_trades_are_immutable_individually() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await;
        let group = db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "100")]))
            .await
            .unwrap();

        // PUT /sells on the transfer-out Sell → rejected.
        let err = sell::db_upsert_sell(
            &pool,
            group.sell.id,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                date: d(2024, 6, 1),
                settlement_date: Some(d(2024, 6, 1)),
                listing_id: 1,
                average_price: dec("9.99"),
                quantity: dec("100"),
                currency: "USD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "USD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                holding_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("100"),
                }],
            },
        )
        .await;
        assert!(matches!(err, Err(SellError::TransferSell)));

        // DELETE /sells on it → refused (the group goes via DELETE /transfers).
        assert_eq!(
            sell::db_delete_sell(&pool, group.sell.id).await.unwrap(),
            sell::DeleteOutcome::TransferSell
        );

        // PUT /trades on a transfer-in Buy → rejected.
        let mut edited = group.transfer_ins[0].clone();
        edited.quantity = dec("9999");
        assert!(matches!(
            trade::db_upsert(&pool, &edited).await,
            Err(trade::UpsertError::TransferTrade)
        ));

        // DELETE /trades on either → refused.
        assert_eq!(
            trade::db_delete(&pool, group.transfer_ins[0].id).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );
        assert_eq!(
            trade::db_delete(&pool, group.sell.id).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );
    }

    #[tokio::test]
    async fn invalid_transfers_are_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await;

        // Same source and destination.
        assert!(matches!(
            db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 2, vec![(1, "100")])).await,
            Err(TransferError::SameAccount)
        ));

        // No allocations.
        assert!(matches!(
            db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![])).await,
            Err(TransferError::NothingToTransfer)
        ));

        // A parcel outside the source account (the vest is in account 2).
        assert!(matches!(
            db_transfer(&pool, 1, &body(d(2024, 6, 1), 1, 2, vec![(1, "100")])).await,
            Err(TransferError::Sell(SellError::PurchaseInDifferentAccount))
        ));

        // Moving more than the parcel holds.
        assert!(matches!(
            db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "101")])).await,
            Err(TransferError::Sell(SellError::PurchaseQuantityExceeded))
        ));

        // A parcel of a different listing.
        insert_listing(&pool, 2, "OTHER").await;
        let mut wrong_listing = body(d(2024, 6, 1), 2, 1, vec![(1, "100")]);
        wrong_listing.listing_id = 2;
        assert!(matches!(
            db_transfer(&pool, 1, &wrong_listing).await,
            Err(TransferError::ListingMismatch)
        ));

        // Nothing was persisted by any rejection.
        let transfers: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transfers").fetch_one(&pool).await.unwrap();
        assert_eq!(transfers, 0);
        let trades: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE transfer_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(trades, 0);

        // A recorded transfer is immutable: re-PUT of the same id is rejected.
        db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "100")])).await.unwrap();
        assert!(matches!(
            db_transfer(&pool, 1, &body(d(2024, 7, 1), 2, 1, vec![(1, "100")])).await,
            Err(TransferError::AlreadyExists)
        ));
    }

    /// AMIT cost-base reductions and a return of capital received while the
    /// parcel sat in the source account reduce the carried cost base — the
    /// destination parcel carries the *remaining reduced* cost base.
    #[tokio::test]
    async fn amit_and_roc_reductions_carry_into_the_transferred_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await; // $12,000

        // An AMMA statement (in the plan account) with a $1/unit decrease
        // over the 100 units → −$100.
        crate::entities::amma::db_upsert(
            &pool,
            &crate::entities::amma::AmmaStatement {
                id: 1,
                listing_id: 1,
                holding_account_id: 2,
                tax_year_end_date: d(2023, 6, 30),
                units_held: dec("100"),
                date_received: d(2023, 7, 15),
                australian_interest: Decimal::ZERO,
                australian_dividends_unfranked: Decimal::ZERO,
                franked_dividends: Decimal::ZERO,
                franking_credits: Decimal::ZERO,
                net_rent: Decimal::ZERO,
                foreign_income: Decimal::ZERO,
                foreign_tax_credits: Decimal::ZERO,
                other_income: Decimal::ZERO,
                cgt_discount_gains: Decimal::ZERO,
                cgt_indexation_gains: Decimal::ZERO,
                cgt_other_gains: Decimal::ZERO,
                capital_losses_applied: Decimal::ZERO,
                tax_deferred_amount: Decimal::ZERO,
                tax_free_amount: Decimal::ZERO,
                cost_base_adjustment: dec("1"),
                tfn_withholding_tax: Decimal::ZERO,
                currency: "USD".to_string(),
            },
        )
        .await
        .unwrap();
        crate::entities::amit_adjustment::db_upsert(
            &pool,
            &crate::entities::amit_adjustment::AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: dec("100"),
            },
        )
        .await
        .unwrap();

        // A $2/unit return of capital while held → −$200.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 5,
                listing_id: 1,
                date: d(2024, 3, 1),
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: dec("2"),
                    currency: "USD".to_string(),
                },
            },
        )
        .await
        .unwrap();

        let group = db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "100")]))
            .await
            .unwrap();

        // $12,000 − $100 (AMIT) − $200 (ROC) = $11,700 carried.
        assert_eq!(group.transfer_ins[0].brokerage, dec("11700"));
    }

    // API-level tests

    #[tokio::test]
    async fn api_transfer_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await;
        let app = || router().with_state(pool.clone());

        let body = serde_json::json!({
            "listing_id": 1,
            "date": "2024-06-01",
            "from_account_id": 2,
            "to_account_id": 1,
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "100" } ]
        });
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/transfers/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["sell"]["holding_account_id"], 2);
        assert_eq!(v["transfer_ins"][0]["holding_account_id"], 1);
        assert_eq!(v["transfer_ins"][0]["brokerage"], "12000");
        assert_eq!(v["transfer_ins"][0]["deemed_acquisition_date"], "2023-03-01");

        // GET list/one.
        let resp = app()
            .oneshot(Request::builder().uri("/transfers/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // DELETE restores; a second DELETE is 404.
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/transfers/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/transfers/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_invalid_transfer_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await;

        // Same from/to account.
        let body = serde_json::json!({
            "listing_id": 1,
            "date": "2024-06-01",
            "from_account_id": 2,
            "to_account_id": 2,
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "100" } ]
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/transfers/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        let detail = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(detail.contains("same"), "detail: {detail}");

        // Unknown destination account → FK violation → 422.
        let body = serde_json::json!({
            "listing_id": 1,
            "date": "2024-06-01",
            "from_account_id": 2,
            "to_account_id": 99,
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "100" } ]
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/transfers/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
