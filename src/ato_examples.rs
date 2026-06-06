//! Acceptance tests reproducing the worked examples from the ATO guidance
//! mirrored in `docs/` (see `docs/OVERVIEW.md`). Each test cites the document
//! and example it reproduces, enters the example's facts purely through the
//! HTTP API (the full `app::router` — no `db_*` shortcuts), reads the result
//! back through the report endpoints, and asserts the figures the ATO states.
//!
//! Examples whose feature is NOT implemented yet are still encoded below as
//! `#[ignore]`d tests expressing the intended behaviour — each `#[ignore]`
//! message and the TODO section it cites point at each other. Remove the
//! `#[ignore]` when the feature lands (adjusting any speculative entry API to
//! the real one).
//!
//! Worked examples in the docs that are NOT reproduced here at all, and why:
//! - `docs/cgt-cost-base.md` "Example: effect of capital works deduction on
//!   reduced cost base" and "Example: recouped expenditure" — both need the
//!   reduced cost base and cost-base elements 3–5, which are not modelled yet
//!   (TODO "Reduced cost base and the five cost-base elements", NEEDS
//!   CLARIFICATION). What the asserted outcome looks like depends on that
//!   clarification, so no meaningful ignored test can be written yet.
//! - `docs/lic-capital-gain-deduction.md` "Example: Beneficiary of a trust or
//!   partner in partnership" — needs partnership/trust taxpayer entities,
//!   which are not modelled (TODO "Taxpayer entity type and CGT discount
//!   rate", NEEDS CLARIFICATION).
//! - `docs/you-and-your-shares-dividends.md` "Example 7: substantially
//!   identical shares" (Jessica) — needs the 45-day rule's last-in-first-out
//!   parcel identification on top of the holding-period rule itself; covered
//!   by the same TODO section as Matthew's Example 6 below.
//! - `docs/amma-statement-guidance-notes.md` running example ("In our example,
//!   this is $155") — the underlying Part C component table is not included in
//!   the mirrored copy, so the example is not reproducible from the doc alone.
//! - "Guide to foreign income tax offset rules 2025" Example 16 (Anna,
//!   ato.gov.au law view SAV/FOROFFSET/00004) — the FITO offset-limit
//!   calculation compares personal income-tax liabilities with and without the
//!   foreign income (employment income, deductions, Medicare levy), which is
//!   outside this system's data model; the TODO "Foreign income tax offset
//!   (FITO) cap" item covers only the $1,000 de-minimis cap this system can
//!   apply from its own data.
//!
//! The ATO examples use real property (land, an investment property); the data
//! model records every asset as a listing traded in units, so each property is
//! entered as 1 unit at the property's price with the incidental costs as
//! brokerage. The CGT arithmetic the examples demonstrate is identical.

use crate::entities::trade::{Trade, TradeType};
use crate::reports::net_capital_gain::NetCapitalGainYear;
use crate::reports::portfolio::HoldingOverview;
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

/// POST a JSON body (action/report endpoints) and deserialize the JSON response,
/// requiring the given success status.
async fn api_post<T: serde::de::DeserializeOwned>(
    pool: &SqlitePool,
    path: &str,
    body: Value,
    expect: StatusCode,
) -> T {
    let resp = router(pool)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), expect, "POST {path} failed");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
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

/// `docs/cgt-dividend-reinvestment-plans.md` — "Example: dividend reinvestment plans".
///
/// > Natalie owns 1,440 shares in a company. In November 2024, the company
/// > declared a dividend of 25 cents per share. Natalie was offered the choice of:
/// > - taking the dividend as a cash payment of $360 (1,440 × 25 cents)
/// > - reinvesting the dividend to acquire 45 more shares at $8 per share ($360 ÷ $8).
/// > Natalie decided to participate in the dividend reinvestment plan and
/// > received 45 new shares on 20 December 2024. This means:
/// > - she must declare the $360 dividend as assessable dividend income in her
/// >   2024–25 tax return
/// > - for CGT purposes, she acquired the 45 new shares for $360 on 20 December 2024.
#[tokio::test]
async fn drp_example_natalie_reinvested_dividend() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "NTLE").await;
    // Enrol the holding in the DRP, then record the $360 distribution.
    api_put(&pool, "/drp_enrolments/1", json!({ "residual_handling": "CarryForward" })).await;
    api_put(
        &pool,
        "/income/1",
        json!({
            "listing_id": 1,
            "date_paid": "2024-12-20",
            "unfranked_amount": "360",
            "currency": "AUD",
        }),
    )
    .await;

    // Reinvest at $8 per share: $360 ÷ $8 = 45 new shares acquired for $360.
    let trade: Trade = api_post(
        &pool,
        "/income/1/reinvest",
        json!({ "reinvestment_price": "8" }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(trade.trade_type, TradeType::DRP);
    assert_eq!(trade.quantity, dec("45"), "45 new shares");
    assert_eq!(trade.average_price, dec("8"), "at $8 per share");
    assert_eq!(trade.date.to_string(), "2024-12-20", "received on 20 December 2024");

    // For CGT purposes the 45 shares were acquired for $360.
    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert_eq!(holdings.len(), 1);
    assert_eq!(holdings[0].quantity, dec("45"));
    assert_eq!(holdings[0].total_cost_base, dec("360"), "acquired the 45 new shares for $360");

    // The $360 dividend is assessable income in her 2024–25 tax return.
    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1);
    assert_eq!(years[0].tax_year, 2025); // paid Dec 2024 → 2024–25
    assert_eq!(years[0].dividends_assessable, dec("360"), "declares the $360 dividend");
}

/// `docs/cgt-keeping-records-shares.md` — "Example: identifying when shares or
/// units were acquired".
///
/// > Boris is an investor. He:
/// > - bought 1,000 shares in a company in 2023 for $5 each
/// > - bought 3,000 shares in the same company in 2024 for $10 each
/// > - sold 1,500 of the shares in 2025 for $8 each.
/// > He decides to sell 1,500 of the shares he bought in 2024 in order to claim
/// > a capital loss in the 2025 income year. As a result, Boris will still have:
/// > - 1,000 shares with an acquisition cost of $5
/// > - 1,500 shares with an acquisition cost of $10.
///
/// Specific parcel identification is exactly what `PUT /sells` parcel
/// allocations record: the sale is allocated against the 2024 parcel, producing
/// the (8 − 10) × 1,500 = $3,000 capital loss and leaving the 2023 parcel intact.
#[tokio::test]
async fn keeping_records_example_boris_identifying_shares_sold() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "BORI").await;
    put_buy(&pool, 1, 1, "2023-05-15", "1000", "5", "0").await;
    put_buy(&pool, 2, 1, "2024-05-15", "3000", "10", "0").await;
    // Sell 1,500 in 2025 for $8, nominating the 2024 parcel (trade 2).
    put_sell(&pool, 3, 1, "2025-05-15", "1500", "8", "0", 2).await;

    // The nominated parcel makes it a $3,000 capital loss in the 2025 income year.
    let sales: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(sales.len(), 1);
    assert_eq!(sales[0].cost_base, dec("15000"), "1,500 of the $10 (2024) shares");
    assert_eq!(sales[0].proceeds, dec("12000"), "1,500 × $8");
    assert_eq!(sales[0].capital_gain_loss, dec("-3000"), "the claimed capital loss");
    assert_eq!(sales[0].capital_loss, dec("3000"));
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years[0].tax_year, 2025, "loss claimed in the 2025 income year");
    assert_eq!(years[0].capital_losses, dec("3000"));

    // Boris still has 1,000 × $5 + 1,500 × $10 = 2,500 shares costing $20,000.
    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert_eq!(holdings.len(), 1);
    assert_eq!(holdings[0].quantity, dec("2500"));
    assert_eq!(holdings[0].total_cost_base, dec("20000"));
}

/// `docs/you-and-your-shares-dividends.md` — "Example 1: payment of dividends" /
/// "Example 2: assessable dividend income" (You and your shares 2025).
///
/// > On 15 February 2025, an Australian resident company Coals Tyer Ltd pays
/// > John, a resident individual, a fully franked dividend of $700 and an
/// > unfranked dividend of $200. John's assessable income for 2024–25 in
/// > respect of the dividend is: unfranked $200 + franked $700 + franking
/// > credit $300 = total assessable dividend income $1,200.
#[tokio::test]
async fn you_and_your_shares_examples_1_2_john_assessable_dividend_income() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "CTYL").await;
    api_put(
        &pool,
        "/income/1",
        json!({
            "listing_id": 1,
            "date_paid": "2025-02-15",
            "franked_amount": "700",
            "unfranked_amount": "200",
            "franking_credits": "300",
            "currency": "AUD",
        }),
    )
    .await;

    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2025); // paid Feb 2025 → 2024–25
    assert_eq!(y.dividends_assessable, dec("900"), "unfranked $200 + franked $700");
    assert_eq!(y.franking_credits, dec("300"), "franking credit $300");
    assert_eq!(
        y.dividends_assessable + y.franking_credits,
        dec("1200"),
        "total assessable dividend income $1,200 (the franking-credit gross-up)"
    );
}

/// `docs/you-and-your-shares-dividends.md` — "Example 6: franking credits
/// entitlement greater than $5,000" (You and your shares 2025).
///
/// > Matthew acquires a single parcel of shares on 1 March 2025. On 8 April
/// > 2025 Matthew receives fully franked dividends of $13,066 (which had
/// > franking credits attached of $5,600) for 2024–25. On 10 April 2025
/// > Matthew sells that parcel of shares. Because he hadn't held the shares
/// > for at least 45 days and didn't qualify for the small shareholder
/// > exemption, he fails the holding period test and can't obtain the benefit
/// > of the franking credits. Matthew shows a dividend of $13,066 as a franked
/// > amount in his tax return but doesn't show the amount of franking credits.
///
/// Blocked: the 45-day holding-period rule and $5,000 small-shareholder
/// exemption are not implemented (TODO "Franking-credit entitlement rules").
/// Today the tax summary reports all attached credits, so this test would see
/// $5,600 instead of $0. Un-ignore it when the rule lands.
#[tokio::test]
#[ignore = "blocked on TODO 'Franking-credit entitlement rules': 45-day holding period rule not implemented"]
async fn you_and_your_shares_example_6_matthew_holding_period_rule() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "MTHW").await;
    // Acquired 1 March 2025; ex-dividend in March; sold 10 April 2025 — held
    // at risk well under the required 45 days.
    put_buy(&pool, 1, 1, "2025-03-01", "1000", "50", "0").await;
    api_put(
        &pool,
        "/income/1",
        json!({
            "listing_id": 1,
            "date_paid": "2025-04-08",
            "ex_date": "2025-03-14",
            "franked_amount": "13066",
            "franking_credits": "5600",
            "currency": "AUD",
        }),
    )
    .await;
    put_sell(&pool, 2, 1, "2025-04-10", "1000", "50", "0", 1).await;

    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2025);
    // He still shows the $13,066 franked amount as income…
    assert_eq!(y.dividends_assessable, dec("13066"), "shows the dividend as a franked amount");
    // …but has no entitlement to any part of the $5,600 franking credits
    // (credits > $5,000, so the small-shareholder exemption can't restore them).
    assert_eq!(
        y.franking_credits,
        Decimal::ZERO,
        "no entitlement to the $5,600 franking credits — held under 45 days"
    );
}

/// `docs/cgt-non-assessable-payments.md` — "Example 45: Non-assessable payments"
/// (Guide to capital gains tax; CGT event G1).
///
/// > Rob bought 1,500 shares in RAP Ltd on 1 July 1994 for $5 each, including
/// > brokerage and stamp duty. On 30 November 2007, as part of a
/// > shareholder-approved scheme for the reduction of RAP Ltd's share capital,
/// > he received a non-assessable payment of 50 cents per share. As the amount
/// > of the payment is not more than the cost base, he reduces the cost base of
/// > each share at 30 November 2007 by the amount of the payment to $4.50
/// > ($5.00 – 50 cents).
///
/// Blocked: a company return of capital (CGT event G1) is not modelled (TODO
/// "Corporate actions / additional CGT events" — "Return of capital (non-AMIT,
/// CGT event G1)"). The `PUT /corporate_actions/1` entry below is a sketch of
/// the intended API — adjust it to the real shape when the feature lands, then
/// un-ignore.
#[tokio::test]
#[ignore = "blocked on TODO 'Corporate actions / additional CGT events': return of capital (CGT event G1) not implemented"]
async fn cgt_non_assessable_payments_example_45_rob_return_of_capital() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "RAP").await;
    put_buy(&pool, 1, 1, "1994-07-01", "1500", "5", "0").await;
    // Speculative entry API for the G1 return of capital — to be replaced with
    // the real corporate-actions endpoint when implemented.
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2007-11-30",
            "amount_per_unit": "0.50",
            "currency": "AUD",
        }),
    )
    .await;

    // Cost base reduced to $4.50 per share: 1,500 × $4.50 = $6,750.
    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert_eq!(holdings.len(), 1);
    assert_eq!(holdings[0].quantity, dec("1500"));
    assert_eq!(
        holdings[0].total_cost_base,
        dec("6750"),
        "cost base reduced to $4.50 ($5.00 − 50 cents) per share"
    );
    assert_eq!(holdings[0].avg_cost_base_per_unit, dec("4.50"));

    // The payment is within the cost base, so no capital gain arises (a G1
    // payment can never create a capital loss either).
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert!(
        years.iter().all(|y| y.net_capital_gain == Decimal::ZERO),
        "payment not more than cost base → no capital gain"
    );
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
