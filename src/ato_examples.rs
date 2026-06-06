//! Acceptance tests reproducing the worked examples from the ATO guidance
//! mirrored in `docs/` (see `docs/OVERVIEW.md`). Each test cites the document
//! and example it reproduces, enters the example's facts purely through the
//! HTTP API (the full `app::router` — no `db_*` shortcuts), reads the result
//! back through the report endpoints, and asserts the figures the ATO states.
//!
//! Worked examples in the docs that are NOT reproduced here, and why:
//! - `docs/cgt-cost-base.md` "Example: effect of capital works deduction on
//!   reduced cost base" and "Example: recouped expenditure" — both need the
//!   reduced cost base and cost-base elements 3–5, which are not modelled yet
//!   (TODO "Reduced cost base and the five cost-base elements", NEEDS
//!   CLARIFICATION).
//! - `docs/lic-capital-gain-deduction.md` "Example: Beneficiary of a trust or
//!   partner in partnership" — needs partnership/trust taxpayer entities,
//!   which are not modelled (TODO "Taxpayer entity type and CGT discount
//!   rate", NEEDS CLARIFICATION).
//! - `docs/amma-statement-guidance-notes.md` running example ("In our example,
//!   this is $155") — the underlying Part C component table is not included in
//!   the mirrored copy, so the example is not reproducible from the doc alone.
//!
//! The ATO examples use real property (land, an investment property); the data
//! model records every asset as a listing traded in units, so each property is
//! entered as 1 unit at the property's price with the incidental costs as
//! brokerage. The CGT arithmetic the examples demonstrate is identical.

use crate::reports::net_capital_gain::NetCapitalGainYear;
use crate::reports::realised_gains::RealisedGainLoss;
use crate::reports::tax_summary::TaxYearSummary;
use crate::{app, infra::db, infra::scheduler};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

async fn test_pool() -> SqlitePool {
    db::init(":memory:").await.unwrap()
}

/// The full application router, exactly as `main` serves it.
fn router(pool: &SqlitePool) -> axum::Router {
    app::router(pool.clone(), scheduler::registry(pool.clone(), ":memory:".to_string()))
}

/// PUT a JSON body to the API and require the entity-write success status (204).
async fn api_put(pool: &SqlitePool, path: &str, body: Value) {
    let resp = router(pool)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "PUT {path} failed");
}

/// GET a report endpoint and deserialize the JSON response.
async fn api_get<T: serde::de::DeserializeOwned>(pool: &SqlitePool, path: &str) -> T {
    let resp = router(pool)
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET {path} failed");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// Register an AUD listing on the seeded XASX exchange via `PUT /listings/:id`.
async fn put_listing(pool: &SqlitePool, id: i64, ticker: &str) {
    api_put(
        pool,
        &format!("/listings/{id}"),
        json!({
            "exchange_mic": "XASX",
            "ticker": ticker,
            "name": ticker,
            "isin": null,
            "security_type": "Share",
            "currency": "AUD",
            "amit": false,
        }),
    )
    .await;
}

/// Enter a Buy via `PUT /trades/:id` (AUD, with optional incidental costs as brokerage).
async fn put_buy(pool: &SqlitePool, id: i64, listing_id: i64, date: &str, qty: &str, price: &str, brokerage: &str) {
    api_put(
        pool,
        &format!("/trades/{id}"),
        json!({
            "trade_type": "Buy",
            "date": date,
            "listing_id": listing_id,
            "average_price": price,
            "quantity": qty,
            "currency": "AUD",
            "brokerage": brokerage,
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1",
        }),
    )
    .await;
}

/// Enter a Sell with its parcel allocation atomically via `PUT /sells/:id`.
async fn put_sell(
    pool: &SqlitePool,
    id: i64,
    listing_id: i64,
    date: &str,
    qty: &str,
    price: &str,
    brokerage: &str,
    purchase_trade_id: i64,
) {
    api_put(
        pool,
        &format!("/sells/{id}"),
        json!({
            "date": date,
            "listing_id": listing_id,
            "average_price": price,
            "quantity": qty,
            "currency": "AUD",
            "brokerage": brokerage,
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "allocations": [
                { "purchase_trade_id": purchase_trade_id, "quantity_allocated": qty }
            ],
        }),
    )
    .await;
}

fn dec(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// `docs/cgt-how-to-calculate.md` — "Example: CGT with discount".
///
/// > Justin, an Australian resident, buys a block of land. He owns it for
/// > 18 months and sells it, making a profit of $10,000. He has no capital
/// > losses. Justin is entitled to the 50% CGT discount for the land. He will
/// > declare a capital gain of $5,000 in his tax return.
///
/// Entered as 1 unit bought for $100,000 and sold 18 months later for
/// $110,000 (the doc fixes only the profit). Held > 12 months → the gain is
/// discount-eligible and the declared net capital gain is half the $10,000.
#[tokio::test]
async fn cgt_how_to_calculate_example_cgt_with_discount() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "LAND").await;
    put_buy(&pool, 1, 1, "2023-01-10", "1", "100000", "0").await;
    put_sell(&pool, 2, 1, "2024-07-10", "1", "110000", "0", 1).await; // 18 months later

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2025); // sold July 2024 → FY2024-25
    assert_eq!(y.discount_eligible_gains, dec("10000"), "the $10,000 profit");
    assert_eq!(y.capital_losses, Decimal::ZERO, "he has no capital losses");
    assert_eq!(y.cgt_discount, dec("5000"), "50% CGT discount");
    assert_eq!(y.net_capital_gain, dec("5000"), "declares a capital gain of $5,000");
}

/// `docs/cgt-how-to-calculate.md` — "Example: working out CGT for a single asset".
///
/// > Rhi buys an investment property for $500,000 and sells it 5 years later
/// > for $600,000. She has no other capital gains or losses.
/// > 1. The capital proceeds from the CGT event are $600,000.
/// >    - purchase costs of $500,000 + $15,000 stamp duty + $1,200 conveyancing fees
/// >    - sale costs of $1,300 conveyancing fees + $12,500 agent's commission.
/// > 3. Rhi's capital gain on the investment property is $600,000 − $530,000 = $70,000.
/// > 7. The CGT discount is $70,000 × 50% = $35,000.
/// > 8. Rhi reports a net capital gain of $35,000 and a capital gain of $70,000.
///
/// The doc folds the $13,800 sale costs into the $530,000 it subtracts; this
/// model nets them off the proceeds instead (proceeds $586,200 vs cost base
/// $516,200) — the $70,000 gain is the same either way.
#[tokio::test]
async fn cgt_how_to_calculate_example_single_asset() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "PROP").await;
    // Purchase costs: $15,000 stamp duty + $1,200 conveyancing = $16,200 brokerage.
    put_buy(&pool, 1, 1, "2020-06-04", "1", "500000", "16200").await;
    // Sale costs: $1,300 conveyancing + $12,500 agent's commission = $13,800 brokerage.
    put_sell(&pool, 2, 1, "2025-06-04", "1", "600000", "13800", 1).await; // 5 years later

    let sales: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(sales.len(), 1);
    let s = &sales[0];
    assert_eq!(s.cost_base, dec("516200"), "purchase price + stamp duty + conveyancing");
    assert_eq!(s.proceeds, dec("586200"), "$600,000 less the $13,800 sale costs");
    assert_eq!(s.capital_gain_loss, dec("70000"), "step 3: the $70,000 capital gain");
    assert_eq!(s.discount_eligible_gain, dec("70000"), "owned at least 12 months");

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2025); // contract dated 4 June 2025 → FY ending 30 June 2025
    assert_eq!(y.cgt_discount, dec("35000"), "step 7: $70,000 × 50% = $35,000");
    assert_eq!(y.net_capital_gain, dec("35000"), "step 8: net capital gain of $35,000");
}

/// `docs/cgt-how-to-calculate.md` — "Example: working out CGT for multiple assets".
///
/// > Take the same facts as above, except that in addition to the investment
/// > property, Rhi also sells some shares in the same financial year:
/// > - Rhi bought 1,000 shares at $10 each for a total of $10,000, including
/// >   stamp duty and brokerage costs.
/// > - Rhi sells the shares (at a loss) for $5,500. There are no brokerage
/// >   costs on the sale of the shares.
/// > 3. Rhi's capital loss on the shares is $5,500 − $10,000 = ($4,500).
/// > 5. Rhi's net capital gain is $70,000 − $4,500 = $65,500.
/// > 7. The CGT discount reduces the remaining gain: $65,500 × 50% = $32,750.
/// > 8. Rhi reports a net capital gain of $32,750 and a capital gain of $70,000.
#[tokio::test]
async fn cgt_how_to_calculate_example_multiple_assets() {
    let pool = test_pool().await;
    // Same property facts as the single-asset example.
    put_listing(&pool, 1, "PROP").await;
    put_buy(&pool, 1, 1, "2020-06-04", "1", "500000", "16200").await;
    put_sell(&pool, 2, 1, "2025-06-04", "1", "600000", "13800", 1).await;
    // Plus the shares sold at a loss in the same financial year.
    put_listing(&pool, 2, "SHRS").await;
    put_buy(&pool, 3, 2, "2024-09-02", "1000", "10", "0").await; // $10,000 all-in
    put_sell(&pool, 4, 2, "2025-06-10", "1000", "5.50", "0", 3).await; // $5,500, no sale costs

    let sales: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    let shares = sales.iter().find(|s| s.sale_trade_id == 4).unwrap();
    assert_eq!(shares.capital_gain_loss, dec("-4500"), "step 3: a $4,500 capital loss");
    assert_eq!(shares.capital_loss, dec("4500"));

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2025);
    assert_eq!(y.discount_eligible_gains, dec("70000"), "the property gain");
    assert_eq!(y.capital_losses, dec("4500"), "the share loss");
    // Step 5: losses come off before the discount → $70,000 − $4,500 = $65,500.
    assert_eq!(y.net_discount_eligible_gain, dec("65500"));
    assert_eq!(y.cgt_discount, dec("32750"), "step 7: $65,500 × 50% = $32,750");
    assert_eq!(y.net_capital_gain, dec("32750"), "step 8: net capital gain of $32,750");
    assert_eq!(y.capital_loss_carried_forward, Decimal::ZERO);
}

/// `docs/lic-capital-gain-deduction.md` — "Example: Resident individual".
///
/// > Ben, an Australian resident, is a shareholder in XYZ Ltd, a LIC. On
/// > 21 February 2025, Ben received a fully franked dividend from XYZ Ltd of
/// > $70, with an eligible capital gain amount (attributable part) of $50.
/// > Ben includes the following amounts in his 2024–25 tax return:
/// > - Dividends – Franked amount: $70.
/// > - Dividends – Franking credit: $30.
/// > - Dividend deductions: $25 (50% deduction for LIC capital gain).
///
/// The LIC advises the $50 attributable part; the deduction recorded on the
/// income record is the individual's 50% of it ($25), per the doc.
#[tokio::test]
async fn lic_capital_gain_deduction_example_resident_individual() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "XYZ").await;
    api_put(
        &pool,
        "/income/1",
        json!({
            "listing_id": 1,
            "date_paid": "2025-02-21",
            "franked_amount": "70",
            "franking_credits": "30",
            "lic_capital_gain_deduction": "25",
            "currency": "AUD",
        }),
    )
    .await;

    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2025); // paid Feb 2025 → 2024–25 return
    assert_eq!(y.dividends_assessable, dec("70"), "Dividends – Franked amount: $70");
    assert_eq!(y.franking_credits, dec("30"), "Dividends – Franking credit: $30");
    assert_eq!(
        y.lic_capital_gain_deduction,
        dec("25"),
        "Dividend deductions: $25 (50% deduction for LIC capital gain)"
    );
}
