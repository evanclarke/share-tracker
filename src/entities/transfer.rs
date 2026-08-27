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

use crate::domain::cost_base::ParcelRow;
use crate::domain::rollover;
use crate::entities::corporate_action::checked_as_acquired_quantity;
use crate::entities::sell::{self, AllocationInput};
use crate::entities::trade::{self, Trade};
use crate::infra::db::write_tx;
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

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
#[serde(deny_unknown_fields)]
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
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    pub fee_market_price: Option<Decimal>,
    /// The fee disposal's manual AUD fallback FX rate for a non-AUD-priced
    /// crypto listing. When omitted, the transfer month's ATO rate is
    /// resolved and bound (AUD resolves to 1); a non-AUD fee in a month with
    /// no imported rate is refused rather than defaulted to parity
    /// ([`TransferError::MissingFeeFxRate`]).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
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

#[derive(thiserror::Error, Debug)]
pub enum TransferError {
    #[error("transfer write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// Source and destination accounts are the same — nothing to move.
    #[error("the source and destination accounts are the same")]
    SameAccount,
    /// A transfer is recorded and executed in one step and is immutable:
    /// delete it (`DELETE /transfers/:id`) and re-transfer instead.
    #[error("a transfer with that id already exists and is immutable")]
    AlreadyExists,
    /// No allocations — a transfer must move at least one parcel.
    #[error("a transfer must move at least one parcel")]
    NothingToTransfer,
    /// A referenced parcel (moved or fee) does not belong to the transfer's
    /// listing.
    #[error("a selected parcel does not belong to the transfer's listing")]
    ListingMismatch,
    /// A network fee was specified (`fee_allocations` non-empty) without a
    /// positive per-unit market value for the fee crypto — the disposal needs
    /// its capital proceeds.
    #[error("a network fee was specified without a positive per-unit market value")]
    FeeMarketPriceMissing,
    /// The Sell-side invariants failed: missing/over-allocated parcel, a
    /// non-Buy/DRP parcel, a parcel outside the source account, or a transfer
    /// dated after today (SCENARIOS S-10).
    #[error("the transfer-out Sell was rejected: {0}")]
    Sell(#[source] sell::SellError),
    /// The listing already carries a whole-holding operation dated on or after
    /// the transfer date — a scrip-for-scrip exchange, a demerger, or a
    /// worthless-shares recognise. The transfer-in parcels are dated the
    /// transfer's own date, so they would land behind an operation that
    /// consumed every parcel open at its date and could never be consumed by it
    /// (SCENARIOS V-d). A transfer *itself* is not one of the three — it moves
    /// a quantity the taxpayer chose, so a parcel left behind is legitimate —
    /// but the parcels it creates are subject to the same rule as any other,
    /// compared on the trade's own `date` and never on the deemed acquisition
    /// date the transfer-in carries forward. Defence in depth: the source
    /// parcels an in-range transfer would draw on were consumed by that
    /// operation, so `sell::SellError::PurchaseQuantityExceeded` normally
    /// refuses first — this catches the case where the state already predates
    /// the guard. Wording and recovery in `domain::whole_holding`. Mapped to
    /// 422.
    #[error("the transfer-in parcels are dated behind a whole-holding operation: {0}")]
    BackDatedOverWholeHolding(#[source] crate::domain::whole_holding::BackDatedParcel),
    /// The requested units, re-based back into the parcel's as-acquired basis
    /// across a **consolidation** of the listing, are past `Decimal`'s range.
    ///
    /// A transfer moves units one for one and applies no ratio of its own, so
    /// this is *not* the ratio-driven overflow a scrip exchange or demerger
    /// has: it can only arise from a request that names far more units than
    /// the parcel could hold, which `sell::SellError::PurchaseQuantityExceeded`
    /// would refuse — except that the re-base is computed first, so the
    /// arithmetic used to panic (a logged `500` with an empty body) before the
    /// allocation check could answer at all. Mapped to 422 naming the
    /// arithmetic, which is what says the quantity asked for is impossible.
    #[error("the moved quantity is beyond the representable range: {0}")]
    UnrepresentableMovedQuantity(#[source] crate::domain::cost_base::UnrepresentableQuantity),
    /// A network-fee disposal on a non-AUD listing whose transfer month has
    /// no imported ATO rate, with no `fee_fx_rate` in the body: the fee Sell
    /// is a real disposal in the gains reports, and binding a placeholder 1
    /// would silently convert its proceeds at parity via
    /// `FxOverride::Fallback(1)` — the path the ESS vest refuses
    /// (`ess_vest::VestError::MissingFxRate`). Mapped to 422.
    #[error("no ATO FX rate for {currency} in {month} and the request states no fee_fx_rate")]
    MissingFeeFxRate { currency: String, month: String },
    // There is deliberately no sibling of
    // `trade::UpsertError::UnrepresentableRebasedQuantity` here, though a
    // transfer-in Buy is one of the eight parcel-creating writes that rule
    // covers. It is the one that cannot reach it, and the reason is that a
    // transfer's destination listing *is* its source listing: the transfer-in
    // is dated the transfer date and carries at most the units the source
    // parcel held then, so every ratio recorded after the transfer date re-bases
    // that parcel by the same factor and at least as far, while the ratios on or
    // before it apply to the parcel alone. A transfer-in past the range
    // therefore implies a source parcel past it, which
    // `corporate_action::rebased_quantity_beyond_range` already refuses — at
    // the parcel's own write, and again at any later action write that would
    // put it there. Measured as well as argued: with a 1e26-unit parcel
    // consolidated 1-for-1000 and transferred, the split that would overflow
    // the transfer-in is refused for overflowing the parcel it came from.
}

impl From<sell::SellError> for TransferError {
    fn from(e: sell::SellError) -> Self {
        match e {
            sell::SellError::Db(err) => TransferError::Db(err),
            other => TransferError::Sell(other),
        }
    }
}

impl From<TransferError> for ApiError {
    fn from(e: TransferError) -> Self {
        match e {
            TransferError::SameAccount => ApiError::unprocessable(
                "the source and destination accounts are the same — nothing to move",
            ),
            TransferError::AlreadyExists => ApiError::unprocessable(
                "a transfer with that id already exists and is immutable — delete it to \
                 re-transfer",
            ),
            TransferError::NothingToTransfer => {
                ApiError::unprocessable("a transfer must move at least one parcel")
            }
            TransferError::ListingMismatch => ApiError::unprocessable(
                "a selected parcel does not belong to the transfer's listing",
            ),
            // The same body every parcel-creating path answers for this fact —
            // here the parcels are the transfer's own transfer-ins.
            TransferError::BackDatedOverWholeHolding(e) => ApiError::Unprocessable(e.message()),
            // The units asked for cannot be re-based into the parcel's own
            // basis at all → 422 quoting the arithmetic, the same wording every
            // beyond-the-range refusal answers with.
            TransferError::UnrepresentableMovedQuantity(e) => ApiError::Unprocessable(e.message()),
            TransferError::MissingFeeFxRate { currency, month } => {
                ApiError::unprocessable(format!(
                    "the network fee is disposed of in {currency} but no ATO/RBA rate has been \
                     imported for {currency} in {month} and the request states no fee_fx_rate — \
                     supply fee_fx_rate or import that month's RBA rates; recording without one \
                     would convert the fee proceeds at parity (1 AUD per {currency})"
                ))
            }
            TransferError::FeeMarketPriceMissing => ApiError::unprocessable(
                "a network fee was specified without a positive per-unit market value for the \
                 fee crypto — the disposal needs its capital proceeds",
            ),
            // The Sell core's rejections, each said in the transfer's own terms
            // (SCENARIOS N-04, N-12): a user who typed the wrong date, or moved
            // more units than a parcel still holds, must be told *that* — the
            // one sentence listing every cause at once named neither, and did
            // not even list the date one.
            TransferError::Sell(err) => {
                tracing::warn!(error = ?err, "transfer rejected by a sell invariant");
                match err {
                    sell::SellError::PurchaseParcelMissing => {
                        ApiError::unprocessable("a selected parcel does not exist")
                    }
                    sell::SellError::PurchaseTradeNotBuyOrDrp => ApiError::unprocessable(
                        "a selected parcel is not a Buy or DRP trade — only a parcel of units can \
                         be moved",
                    ),
                    sell::SellError::PurchaseQuantityExceeded => ApiError::unprocessable(
                        "the units to move exceed what a selected parcel still holds — a parcel \
                         already sold, moved, or covering a network fee cannot be moved again",
                    ),
                    sell::SellError::PurchaseAfterSale => ApiError::unprocessable(
                        "a selected parcel is dated after the transfer date — units cannot be \
                         moved before they were acquired",
                    ),
                    sell::SellError::PurchaseInDifferentAccount => ApiError::unprocessable(
                        "a selected parcel is not held in the source account — move it from the \
                         account that holds it, or fix the source account",
                    ),
                    sell::SellError::AllocationNotPositive => ApiError::unprocessable(
                        "each parcel's units to move must be a positive quantity",
                    ),
                    // The one amounts rejection a transfer can reach
                    // (SCENARIOS S-10): its date is the user's own, and the
                    // parcels it creates are dated by it.
                    sell::SellError::Amounts(trade::AmountsError::FutureDate) => {
                        ApiError::unprocessable(
                            "the transfer is dated after today — units cannot be moved before \
                             the move happens",
                        )
                    }
                    // Everything else the Sell core can reject is unreachable
                    // from a transfer, which builds the Sell itself: its
                    // quantity is the allocations' own sum (never a mismatch),
                    // its price is 0 with the listing's currency and a fresh id
                    // (so no statement-total, FX or frozen-Sell rejection, and
                    // the only amounts rejection left is the date bound handled
                    // above), and the listing check above runs first.
                    other => {
                        tracing::error!(error = ?other, "unexpected sell rejection from a transfer");
                        ApiError::unprocessable(
                            "the selected parcels were rejected by the Sell invariants",
                        )
                    }
                }
            }
            TransferError::Db(err) => err.into(),
        }
    }
}

impl CrudEntity for Transfer {
    type Key = i64;
    const TABLE: &'static str = "transfers";
    const COLUMNS: &'static str =
        "id, listing_id, date, from_account_id, to_account_id, fee_sale_trade_id";
    const ORDER_BY: &'static str = "date, id";
    const NOUN: &'static str = "transfer";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/transfers", get(http::list_handler::<Transfer>))
        .route(
            "/transfers/{id}",
            get(http::get_handler::<Transfer>)
                .put(upsert)
                .delete(delete),
        )
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Transfer>, sqlx::Error> {
    http::crud_get(pool, id).await
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

    let mut tx = write_tx(pool).await?;

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM transfers WHERE id = ?)")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if exists {
        return Err(TransferError::AlreadyExists);
    }

    // The transfer-in parcels are dated the transfer date, so an executed
    // scrip-for-scrip exchange, demerger, or worthless-shares recognise of this
    // listing dated on or after it would leave them stranded — consumed by
    // nothing, open forever (SCENARIOS V-d). Checked before anything is
    // written, so this transfer's own rows cannot be mistaken for the offender.
    if let Some(back_dated) = crate::domain::whole_holding::db_back_dated_parcel(
        &mut tx,
        body.listing_id,
        body.date,
        None,
    )
    .await?
    {
        return Err(TransferError::BackDatedOverWholeHolding(back_dated));
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

    // Cost-base inputs, shared with the scrip-for-scrip exchange and the
    // demerge: splits re-base units, AMIT adjustments and return-of-capital
    // payments up to the transfer date reduce the carried cost base.
    let inputs = rollover::CostBaseInputs::load(&mut tx, body.listing_id, body.date).await?;
    let transfer_ins = transfer_ins(&mut tx, body, &inputs).await?;

    // The transfer-out Sell: zero proceeds (nothing is disposed of; the
    // transfer_id keeps it out of every gains report), consuming the chosen
    // parcels in the source account.
    let listing_currency: String = sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
        .bind(body.listing_id)
        .fetch_one(&mut *tx)
        .await?;
    let sell_body = rollover::closing_sell_body(
        body.date,
        body.listing_id,
        body.from_account_id,
        Decimal::ZERO,
        listing_currency.clone(),
        Decimal::ONE,
        body.allocations
            .iter()
            .map(|a| AllocationInput {
                purchase_trade_id: a.purchase_trade_id,
                quantity_allocated: a.quantity_allocated,
            })
            .collect(),
    );
    // No id of our own: the database assigns one its AUTOINCREMENT sequence
    // has never issued, so this Sell can never land on a deleted trade's id
    // (SCENARIOS U-a).
    let sell_id = sell::upsert_sell_in_tx(
        &mut tx,
        None,
        &sell_body,
        trade::Settlement::stated(body.date),
        None,
        None,
        None,
        Some(id),
        None,
    )
    .await?;

    // The transfer-in Buys: one per consumed parcel, dated the transfer date,
    // in the destination account, carrying the moved units' cost base (on the
    // brokerage column, price 0) and acquisition date.
    let mut transfer_in_ids = Vec::with_capacity(transfer_ins.len());
    for t in &transfer_ins {
        // Each Buy takes the id its own INSERT was given.
        let buy_id = rollover::insert_replacement_buy(
            &mut tx,
            &rollover::ReplacementBuy {
                date: body.date,
                listing_id: body.listing_id,
                quantity: t.quantity,
                cost_base: t.carried_cost_base,
                currency: &t.currency,
                fx_rate: t.fx_rate,
                spot_fx_rate: t.spot_fx_rate,
                deemed_acquisition_date: t.deemed_acquisition_date,
                holding_account_id: body.to_account_id,
            },
            rollover::Provenance::Transfer(id),
        )
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
    let fee_sale_id = write_fee_sale(&mut tx, id, body, listing_currency).await?;

    tx.commit().await?;

    // Read the freshly created rows back so the response is exactly what was
    // stored.
    let transfer = db_get(pool, id).await?.ok_or(sqlx::Error::RowNotFound)?;
    let sell = trade::db_get(pool, sell_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    let fee_sale = match fee_sale_id {
        Some(fid) => Some(
            trade::db_get(pool, fid)
                .await?
                .ok_or(sqlx::Error::RowNotFound)?,
        ),
        None => None,
    };
    Ok(TransferGroup {
        transfer,
        sell,
        transfer_ins: rollover::created_trades(pool, transfer_in_ids).await?,
        fee_sale,
    })
}

/// A transfer-in Buy to create: the moved units (transfer-date basis), their
/// share of the parcel's remaining reduced cost base, and the carried
/// acquisition date.
struct TransferIn {
    quantity: Decimal,
    carried_cost_base: Decimal,
    currency: String,
    fx_rate: Decimal,
    spot_fx_rate: Option<Decimal>,
    deemed_acquisition_date: NaiveDate,
}

/// One `TransferIn` per requested allocation: the moved units' share of that
/// parcel's remaining reduced cost base, and what the new parcel carries over
/// from the old one.
async fn transfer_ins(
    conn: &mut sqlx::SqliteConnection,
    body: &TransferBody,
    inputs: &rollover::CostBaseInputs,
) -> Result<Vec<TransferIn>, TransferError> {
    let mut out = Vec::with_capacity(body.allocations.len());
    for alloc in &body.allocations {
        let parcel: Option<ParcelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {} FROM trades WHERE id = ?",
            ParcelRow::columns()
        )))
        .bind(alloc.purchase_trade_id)
        .fetch_optional(&mut *conn)
        .await?;
        // A missing parcel (or a non-Buy/DRP, over-allocation, or
        // wrong-account one) is also caught by the Sell core; the listing
        // check is the one thing it does not enforce.
        let Some(parcel) = parcel else {
            return Err(TransferError::Sell(sell::SellError::PurchaseParcelMissing));
        };
        if parcel.listing_id != body.listing_id {
            return Err(TransferError::ListingMismatch);
        }

        // The moved units' share of the parcel's remaining reduced cost base,
        // in the parcel's own currency: the shared pipeline, pro-rated over
        // the *as-acquired* moved units so a partial transfer carries exactly
        // its share.
        //
        // Checked rather than computed: where a consolidation sits between the
        // parcel and the transfer date the re-base multiplies the requested
        // units *up*, and a request naming more units than could ever have been
        // held overflows here — before the allocation check that would have
        // refused it (`TransferError::UnrepresentableMovedQuantity`).
        let moved_as_acquired = checked_as_acquired_quantity(
            ("quantity_allocated", alloc.quantity_allocated),
            &inputs.splits,
            parcel.date,
            body.date,
        )
        .map_err(TransferError::UnrepresentableMovedQuantity)?;
        let carried_cost_base = inputs.carried_cost_base(&parcel, moved_as_acquired)?;

        out.push(TransferIn {
            quantity: alloc.quantity_allocated,
            carried_cost_base,
            currency: parcel.currency.clone(),
            fx_rate: parcel.fx_rate,
            spot_fx_rate: parcel.spot_fx_rate,
            // Chain through an earlier rollover/transfer: the discount clock
            // always runs from the first acquisition — the parcel's own
            // `acquired()`, deemed date where it carries one.
            deemed_acquisition_date: parcel.acquired(),
        });
    }
    Ok(out)
}

/// The optional network-fee disposal, written on the same transaction and
/// linked to the transfer. `None` when the body asked for none.
async fn write_fee_sale(
    conn: &mut sqlx::SqliteConnection,
    transfer_id: i64,
    body: &TransferBody,
    listing_currency: String,
) -> Result<Option<i64>, TransferError> {
    if body.fee_allocations.is_empty() {
        return Ok(None);
    }
    let fee_market_price = match body.fee_market_price {
        Some(p) if p > Decimal::ZERO => p,
        _ => return Err(TransferError::FeeMarketPriceMissing),
    };
    // Fee parcels must be of the transfer's listing (the Sell core checks the
    // source-account and capacity invariants, but not the listing).
    for alloc in &body.fee_allocations {
        let parcel_listing: Option<i64> =
            sqlx::query_scalar("SELECT listing_id FROM trades WHERE id = ?")
                .bind(alloc.purchase_trade_id)
                .fetch_optional(&mut *conn)
                .await?;
        match parcel_listing {
            None => return Err(TransferError::Sell(sell::SellError::PurchaseParcelMissing)),
            Some(l) if l != body.listing_id => return Err(TransferError::ListingMismatch),
            Some(_) => {}
        }
    }

    // The rate the fee Sell carries. Unlike the transfer legs it is a *real
    // disposal* in the gains reports, and `trades.fx_rate` is the fallback
    // applied when no ATO monthly rate exists for the month
    // (`infra::fx::pick_rate`) — so a placeholder 1 would book USD fee
    // proceeds 1:1 as AUD exactly when the transfer month's rate is missing,
    // the silent-parity path the ESS vest refuses
    // (`ess_vest::VestError::MissingFxRate`). The body's stated rate is bound
    // when the caller gave one; otherwise the month's ATO rate is resolved
    // and bound (AUD resolves to 1), and a month with neither is refused
    // rather than invented (the fee price is always positive here, so there
    // is always an amount to convert).
    let fee_fx_rate = match body.fee_fx_rate {
        Some(rate) => rate,
        None => {
            let fx = crate::infra::fx::FxRates::load(&mut *conn).await?;
            fx.resolve_rate(
                &listing_currency,
                body.date,
                crate::infra::fx::FxOverride::None,
            )
            .map_err(|_| TransferError::MissingFeeFxRate {
                currency: listing_currency.clone(),
                month: body.date.format("%Y-%m").to_string(),
            })?
        }
    };

    // An ordinary Sell at the fee crypto's market value: no `transfer_id`, so
    // the gains reports count it like any disposal.
    let fee_body = rollover::closing_sell_body(
        body.date,
        body.listing_id,
        body.from_account_id,
        fee_market_price,
        listing_currency,
        fee_fx_rate,
        body.fee_allocations
            .iter()
            .map(|a| AllocationInput {
                purchase_trade_id: a.purchase_trade_id,
                quantity_allocated: a.quantity_allocated,
            })
            .collect(),
    );
    // As for the transfer-out Sell above: the database assigns the id.
    let fee_sale_id = sell::upsert_sell_in_tx(
        &mut *conn,
        None,
        &fee_body,
        trade::Settlement::stated(body.date),
        None,
        None,
        None,
        None,
        None,
    )
    .await?;
    sqlx::query("UPDATE transfers SET fee_sale_trade_id = ? WHERE id = ?")
        .bind(fee_sale_id)
        .bind(transfer_id)
        .execute(&mut *conn)
        .await?;
    Ok(Some(fee_sale_id))
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
    let mut tx = write_tx(pool).await?;

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

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<TransferBody>,
) -> Result<(StatusCode, Json<TransferGroup>), ApiError> {
    // 201 with the created group (like the scrip exchange): the client
    // needs the created trade ids, which a bare 204 would hide.
    let group = db_transfer(&pool, id, &body).await?;
    Ok((StatusCode::CREATED, Json(group)))
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match db_delete(&pool, id).await? {
        DeleteOutcome::Deleted => Ok(StatusCode::NO_CONTENT),
        DeleteOutcome::NotFound => Err(ApiError::not_found("no transfer with that id")),
        DeleteOutcome::TransferInReferenced => Err(ApiError::unprocessable(
            "a transferred-in parcel is consumed by a later sale, AMIT adjustment, or income \
             link — remove those first",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action;
    use crate::entities::holding_account::{self, HoldingAccount};
    use crate::entities::sell::{SellBody, SellError};
    use crate::entities::trade::TradeType;
    use crate::test_support::{self, ApiClient, dec};

    async fn test_pool() -> SqlitePool {
        let pool = test_support::test_pool().await;
        // Account 1 ('Default') is seeded; 2 is the employer plan the RSU
        // scenario transfers out of.
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
        )
        .await
        .unwrap();
        pool
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .mic("XNYS")
            .ticker(ticker)
            .name(ticker)
            .security_type(crate::entities::listing::SecurityType::Share)
            .currency("USD")
            .insert(pool)
            .await;
    }

    /// An RSU vest: a Buy at market value in the plan account (account 2).
    async fn insert_vest(pool: &SqlitePool, id: i64, date: NaiveDate, qty: &str, price: &str) {
        test_support::buy(id, 1)
            .date(date)
            .settlement(date)
            .qty(qty.parse().unwrap())
            .price(price.parse().unwrap())
            .currency("USD")
            .fx_rate(dec("1.5"))
            .spot_fx_rate(dec("1.4034"))
            .account(2)
            .insert(pool)
            .await;
    }

    fn body(date: NaiveDate, from: i64, to: i64, allocations: Vec<(i64, &str)>) -> TransferBody {
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
        // The vest's deliberate spot override carries over too, so the AUD
        // cost base is unchanged by the move (the spot rate keeps winning at
        // the deemed acquisition month).
        assert_eq!(t.spot_fx_rate, Some(dec("1.4034")));
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
        // 0.12345678 BTC bought at A$60,000 in account 2.
        insert_crypto(&pool, "0.12345678", "60000").await;

        let group = db_transfer(
            &pool,
            1,
            &body(d(2024, 6, 1), 2, 1, vec![(1, "0.12345678")]),
        )
        .await
        .unwrap();

        let t = &group.transfer_ins[0];
        assert_eq!(t.holding_account_id, 1);
        assert_eq!(
            t.quantity,
            dec("0.12345678"),
            "satoshi-scale quantity preserved"
        );
        // The whole A$7,407.4068 cost base moves on the brokerage column...
        assert_eq!(t.average_price, Decimal::ZERO);
        assert_eq!(t.brokerage, dec("7407.40680000"));
        // ...with the original acquisition date driving the discount clock.
        assert_eq!(t.deemed_acquisition_date, Some(d(2023, 3, 1)));
    }

    /// Moving a parcel between holding accounts is not a CGT event, so the
    /// 12-month clock does not restart: bought 2023-03-01, moved 2024-06-01,
    /// sold 2024-09-01 — only 3 months in the destination account, but
    /// 18 months of ownership, so the whole gain is discount-eligible. A
    /// report anchoring on the transfer-in Buy's own date would call it
    /// non-discountable (SCENARIOS C-10).
    #[tokio::test]
    async fn transferred_parcel_keeps_its_discount_clock_through_a_later_sale() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("AAA")
            .name("AAA")
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(d(2023, 3, 1))
            .settlement(d(2023, 3, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .account(2)
            .insert(&pool)
            .await;

        let group = db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "100")]))
            .await
            .unwrap();
        let parcel = &group.transfer_ins[0];
        assert_eq!(parcel.date, d(2024, 6, 1));
        assert_eq!(parcel.deemed_acquisition_date, Some(d(2023, 3, 1)));

        crate::entities::sell::db_upsert_sell(
            &pool,
            30,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: d(2024, 9, 3),
                settlement_date: Some(d(2024, 9, 3)),
                listing_id: 1,
                average_price: dec("15"),
                quantity: dec("100"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: parcel.id,
                    quantity_allocated: dec("100"),
                }],
            },
        )
        .await
        .unwrap();

        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        // The zero-proceeds transfer-out Sell is not a disposal.
        assert_eq!(realised.len(), 1);
        let g = &realised[0];
        assert_eq!(g.cost_base, dec("1000"));
        assert_eq!(g.proceeds, dec("1500"));
        assert_eq!(g.capital_gain_loss, dec("500"));
        assert_eq!(g.discount_eligible_gain, dec("500"));
        assert_eq!(g.non_discountable_gain, Decimal::ZERO);
        assert_eq!(g.parcels[0].acquisition_date, d(2023, 3, 1));
    }

    /// Seed an AUD-priced BTC `Crypto` listing (id 1) and a single parcel
    /// (trade id 1) of `qty` units bought at A$`price`/unit on 2023-03-01 in
    /// account 2 — held long enough that a 2024-06-01 disposal is
    /// discount-eligible.
    async fn insert_crypto(pool: &SqlitePool, qty: &str, price: &str) {
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .name("Bitcoin")
            .insert(pool)
            .await;
        test_support::buy(1, 1)
            .date(d(2023, 3, 1))
            .settlement(d(2023, 3, 1))
            .qty(dec(qty))
            .price(dec(price))
            .account(2)
            .insert(pool)
            .await;
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
            &fee_body(
                d(2024, 6, 1),
                2,
                1,
                vec![(1, "0.5")],
                vec![(1, "0.001")],
                "80000",
            ),
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
        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
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

        let mut no_price = fee_body(
            d(2024, 6, 1),
            2,
            1,
            vec![(1, "0.5")],
            vec![(1, "0.001")],
            "0",
        );
        no_price.fee_market_price = None;
        assert!(matches!(
            db_transfer(&pool, 1, &no_price).await,
            Err(TransferError::FeeMarketPriceMissing)
        ));

        // A zero price is just as invalid (the fee crypto has market value).
        let zero_price = fee_body(
            d(2024, 6, 1),
            2,
            1,
            vec![(1, "0.5")],
            vec![(1, "0.001")],
            "0",
        );
        assert!(matches!(
            db_transfer(&pool, 1, &zero_price).await,
            Err(TransferError::FeeMarketPriceMissing)
        ));

        let transfers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transfers")
            .fetch_one(&pool)
            .await
            .unwrap();
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
                &fee_body(
                    d(2024, 6, 1),
                    2,
                    1,
                    vec![(1, "0.6")],
                    vec![(1, "0.5")],
                    "80000"
                ),
            )
            .await,
            Err(TransferError::Sell(SellError::PurchaseQuantityExceeded))
        ));
        let transfers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transfers")
            .fetch_one(&pool)
            .await
            .unwrap();
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
            &fee_body(
                d(2024, 6, 1),
                2,
                1,
                vec![(1, "0.5")],
                vec![(1, "0.001")],
                "80000",
            ),
        )
        .await
        .unwrap();
        let fee_id = group.fee_sale.unwrap().id;

        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Deleted);

        assert!(
            trade::db_get(&pool, fee_id).await.unwrap().is_none(),
            "fee Sell gone"
        );
        let allocs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(allocs, 0, "fee + transfer-out allocations freed");
        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
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
            &fee_body(
                d(2024, 6, 1),
                2,
                1,
                vec![(1, "0.5")],
                vec![(1, "0.001")],
                "80000",
            ),
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
                spot_fx_rate: None,
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
                spot_fx_rate: None,
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

        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert!(
            realised.is_empty(),
            "an own-account transfer is not a disposal"
        );
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
                spot_fx_rate: None,
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

        assert_eq!(
            db_delete(&pool, 1).await.unwrap(),
            DeleteOutcome::TransferInReferenced
        );

        // Removing the later sale unblocks the delete.
        assert_eq!(
            sell::db_delete_sell(&pool, 50).await.unwrap(),
            sell::DeleteOutcome::Deleted
        );
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
                spot_fx_rate: None,
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
            trade::db_delete(&pool, group.transfer_ins[0].id)
                .await
                .unwrap(),
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
        let transfers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transfers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(transfers, 0);
        let trades: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE transfer_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(trades, 0);

        // A recorded transfer is immutable: re-PUT of the same id is rejected.
        db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "100")]))
            .await
            .unwrap();
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
        test_support::amma(1, 1)
            .units(dec("100"))
            .cost_base_adjustment(dec("1"))
            .with(|a| {
                a.holding_account_id = 2;
                a.tax_year_end_date = d(2023, 6, 30);
                a.date_received = d(2023, 7, 15);
                a.currency = "USD".to_string();
            })
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("100")).await;

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
                    record_date: None,
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

    /// SCENARIOS N-06: a return of capital paid **on** the transfer date is the
    /// boundary between what the operation folds into the carried cost base and
    /// what the reports still apply to the replacement parcel. It used to fall
    /// inside both, so the payment came off the units twice — a $500 parcel
    /// that had received $100 reported a $400 carried figure and a $300 cost
    /// base. The operation date belongs to the operation.
    #[tokio::test]
    async fn a_return_of_capital_on_the_transfer_date_is_counted_once() {
        // The reported figures are AUD (the vest listing is USD), so the claim
        // is that all three boundary dates agree — one $200 reduction, wherever
        // it is applied — not a spelled-out AUD amount.
        let mut reported = Vec::new();
        for (payment_date, expected_carried) in [
            (d(2024, 5, 31), dec("11800")),
            (d(2024, 6, 1), dec("11800")),
            (d(2024, 6, 2), dec("12000")),
        ] {
            let pool = test_pool().await;
            insert_listing(&pool, 1, "ICE").await;
            insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await; // $12,000
            corporate_action::db_upsert(
                &pool,
                &corporate_action::CorporateAction {
                    id: 5,
                    listing_id: 1,
                    date: payment_date,
                    kind: corporate_action::ActionKind::ReturnOfCapital {
                        amount_per_unit: dec("2"),
                        currency: "USD".to_string(),
                        record_date: None,
                    },
                },
            )
            .await
            .unwrap();

            let group = db_transfer(&pool, 1, &body(d(2024, 6, 1), 2, 1, vec![(1, "100")]))
                .await
                .unwrap();
            assert_eq!(
                group.transfer_ins[0].brokerage, expected_carried,
                "carried cost base for a payment dated {payment_date}"
            );

            // Whichever side of the boundary the payment falls on, the units'
            // reported cost base is the same $11,800 — the payment reduces them
            // exactly once.
            let parcels = crate::domain::open_parcels::load(
                &mut pool.acquire().await.unwrap(),
                Some(d(2024, 6, 30)),
            )
            .await
            .unwrap();
            let replacement = parcels
                .iter()
                .find(|p| p.parcel.id == group.transfer_ins[0].id)
                .expect("the replacement parcel is open");
            reported.push(replacement.cost_base.adjusted);
        }
        assert_eq!(
            reported[0], reported[1],
            "a payment on the transfer date must reduce the units once, like one the day before"
        );
        assert_eq!(
            reported[1], reported[2],
            "and like one the day after, which the replacement parcel receives itself"
        );
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
        let resp = ApiClient::over(app()).put("/transfers/1", &body).await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let v: serde_json::Value = resp.json();
        assert_eq!(v["sell"]["holding_account_id"], 2);
        assert_eq!(v["transfer_ins"][0]["holding_account_id"], 1);
        assert_eq!(v["transfer_ins"][0]["brokerage"], "12000");
        assert_eq!(
            v["transfer_ins"][0]["deemed_acquisition_date"],
            "2023-03-01"
        );

        // GET list/one.
        let resp = ApiClient::over(app()).get("/transfers/1").await;
        assert_eq!(resp.status, StatusCode::OK);

        // DELETE restores; a second DELETE is 404.
        let resp = ApiClient::over(app()).delete("/transfers/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let resp = ApiClient::over(app()).delete("/transfers/1").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
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
        let resp = ApiClient::over(router().with_state(pool.clone()))
            .put("/transfers/1", &body)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(detail.contains("same"), "detail: {detail}");

        // Unknown destination account → FK violation → 422.
        let body = serde_json::json!({
            "listing_id": 1,
            "date": "2024-06-01",
            "from_account_id": 2,
            "to_account_id": 99,
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "100" } ]
        });
        let resp = ApiClient::over(router().with_state(pool))
            .put("/transfers/1", &body)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// SCENARIOS N-04, N-12: each Sell-side rejection a transfer can reach says
    /// what is actually wrong. They all used to answer one sentence listing
    /// every cause at once — which named neither the date case nor the
    /// non-positive one, and told a user with a wrong date that the parcel was
    /// "missing, over-allocated, not a Buy/DRP, or held in a different
    /// account", none of which was true.
    #[tokio::test]
    async fn each_parcel_rejection_names_its_own_cause() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        // The vest parcel sits in account 2, dated 2023-03-01.
        insert_vest(&pool, 1, d(2023, 3, 1), "100", "120").await;
        let client = ApiClient::over(router().with_state(pool.clone()));
        let attempt = async |id: i64, from: i64, to: i64, date: &str, parcel: i64, units: &str| {
            let body = serde_json::json!({
                "listing_id": 1,
                "date": date,
                "from_account_id": from,
                "to_account_id": to,
                "allocations": [ { "purchase_trade_id": parcel, "quantity_allocated": units } ]
            });
            let resp = client.put(format!("/transfers/{id}"), &body).await;
            assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
            resp.text().to_string()
        };

        let detail = attempt(1, 2, 1, "2022-01-01", 1, "100").await;
        assert!(
            detail.contains("dated after the transfer date"),
            "detail: {detail}"
        );
        // SCENARIOS S-10: and a transfer dated after today — the parcels it
        // creates would be dated then too, and would not be held today. The
        // shared `trade::check_amounts` bound reaches every operation that
        // writes its Sell through the Sell core; a transfer's date is the
        // user's own, so it is the one that says so in its own terms.
        let tomorrow = (crate::infra::date::today() + chrono::Days::new(1)).to_string();
        let detail = attempt(3, 2, 1, &tomorrow, 1, "100").await;
        assert!(
            detail.contains("transfer is dated after today"),
            "detail: {detail}"
        );
        let detail = attempt(2, 2, 1, "2024-06-01", 1, "101").await;
        assert!(
            detail.contains("exceed what a selected parcel still holds"),
            "detail: {detail}"
        );
        let detail = attempt(3, 2, 1, "2024-06-01", 99, "100").await;
        assert!(detail.contains("does not exist"), "detail: {detail}");
        let detail = attempt(4, 1, 2, "2024-06-01", 1, "100").await;
        assert!(
            detail.contains("not held in the source account"),
            "detail: {detail}"
        );
        let detail = attempt(5, 2, 1, "2024-06-01", 1, "0").await;
        assert!(detail.contains("positive quantity"), "detail: {detail}");

        // A Sell is not a parcel: allocating one is rejected by type, not by
        // any of the above.
        test_support::sell(50, 1)
            .date(d(2024, 1, 5))
            .qty(dec("10"))
            .account(2)
            .with(|t| t.average_price = dec("130"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 50, 50, 1, dec("10")).await;
        let detail = attempt(6, 2, 1, "2024-06-01", 50, "10").await;
        assert!(
            detail.contains("not a Buy or DRP trade"),
            "detail: {detail}"
        );

        // Nothing persisted by any of them.
        let transfers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transfers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(transfers, 0);
    }

    /// SCENARIOS V-d, the transfer's own output: a transfer-in parcel takes the
    /// **transfer's** date (its deemed acquisition date is the moved parcel's,
    /// decades earlier, and is deliberately not what is compared), so it can
    /// land behind a whole-holding operation of the listing that has already
    /// run.
    ///
    /// Reaching that state needs a source parcel the operation left open, which
    /// nothing can create any more — so it is built the way a pre-guard build
    /// wrote it, straight into `trades`
    /// (`test_support::insert_parcel_bypassing_checks`). Ordinarily the
    /// operation has consumed everything the transfer could draw on and
    /// `SellError::PurchaseQuantityExceeded` refuses first; this is the guard
    /// behind that.
    #[tokio::test]
    async fn transfer_in_dated_before_an_executed_recognise_is_refused() {
        let pool = test_pool().await;
        test_support::recognised_worthless_listing(
            &pool,
            1,
            "DEAD",
            d(2024, 1, 2),
            90,
            d(2024, 6, 13),
        )
        .await;
        // The stranded parcel a pre-guard build could write.
        test_support::insert_parcel_bypassing_checks(&pool, 500, 1, d(2024, 3, 5), "40", "2").await;

        let err = db_transfer(
            &pool,
            1,
            &TransferBody {
                listing_id: 1,
                date: d(2024, 4, 2),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 500,
                    quantity_allocated: dec("40"),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, TransferError::BackDatedOverWholeHolding(_)),
            "expected the whole-holding refusal, got: {err:?}"
        );
        let created: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM transfers WHERE id = 1)")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!created);
    }

    /// Why the three parcel-substituting operations got no cost-base bound of
    /// their own (SCENARIOS W-e): a replacement Buy is written with a **zero**
    /// price and the carried cost base on its `brokerage` column, so its
    /// initial cost is that carried figure and nothing is multiplied. The
    /// source parcel's own cost base is already bounded by
    /// `trade::check_amounts`, so the replacement cannot exceed what the
    /// database could already hold — which this pins from the far end: a
    /// parcel costed at very nearly `Decimal::MAX` transfers, and its
    /// replacement carries the whole figure.
    #[tokio::test]
    async fn a_maximal_cost_base_still_transfers_because_the_replacement_multiplies_nothing() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        // 1e14 units at 7e14 — a cost base of 7e28, just inside the ceiling.
        insert_vest(
            &pool,
            1,
            d(2023, 3, 1),
            "100000000000000",
            "700000000000000",
        )
        .await;

        let group = db_transfer(
            &pool,
            1,
            &body(d(2024, 6, 1), 2, 1, vec![(1, "100000000000000")]),
        )
        .await
        .unwrap();

        let t = &group.transfer_ins[0];
        assert_eq!(t.average_price, Decimal::ZERO);
        assert_eq!(t.brokerage, dec("70000000000000000000000000000"));
    }

    /// "A replacement quantity no `Decimal` can hold" — the transfer's answer
    /// to the question the section's heading raised about it.
    ///
    /// A transfer moves units one for one and applies no ratio of its own, so
    /// it cannot reach the overflow a scrip exchange or demerger does: the
    /// whole of a 1e27-unit holding moves and its replacement is 1e27 units
    /// (the control, first). What it *does* re-base is the units asked for,
    /// back into the parcel's as-acquired basis, and a **consolidation**
    /// between the two dates multiplies that up — so a request naming more
    /// units than could ever have been held overflowed there, a logged `500`
    /// with an empty body, before the over-allocation check that would have
    /// refused it could answer. Refused `422` naming the arithmetic instead,
    /// while a request for units the parcel actually holds still moves.
    #[tokio::test]
    async fn a_moved_quantity_no_decimal_can_hold_is_refused_naming_it() {
        // The control: 1:1, no ratio anywhere, the whole enormous holding
        // moves and the replacement carries every unit.
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "1000000000000000000000000000", "0").await;
        let group = db_transfer(
            &pool,
            1,
            &body(
                d(2024, 6, 3),
                2,
                1,
                vec![(1, "1000000000000000000000000000")],
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            group.transfer_ins[0].quantity,
            dec("1000000000000000000000000000")
        );

        // A 1-for-1000 consolidation between the parcel and the transfer.
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        insert_vest(&pool, 1, d(2023, 3, 1), "1000000000000000000000000000", "0").await;
        crate::entities::corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: d(2023, 6, 1),
                kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                    split_new_units: Decimal::ONE,
                    split_old_units: Decimal::from(1000),
                },
            },
        )
        .await
        .unwrap();

        // The holding is 1e24 units after the consolidation; asking to move
        // the pre-consolidation figure re-bases to 1e30 as-acquired units.
        let err = db_transfer(
            &pool,
            1,
            &body(
                d(2024, 6, 3),
                2,
                1,
                vec![(1, "1000000000000000000000000000")],
            ),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, TransferError::UnrepresentableMovedQuantity(_)),
            "expected the unrepresentable-quantity refusal, got: {err:?}"
        );
        let response = ApiClient::over(router().with_state(pool.clone()))
            .put(
                "/transfers/1",
                &serde_json::json!({
                    "listing_id": 1,
                    "date": "2024-06-03",
                    "from_account_id": 2,
                    "to_account_id": 1,
                    "allocations": [
                        {"purchase_trade_id": 1,
                         "quantity_allocated": "1000000000000000000000000000"}
                    ]
                }),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains(
                "quantity_allocated 1000000000000000000000000000 × old units 1000 / new units 1"
            ),
            "the arithmetic is not named: {detail}"
        );
        assert!(
            detail.contains(&Decimal::MAX.to_string()),
            "the limit is not named: {detail}"
        );

        // The control on the same database: the units actually held move, and
        // the re-base back across the consolidation is exact.
        let group = db_transfer(
            &pool,
            1,
            &body(d(2024, 6, 3), 2, 1, vec![(1, "1000000000000000000000000")]),
        )
        .await
        .unwrap();
        assert_eq!(
            group.transfer_ins[0].quantity,
            dec("1000000000000000000000000")
        );
    }

    /// A `ShareSplit` of listing 1 — a consolidation where `new < old`.
    async fn insert_split(pool: &SqlitePool, id: i64, date: NaiveDate, new: &str, old: &str) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id: 1,
                date,
                kind: corporate_action::ActionKind::ShareSplit {
                    split_new_units: dec(new),
                    split_old_units: dec(old),
                },
            },
        )
        .await
        .unwrap();
    }

    /// The eighth parcel-creating write, and the one the re-based-quantity rule
    /// deliberately does **not** guard — because it cannot reach the bound.
    ///
    /// A transfer's destination listing is its source listing, and a
    /// transfer-in carries at most the units the source parcel held at the
    /// transfer date, so every ratio recorded *after* that date re-bases the
    /// source parcel by the same factor and at least as far. A transfer-in past
    /// the range therefore implies a source parcel past it — which is refused
    /// at the parcel's own write, and again at the action write that would put
    /// it there. This drives that: a 1e26-unit parcel consolidated 1-for-1000
    /// transfers its whole 1e23-unit holding, and the later 1e6-for-1 split
    /// that would take the transfer-in to 1e29 is refused for taking the parcel
    /// it came from to the same place — quoting the *parcel's* 1e26, which is
    /// what says the source is what bounds it.
    #[tokio::test]
    async fn db_a_transfer_in_is_bounded_by_the_parcel_it_moves_from() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BIG").await;
        insert_vest(&pool, 1, d(2024, 1, 16), "100000000000000000000000000", "0").await;
        // A 1-for-1000 consolidation: the holding is 1e23 units from here.
        insert_split(&pool, 10, d(2024, 2, 1), "1", "1000").await;

        db_transfer(
            &pool,
            1,
            &body(d(2024, 3, 1), 2, 1, vec![(1, "100000000000000000000000")]),
        )
        .await
        .unwrap();

        // The split that would re-base the transfer-in to 1e29 is refused,
        // naming the parcel it came from rather than the transfer-in.
        let err = corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 11,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: corporate_action::ActionKind::ShareSplit {
                    split_new_units: dec("1000000"),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap_err();
        let detail = ApiError::from(err).to_string();
        assert!(
            detail.contains("quantity 100000000000000000000000000 × new units 1000000"),
            "the source parcel is not what bounds it: {detail}"
        );

        // And the transfer itself stands, with both legs readable.
        let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
            .get_json("/portfolio/open-parcels")
            .await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["holding_account_id"], 1);
        assert_eq!(rows[0]["remaining_quantity"], "100000000000000000000000");
    }

    // --- fx-default family (code review 2026-08-25) ---

    /// A USD-quoted crypto whose fee disposal must convert USD proceeds. The
    /// parcel carries its own stated rate, so only the *fee Sell* under test
    /// is missing one.
    async fn insert_usd_crypto(pool: &SqlitePool, qty: &str, price: &str) {
        test_support::listing(1)
            .crypto()
            .ticker("ETH")
            .name("Ether")
            .currency("USD")
            .insert(pool)
            .await;
        test_support::buy(1, 1)
            .date(d(2023, 3, 1))
            .settlement(d(2023, 3, 1))
            .qty(dec(qty))
            .price(dec(price))
            .currency("USD")
            .fx_rate(dec("1.5"))
            .account(2)
            .insert(pool)
            .await;
    }

    /// Fx-default family (code review 2026-08-25): the network-fee Sell is a
    /// *real disposal* in the gains reports (no `transfer_id`), and with
    /// `fee_fx_rate` omitted in a month with no imported RBA rate it used to
    /// store parity silently — USD fee proceeds booked 1:1 as AUD. Now
    /// refused like the ESS vest, naming the currency, the month, and the
    /// remedies, with nothing persisted.
    #[tokio::test]
    async fn a_foreign_fee_disposal_with_fx_omitted_in_a_missing_month_is_refused_naming_the_month()
    {
        let pool = test_pool().await;
        insert_usd_crypto(&pool, "1.0", "3000").await;

        let resp = ApiClient::over(router().with_state(pool.clone()))
            .put(
                "/transfers/1",
                &serde_json::json!({
                    "listing_id": 1,
                    "date": "2024-06-01",
                    "from_account_id": 2,
                    "to_account_id": 1,
                    "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "0.5" } ],
                    "fee_allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "0.001" } ],
                    "fee_market_price": "4000",
                }),
            )
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body.contains("USD") && body.contains("2024-06"), "{body}");
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transfers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "nothing persisted");
    }

    /// With the month's rate imported an omitted `fee_fx_rate` resolves to
    /// it — the fee Sell carries the real rate, never parity.
    #[tokio::test]
    async fn a_foreign_fee_disposal_resolves_the_imported_month_rate_when_fx_omitted() {
        let pool = test_pool().await;
        insert_usd_crypto(&pool, "1.0", "3000").await;
        crate::entities::rba_fx_rate::db_import_rate(&pool, "USD", "2024-06", dec("0.5"))
            .await
            .unwrap();

        let group = db_transfer(
            &pool,
            1,
            &fee_body(
                d(2024, 6, 1),
                2,
                1,
                vec![(1, "0.5")],
                vec![(1, "0.001")],
                "4000",
            ),
        )
        .await
        .unwrap();
        let fee = group.fee_sale.as_ref().expect("a fee Sell was created");
        assert_eq!(fee.fx_rate, dec("0.5"));
    }

    /// A stated `fee_fx_rate` is bound as given even in a missing-rate
    /// month — the caller said what the fee converted at.
    #[tokio::test]
    async fn a_stated_fee_fx_rate_is_stored_even_when_the_month_is_missing() {
        let pool = test_pool().await;
        insert_usd_crypto(&pool, "1.0", "3000").await;

        let mut b = fee_body(
            d(2024, 6, 1),
            2,
            1,
            vec![(1, "0.5")],
            vec![(1, "0.001")],
            "4000",
        );
        b.fee_fx_rate = Some(dec("0.62"));
        let group = db_transfer(&pool, 1, &b).await.unwrap();
        let fee = group.fee_sale.as_ref().expect("a fee Sell was created");
        assert_eq!(fee.fx_rate, dec("0.62"));
    }
}
