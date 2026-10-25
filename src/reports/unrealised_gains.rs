//! Unrealised gains and losses on the open holdings per (listing, holding account): market value
//! against AUD cost base, priced from the request's price map or, when it asks, live quotes. Built
//! on `domain::open_parcels`.

use crate::domain::{cgt_discount, open_parcels};
use crate::entities::closing_price::{self, SharedFetcher};
use crate::infra::currency::Aud;
use crate::infra::extract::Json;
use crate::infra::http::ApiError;
use crate::infra::id::{HoldingAccountId, ListingId};
use crate::infra::{date, db};
use axum::Extension;
use axum::extract::State;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// Cost base is in AUD (each parcel converted via the ATO FX rate). The supplied
/// `current_price` is taken as AUD too, so `market_value` and
/// `unrealised_gain_loss` are AUD.
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize)]
pub struct UnrealisedGain {
    pub listing_id: ListingId,
    /// The holding account the parcels sit in: the same listing held in two
    /// accounts reports as two rows.
    pub holding_account_id: HoldingAccountId,
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
    /// The AUD conversion of this row's price used an earlier month's FX rate
    /// because the valuation month's is not published yet
    /// (`infra::fx::resolve_valuation_rate`): the valuation is provisional.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fx_provisional: bool,
    /// This row's price is not its valuation day's own close: the price
    /// provider has stopped quoting the security (`listings.unpriced_from`),
    /// so the last stored ok close was carried forward
    /// (`reports::valuation`, SCENARIOS Q-02). Unlike `fx_provisional` there
    /// is no later fact that clears it — the day is never going to be quoted
    /// — so a regeneration reproduces it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub price_carried_forward: bool,
    /// Why a live price could not be obtained: the row is left unvalued with
    /// the reason rather than silently zeroed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_unavailable: Option<String>,
}

#[derive(utoipa::ToSchema, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnrealisedGainsRequest {
    /// Current price per unit by listing id, expected in AUD so it lines up with
    /// the AUD-denominated cost base. An explicit price overrides a
    /// live-fetched one.
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_aud_map")]
    #[schema(value_type = HashMap<ListingId, Decimal>)]
    pub prices: HashMap<ListingId, Aud>,
    /// The date to report the position at (`YYYY-MM-DD`); later facts are excluded and the
    /// CGT discount is judged at it. Omitted: today.
    #[serde(default)]
    pub as_of_date: Option<NaiveDate>,
    /// Fetch the current price live from the price source for every held
    /// listing without an explicit price (off by default — see
    /// `portfolio::OverviewRequest::live`).
    #[serde(default)]
    pub live: bool,
}

pub fn router() -> OpenApiRouter<SqlitePool> {
    OpenApiRouter::new().routes(routes!(unrealised_gains_handler))
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
    let mut tx = db::read_tx(pool).await?;
    let gains = db_unrealised_gains_on(&mut tx, as_of_date).await?;
    tx.commit().await?;
    Ok(gains)
}

/// The same report read on the caller's own connection, for a caller that
/// folds it into a wider single-snapshot transaction — snapshot generation
/// reads it inside the write transaction that stores the result, so a fact
/// committed elsewhere cannot land between the figures and their `stale = 0`
/// (SCENARIOS X-a).
pub async fn db_unrealised_gains_on(
    conn: &mut sqlx::SqliteConnection,
    as_of_date: NaiveDate,
) -> Result<Vec<UnrealisedGain>, sqlx::Error> {
    // The open parcels and their AUD cost bases come from the shared loader
    // (`domain::open_parcels`), bounded at the report's as-of date: trades,
    // sales, AMIT statements and return-of-capital payments after it are
    // excluded and quantities come back in that date's unit basis.
    let open = open_parcels::load(conn, Some(as_of_date)).await?;

    let mut holding_qty: HashMap<(ListingId, HoldingAccountId), Decimal> = HashMap::new();
    let mut holding_cost_base: HashMap<(ListingId, HoldingAccountId), Decimal> = HashMap::new();
    let mut holding_cgt_eligible_qty: HashMap<(ListingId, HoldingAccountId), Decimal> =
        HashMap::new();

    for p in &open {
        let key = (p.parcel.listing_id, p.parcel.holding_account_id);
        *holding_qty.entry(key).or_insert(Decimal::ZERO) += p.remaining_as_of.quantity();
        *holding_cost_base.entry(key).or_insert(Decimal::ZERO) += p.cost_base.adjusted;

        // CGT discount, on the shared ownership rule (`domain::cgt_discount`).
        // A split does not restart the clock — the converted shares keep the
        // original acquisition date (TD 2000/10) — and a scrip-for-scrip
        // replacement parcel counts the combined holding period from its
        // deemed acquisition date (`ParcelRow::acquired`).
        if cgt_discount::discount_eligible(p.parcel.acquired(), as_of_date) {
            *holding_cgt_eligible_qty.entry(key).or_insert(Decimal::ZERO) +=
                p.remaining_as_of.quantity();
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
                fx_provisional: false,
                price_carried_forward: false,
                price_unavailable: None,
            }
        })
        .collect();

    result.sort_by_key(|h| (h.listing_id, h.holding_account_id));
    Ok(result)
}

#[utoipa::path(
    post,
    path = "/portfolio/unrealised_gains",
    summary = "Unrealised gains and losses; body carries the optional price-override map, live flag and as_of_date (omitted = today's live position)."
)]
async fn unrealised_gains_handler(
    State(pool): State<SqlitePool>,
    fetcher: Option<Extension<SharedFetcher>>,
    body: Option<Json<UnrealisedGainsRequest>>,
) -> Result<Json<Vec<UnrealisedGain>>, ApiError> {
    let req = body.map(|Json(req)| req).unwrap_or_default();
    // An omitted `as_of_date` is **today's live position** — the live-view
    // resolver (`infra::date::as_of_or_today`), never `as_of_or_open`'s
    // open-ended "every recorded fact" sentinel; resolved through the shared
    // helper rather than re-deriving "today" inline.
    let as_of_date = date::as_of_or_today(req.as_of_date);

    let mut gains = db_unrealised_gains(&pool, as_of_date)
        .await
        .map_err(ApiError::from)?;

    let live = closing_price::resolve_live_prices(
        &pool,
        fetcher.as_ref().map(|f| f.0.as_ref()),
        closing_price::LiveQuotes::requested(req.live),
        &req.prices,
        gains.iter().map(|g| g.listing_id),
    )
    .await
    .map_err(ApiError::from)?;

    for g in &mut gains {
        if let Some(price) = req.prices.get(&g.listing_id).map(|p| p.amount()) {
            g.current_price = Some(price);
            g.market_value = Some(g.quantity * price);
            g.unrealised_gain_loss = Some(g.quantity * price - g.total_cost_base);
        } else if let Some(result) = live.get(&g.listing_id) {
            match result {
                Ok(v) => {
                    let price = v.aud_price.amount();
                    g.current_price = Some(price);
                    g.market_value = Some(g.quantity * price);
                    g.unrealised_gain_loss = Some(g.quantity * price - g.total_cost_base);
                    g.price_as_of = Some(v.as_of.clone());
                    g.fx_provisional = v.fx_provisional;
                }
                Err(reason) => g.price_unavailable = Some(reason.clone()),
            }
        }
    }

    Ok(Json(gains))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::entities::corporate_action::FractionRule;
    use crate::entities::{corporate_action, scrip_exchange};
    use crate::test_support::{self, ApiClient, allocate, dec, test_pool, ymd};
    use axum::http::StatusCode;

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .insert(pool)
            .await;
    }

    async fn insert_buy(
        pool: &SqlitePool,
        id: i64,
        listing_id: ListingId,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        test_support::buy(id, listing_id.0)
            .date(date)
            .qty(qty)
            .price(price)
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    async fn insert_sell(pool: &SqlitePool, id: i64, listing_id: ListingId, qty: Decimal) {
        test_support::sell(id, listing_id.0)
            .date(ymd(2025, 5, 30))
            .qty(qty)
            .price(Decimal::from(120))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let as_of = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

        // exactly 12 months later — NOT eligible (need strictly more than 12 months)
        let as_of_exact = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        let gains = db_unrealised_gains(&pool, as_of_exact).await.unwrap();
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::ZERO);

        // one day past 12 months — eligible
        let as_of_eligible = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap();
        let gains = db_unrealised_gains(&pool, as_of_eligible).await.unwrap();
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_partial_sell_reduces_holding_and_eligible_qty() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let as_of = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(&pool, 2, ListingId(1), Decimal::from(40)).await;
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
        let old_date = NaiveDate::from_ymd_opt(2023, 1, 3).unwrap();
        let new_date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        // as_of is 2025-06-01: old parcel is >12mo, new parcel is <6mo
        let as_of = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            old_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_buy(
            &pool,
            2,
            ListingId(1),
            new_date,
            Decimal::from(50),
            Decimal::from(12),
        )
        .await;

        let gains = db_unrealised_gains(&pool, as_of).await.unwrap();
        assert_eq!(gains.len(), 1);
        assert_eq!(gains[0].quantity, Decimal::from(150));
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_amit_reduces_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let as_of = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAF").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

        test_support::amma(1, 1)
            .cost_base_adjustment(dec("0.05"))
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

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
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // 50c/unit return of capital while all 100 units are held.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: ListingId(1),
                date: NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: "0.50".parse().unwrap(),
                    currency: "AUD".into(),
                    record_date: None,
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // 2-for-1 split six months in — within the 12-month window.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: ListingId(1),
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::ShareSplit {
                    fraction_rule: FractionRule::Exact,
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                    record_date: None,
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

        let body = serde_json::json!({ "as_of_date": "2025-06-01" });
        let resp = client(&pool)
            .post("/portfolio/unrealised_gains", &body)
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let gains: Vec<UnrealisedGain> = resp.json();
        assert_eq!(gains.len(), 1);
        assert!(gains[0].current_price.is_none());
        assert!(gains[0].unrealised_gain_loss.is_none());
    }

    #[tokio::test]
    async fn api_with_prices_computes_gain_and_loss() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        // cost base = 10 * 100 + 9.95 + 0.995 = 1010.945
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

        let body = serde_json::json!({ "prices": { "1": "15.00" }, "as_of_date": "2025-06-01" });
        let resp = client(&pool)
            .post("/portfolio/unrealised_gains", &body)
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let gains: Vec<UnrealisedGain> = resp.json();
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

        // as_of = 2025-01-02: exactly 12 months, NOT eligible
        let body = serde_json::json!({ "as_of_date": "2025-01-02" });
        let resp = client(&pool)
            .post("/portfolio/unrealised_gains", &body)
            .await;
        let gains: Vec<UnrealisedGain> = resp.json();
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::ZERO);

        // as_of = 2025-01-03: one day past 12 months — eligible
        let body = serde_json::json!({ "as_of_date": "2025-01-03" });
        let resp = client(&pool)
            .post("/portfolio/unrealised_gains", &body)
            .await;
        let gains: Vec<UnrealisedGain> = resp.json();
        assert_eq!(gains[0].cgt_discount_eligible_quantity, Decimal::from(100));
    }

    /// With `live` and no explicit price, the holding is valued from the price
    /// source's latest quote: market value, unrealised gain, and the as-of time
    /// all come through.
    #[tokio::test]
    async fn api_live_fetch_values_holding_with_as_of() {
        use crate::entities::closing_price::test_support::QuoteStub;
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        // cost base = 10 × 100 + 9.95 + 0.995 = 1010.945
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        let as_of = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 6, 5, 6, 30, 0).unwrap();
        let fetcher = QuoteStub::default()
            .with_quote(1, "15.00", "AUD", as_of)
            .shared();

        let body = serde_json::json!({ "live": true, "as_of_date": "2026-06-05" });
        let resp = ApiClient::over(router().with_state(pool).layer(axum::Extension(fetcher)))
            .post("/portfolio/unrealised_gains", &body)
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let gains: Vec<UnrealisedGain> = resp.json();
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        let fetcher = QuoteStub::failing("provider down").shared();

        let body = serde_json::json!({ "live": true, "as_of_date": "2026-06-05" });
        let resp = ApiClient::over(router().with_state(pool).layer(axum::Extension(fetcher)))
            .post("/portfolio/unrealised_gains", &body)
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let gains: Vec<UnrealisedGain> = resp.json();
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
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_buy(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2025, 1, 10).unwrap(),
            Decimal::from(50),
            Decimal::from(12),
        )
        .await;
        // Sell 40 of the first parcel on 2025-06-01 (the helper's sale date).
        insert_sell(&pool, 3, ListingId(1), Decimal::from(40)).await;
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: ListingId(1),
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::ScripForScrip {
                    fraction_rule: FractionRule::Exact,
                    scrip_listing_id: ListingId(2),
                    scrip_new_units: Decimal::from(2),
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

    pub(crate) mod properties {
        use super::*;
        use crate::infra::id::TradeId;
        use crate::reports::{open_parcels, realised_gains};
        use chrono::{Datelike, Days, Weekday};
        use proptest::prelude::*;
        use serde_json::{Value, json};

        #[derive(Clone, Debug)]
        pub(crate) enum Fact {
            Buy {
                account: i64,
                units: i64,
                cents: i64,
                brokerage: i64,
            },
            Sell {
                account: i64,
                pct: i64,
            },
            Transfer {
                to_two: bool,
                pct: i64,
            },
            Split {
                new: i64,
                old: i64,
            },
            Bonus {
                units: i64,
                held: i64,
            },
            ReturnOfCapital {
                cents: i64,
                record: bool,
            },
            Amma {
                cents: i64,
            },
            Scrip {
                new: i64,
                old: i64,
            },
        }

        pub(crate) fn fact() -> impl Strategy<Value = Fact> {
            prop_oneof![
                4 => (1i64..=2, 1i64..=900, 5i64..=5_000, 0i64..=2_000)
                    .prop_map(|(account, units, cents, brokerage)| Fact::Buy { account, units, cents, brokerage }),
                2 => (1i64..=2, 1i64..=100).prop_map(|(account, pct)| Fact::Sell { account, pct }),
                1 => (any::<bool>(), 1i64..=100).prop_map(|(to_two, pct)| Fact::Transfer { to_two, pct }),
                1 => (1i64..=3, 1i64..=3).prop_map(|(new, old)| Fact::Split { new, old }),
                1 => (1i64..=3, 1i64..=7).prop_map(|(units, held)| Fact::Bonus { units, held }),
                1 => (1i64..=300, any::<bool>()).prop_map(|(cents, record)| Fact::ReturnOfCapital { cents, record }),
                1 => (-200i64..=200).prop_map(|cents| Fact::Amma { cents }),
                1 => (1i64..=3, 1i64..=3).prop_map(|(new, old)| Fact::Scrip { new, old }),
            ]
        }

        /// `date`, or the next day XASX trades.
        async fn trading(pool: &SqlitePool, mut date: NaiveDate) -> NaiveDate {
            loop {
                let holiday: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM exchange_holidays WHERE mic = 'XASX' AND holiday_date = ?)",
                )
                .bind(date)
                .fetch_one(pool)
                .await
                .unwrap();
                if !holiday && !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
                    return date;
                }
                date = date + Days::new(1);
            }
        }

        async fn open(pool: &SqlitePool, date: NaiveDate) -> Vec<open_parcels::OpenParcel> {
            open_parcels::db_open_parcels(pool, Some(date))
                .await
                .unwrap()
        }

        fn allocations(parcels: &[&open_parcels::OpenParcel], pct: i64) -> Vec<Value> {
            parcels
                .iter()
                .map(|p| {
                    let q = p.remaining_quantity;
                    let take = if pct == 100 { q } else { (q * Decimal::from(pct) / Decimal::ONE_HUNDRED).floor() };
                    (p.trade_id, take)
                })
                .filter(|(_, q)| *q > Decimal::ZERO)
                .map(|(id, q)| json!({ "purchase_trade_id": id, "quantity_allocated": q.to_string() }))
                .collect()
        }

        fn total(allocs: &[Value]) -> Decimal {
            allocs
                .iter()
                .map(|a| {
                    a["quantity_allocated"]
                        .as_str()
                        .unwrap()
                        .parse::<Decimal>()
                        .unwrap()
                })
                .sum()
        }

        fn sell_body(
            listing: i64,
            account: i64,
            date: NaiveDate,
            price: Decimal,
            allocs: &[Value],
        ) -> Value {
            json!({
                "listing_id": listing, "holding_account_id": account, "date": date,
                "quantity": total(allocs).to_string(), "average_price": price.to_string(), "currency": "AUD",
                "brokerage": "0", "brokerage_currency": "AUD", "fx_rate": "1", "allocations": allocs,
            })
        }

        /// The rule a rounded run gives an action, read off the action's own
        /// terms so a recorded seed still generates the same history.
        fn rounded_rule(a: i64, b: i64) -> &'static str {
            ["Down", "Up", "Nearest"][usize::try_from((a + b) % 3).unwrap()]
        }

        /// The body of the property: the history `facts`, then every holding
        /// sold at `price_cents` `gap_days` later. `rounded` holds both
        /// accounts to whole units and gives every ratio-driven action a
        /// non-`Exact` rule ([`rounded_rule`]), so each holding is rounded and
        /// its parcels' counts stored; otherwise every action is `Exact` on
        /// unconstrained accounts.
        pub(crate) async fn realise_everything(
            amit: bool,
            facts: Vec<Fact>,
            gap_days: u64,
            price_cents: (i64, i64),
            rounded: bool,
        ) -> Result<(), TestCaseError> {
            let pool = test_pool().await;
            test_support::listing(1)
                .ticker("UNR")
                .amit(amit)
                .insert(&pool)
                .await;
            test_support::listing(2).ticker("NEW").insert(&pool).await;
            let app = ApiClient::full(&pool);
            let scale = rounded.then_some(0);
            app.put_ok(
                "/holding_accounts/1",
                &json!({ "name": "Default", "unit_scale": scale }),
            )
            .await;
            app.put_ok(
                "/holding_accounts/2",
                &json!({ "name": "Second", "unit_scale": scale }),
            )
            .await;
            let rule = |a: i64, b: i64| if rounded { rounded_rule(a, b) } else { "Exact" };
            let next_id = |table: &'static str| {
                let pool = pool.clone();
                async move {
                    sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                        "SELECT COALESCE(MAX(id), 0) + 1 FROM {table}"
                    )))
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                }
            };
            let mut exchanged = false;
            let mut day = ymd(2021, 3, 3);
            for fact in &facts {
                day = trading(&pool, day + Days::new(40)).await;
                let on = day;
                let of = |account: i64,
                          parcels: &[open_parcels::OpenParcel]|
                 -> Vec<open_parcels::OpenParcel> {
                    parcels
                        .iter()
                        .filter(|p| p.listing_id == 1 && p.holding_account_id == account)
                        .cloned()
                        .collect()
                };
                match fact.clone() {
                    Fact::Buy {
                        account,
                        units,
                        cents,
                        brokerage,
                    } if !exchanged => {
                        app.put(format!("/trades/{}", next_id("trades").await), &json!({
                            "listing_id": 1, "holding_account_id": account, "date": on,
                            "trade_type": "Buy", "quantity": units.to_string(),
                            "average_price": Decimal::new(cents, 2).to_string(), "currency": "AUD",
                            "brokerage": Decimal::new(brokerage, 2).to_string(),
                            "brokerage_currency": "AUD", "fx_rate": "1",
                        }))
                        .await
                        .expect_status(StatusCode::CREATED);
                    }
                    Fact::Sell { account, pct } => {
                        let parcels = of(account, &open(&pool, on).await);
                        let allocs = allocations(&parcels.iter().collect::<Vec<_>>(), pct);
                        if !allocs.is_empty() {
                            app.post(
                                "/sells",
                                &sell_body(1, account, on, Decimal::new(321, 2), &allocs),
                            )
                            .await
                            .expect_status(StatusCode::CREATED);
                        }
                    }
                    Fact::Transfer { to_two, pct } => {
                        let (from, to) = if to_two { (1, 2) } else { (2, 1) };
                        let parcels = of(from, &open(&pool, on).await);
                        let allocs = allocations(&parcels.iter().collect::<Vec<_>>(), pct);
                        if !allocs.is_empty() {
                            app.post("/transfers", &json!({
                                "listing_id": 1, "date": on, "from_account_id": from, "to_account_id": to,
                                "allocations": allocs, "fee_allocations": [],
                            }))
                            .await
                            .expect_status(StatusCode::CREATED);
                        }
                    }
                    Fact::Split { new, old } if !exchanged => {
                        app.put(format!("/corporate_actions/{}", next_id("corporate_actions").await), &json!({
                            "listing_id": 1, "date": on, "action_type": "ShareSplit", "fraction_rule": rule(new, old),
                            "split_new_units": new.to_string(), "split_old_units": old.to_string(),
                        }))
                        .await
                        .expect_status(StatusCode::CREATED);
                    }
                    Fact::Bonus { units, held } if !exchanged => {
                        app.put(format!("/corporate_actions/{}", next_id("corporate_actions").await), &json!({
                            "listing_id": 1, "date": on, "action_type": "BonusIssue", "fraction_rule": rule(units, held),
                            "bonus_units": units.to_string(), "bonus_held_units": held.to_string(),
                        }))
                        .await
                        .expect_status(StatusCode::CREATED);
                    }
                    Fact::ReturnOfCapital { cents, record } if !amit && !exchanged => {
                        let record_date = record.then(|| on - Days::new(5));
                        app.put(format!("/corporate_actions/{}", next_id("corporate_actions").await), &json!({
                            "listing_id": 1, "date": on, "action_type": "ReturnOfCapital",
                            "amount_per_unit": Decimal::new(cents, 2).to_string(), "currency": "AUD",
                            "record_date": record_date,
                        }))
                        .await
                        .expect_status(StatusCode::CREATED);
                    }
                    Fact::Amma { cents } if amit && !exchanged => {
                        // The latest year ended before today's fact,
                        // for the account holding the most at it.
                        let year = if on.month() >= 7 {
                            on.year()
                        } else {
                            on.year() - 1
                        };
                        let year_end = ymd(year, 6, 30);
                        let entered: bool = sqlx::query_scalar(
                            "SELECT EXISTS (SELECT 1 FROM amma_statements WHERE tax_year_end_date = ?)",
                        )
                        .bind(year_end)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                        let at_year_end = open(&pool, year_end).await;
                        let held: Decimal = of(1, &at_year_end)
                            .iter()
                            .map(|p| p.remaining_quantity)
                            .sum();
                        if !entered && held > Decimal::ZERO {
                            let id = next_id("amma_statements").await;
                            test_support::amma(id, 1)
                                .units(held)
                                .cost_base_adjustment(Decimal::new(cents, 2))
                                .with(|a| {
                                    a.tax_year_end_date = year_end;
                                    a.date_received = on;
                                })
                                .insert(&pool)
                                .await;
                            app.post(
                                format!("/amma_statements/{id}/generate_adjustments"),
                                &json!({}),
                            )
                            .await
                            .expect_status(StatusCode::CREATED);
                        }
                    }
                    Fact::Scrip { new, old } if !exchanged => {
                        let id = next_id("corporate_actions").await;
                        app.put(format!("/corporate_actions/{id}"), &json!({
                            "listing_id": 1, "date": on, "action_type": "ScripForScrip", "fraction_rule": rule(new, old),
                            "scrip_listing_id": 2, "scrip_new_units": new.to_string(),
                            "scrip_old_units": old.to_string(),
                        }))
                        .await
                        .expect_status(StatusCode::CREATED);
                        if open(&pool, on).await.iter().any(|p| p.listing_id == 1) {
                            app.post_empty(format!("/corporate_actions/{id}/exchange"))
                                .await
                                .expect_status(StatusCode::CREATED);
                            exchanged = true;
                        }
                    }
                    _ => {}
                }
            }

            let as_of = trading(&pool, day + Days::new(gap_days)).await;
            let prices = [
                Decimal::new(price_cents.0, 2),
                Decimal::new(price_cents.1, 2),
            ];
            let resp = app
                .post(
                    "/portfolio/unrealised_gains",
                    &json!({
                        "prices": { "1": prices[0].to_string(), "2": prices[1].to_string() },
                        "as_of_date": as_of,
                    }),
                )
                .await
                .expect_status(StatusCode::OK);
            let rows: Vec<UnrealisedGain> = resp.json();
            let schedule = open(&pool, as_of).await;

            for row in &rows {
                let price = if row.listing_id == 1 {
                    prices[0]
                } else {
                    prices[1]
                };
                let parcels: Vec<&open_parcels::OpenParcel> = schedule
                    .iter()
                    .filter(|p| {
                        p.listing_id == row.listing_id
                            && p.holding_account_id == row.holding_account_id
                    })
                    .collect();
                let allocs = allocations(&parcels, 100);
                prop_assert_eq!(total(&allocs), row.quantity);
                let sell: Value = app
                    .post(
                        "/sells",
                        &sell_body(
                            row.listing_id.0,
                            row.holding_account_id.0,
                            as_of,
                            price,
                            &allocs,
                        ),
                    )
                    .await
                    .expect_status(StatusCode::CREATED)
                    .json();
                let sell_id = TradeId(sell["id"].as_i64().unwrap());
                let realised = realised_gains::db_realised_gains(&pool).await.unwrap();
                let disposal = realised
                    .iter()
                    .find(|r| r.disposal.sell() == Some(sell_id))
                    .unwrap();
                let mut eligible = Decimal::ZERO;
                for p in &parcels {
                    let sold = disposal
                        .parcels
                        .iter()
                        .find(|d| d.purchase_trade_id == p.trade_id)
                        .unwrap();
                    prop_assert_eq!(
                        sold.cost_base,
                        p.remaining_cost_base,
                        "parcel {} cost base",
                        p.trade_id
                    );
                    if sold.discount_eligible {
                        eligible += sold.units;
                    }
                }
                prop_assert_eq!(
                    eligible,
                    row.cgt_discount_eligible_quantity,
                    "eligible units"
                );
                let close = |a: Decimal, b: Decimal| (a - b).abs() <= Decimal::new(1, 20);
                prop_assert!(
                    close(disposal.cost_base, row.total_cost_base),
                    "cost base {} against {}",
                    disposal.cost_base,
                    row.total_cost_base
                );
                prop_assert_eq!(Some(disposal.proceeds), row.market_value);
                prop_assert!(
                    close(
                        disposal.capital_gain_loss,
                        row.unrealised_gain_loss.unwrap()
                    ),
                    "gain {} against {:?}",
                    disposal.capital_gain_loss,
                    row.unrealised_gain_loss
                );
            }
            Ok(())
        }

        proptest! {
            #![proptest_config(test_support::db_proptest_config())]

            /// An unrealised gain is the gain the holding would realise if
            /// sold that day at that price: selling every open parcel of each
            /// (listing, account) row at the row's price, brokerage-free,
            /// realises each parcel's cost base exactly as the open-parcels
            /// schedule carried it, and in total the row's cost base, market
            /// value, gain and discount-eligible units — after any history of
            /// buys, sales, transfers, splits, bonus issues, returns of capital
            /// (with or without a record date), AMMA statements and scrip
            /// exchanges.
            #[test]
            fn an_unrealised_gain_is_what_selling_everything_that_day_realises(
                amit in any::<bool>(),
                facts in prop::collection::vec(fact(), 1..=12),
                gap_days in 1u64..=500,
                price_cents in (1i64..=9_000, 1i64..=9_000),
            ) {
                test_support::block_on(realise_everything(amit, facts, gap_days, price_cents, false))?;
            }
        }
    }
}
