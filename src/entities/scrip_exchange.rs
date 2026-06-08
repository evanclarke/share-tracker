//! Atomic scrip-for-scrip exchange: substitute every open parcel of a
//! `ScripForScrip` action's original (target) listing with parcels of the
//! replacement listing, applying the rollover (Subdiv 124-M; see
//! `docs/ato/takeovers-and-scrip-for-scrip.md`).
//!
//! The rollover disregards the capital gain on the original shares and deems
//! the replacement shares acquired *for the cost base of the original
//! interest*, with the combined holding period counting toward the 12-month
//! CGT discount. The exchange therefore creates, in one transaction:
//!
//! - a **closing Sell** on the original listing dated the exchange date —
//!   price 0, with parcel allocations consuming every open parcel, written
//!   through the shared `/sells` core so all its invariants hold. It carries
//!   `scrip_action_id`, which excludes it from the realised-gains and
//!   net-capital-gain reports (the disposal happens, but its gain is
//!   disregarded; the zero proceeds never surface as a loss), and
//! - one **replacement Buy** per consumed parcel on the replacement listing,
//!   dated the exchange date (so later splits and returns of capital on the
//!   replacement listing apply only from then), with quantity = the parcel's
//!   remaining units at the exchange date × the exchange ratio. The parcel's
//!   remaining reduced cost base (AMIT- and return-of-capital-adjusted,
//!   floored at nil) is carried on the `brokerage` column with a zero price —
//!   numerically part of the single cost base everywhere, with no division —
//!   and the parcel's acquisition date (chaining through any earlier
//!   exchange) is carried as `deemed_acquisition_date`, which drives the
//!   discount clock and the AUD translation month of the cost base in the
//!   reports. The trade's `currency` and manual `fx_rate` fallback also carry
//!   over, so a non-AUD parcel's AUD cost base is unchanged by the exchange.
//!
//! The created trades form the exchange group (`trades.scrip_action_id`):
//! each is immutable via `PUT /sells` / `PUT /trades` and protected from
//! individual deletion via `DELETE /trades`; `DELETE /sells` on the closing
//! Sell removes the whole group (refused while a replacement Buy is consumed
//! by later allocations or AMIT adjustments); and the action is frozen
//! against edits and deletes while the group exists.
//!
//! Out of scope (documented in `docs/ato/takeovers-and-scrip-for-scrip.md`):
//! takeovers without rollover (an ordinary market-value disposal — enter the
//! Sell and Buy manually), partial rollover with a cash component, multiple
//! replacement share classes, pre-CGT originals, and exchanges that would
//! crystallise a capital loss (the law does not allow rolling over a loss).

use crate::entities::corporate_action::{
    self, ActionKind, RocEvent, per_unit_reduction, sold_in_acquired_units,
    split_adjusted_quantity,
};
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

/// The two sides of an exchange: the closing Sell on the original listing
/// and the replacement Buys it was substituted with (one per consumed
/// parcel, in the original parcels' date order).
#[derive(Debug, Serialize)]
pub struct Exchange {
    pub sell: Trade,
    pub replacements: Vec<Trade>,
}

#[derive(Debug)]
pub enum ExchangeError {
    Db(sqlx::Error),
    /// No corporate action with that id.
    ActionNotFound,
    /// The action is not a ScripForScrip.
    NotAScripForScrip,
    /// The action has already been exchanged (trades reference it). Delete
    /// the closing Sell via `DELETE /sells` first to redo it.
    AlreadyExchanged,
    /// Nothing of the original listing is held at the exchange date — there
    /// is nothing to substitute.
    NothingHeld,
    /// The original listing has a trade dated on or after the exchange date.
    /// The takeover delisted it, so such a trade contradicts the action —
    /// fix the data before exchanging.
    TradedOnOrAfterExchangeDate,
    /// The Sell-side invariants failed (defensive: the exchange constructs
    /// its allocations to satisfy them).
    Sell(sell::SellError),
}

impl From<sqlx::Error> for ExchangeError {
    fn from(e: sqlx::Error) -> Self {
        ExchangeError::Db(e)
    }
}

impl From<sell::SellError> for ExchangeError {
    fn from(e: sell::SellError) -> Self {
        match e {
            sell::SellError::Db(err) => ExchangeError::Db(err),
            other => ExchangeError::Sell(other),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/corporate_actions/{id}/exchange", post(exchange))
}

/// Substitute every open parcel of the action's original listing with
/// replacement-listing parcels, atomically. The exchange takes no parameters:
/// the action's terms and the holdings at its date determine everything.
pub async fn db_exchange(pool: &SqlitePool, action_id: i64) -> Result<Exchange, ExchangeError> {
    let mut tx = pool.begin().await?;

    let action = match corporate_action::db_get_tx(&mut *tx, action_id).await? {
        Some(a) => a,
        None => return Err(ExchangeError::ActionNotFound),
    };
    let (scrip_listing_id, new_units, old_units) = match &action.kind {
        ActionKind::ScripForScrip { scrip_listing_id, scrip_new_units, scrip_old_units } => {
            (*scrip_listing_id, *scrip_new_units, *scrip_old_units)
        }
        _ => return Err(ExchangeError::NotAScripForScrip),
    };

    let already: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE scrip_action_id = ?)")
            .bind(action_id)
            .fetch_one(&mut *tx)
            .await?;
    if already {
        return Err(ExchangeError::AlreadyExchanged);
    }

    // Every trade of the original listing must predate the exchange — the
    // takeover delisted it, so a later-dated trade contradicts the action.
    let late_trade: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM trades WHERE listing_id = ? AND date >= ?)",
    )
    .bind(action.listing_id)
    .bind(action.date)
    .fetch_one(&mut *tx)
    .await?;
    if late_trade {
        return Err(ExchangeError::TradedOnOrAfterExchangeDate);
    }

    // The original listing's open parcels, with the same remaining-quantity
    // and reduced-cost-base rules as the open-parcels report (as-acquired
    // units internally; allocations re-based across splits).
    let parcel_rows = sqlx::query(
        "SELECT id, date, quantity, average_price, brokerage, gst_on_brokerage, currency, \
                fx_rate, deemed_acquisition_date, holding_account_id \
         FROM trades WHERE listing_id = ? AND trade_type IN ('Buy', 'DRP') ORDER BY date, id",
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

    let splits =
        corporate_action::db_splits_for_listing(&mut *tx, action.listing_id).await?;
    let roc_rows = sqlx::query(
        "SELECT date, amount_per_unit, currency FROM corporate_actions \
         WHERE action_type = 'ReturnOfCapital' AND listing_id = ? ORDER BY date, id",
    )
    .bind(action.listing_id)
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

    /// A replacement Buy to create: the consumed parcel's exchange-date units
    /// scaled by the ratio, its remaining reduced cost base, and its carried
    /// acquisition date.
    struct Replacement {
        parcel_id: i64,
        at_date_units: Decimal,
        new_quantity: Decimal,
        carried_cost_base: Decimal,
        currency: String,
        fx_rate: Decimal,
        deemed_acquisition_date: NaiveDate,
        /// A replacement parcel stays in the account of the parcel it
        /// substitutes.
        holding_account_id: i64,
    }

    let mut replacements: Vec<Replacement> = Vec::new();
    for row in &parcel_rows {
        let parcel_id: i64 = row.try_get("id")?;
        let date: NaiveDate = row.try_get("date")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        let price = parse_dec("average_price", row.try_get("average_price")?)?;
        let brok = parse_dec("brokerage", row.try_get("brokerage")?)?;
        let gst = parse_dec("gst_on_brokerage", row.try_get("gst_on_brokerage")?)?;
        let currency: String = row.try_get("currency")?;
        let fx_rate = parse_dec("fx_rate", row.try_get("fx_rate")?)?;
        let deemed: Option<NaiveDate> = row.try_get("deemed_acquisition_date")?;
        let holding_account_id: i64 = row.try_get("holding_account_id")?;

        let sold = sold_in_acquired_units(
            qty_sold.get(&parcel_id).map_or(&[][..], |v| v),
            &splits,
            date,
        );
        let remaining = qty - sold;
        if remaining <= Decimal::ZERO {
            continue;
        }

        // Remaining reduced cost base in the parcel's own currency: the
        // open-parcels report's formula (AMIT-reduced, then return-of-capital
        // payments on the remaining units, both flooring at nil).
        let initial_cost = price * qty + brok + gst;
        let amit = *amit_reductions.get(&parcel_id).unwrap_or(&Decimal::ZERO);
        let net_cost = (initial_cost - amit).max(Decimal::ZERO);
        let roc_per_unit =
            per_unit_reduction(&roc_events, &splits, &currency, date, Some(action.date))?;
        let carried_cost_base = if qty > Decimal::ZERO {
            (net_cost * remaining / qty - roc_per_unit * remaining).max(Decimal::ZERO)
        } else {
            Decimal::ZERO
        };

        // The exchange ratio applies to units as held at the exchange date.
        let at_date_units = split_adjusted_quantity(remaining, &splits, date, Some(action.date));
        replacements.push(Replacement {
            parcel_id,
            at_date_units,
            new_quantity: at_date_units * new_units / old_units,
            currency,
            fx_rate,
            carried_cost_base,
            // Chain through an earlier exchange: the clock always runs from
            // the first acquisition in the rollover chain.
            deemed_acquisition_date: deemed.unwrap_or(date),
            holding_account_id,
        });
    }
    if replacements.is_empty() {
        return Err(ExchangeError::NothingHeld);
    }

    // The closing Sell: zero proceeds (the rollover disregards the gain and
    // this Sell never reaches the realised-gains report), consuming every
    // open parcel. Settlement is the exchange date — nothing market-settles.
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
        quantity: replacements.iter().map(|r| r.at_date_units).sum(),
        currency: listing_currency.clone(),
        brokerage: Decimal::ZERO,
        gst_on_brokerage: Decimal::ZERO,
        brokerage_currency: listing_currency,
        fx_rate: Decimal::ONE,
        contract_note_ref: None,
        allocations: replacements
            .iter()
            .map(|r| AllocationInput {
                purchase_trade_id: r.parcel_id,
                quantity_allocated: r.at_date_units,
            })
            .collect(),
    };
    sell::upsert_sell_in_tx(&mut tx, sell_id, &sell_body, action.date, None, Some(action_id), None, None, None)
        .await?;

    // The replacement Buys: one per consumed parcel, dated the exchange date,
    // carrying the parcel's cost base (on the brokerage column, price 0) and
    // acquisition date.
    let mut replacement_ids = Vec::with_capacity(replacements.len());
    for (i, r) in replacements.iter().enumerate() {
        let buy_id = sell_id + 1 + i as i64;
        sqlx::query(
            "INSERT INTO trades \
             (id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
              currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, \
              scrip_action_id, deemed_acquisition_date, holding_account_id) \
             VALUES (?, 'Buy', ?, ?, ?, '0', ?, ?, ?, '0', ?, ?, ?, ?, ?)",
        )
        .bind(buy_id)
        .bind(action.date)
        .bind(action.date)
        .bind(scrip_listing_id)
        .bind(r.new_quantity.to_string())
        .bind(&r.currency)
        .bind(r.carried_cost_base.to_string())
        .bind(&r.currency)
        .bind(r.fx_rate.to_string())
        .bind(action_id)
        .bind(r.deemed_acquisition_date)
        .bind(r.holding_account_id)
        .execute(&mut *tx)
        .await?;
        replacement_ids.push(buy_id);
    }

    tx.commit().await?;

    // Read the freshly created rows back so the response is exactly what was
    // stored.
    let sell = trade::db_get(pool, sell_id)
        .await?
        .ok_or_else(|| ExchangeError::Db(sqlx::Error::RowNotFound))?;
    let mut created = Vec::with_capacity(replacement_ids.len());
    for id in replacement_ids {
        created.push(
            trade::db_get(pool, id)
                .await?
                .ok_or_else(|| ExchangeError::Db(sqlx::Error::RowNotFound))?,
        );
    }
    Ok(Exchange { sell, replacements: created })
}

async fn exchange(
    State(pool): State<SqlitePool>,
    Path(action_id): Path<i64>,
) -> Result<(StatusCode, Json<Exchange>), (StatusCode, String)> {
    let unprocessable = |msg: &str| Err((StatusCode::UNPROCESSABLE_ENTITY, msg.to_string()));
    match db_exchange(&pool, action_id).await {
        Ok(exchange) => Ok((StatusCode::CREATED, Json(exchange))),
        Err(ExchangeError::ActionNotFound) => {
            Err((StatusCode::NOT_FOUND, "no corporate action with that id".to_string()))
        }
        Err(ExchangeError::NotAScripForScrip) => {
            unprocessable("that corporate action is not a scrip-for-scrip exchange")
        }
        Err(ExchangeError::AlreadyExchanged) => unprocessable(
            "this exchange has already been applied — delete its closing Sell first to redo it",
        ),
        Err(ExchangeError::NothingHeld) => {
            unprocessable("nothing of the original listing is held at the exchange date")
        }
        Err(ExchangeError::TradedOnOrAfterExchangeDate) => unprocessable(
            "the original listing has a trade dated on or after the exchange date — \
             fix that trade before exchanging",
        ),
        Err(ExchangeError::Sell(e)) => {
            tracing::warn!(error = ?e, "scrip-for-scrip exchange rejected by a sell invariant");
            unprocessable("the exchange's parcel allocations are invalid")
        }
        Err(ExchangeError::Db(e)) => {
            tracing::error!(error = %e, "scrip-for-scrip exchange failed");
            Err((StatusCode::INTERNAL_SERVER_ERROR, String::new()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::trade::TradeType;
    use crate::entities::{corporate_action::CorporateAction, listing};
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

    /// A 2-for-1 takeover of listing 1 by listing 2 on the given date: every
    /// 1 unit of listing 1 becomes 2 units of listing 2.
    async fn insert_scrip(pool: &SqlitePool, id: i64, date: NaiveDate) {
        insert_scrip_terms(pool, id, 1, 2, date, "2", "1").await;
    }

    async fn insert_scrip_terms(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        scrip_listing_id: i64,
        date: NaiveDate,
        new: &str,
        old: &str,
    ) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id,
                date,
                kind: ActionKind::ScripForScrip {
                    scrip_listing_id,
                    scrip_new_units: new.parse().unwrap(),
                    scrip_old_units: old.parse().unwrap(),
                },
            },
        )
        .await
        .unwrap();
    }

    async fn sell_units(pool: &SqlitePool, sell_id: i64, parcel_id: i64, date: NaiveDate, qty: &str) {
        sell::db_upsert_sell(
            pool,
            sell_id,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date,
                settlement_date: Some(date),
                listing_id: 1,
                average_price: dec("10"),
                quantity: dec(qty),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: parcel_id,
                    quantity_allocated: dec(qty),
                }],
            },
        )
        .await
        .unwrap();
    }

    // DB-level tests

    /// The core substitution: each open parcel becomes a replacement parcel
    /// carrying its cost base and acquisition date; the original holding is
    /// closed by a zero-proceeds Sell consuming every parcel.
    #[tokio::test]
    async fn exchange_substitutes_parcels_carrying_cost_base_and_acquisition_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // Two parcels: 1,000 @ $1.50 (2020) and 500 @ $2.00 (2023).
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_buy(&pool, 2, 1, d(2023, 3, 1), "500", "2.00").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let ex = db_exchange(&pool, 10).await.unwrap();

        // The closing Sell: all 1,500 units at zero proceeds on the exchange
        // date, marked with the action's provenance.
        assert_eq!(ex.sell.trade_type, TradeType::Sell);
        assert_eq!(ex.sell.listing_id, 1);
        assert_eq!(ex.sell.date, d(2024, 7, 1));
        assert_eq!(ex.sell.settlement_date, d(2024, 7, 1));
        assert_eq!(ex.sell.quantity, dec("1500"));
        assert_eq!(ex.sell.average_price, Decimal::ZERO);
        assert_eq!(ex.sell.scrip_action_id, Some(10));

        // Two replacement Buys on listing 2, 2-for-1, each carrying its
        // parcel's cost base (price 0 + brokerage = exact) and acquisition
        // date, dated at the exchange.
        assert_eq!(ex.replacements.len(), 2);
        let r1 = &ex.replacements[0];
        assert_eq!(r1.trade_type, TradeType::Buy);
        assert_eq!(r1.listing_id, 2);
        assert_eq!(r1.date, d(2024, 7, 1));
        assert_eq!(r1.quantity, dec("2000"));
        assert_eq!(r1.average_price, Decimal::ZERO);
        assert_eq!(r1.brokerage, dec("1500"));
        assert_eq!(r1.deemed_acquisition_date, Some(d(2020, 10, 1)));
        assert_eq!(r1.scrip_action_id, Some(10));
        let r2 = &ex.replacements[1];
        assert_eq!(r2.quantity, dec("1000"));
        assert_eq!(r2.brokerage, dec("1000"));
        assert_eq!(r2.deemed_acquisition_date, Some(d(2023, 3, 1)));

        // The allocations consume both parcels exactly.
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations WHERE sale_trade_id = ?")
                .bind(ex.sell.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 2);
    }

    /// A partly sold parcel carries only its remaining units' share of the
    /// cost base (incl. brokerage) into the replacement.
    #[tokio::test]
    async fn partly_sold_parcel_carries_only_the_remaining_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // 1,000 @ $1.50 + $50 brokerage = $1,550; sell 400 → 600 remain at
        // 60% of the cost base = $930.
        trade::db_upsert(
            &pool,
            &Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id: 1,
                trade_type: TradeType::Buy,
                date: d(2020, 10, 1),
                settlement_date: d(2020, 10, 1),
                listing_id: 1,
                average_price: dec("1.50"),
                quantity: dec("1000"),
                currency: "AUD".to_string(),
                brokerage: dec("50"),
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
        sell_units(&pool, 2, 1, d(2022, 5, 1), "400").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let ex = db_exchange(&pool, 10).await.unwrap();

        assert_eq!(ex.sell.quantity, dec("600"));
        assert_eq!(ex.replacements.len(), 1);
        assert_eq!(ex.replacements[0].quantity, dec("1200"));
        assert_eq!(ex.replacements[0].brokerage, dec("930"));
    }

    /// AMIT cost-base reductions and return-of-capital payments received
    /// while held reduce the carried cost base — the replacement carries the
    /// *reduced* cost base of the original interest.
    #[tokio::test]
    async fn amit_and_roc_reductions_carry_into_the_replacement_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;

        // An AMMA statement with a 10c/unit cost-base decrease over the
        // parcel's 1,000 units → −$100.
        crate::entities::amma::db_upsert(
            &pool,
            &crate::entities::amma::AmmaStatement {
                holding_account_id: 1,
                id: 1,
                listing_id: 1,
                tax_year_end_date: d(2021, 6, 30),
                units_held: dec("1000"),
                date_received: d(2021, 7, 15),
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
                cost_base_adjustment: dec("0.10"),
                tfn_withholding_tax: Decimal::ZERO,
                currency: "AUD".to_string(),
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
                quantity: dec("1000"),
            },
        )
        .await
        .unwrap();

        // A 5c/unit return of capital while held → −$50.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 5,
                listing_id: 1,
                date: d(2022, 3, 1),
                kind: ActionKind::ReturnOfCapital {
                    amount_per_unit: dec("0.05"),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();

        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        let ex = db_exchange(&pool, 10).await.unwrap();

        // $1,500 − $100 (AMIT) − $50 (ROC) = $1,350 carried.
        assert_eq!(ex.replacements[0].brokerage, dec("1350"));
    }

    /// A split on the original listing before the exchange re-bases the
    /// units the ratio applies to; the cost base is unchanged by the split.
    #[tokio::test]
    async fn split_before_the_exchange_rebases_the_exchanged_units() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        // 3-for-1 split: the 1,000 as-acquired units are 3,000 at the exchange.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 5,
                listing_id: 1,
                date: d(2022, 1, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("3"),
                    split_old_units: dec("1"),
                },
            },
        )
        .await
        .unwrap();
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let ex = db_exchange(&pool, 10).await.unwrap();

        // 3,000 exchange-date units close; 2-for-1 → 6,000 replacement units
        // carrying the unchanged $1,500 cost base and the 2020 acquisition.
        assert_eq!(ex.sell.quantity, dec("3000"));
        assert_eq!(ex.replacements[0].quantity, dec("6000"));
        assert_eq!(ex.replacements[0].brokerage, dec("1500"));
        assert_eq!(ex.replacements[0].deemed_acquisition_date, Some(d(2020, 10, 1)));
    }

    /// A second exchange chains the deemed acquisition date from the first —
    /// the discount clock always runs from the original acquisition.
    #[tokio::test]
    async fn chained_exchange_carries_the_original_acquisition_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "MID").await;
        insert_listing(&pool, 3, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        db_exchange(&pool, 10).await.unwrap();

        // Listing 2 is itself taken over by listing 3, 1-for-2.
        insert_scrip_terms(&pool, 11, 2, 3, d(2025, 2, 1), "1", "2").await;
        let ex = db_exchange(&pool, 11).await.unwrap();

        assert_eq!(ex.replacements.len(), 1);
        assert_eq!(ex.replacements[0].listing_id, 3);
        assert_eq!(ex.replacements[0].quantity, dec("1000"));
        assert_eq!(ex.replacements[0].brokerage, dec("1500"));
        assert_eq!(ex.replacements[0].deemed_acquisition_date, Some(d(2020, 10, 1)));
    }

    #[tokio::test]
    async fn invalid_exchanges_are_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;

        // Missing action.
        assert!(matches!(
            db_exchange(&pool, 99).await,
            Err(ExchangeError::ActionNotFound)
        ));

        // Not a ScripForScrip.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("2"),
                    split_old_units: dec("1"),
                },
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            db_exchange(&pool, 1).await,
            Err(ExchangeError::NotAScripForScrip)
        ));

        // Nothing held at the exchange date.
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        assert!(matches!(db_exchange(&pool, 10).await, Err(ExchangeError::NothingHeld)));

        // A target trade dated on/after the exchange date contradicts the
        // takeover.
        insert_buy(&pool, 1, 1, d(2024, 7, 1), "100", "1.50").await;
        assert!(matches!(
            db_exchange(&pool, 10).await,
            Err(ExchangeError::TradedOnOrAfterExchangeDate)
        ));

        // Nothing was persisted by any of the rejections.
        let trades: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM trades WHERE scrip_action_id IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(trades, 0);
    }

    #[tokio::test]
    async fn a_second_exchange_of_the_same_action_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        db_exchange(&pool, 10).await.unwrap();

        assert!(matches!(
            db_exchange(&pool, 10).await,
            Err(ExchangeError::AlreadyExchanged)
        ));
    }

    /// The group is immutable trade by trade: the closing Sell rejects
    /// `PUT /sells`, the replacement Buys reject `PUT /trades`, and neither
    /// can be deleted individually via `DELETE /trades`.
    #[tokio::test]
    async fn exchange_trades_are_immutable_individually() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        let ex = db_exchange(&pool, 10).await.unwrap();

        // PUT /sells on the closing Sell → rejected.
        let err = sell::db_upsert_sell(
            &pool,
            ex.sell.id,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: d(2024, 7, 1),
                settlement_date: Some(d(2024, 7, 1)),
                listing_id: 1,
                average_price: dec("9.99"),
                quantity: dec("1000"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("1000"),
                }],
            },
        )
        .await;
        assert!(matches!(err, Err(sell::SellError::ScripExchangeSell)));

        // PUT /trades on a replacement Buy → rejected.
        let mut edited = ex.replacements[0].clone();
        edited.quantity = dec("9999");
        assert!(matches!(
            trade::db_upsert(&pool, &edited).await,
            Err(trade::UpsertError::ScripExchangeTrade)
        ));

        // DELETE /trades on a replacement Buy (or the Sell) → refused.
        assert_eq!(
            trade::db_delete(&pool, ex.replacements[0].id).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );
        assert_eq!(
            trade::db_delete(&pool, ex.sell.id).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );
    }

    /// `DELETE /sells` on the closing Sell removes the whole group and
    /// restores the pre-exchange holding; it is refused while a replacement
    /// Buy is consumed by a later sale.
    #[tokio::test]
    async fn deleting_the_closing_sell_removes_the_whole_group() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        let ex = db_exchange(&pool, 10).await.unwrap();

        // A later sale out of the replacement parcel blocks the group delete.
        sell::db_upsert_sell(
            &pool,
            50,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: d(2025, 1, 10),
                settlement_date: Some(d(2025, 1, 12)),
                listing_id: 2,
                average_price: dec("3.00"),
                quantity: dec("500"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: ex.replacements[0].id,
                    quantity_allocated: dec("500"),
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(
            sell::db_delete_sell(&pool, ex.sell.id).await.unwrap(),
            sell::DeleteOutcome::ReplacementReferenced
        );

        // Remove the later sale; the group then deletes as a whole and the
        // action thaws (it can be deleted again).
        assert_eq!(
            sell::db_delete_sell(&pool, 50).await.unwrap(),
            sell::DeleteOutcome::Deleted
        );
        assert_eq!(
            sell::db_delete_sell(&pool, ex.sell.id).await.unwrap(),
            sell::DeleteOutcome::Deleted
        );
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM trades WHERE scrip_action_id IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0);
        // The original parcel is open again.
        let allocs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(allocs, 0);
        assert!(corporate_action::db_delete(&pool, 10).await.unwrap());
    }

    /// The action is frozen while its exchange group exists.
    #[tokio::test]
    async fn referenced_action_cannot_be_edited_or_deleted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        db_exchange(&pool, 10).await.unwrap();

        let action = corporate_action::db_get(&pool, 10).await.unwrap().unwrap();
        assert!(matches!(
            corporate_action::db_upsert(&pool, &action).await,
            Err(corporate_action::WriteError::ReferencedByTrade)
        ));
        assert!(corporate_action::db_delete(&pool, 10).await.is_err());
    }

    /// The exchange closes the whole holding across every holding account;
    /// each replacement parcel stays in the account of the parcel it
    /// substitutes.
    #[tokio::test]
    async fn replacements_stay_in_each_parcels_holding_account() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount { id: 2, name: "ICE Employee Plan".to_string() },
        )
        .await
        .unwrap();
        // One parcel per account.
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_buy(&pool, 2, 1, d(2023, 3, 1), "500", "2.00").await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let ex = db_exchange(&pool, 10).await.unwrap();

        assert_eq!(ex.replacements.len(), 2);
        assert_eq!(ex.replacements[0].holding_account_id, 1);
        assert_eq!(ex.replacements[1].holding_account_id, 2);
    }

    // API-level tests

    #[tokio::test]
    async fn api_exchange_creates_the_group() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/10/exchange")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["sell"]["quantity"], "1000");
        assert_eq!(v["replacements"][0]["quantity"], "2000");
        assert_eq!(v["replacements"][0]["deemed_acquisition_date"], "2020-10-01");
    }

    #[tokio::test]
    async fn api_exchange_maps_errors_to_statuses() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;

        // Missing action → 404.
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/99/exchange")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Nothing held → 422.
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/10/exchange")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
