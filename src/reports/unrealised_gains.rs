use crate::domain::cost_base::{self, ParcelRow};
use crate::entities::closing_price::{self, SharedFetcher};
use crate::infra::decimal::parse_dec;
use crate::infra::fx::FxRates;
use crate::infra::http::ApiError;
use axum::{Extension, Json, Router, extract::State, routing::post};
use chrono::{Months, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// Cost base is in AUD (each parcel converted via the ATO FX rate). The supplied
/// `current_price` is taken as AUD too, so `market_value` and
/// `unrealised_gain_loss` are AUD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnrealisedGain {
    pub listing_id: i64,
    /// The holding account the parcels sit in: the same listing held in two
    /// accounts reports as two rows.
    pub holding_account_id: i64,
    pub quantity: Decimal,
    pub total_cost_base: Decimal,
    pub current_price: Option<Decimal>,
    pub market_value: Option<Decimal>,
    pub unrealised_gain_loss: Option<Decimal>,
    /// Portion of open quantity in parcels acquired more than 12 months before `as_of_date`.
    pub cgt_discount_eligible_quantity: Decimal,
    /// The price source's quote timestamp (the "as at" moment) when
    /// `current_price` came from a live fetch; absent for an explicitly
    /// supplied price or a stored snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_as_of: Option<String>,
    /// Why a live price could not be obtained: the row is left unvalued with
    /// the reason rather than silently zeroed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_unavailable: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UnrealisedGainsRequest {
    /// Current price per unit by listing id, expected in AUD so it lines up with
    /// the AUD-denominated cost base. An explicit price overrides a
    /// live-fetched one.
    #[serde(default)]
    pub prices: HashMap<i64, Decimal>,
    #[serde(default)]
    pub as_of_date: Option<NaiveDate>,
    /// Fetch the current price live from the price source for every held
    /// listing without an explicit price (off by default — see
    /// `portfolio::OverviewRequest::live`).
    #[serde(default)]
    pub live: bool,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route(
        "/portfolio/unrealised-gains",
        post(unrealised_gains_handler),
    )
}

/// The report is the position *as at* `as_of_date`: trades, sales, corporate
/// actions, and AMIT adjustments (by their statement's year end) dated after it
/// are excluded, so a snapshot generated for a past day reflects what was held
/// then even when later facts have since been recorded.
pub async fn db_unrealised_gains(
    pool: &SqlitePool,
    as_of_date: NaiveDate,
) -> Result<Vec<UnrealisedGain>, sqlx::Error> {
    // One read transaction: every input below comes from the same snapshot,
    // so an interleaved write can't yield e.g. an allocation whose parcel is
    // missing from the same read.
    let mut tx = pool.begin().await?;
    let trade_rows: Vec<ParcelRow> = sqlx::query_as(&format!(
        "SELECT {} FROM trades WHERE trade_type IN ('Buy', 'DRP') AND date <= ?",
        ParcelRow::COLUMNS
    ))
    .bind(as_of_date)
    .fetch_all(&mut *tx)
    .await?;

    if trade_rows.is_empty() {
        // Nothing held; the dropped transaction was read-only.
        return Ok(vec![]);
    }

    // units sold per purchase parcel, with each sale's date so the allocated
    // quantity (in sale-date units) can be re-based across splits
    let alloc_rows = sqlx::query(
        "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE s.date <= ?",
    )
    .bind(as_of_date)
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

    let cba_reduction =
        crate::entities::amit_adjustment::db_cost_base_reductions_up_to(&mut *tx, Some(as_of_date))
            .await?;
    let roc_events =
        crate::entities::corporate_action::db_return_of_capital_events(&mut *tx).await?;
    // share splits/consolidations per listing (quantity re-basing)
    let split_events = crate::entities::corporate_action::db_share_split_events(&mut *tx).await?;
    // every imported ATO FX rate — per-parcel conversions below are map
    // lookups, not one DB round-trip each
    let fx = FxRates::load(&mut *tx).await?;
    tx.commit().await?;

    let mut holding_qty: HashMap<(i64, i64), Decimal> = HashMap::new();
    let mut holding_cost_base: HashMap<(i64, i64), Decimal> = HashMap::new();
    let mut holding_cgt_eligible_qty: HashMap<(i64, i64), Decimal> = HashMap::new();

    for t in &trade_rows {
        // A scrip-for-scrip replacement parcel carries the consumed parcel's
        // acquisition date (`t.acquired()`): it drives the discount clock and
        // the AUD translation month; split/ROC applicability stays on the
        // trade date.
        let splits = split_events.get(&t.listing_id).map_or(&[][..], |v| v);
        // Internal cost-base arithmetic stays in the parcel's as-acquired units;
        // each sale's allocated quantity is re-based back across any splits.
        let sold = crate::entities::corporate_action::sold_in_acquired_units(
            qty_sold.get(&t.id).map_or(&[][..], |v| v),
            splits,
            t.date,
        );
        let remaining = t.quantity - sold;
        if remaining <= Decimal::ZERO {
            continue;
        }

        // Adjusted cost base of the remaining units via the shared pipeline
        // (`domain::cost_base`), converted to AUD at the (possibly deemed)
        // acquisition month so the holding's cost base is AUD. `up_to` is the
        // report's as-of date.
        let remaining_cost = cost_base::adjusted_cost_base(
            &t.parcel(),
            remaining,
            *cba_reduction.get(&t.id).unwrap_or(&Decimal::ZERO),
            roc_events.get(&t.listing_id).map_or(&[][..], |v| v),
            splits,
            Some(as_of_date),
        )?
        .into_aud_with(&fx, &t.currency, t.acquired(), Some(t.fx_rate))?
        .adjusted;

        // Quantities are reported in the unit basis of `as_of_date` (splits up
        // to that date applied) so they line up with a price as of that date.
        let remaining_as_of = crate::entities::corporate_action::split_adjusted_quantity(
            remaining,
            splits,
            t.date,
            Some(as_of_date),
        );
        let key = (t.listing_id, t.holding_account_id);
        *holding_qty.entry(key).or_insert(Decimal::ZERO) += remaining_as_of;
        *holding_cost_base.entry(key).or_insert(Decimal::ZERO) += remaining_cost;

        // CGT discount: parcel held strictly more than 12 months. A split does
        // not restart the clock — the converted shares keep the original
        // acquisition date (TD 2000/10) — and a scrip-for-scrip replacement
        // parcel counts the combined holding period from its deemed
        // acquisition date.
        if as_of_date > t.acquired() + Months::new(12) {
            *holding_cgt_eligible_qty.entry(key).or_insert(Decimal::ZERO) += remaining_as_of;
        }
    }

    let mut result: Vec<UnrealisedGain> = holding_qty
        .into_iter()
        .filter(|(_, qty)| *qty > Decimal::ZERO)
        .map(|(key, qty)| {
            let (listing_id, holding_account_id) = key;
            let cost_base = holding_cost_base
                .get(&key)
                .copied()
                .unwrap_or(Decimal::ZERO);
            let cgt_eligible = holding_cgt_eligible_qty
                .get(&key)
                .copied()
                .unwrap_or(Decimal::ZERO);
            UnrealisedGain {
                listing_id,
                holding_account_id,
                quantity: qty,
                total_cost_base: cost_base,
                current_price: None,
                market_value: None,
                unrealised_gain_loss: None,
                cgt_discount_eligible_quantity: cgt_eligible,
                price_as_of: None,
                price_unavailable: None,
            }
        })
        .collect();

    result.sort_by_key(|h| (h.listing_id, h.holding_account_id));
    Ok(result)
}

async fn unrealised_gains_handler(
    State(pool): State<SqlitePool>,
    fetcher: Option<Extension<SharedFetcher>>,
    body: Option<Json<UnrealisedGainsRequest>>,
) -> Result<Json<Vec<UnrealisedGain>>, ApiError> {
    let req = body.map(|Json(req)| req).unwrap_or_default();
    let as_of_date = req
        .as_of_date
        .unwrap_or_else(|| chrono::Local::now().date_naive());

    let mut gains = db_unrealised_gains(&pool, as_of_date)
        .await
        .map_err(ApiError::from)?;

    let live = closing_price::resolve_live_prices(
        &pool,
        fetcher.as_ref().map(|f| f.0.as_ref()),
        req.live,
        &req.prices,
        gains.iter().map(|g| g.listing_id),
    )
    .await
    .map_err(ApiError::from)?;

    for g in &mut gains {
        if let Some(&price) = req.prices.get(&g.listing_id) {
            g.current_price = Some(price);
            g.market_value = Some(g.quantity * price);
            g.unrealised_gain_loss = Some(g.quantity * price - g.total_cost_base);
        } else if let Some(result) = live.get(&g.listing_id) {
            match result {
                Ok(v) => {
                    g.current_price = Some(v.aud_price);
                    g.market_value = Some(g.quantity * v.aud_price);
                    g.unrealised_gain_loss = Some(g.quantity * v.aud_price - g.total_cost_base);
                    g.price_as_of = Some(v.as_of.clone());
                }
                Err(reason) => g.price_unavailable = Some(reason.clone()),
            }
        }
    }

    Ok(Json(gains))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entities::{amit_adjustment, amma, corporate_action, listing, parcel_allocation, trade},
        infra::db,
    };
    use axum::http::StatusCode;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
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
                security_type: listing::SecurityType::ETF,
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
        qty: Decimal,
        price: Decimal,
    ) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id,
                trade_type: trade::TradeType::Buy,
                date,
                settlement_date: date + chrono::Duration::days(2),
                listing_id,
                average_price: price,
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: "9.95".parse().unwrap(),
                gst_on_brokerage: "0.995".parse().unwrap(),
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

    async fn insert_sell(pool: &SqlitePool, id: i64, listing_id: i64, qty: Decimal) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id,
                trade_type: trade::TradeType::Sell,
                date: NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
                settlement_date: NaiveDate::from_ymd_opt(2025, 6, 3).unwrap(),
                listing_id,
                average_price: Decimal::from(120),
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: "9.95".parse().unwrap(),
                gst_on_brokerage: "0.995".parse().unwrap(),
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

    async fn allocate(pool: &SqlitePool, id: i64, sale_id: i64, buy_id: i64, qty: Decimal) {
        parcel_allocation::db_upsert(
            pool,
            &parcel_allocation::ParcelAllocation {
                id,
                sale_trade_id: sale_id,
                purchase_trade_id: buy_id,
                quantity_allocated: qty,
            },
        )
        .await
        .unwrap();
    }

    // DB-level tests

    #[tokio::test]
    async fn db_no_trades_returns_empty() {
        let pool = test_pool().await;
        let as_of = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        let gains = db_unrealised_gains(&pool, as_of).await.unwrap();
        assert!(gains.is_empty());
    }

    #[tokio::test]
    async fn db_gain_loss_and_cost_base_correct() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let as_of = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;

        let gains = db_unrealised_gains(&pool, as_of).await.unwrap();
        assert_eq!(gains.len(), 1);
        let g = &gains[0];
        assert_eq!(g.listing_id, 1);
        assert_eq!(g.quantity, Decimal::from(100));
        // cost = 10 * 100 + 9.95 + 0.995 = 1010.945
        assert_eq!(g.total_cost_base, "1010.945".parse::<Decimal>().unwrap());
        // market fields absent without prices
        assert!(g.current_price.is_none());
        assert!(g.market_value.is_none());
        assert!(g.unrealised_gain_loss.is_none());
    }

    #[tokio::test]
    async fn db_cgt_discount_eligible_after_12_months() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;

        // exactly 12 months later — NOT eligible (need strictly more than 12 months)
        let as_of_exact = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let gains = db_unrealised_gains(&pool, as_of_exact).await.unwrap();
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::ZERO);

        // one day past 12 months — eligible
        let as_of_eligible = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        let gains = db_unrealised_gains(&pool, as_of_eligible).await.unwrap();
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_partial_sell_reduces_holding_and_eligible_qty() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let as_of = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, Decimal::from(40)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;

        let gains = db_unrealised_gains(&pool, as_of).await.unwrap();
        assert_eq!(gains.len(), 1);
        assert_eq!(gains[0].quantity, Decimal::from(60));
        // remaining cost = 1010.945 * 60 / 100 = 606.567
        assert_eq!(
            gains[0].total_cost_base,
            "606.567".parse::<Decimal>().unwrap()
        );
        // all remaining 60 units are from a >12mo parcel
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::from(60));
    }

    #[tokio::test]
    async fn db_mixed_parcel_ages_eligible_qty_correct() {
        let pool = test_pool().await;
        let old_date = NaiveDate::from_ymd_opt(2023, 1, 1).unwrap();
        let new_date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        // as_of is 2025-06-01: old parcel is >12mo, new parcel is <6mo
        let as_of = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, old_date, Decimal::from(100), Decimal::from(10)).await;
        insert_buy(&pool, 2, 1, new_date, Decimal::from(50), Decimal::from(12)).await;

        let gains = db_unrealised_gains(&pool, as_of).await.unwrap();
        assert_eq!(gains.len(), 1);
        assert_eq!(gains[0].quantity, Decimal::from(150));
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_amit_reduces_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let as_of = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAF").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;

        amma::db_upsert(
            &pool,
            &amma::AmmaStatement {
                holding_account_id: 1,
                id: 1,
                listing_id: 1,
                tax_year_end_date: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
                units_held: Decimal::from(100),
                date_received: NaiveDate::from_ymd_opt(2024, 8, 15).unwrap(),
                cost_base_adjustment: "0.05".parse().unwrap(),
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
                tfn_withholding_tax: Decimal::ZERO,
                currency: "AUD".to_string(),
            },
        )
        .await
        .unwrap();
        amit_adjustment::db_upsert(
            &pool,
            &amit_adjustment::AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();

        let gains = db_unrealised_gains(&pool, as_of).await.unwrap();
        assert_eq!(gains.len(), 1);
        // initial = 1010.945, AMIT = 100 * 0.05 = 5.00, net = 1005.945
        assert_eq!(
            gains[0].total_cost_base,
            "1005.945".parse::<Decimal>().unwrap()
        );
    }

    /// A return of capital (CGT event G1) reduces the holding's cost base by the
    /// per-unit payment for units held on the payment date, so the unrealised
    /// gain grows by the same amount (`docs/ato/cgt-non-assessable-payments.md`).
    #[tokio::test]
    async fn db_return_of_capital_reduces_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Buy 100 @ $10 on 2024-01-01 → cost base 1010.945 (incl. brokerage).
        insert_buy(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // 50c/unit return of capital while all 100 units are held.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: "0.50".parse().unwrap(),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();

        let gains = db_unrealised_gains(&pool, NaiveDate::from_ymd_opt(2024, 6, 1).unwrap())
            .await
            .unwrap();
        assert_eq!(gains.len(), 1);
        // 1010.945 − 100 × 0.50 = 960.945
        assert_eq!(
            gains[0].total_cost_base,
            "960.945".parse::<Decimal>().unwrap()
        );
    }

    /// TD 2000/10: the converted shares keep the original acquisition date, so
    /// a split inside the last 12 months does not reset discount eligibility,
    /// and the as-of quantity reflects the split.
    #[tokio::test]
    async fn db_share_split_adjusts_quantity_and_keeps_acquisition_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // 2-for-1 split six months in — within the 12-month window.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();

        // 13 months after the original acquisition: eligible despite the split.
        let as_of = NaiveDate::from_ymd_opt(2025, 2, 1).unwrap();
        let gains = db_unrealised_gains(&pool, as_of).await.unwrap();
        assert_eq!(gains.len(), 1);
        assert_eq!(gains[0].quantity, Decimal::from(200));
        // Total cost base unchanged by the split.
        assert_eq!(
            gains[0].total_cost_base,
            "1010.945".parse::<Decimal>().unwrap()
        );
        // All 200 post-split units carry the 2024-01-01 acquisition date.
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::from(200));

        // As of a date before the split, the quantity is still pre-split.
        let before = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        let gains = db_unrealised_gains(&pool, before).await.unwrap();
        assert_eq!(gains[0].quantity, Decimal::from(100));
    }

    // API-level tests

    #[tokio::test]
    async fn api_without_prices_returns_no_gain() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;

        let body = serde_json::json!({ "as_of_date": "2025-06-01" });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/unrealised-gains")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let gains: Vec<UnrealisedGain> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(gains.len(), 1);
        assert!(gains[0].current_price.is_none());
        assert!(gains[0].unrealised_gain_loss.is_none());
    }

    #[tokio::test]
    async fn api_with_prices_computes_gain_and_loss() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        // cost base = 10 * 100 + 9.95 + 0.995 = 1010.945
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;

        let body = serde_json::json!({ "prices": { "1": "15.00" }, "as_of_date": "2025-06-01" });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/unrealised-gains")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let gains: Vec<UnrealisedGain> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(gains.len(), 1);
        let g = &gains[0];
        assert_eq!(g.current_price, Some("15.00".parse::<Decimal>().unwrap()));
        // market_value = 100 * 15 = 1500
        assert_eq!(g.market_value, Some(Decimal::from(1500)));
        // gain = 1500 - 1010.945 = 489.055
        assert_eq!(
            g.unrealised_gain_loss,
            Some("489.055".parse::<Decimal>().unwrap())
        );
        // parcel is >12 months old (2024-01-01 + 12mo = 2025-01-01, as_of = 2025-06-01 > 2025-01-01)
        assert_eq!(g.cgt_discount_eligible_quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn api_gain_loss_discount_eligibility() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;

        // as_of = 2025-01-01: exactly 12 months, NOT eligible
        let body = serde_json::json!({ "as_of_date": "2025-01-01" });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/unrealised-gains")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let gains: Vec<UnrealisedGain> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::ZERO);

        // as_of = 2025-01-02: one day past 12 months — eligible
        let body = serde_json::json!({ "as_of_date": "2025-01-02" });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/unrealised-gains")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let gains: Vec<UnrealisedGain> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::from(100));
    }

    /// With `live` and no explicit price, the holding is valued from the price
    /// source's latest quote: market value, unrealised gain, and the as-of time
    /// all come through.
    #[tokio::test]
    async fn api_live_fetch_values_holding_with_as_of() {
        use crate::entities::closing_price::test_support::QuoteStub;
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        // cost base = 10 × 100 + 9.95 + 0.995 = 1010.945
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        let as_of = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 6, 5, 6, 30, 0).unwrap();
        let fetcher = QuoteStub::default()
            .with_quote(1, "15.00", "AUD", as_of)
            .shared();

        let body = serde_json::json!({ "live": true, "as_of_date": "2026-06-05" });
        let resp = router()
            .with_state(pool)
            .layer(axum::Extension(fetcher))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/unrealised-gains")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let gains: Vec<UnrealisedGain> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(gains[0].market_value, Some(Decimal::from(1500)));
        assert_eq!(
            gains[0].unrealised_gain_loss,
            Some("489.055".parse().unwrap())
        );
        assert_eq!(
            gains[0].price_as_of.as_deref(),
            Some(as_of.to_rfc3339().as_str())
        );
    }

    /// A blanket live-fetch failure leaves the holding unvalued with a reason —
    /// never a silent zero — while the report still returns the row.
    #[tokio::test]
    async fn api_live_fetch_failure_degrades_gracefully() {
        use crate::entities::closing_price::test_support::QuoteStub;
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        let fetcher = QuoteStub::failing("provider down").shared();

        let body = serde_json::json!({ "live": true, "as_of_date": "2026-06-05" });
        let resp = router()
            .with_state(pool)
            .layer(axum::Extension(fetcher))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/unrealised-gains")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let gains: Vec<UnrealisedGain> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(gains.len(), 1);
        assert!(gains[0].market_value.is_none());
        assert!(gains[0].unrealised_gain_loss.is_none());
        assert!(
            gains[0]
                .price_unavailable
                .as_deref()
                .unwrap()
                .contains("provider down")
        );
    }

    /// The report is the position *as at* `as_of_date`: a parcel bought and a
    /// sale recorded with later dates are excluded, so a past-date snapshot
    /// regenerated after new facts were entered still shows that day's actual
    /// position.
    #[tokio::test]
    async fn db_facts_dated_after_as_of_are_excluded() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_buy(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2025, 1, 10).unwrap(),
            Decimal::from(50),
            Decimal::from(12),
        )
        .await;
        // Sell 40 of the first parcel on 2025-06-01 (the helper's sale date).
        insert_sell(&pool, 3, 1, Decimal::from(40)).await;
        allocate(&pool, 1, 3, 1, Decimal::from(40)).await;

        // As at mid-2024 neither the second parcel nor the sale exists.
        let gains = db_unrealised_gains(&pool, NaiveDate::from_ymd_opt(2024, 6, 1).unwrap())
            .await
            .unwrap();
        assert_eq!(gains.len(), 1);
        assert_eq!(gains[0].quantity, Decimal::from(100));
        assert_eq!(
            gains[0].total_cost_base,
            "1010.945".parse::<Decimal>().unwrap()
        );

        // As at mid-2025 both later facts count: 100 + 50 − 40 = 110.
        let gains = db_unrealised_gains(&pool, NaiveDate::from_ymd_opt(2025, 7, 1).unwrap())
            .await
            .unwrap();
        assert_eq!(gains[0].quantity, Decimal::from(110));
    }

    /// A scrip-for-scrip replacement parcel's discount clock runs from its
    /// deemed (carried) acquisition date — the rollover's combined holding
    /// period — not from the exchange date.
    #[tokio::test]
    async fn db_scrip_replacement_discount_counts_the_combined_period() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: Decimal::from(2),
                    scrip_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        crate::entities::scrip_exchange::db_exchange(&pool, 10)
            .await
            .unwrap();

        // 2025-02-01: over 12 months after the original buy, under 12 months
        // after the exchange — eligible via the combined period.
        let gains = db_unrealised_gains(&pool, NaiveDate::from_ymd_opt(2025, 2, 1).unwrap())
            .await
            .unwrap();
        assert_eq!(gains.len(), 1);
        assert_eq!(gains[0].listing_id, 2);
        assert_eq!(gains[0].quantity, Decimal::from(200));
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::from(200));

        // 2024-12-01: under 12 months even from the original buy — not yet.
        let gains = db_unrealised_gains(&pool, NaiveDate::from_ymd_opt(2024, 12, 1).unwrap())
            .await
            .unwrap();
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::ZERO);
    }
}
