//! Atomic worthless-shares loss recognition: close every open parcel of a
//! `WorthlessShares` action's listing through a single Sell at nil proceeds,
//! *recognising* the capital loss (CGT events G3 and C2; see
//! `docs/ato/worthless-shares.md`).
//!
//! A failed company's shares can produce a capital loss without an ordinary
//! disposal — either when a liquidator/administrator declares them worthless
//! (CGT event G3, s 104-145) or when the company is deregistered (CGT event C2,
//! s 104-25). Both give a capital loss equal to each parcel's **remaining
//! reduced cost base**; the loss is never income and never discounted. The
//! recognise operation creates, in one transaction:
//!
//! - a **closing Sell** on the listing dated the event date — price 0, with
//!   parcel allocations consuming every open parcel, written through the shared
//!   `/sells` core so all its invariants hold. It carries `worthless_action_id`.
//!
//! Crucially — *unlike* the scrip-for-scrip and demerger closing Sells, whose
//! provenance columns exclude them from the gains reports because the rollover
//! disregards the gain — this Sell is **not** excluded: the realised-gains
//! report treats it as an ordinary disposal at nil proceeds, so each consumed
//! parcel's reduced cost base surfaces as a `capital_loss`, which then flows
//! through the net-capital-gain report's loss pool and carry-forward like any
//! realised loss. There are no replacement Buys (the shares are simply gone).
//!
//! The created Sell forms the (single-trade) worthless group
//! (`trades.worthless_action_id`): it is immutable via `PUT /sells` /
//! `PUT /trades` and protected from individual deletion via `DELETE /trades`;
//! `DELETE /sells` on it restores the pre-event holding; and the action is
//! frozen against edits and deletes while it exists.
//!
//! Out of scope (documented in `docs/ato/worthless-shares.md`): the G3 opt-in
//! eligibility tests (the user's determination), the cost-base-reset-to-nil
//! bookkeeping for shares still held after a G3 declaration (the operation
//! closes the whole holding), worthless *financial instruments* other than
//! shares, and the 18-month later-recovery timing rule.

use crate::infra::http::ApiError;
use crate::entities::corporate_action::{self, ActionKind, sold_in_acquired_units, split_adjusted_quantity};
use crate::entities::sell::{self, AllocationInput, SellBody};
use crate::entities::trade::{self, Trade};
use crate::infra::decimal::parse_dec;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// The result of recognising a worthless holding: the closing Sell that
/// consumed every open parcel at nil proceeds.
#[derive(Debug, Serialize)]
pub struct Recognise {
    pub sell: Trade,
}

#[derive(Debug)]
pub enum RecogniseError {
    Db(sqlx::Error),
    /// No corporate action with that id.
    ActionNotFound,
    /// The action is not a WorthlessShares.
    NotWorthlessShares,
    /// The action has already been recognised (a Sell references it). Delete
    /// the closing Sell via `DELETE /sells` first to redo it.
    AlreadyRecognised,
    /// Nothing of the listing is held at the event date — there is no loss to
    /// recognise.
    NothingHeld,
    /// The listing has a trade dated on or after the event date. The recognise
    /// closes every open parcel as at that date, so later-dated activity would
    /// draw on parcels the closing Sell consumes (and a failed/delisted company
    /// does not trade on after the event) — fix the data first.
    TradedOnOrAfterEventDate,
    /// The Sell-side invariants failed (defensive: the recognise constructs its
    /// allocations to satisfy them).
    Sell(sell::SellError),
}

impl From<sqlx::Error> for RecogniseError {
    fn from(e: sqlx::Error) -> Self {
        RecogniseError::Db(e)
    }
}

impl From<sell::SellError> for RecogniseError {
    fn from(e: sell::SellError) -> Self {
        match e {
            sell::SellError::Db(err) => RecogniseError::Db(err),
            other => RecogniseError::Sell(other),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/corporate_actions/{id}/recognise", post(recognise))
}

/// Close every open parcel of the action's listing through a single Sell at nil
/// proceeds, recognising the capital loss, atomically. The recognise takes no
/// parameters: the action and the holdings at its date determine everything.
pub async fn db_recognise(pool: &SqlitePool, action_id: i64) -> Result<Recognise, RecogniseError> {
    let mut tx = pool.begin().await?;

    let action = match corporate_action::db_get_tx(&mut *tx, action_id).await? {
        Some(a) => a,
        None => return Err(RecogniseError::ActionNotFound),
    };
    if !matches!(action.kind, ActionKind::WorthlessShares { .. }) {
        return Err(RecogniseError::NotWorthlessShares);
    }

    let already: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE worthless_action_id = ?)")
            .bind(action_id)
            .fetch_one(&mut *tx)
            .await?;
    if already {
        return Err(RecogniseError::AlreadyRecognised);
    }

    // Every trade of the listing must predate the event — the company has
    // failed, so a later-dated trade contradicts the action and would draw on
    // parcels the closing Sell consumes.
    let late_trade: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM trades WHERE listing_id = ? AND date >= ?)",
    )
    .bind(action.listing_id)
    .bind(action.date)
    .fetch_one(&mut *tx)
    .await?;
    if late_trade {
        return Err(RecogniseError::TradedOnOrAfterEventDate);
    }

    // The listing's open parcels (same remaining-quantity rule as the
    // open-parcels report: as-acquired units internally; allocations re-based
    // across splits). The loss itself — each parcel's remaining reduced cost
    // base — is computed by the realised-gains report from the closing Sell's
    // nil proceeds; here we only need the units to consume per parcel.
    let parcel_rows = sqlx::query(
        "SELECT id, date, quantity FROM trades \
         WHERE listing_id = ? AND trade_type IN ('Buy', 'DRP') ORDER BY date, id",
    )
    .bind(action.listing_id)
    .fetch_all(&mut *tx)
    .await?;

    let alloc_rows = sqlx::query(
        "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date \
         FROM parcel_allocations pa \
         JOIN trades s ON s.id = pa.sale_trade_id \
         JOIN trades p ON p.id = pa.purchase_trade_id \
         WHERE p.listing_id = ?",
    )
    .bind(action.listing_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut qty_sold: HashMap<i64, Vec<(NaiveDate, Decimal)>> = HashMap::new();
    for row in &alloc_rows {
        let tid: i64 = row.try_get("purchase_trade_id")?;
        qty_sold.entry(tid).or_default().push((
            row.try_get("sale_date")?,
            parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
        ));
    }

    let splits = corporate_action::db_splits_for_listing(&mut *tx, action.listing_id).await?;

    // Per open parcel, the units still held at the event date (split-adjusted).
    let mut allocations: Vec<AllocationInput> = Vec::new();
    for row in &parcel_rows {
        let parcel_id: i64 = row.try_get("id")?;
        let date: NaiveDate = row.try_get("date")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;

        let sold = sold_in_acquired_units(
            qty_sold.get(&parcel_id).map_or(&[][..], |v| v),
            &splits,
            date,
        );
        let remaining = qty - sold;
        if remaining <= Decimal::ZERO {
            continue;
        }
        // The closing Sell's allocations are in event-date units (the report
        // re-bases them back to as-acquired units when pro-rating cost base).
        let at_date_units = split_adjusted_quantity(remaining, &splits, date, Some(action.date));
        allocations.push(AllocationInput {
            purchase_trade_id: parcel_id,
            quantity_allocated: at_date_units,
        });
    }
    if allocations.is_empty() {
        return Err(RecogniseError::NothingHeld);
    }

    // The closing Sell: zero proceeds, consuming every open parcel across every
    // holding account. Settlement is the event date — nothing market-settles.
    // Like the scrip/demerger closing Sells it is exempt from the per-account
    // allocation check (it closes the whole holding); the loss rows identify
    // this Sell's account, with totals unchanged for the one taxpayer.
    let listing_currency: String =
        sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
            .bind(action.listing_id)
            .fetch_one(&mut *tx)
            .await?;
    let sell_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM trades")
        .fetch_one(&mut *tx)
        .await?;
    let sell_body = SellBody {
        brokerage_includes_gst: false,
        statement_total: None,
        holding_account_id: 1,
        date: action.date,
        settlement_date: Some(action.date),
        listing_id: action.listing_id,
        average_price: Decimal::ZERO,
        quantity: allocations.iter().map(|a| a.quantity_allocated).sum(),
        currency: listing_currency.clone(),
        brokerage: Decimal::ZERO,
        gst_on_brokerage: Decimal::ZERO,
        brokerage_currency: listing_currency,
        fx_rate: Decimal::ONE,
        contract_note_ref: None,
        allocations,
    };
    sell::upsert_sell_in_tx(
        &mut tx,
        sell_id,
        &sell_body,
        action.date,
        None,
        None,
        None,
        None,
        Some(action_id),
    )
    .await?;

    tx.commit().await?;

    let sell = trade::db_get(pool, sell_id)
        .await?
        .ok_or_else(|| RecogniseError::Db(sqlx::Error::RowNotFound))?;
    Ok(Recognise { sell })
}

async fn recognise(
    State(pool): State<SqlitePool>,
    Path(action_id): Path<i64>,
) -> Result<(StatusCode, Json<Recognise>), ApiError> {
    let recognise = db_recognise(&pool, action_id).await?;
    Ok((StatusCode::CREATED, Json(recognise)))
}

impl From<RecogniseError> for ApiError {
    fn from(e: RecogniseError) -> Self {
        match e {
            RecogniseError::ActionNotFound => {
                ApiError::not_found("no corporate action with that id")
            }
            RecogniseError::NotWorthlessShares => {
                ApiError::unprocessable("that corporate action is not a worthless-shares event")
            }
            RecogniseError::AlreadyRecognised => ApiError::unprocessable(
                "this worthless-shares loss has already been recognised — \
                 delete its closing Sell first to redo it",
            ),
            RecogniseError::NothingHeld => {
                ApiError::unprocessable("nothing of the listing is held at the event date")
            }
            RecogniseError::TradedOnOrAfterEventDate => ApiError::unprocessable(
                "the listing has a trade dated on or after the event date — \
                 fix that trade before recognising",
            ),
            RecogniseError::Sell(err) => {
                tracing::warn!(
                    error = ?err,
                    "worthless-shares recognise rejected by a sell invariant"
                );
                ApiError::unprocessable("the recognise's parcel allocations are invalid")
            }
            RecogniseError::Db(err) => err.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action::{CorporateAction, WorthlessEvent};
    use crate::entities::trade::TradeType;
    use crate::entities::listing;
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
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
                exchange_mic: Some("XASX".to_string()),
                ticker: ticker.to_string(),
                name: ticker.to_string(),
                isin: None,
                security_type: listing::SecurityType::Share,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_buy(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        qty: &str,
        price: &str,
    ) {
        trade::db_upsert(
            pool,
            &Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id,
                trade_type: TradeType::Buy,
                date,
                settlement_date: date,
                listing_id,
                average_price: price.parse().unwrap(),
                quantity: qty.parse().unwrap(),
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
            },
        )
        .await
        .unwrap();
    }

    async fn insert_worthless(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate, event: WorthlessEvent) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id,
                date,
                kind: ActionKind::WorthlessShares { worthless_event: event },
            },
        )
        .await
        .unwrap();
    }

    // DB-level tests

    /// The core recognition: every open parcel is closed by a single
    /// zero-proceeds Sell consuming all of it, and each parcel's reduced cost
    /// base surfaces as a capital loss in the realised-gains report (never a
    /// gain, never discounted).
    #[tokio::test]
    async fn recognise_closes_parcels_and_records_the_capital_loss() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        // Two parcels: 1,000 @ $1.50 (2020) and 500 @ $2.00 (2023) = $1,500 + $1,000.
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_buy(&pool, 2, 1, d(2023, 3, 1), "500", "2.00").await;
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::G3Declaration).await;

        let r = db_recognise(&pool, 10).await.unwrap();

        assert_eq!(r.sell.trade_type, TradeType::Sell);
        assert_eq!(r.sell.listing_id, 1);
        assert_eq!(r.sell.date, d(2025, 3, 31));
        assert_eq!(r.sell.quantity, dec("1500"));
        assert_eq!(r.sell.average_price, Decimal::ZERO);
        assert_eq!(r.sell.worthless_action_id, Some(10));

        // Both parcels consumed.
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations WHERE sale_trade_id = ?")
                .bind(r.sell.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 2);

        // The realised-gains report recognises the loss = total reduced cost base.
        let gains = crate::reports::realised_gains::db_realised_gains(&pool).await.unwrap();
        assert_eq!(gains.len(), 1);
        let g = &gains[0];
        assert_eq!(g.sale_trade_id, r.sell.id);
        assert_eq!(g.proceeds, Decimal::ZERO);
        assert_eq!(g.cost_base, dec("2500"));
        assert_eq!(g.capital_gain_loss, dec("-2500"));
        assert_eq!(g.capital_loss, dec("2500"));
        // A loss is never discounted, whatever the holding period.
        assert_eq!(g.discount_eligible_gain, Decimal::ZERO);
        assert_eq!(g.non_discountable_gain, Decimal::ZERO);
    }

    /// A partly sold parcel contributes only its remaining units' reduced cost
    /// base to the loss; the holding is fully closed afterwards.
    #[tokio::test]
    async fn recognise_uses_only_the_remaining_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await; // $1,500
        // Sell 400 before the failure → 600 remain at 60% = $900 reduced cost base.
        sell::db_upsert_sell(
            &pool,
            2,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: d(2022, 5, 1),
                settlement_date: Some(d(2022, 5, 1)),
                listing_id: 1,
                average_price: dec("2.00"),
                quantity: dec("400"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                allocations: vec![AllocationInput { purchase_trade_id: 1, quantity_allocated: dec("400") }],
            },
        )
        .await
        .unwrap();
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::C2Cancellation).await;

        let r = db_recognise(&pool, 10).await.unwrap();
        assert_eq!(r.sell.quantity, dec("600"));

        // The closing Sell's loss is the remaining $900; the earlier Sell's
        // $200 gain (400 × ($2.00 − $1.50)) is a separate row.
        let gains = crate::reports::realised_gains::db_realised_gains(&pool).await.unwrap();
        let closing = gains.iter().find(|g| g.sale_trade_id == r.sell.id).unwrap();
        assert_eq!(closing.cost_base, dec("900"));
        assert_eq!(closing.capital_loss, dec("900"));
    }

    /// The recognised loss reaches the net-capital-gain report's loss pool and
    /// carries forward like any realised loss.
    #[tokio::test]
    async fn recognised_loss_feeds_the_net_capital_gain_loss_pool() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        insert_buy(&pool, 1, 1, d(2023, 7, 1), "1000", "2.00").await; // $2,000
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::G3Declaration).await;
        db_recognise(&pool, 10).await.unwrap();

        let years = crate::reports::net_capital_gain::db_net_capital_gain(&pool).await.unwrap();
        // FY2024/25 (year ending 30 June 2025) carries the $2,000 loss forward
        // (no gains to offset).
        let y = years.iter().find(|y| y.tax_year == 2025).unwrap();
        assert_eq!(y.capital_losses, dec("2000"));
        assert_eq!(y.net_capital_gain, Decimal::ZERO);
        assert_eq!(y.capital_loss_carried_forward, dec("2000"));
    }

    #[tokio::test]
    async fn invalid_recognitions_are_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;

        // Missing action.
        assert!(matches!(db_recognise(&pool, 99).await, Err(RecogniseError::ActionNotFound)));

        // Not a WorthlessShares.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: d(2025, 3, 31),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("2"),
                    split_old_units: dec("1"),
                },
            },
        )
        .await
        .unwrap();
        assert!(matches!(db_recognise(&pool, 1).await, Err(RecogniseError::NotWorthlessShares)));

        // Nothing held at the event date.
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::G3Declaration).await;
        assert!(matches!(db_recognise(&pool, 10).await, Err(RecogniseError::NothingHeld)));

        // A trade dated on/after the event date contradicts the failure.
        insert_buy(&pool, 2, 1, d(2025, 3, 31), "100", "1.50").await;
        assert!(matches!(
            db_recognise(&pool, 10).await,
            Err(RecogniseError::TradedOnOrAfterEventDate)
        ));

        // Nothing was persisted by any of the rejections.
        let trades: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE worthless_action_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(trades, 0);
    }

    #[tokio::test]
    async fn a_second_recognition_of_the_same_action_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::G3Declaration).await;
        db_recognise(&pool, 10).await.unwrap();

        assert!(matches!(db_recognise(&pool, 10).await, Err(RecogniseError::AlreadyRecognised)));
    }

    /// The closing Sell is immutable individually: `PUT /sells` rejects it and
    /// it cannot be deleted via `DELETE /trades`.
    #[tokio::test]
    async fn recognise_sell_is_immutable_individually() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::G3Declaration).await;
        let r = db_recognise(&pool, 10).await.unwrap();

        let err = sell::db_upsert_sell(
            &pool,
            r.sell.id,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: d(2025, 3, 31),
                settlement_date: Some(d(2025, 3, 31)),
                listing_id: 1,
                average_price: dec("9.99"),
                quantity: dec("1000"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                allocations: vec![AllocationInput { purchase_trade_id: 1, quantity_allocated: dec("1000") }],
            },
        )
        .await;
        assert!(matches!(err, Err(sell::SellError::WorthlessSell)));

        assert_eq!(
            trade::db_delete(&pool, r.sell.id).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );
    }

    /// `DELETE /sells` on the closing Sell restores the pre-event holding and
    /// thaws the action.
    #[tokio::test]
    async fn deleting_the_closing_sell_restores_the_holding() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::G3Declaration).await;
        let r = db_recognise(&pool, 10).await.unwrap();

        assert_eq!(sell::db_delete_sell(&pool, r.sell.id).await.unwrap(), sell::DeleteOutcome::Deleted);
        // The original parcel is open again (no allocations remain).
        let allocs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(allocs, 0);
        let worthless_trades: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE worthless_action_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(worthless_trades, 0);
        // The action thaws: it can be deleted again.
        assert!(corporate_action::db_delete(&pool, 10).await.unwrap());
    }

    /// The action is frozen while its recognise Sell exists.
    #[tokio::test]
    async fn referenced_action_cannot_be_edited_or_deleted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::G3Declaration).await;
        db_recognise(&pool, 10).await.unwrap();

        let action = corporate_action::db_get(&pool, 10).await.unwrap().unwrap();
        assert!(matches!(
            corporate_action::db_upsert(&pool, &action).await,
            Err(corporate_action::WriteError::ReferencedByTrade)
        ));
        assert!(corporate_action::db_delete(&pool, 10).await.is_err());
    }

    /// The recognise closes parcels across every holding account.
    #[tokio::test]
    async fn recognise_closes_parcels_in_every_account() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount { id: 2, name: "ICE Employee Plan".to_string() },
        )
        .await
        .unwrap();
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_buy(&pool, 2, 1, d(2023, 3, 1), "500", "2.00").await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::G3Declaration).await;

        let r = db_recognise(&pool, 10).await.unwrap();
        assert_eq!(r.sell.quantity, dec("1500"));
        let gains = crate::reports::realised_gains::db_realised_gains(&pool).await.unwrap();
        assert_eq!(gains[0].capital_loss, dec("2500"));
    }

    // API-level tests

    #[tokio::test]
    async fn api_recognise_creates_the_closing_sell() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::G3Declaration).await;

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/10/recognise")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["sell"]["quantity"], "1000");
        assert_eq!(v["sell"]["average_price"], "0");
        assert_eq!(v["sell"]["worthless_action_id"], 10);
    }

    #[tokio::test]
    async fn api_recognise_maps_errors_to_statuses() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;

        // Missing action → 404.
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/99/recognise")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Nothing held → 422.
        insert_worthless(&pool, 10, 1, d(2025, 3, 31), WorthlessEvent::G3Declaration).await;
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/10/recognise")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
