//! Acceptance tests reproducing the worked examples from the ATO guidance
//! mirrored in `docs/ato/` (see `docs/ato/OVERVIEW.md`). Each test cites the document
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
//! - `docs/ato/cgt-cost-base.md` "Example: effect of capital works deduction on
//!   reduced cost base" and "Example: recouped expenditure" — both need the
//!   reduced cost base and cost-base elements 3–5, which are not modelled yet
//!   (TODO "Reduced cost base and the five cost-base elements", NEEDS
//!   CLARIFICATION). What the asserted outcome looks like depends on that
//!   clarification, so no meaningful ignored test can be written yet.
//! - `docs/ato/lic-capital-gain-deduction.md` "Example: Beneficiary of a trust or
//!   partner in partnership" — needs partnership/trust taxpayer entities,
//!   which are not modelled (TODO "Taxpayer entity type and CGT discount
//!   rate", NEEDS CLARIFICATION).
//! - `docs/ato/bonus-shares.md` Examples 36–37 (Klaus, Mark) — both turn on
//!   partly paid bonus shares and call payments (and pre-CGT original
//!   shares), which are not modelled; Example 35's post-CGT parcel is
//!   reproduced below.
//! - `docs/ato/rights-issues.md` Examples 39–40 (Shanti) — each example's
//!   post-CGT half is reproduced below (39 via the sell-rights operation, 40
//!   via the exercise operation); both pre-CGT halves (the rights over the
//!   1 June 1985 shares) turn on pre-CGT originals, which are not modelled.
//! - `docs/ato/takeovers-and-scrip-for-scrip.md` Example 28 (Stephanie) —
//!   exchanges into *two* replacement share classes (ordinary + preference)
//!   with the cost base apportioned by market value, which the ScripForScrip
//!   action does not model (it has a single replacement leg). Example 26
//!   (Desiree, a takeover *without* rollover) and Example 27 (Gunther, the
//!   partial rollover with a cash component) are both reproduced below; the
//!   all-scrip mechanics — gain disregarded, cost base carried, combined
//!   holding period — are covered by `scrip_exchange`/report unit tests.
//! - `docs/ato/demergers.md` Examples 31 and 33 (Anita's pre-CGT shares) — both
//!   turn on pre-CGT original interests (and Example 31's no-rollover arm on
//!   the ordinary cost-base rules for the new interests), which are not
//!   modelled; Example 30's all-post-CGT apportionment and Example 32's
//!   discount-clock rule are reproduced below.
//! - `docs/ato/amma-statement-guidance-notes.md` running example ("In our example,
//!   this is $155") — the underlying Part C component table is not included in
//!   the mirrored copy, so the example is not reproducible from the doc alone.
//! - `docs/ato/inherited-assets-cost-base.md` "Example: legal costs incurred to
//!   prove the validity of a will" (Annie) and "Example: legal costs incurred
//!   prior to the deceased's death" (Cassie) — both classify *whether* an
//!   LPR's legal costs are includable in the cost base, a judgement made
//!   before entry; the includable figure enters as the inheritance's LPR
//!   expenditure, exercised by the Maria/Antonio test below.
//! - `docs/ato/crypto-cgt.md` "Example: market value of old crypto asset
//!   determines its disposal proceeds" (Katrina's Coin A → Coin D swap) —
//!   operationally identical to the "new asset" example reproduced below:
//!   either way the swap is entered as a manual Sell at the swap's market
//!   value, and which side supplies that valuation is a judgement made
//!   before entry, so the example would exercise nothing new.
//! - `docs/ato/crypto-wrapping.md` "Example: CGT treatment when exchanging
//!   wrapped tokens" (Kal's BTC → WBTC) — wrapping *is* a crypto-to-crypto
//!   swap, so it is entered exactly as Katrina's example already reproduced
//!   below (a Sell at the exchange's market value plus a Buy of the wrapped
//!   token at the same value); the example would exercise nothing new, and no
//!   wrapped token is a seeded digital-token code.
//! - `docs/ato/crypto-staking-airdrops.md` Anastasia and Merindah stop at
//!   "the money value … is ordinary assessable income" without stating one, so
//!   there is no figure to assert; the rule they state — ordinary income at
//!   the receipt-date market value, and that value as the tokens' cost base —
//!   is reproduced through Craig's example below, which does carry figures.
//!   Calista's paid initial allocation is Josh's example with a non-zero
//!   purchase price, exercising nothing new.
//! - `docs/ato/crypto-chain-splits.md` "Example: protocol change" (Bree) —
//!   definitional: which of two post-split assets is the *new* one is a
//!   judgement made before entry, and it states no figures.
//! - `docs/ato/forex-common-transactions.md` scenario 1 (Tom) and the FRE 2
//!   tail of scenario 2 (Lisa's $1,075 forex realisation loss) — both are
//!   Div 775 forex realisation outcomes on the contract-to-settlement window
//!   (under Tom's and Lisa's elections out of the 12-month rule, revenue
//!   amounts), and the forex measures are not modelled; Lisa's CGT side
//!   (cost base and proceeds translated at the trade dates) is reproduced
//!   below.
//! - `docs/ato/forex-cgt-12-month-rule.md` (Art Ltd, Eleanor) — the default
//!   12-month-rule integration of the settlement-window forex movement (a
//!   cost-base adjustment on an acquisition; a CGT event K10/K11 capital
//!   gain/loss on a disposal) is resolved out of scope as a Known limitation
//!   (`docs/API.md`, pinned by `src/doc_checks.rs`): trades convert at the
//!   monthly rate of the trade date, so a same-rate-month T+2 settlement nets
//!   to nil by construction; per-leg `spot_fx_rate` entry is what makes the
//!   movement visible, and it stays the taxpayer's manual adjustment.
//! - `docs/ato/capital-gains-question-18.md` Example 1's **jewellery leg** and
//!   Example 6 (Kathleen's label V) — a collectable's capital loss is
//!   quarantined to gains from other collectables, and neither collectables
//!   nor personal-use assets are modelled (one loss pool, no asset-class
//!   dimension — a Known limitation in `docs/API.md`). Her share legs and the
//!   whole loss-order → discount chain of Examples 1–5 are reproduced below.
//! - `docs/ato/demergers.md` Example 29 (Peter) — purely definitional (what a
//!   demerger *is*: Company A transfers its Company B shares to shareholders);
//!   it states no figures, so there is nothing to assert.
//! - `docs/ato/cgt-event-timing.md` "Example: insurance policy" (Laurie) and
//!   "Example: no compensation or insurance policy" (Christine) — both are
//!   CGT event C1 (an asset lost or destroyed), where the event date turns on
//!   when compensation was received or when the damage happened. Asset
//!   destruction is not modelled, and as with the TD 2000/52 timing example
//!   the date is a judgement made before entry — the system records the
//!   user-supplied event date. Sue's contract-date example is reproduced below.
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
use crate::reports::realised_gains::{DisposalSource, RealisedGainLoss};
use crate::reports::tax_summary::TaxYearSummary;
use axum::http::StatusCode;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::test_support::{ApiClient, test_pool};

/// PUT a JSON body to the API and require the entity-write success status (204).
async fn api_put(pool: &SqlitePool, path: &str, body: Value) {
    ApiClient::full(pool).put_ok(path, &body).await;
}

/// POST a JSON body (action/report endpoints) and deserialize the JSON response,
/// requiring the given success status.
async fn api_post<T: serde::de::DeserializeOwned>(
    pool: &SqlitePool,
    path: &str,
    body: Value,
    expect: StatusCode,
) -> T {
    ApiClient::full(pool)
        .post(path, &body)
        .await
        .expect_status(expect)
        .json()
}

/// GET a report endpoint and deserialize the JSON response.
async fn api_get<T: serde::de::DeserializeOwned>(pool: &SqlitePool, path: &str) -> T {
    ApiClient::full(pool).get_json(path).await
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

/// A listing for a crypto asset: exchange-less, and its ticker must be a
/// recognised digital-token code (BTC/ETH are the seeded ones).
async fn put_crypto_listing(pool: &SqlitePool, id: i64, ticker: &str) {
    api_put(
        pool,
        &format!("/listings/{id}"),
        json!({
            "ticker": ticker,
            "name": ticker,
            "isin": null,
            "security_type": "Crypto",
            "currency": "AUD",
            "amit": false,
        }),
    )
    .await;
}

/// Enter a Buy via `PUT /trades/:id` (AUD, with optional incidental costs as brokerage).
async fn put_buy(
    pool: &SqlitePool,
    id: i64,
    listing_id: i64,
    date: &str,
    qty: &str,
    price: &str,
    brokerage: &str,
) {
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
// Test fixture: flat positional args read fine here; bundling them into a params
// struct would add ceremony without aiding the tests.
#[allow(clippy::too_many_arguments)]
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

/// `docs/ato/cgt-how-to-calculate.md` — "Example: CGT with discount".
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
    assert_eq!(
        y.discount_eligible_gains,
        dec("10000"),
        "the $10,000 profit"
    );
    assert_eq!(y.capital_losses, Decimal::ZERO, "he has no capital losses");
    assert_eq!(y.cgt_discount, dec("5000"), "50% CGT discount");
    assert_eq!(
        y.net_capital_gain,
        dec("5000"),
        "declares a capital gain of $5,000"
    );
}

/// `docs/ato/cgt-how-to-calculate.md` — "Example: working out CGT for a single asset".
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
    assert_eq!(
        s.cost_base,
        dec("516200"),
        "purchase price + stamp duty + conveyancing"
    );
    assert_eq!(
        s.proceeds,
        dec("586200"),
        "$600,000 less the $13,800 sale costs"
    );
    assert_eq!(
        s.capital_gain_loss,
        dec("70000"),
        "step 3: the $70,000 capital gain"
    );
    assert_eq!(
        s.discount_eligible_gain,
        dec("70000"),
        "owned at least 12 months"
    );

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2025); // contract dated 4 June 2025 → FY ending 30 June 2025
    assert_eq!(
        y.cgt_discount,
        dec("35000"),
        "step 7: $70,000 × 50% = $35,000"
    );
    assert_eq!(
        y.net_capital_gain,
        dec("35000"),
        "step 8: net capital gain of $35,000"
    );
}

/// `docs/ato/cgt-how-to-calculate.md` — "Example: working out CGT for multiple assets".
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
    assert_eq!(
        shares.capital_gain_loss,
        dec("-4500"),
        "step 3: a $4,500 capital loss"
    );
    assert_eq!(shares.capital_loss, dec("4500"));

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2025);
    assert_eq!(y.discount_eligible_gains, dec("70000"), "the property gain");
    assert_eq!(y.capital_losses, dec("4500"), "the share loss");
    // Step 5: losses come off before the discount → $70,000 − $4,500 = $65,500.
    assert_eq!(y.net_discount_eligible_gain, dec("65500"));
    assert_eq!(
        y.cgt_discount,
        dec("32750"),
        "step 7: $65,500 × 50% = $32,750"
    );
    assert_eq!(
        y.net_capital_gain,
        dec("32750"),
        "step 8: net capital gain of $32,750"
    );
    assert_eq!(y.capital_loss_carried_forward, Decimal::ZERO);
}

/// `docs/ato/cgt-dividend-reinvestment-plans.md` — "Example: dividend reinvestment plans".
///
/// > Natalie owns 1,440 shares in a company. In November 2025, the company
/// > declared a dividend of 25 cents per share. Natalie was offered the choice of:
/// > - taking the dividend as a cash payment of $360 (1,440 × 25 cents)
/// > - reinvesting the dividend to acquire 45 more shares at $8 per share ($360 ÷ $8).
/// > Natalie decided to participate in the dividend reinvestment plan and
/// > received 45 new shares on 20 December 2025. This means:
/// > - she must declare the $360 dividend as assessable dividend income in her
/// >   2025–26 tax return
/// > - for CGT purposes, she acquired the 45 new shares for $360 on 20 December 2025.
#[tokio::test]
async fn drp_example_natalie_reinvested_dividend() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "NTLE").await;
    // Enrol the holding in the DRP (an open-ended enrolment period covering the
    // distribution), then record the $360 distribution.
    api_put(
        &pool,
        "/drp_enrolments/1",
        json!({ "listing_id": 1, "enrolment_date": "2025-01-01", "residual_handling": "CarryForward" }),
    )
    .await;
    api_put(
        &pool,
        "/income/1",
        json!({
            "listing_id": 1,
            "date_paid": "2025-12-20",
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
    assert_eq!(
        trade.date.to_string(),
        "2025-12-20",
        "received on 20 December 2025"
    );

    // For CGT purposes the 45 shares were acquired for $360.
    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert_eq!(holdings.len(), 1);
    assert_eq!(holdings[0].quantity, dec("45"));
    assert_eq!(
        holdings[0].total_cost_base,
        dec("360"),
        "acquired the 45 new shares for $360"
    );

    // The $360 dividend is assessable income in her 2025–26 tax return.
    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1);
    assert_eq!(years[0].tax_year, 2026); // paid Dec 2025 → 2025–26
    assert_eq!(
        years[0].dividends_assessable,
        dec("360"),
        "declares the $360 dividend"
    );
}

/// `docs/ato/cgt-keeping-records-shares.md` — "Example: identifying when shares or
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
    assert_eq!(
        sales[0].cost_base,
        dec("15000"),
        "1,500 of the $10 (2024) shares"
    );
    assert_eq!(sales[0].proceeds, dec("12000"), "1,500 × $8");
    assert_eq!(
        sales[0].capital_gain_loss,
        dec("-3000"),
        "the claimed capital loss"
    );
    assert_eq!(sales[0].capital_loss, dec("3000"));
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(
        years[0].tax_year, 2025,
        "loss claimed in the 2025 income year"
    );
    assert_eq!(years[0].capital_losses, dec("3000"));

    // Boris still has 1,000 × $5 + 1,500 × $10 = 2,500 shares costing $20,000.
    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert_eq!(holdings.len(), 1);
    assert_eq!(holdings[0].quantity, dec("2500"));
    assert_eq!(holdings[0].total_cost_base, dec("20000"));
}

/// `docs/ato/cgt-keeping-records-shares.md` — "Example: identifying when shares
/// or units were acquired" (Boris), decision-support side.
///
/// The same facts as `keeping_records_example_boris_identifying_shares_sold`,
/// but *before* the sale is entered: the parcel-selection optimiser's
/// harvest-losses candidate makes Boris's choice — sell 1,500 of the $10
/// (2024) shares to claim the $3,000 capital loss — and the pre-sale what-if
/// previews the 2025 income year with and without that disposal, all without
/// writing a row.
#[tokio::test]
async fn keeping_records_example_boris_optimiser_recommends_the_loss_parcel() {
    use crate::reports::net_capital_gain::WhatIfResponse;
    use crate::reports::parcel_optimiser::{OptimiserResponse, Strategy};
    let pool = test_pool().await;
    put_listing(&pool, 1, "BORI").await;
    put_buy(&pool, 1, 1, "2023-05-15", "1000", "5", "0").await;
    put_buy(&pool, 2, 1, "2024-05-15", "3000", "10", "0").await;

    // Optimise a sale of 1,500 at $8 on Boris's 2025 sale date.
    let r: OptimiserResponse = api_post(
        &pool,
        "/portfolio/parcel-optimiser",
        json!({
            "listing_id": 1, "holding_account_id": 1, "units": "1500",
            "sale_date": "2025-05-15", "price": "8"
        }),
        StatusCode::OK,
    )
    .await;
    let harvest = r
        .strategies
        .iter()
        .find(|s| s.strategy == Strategy::HarvestLosses)
        .unwrap();
    assert_eq!(
        harvest.totals.capital_loss,
        dec("3000"),
        "(10 − 8) × 1,500 — Boris's claimed loss"
    );
    assert_eq!(harvest.totals.capital_gain_loss, dec("-3000"));
    let harvest_allocs: Vec<_> = r
        .allocations
        .iter()
        .filter(|a| a.strategy == Strategy::HarvestLosses)
        .collect();
    assert_eq!(harvest_allocs.len(), 1, "all 1,500 from one parcel");
    assert_eq!(
        harvest_allocs[0].allocation.purchase_trade_id, 2,
        "the 2024 ($10) parcel"
    );
    assert_eq!(harvest_allocs[0].allocation.units, dec("1500"));
    // The FIFO baseline would instead realise a gain on the $5 shares.
    let fifo = r
        .strategies
        .iter()
        .find(|s| s.strategy == Strategy::Fifo)
        .unwrap();
    assert_eq!(
        fifo.totals.capital_gain_loss,
        dec("2000"),
        "FIFO: 1,000 × $3 gain − 500 × $2 loss"
    );

    // The what-if previews the 2025 income year for that choice — a dry run.
    let w: WhatIfResponse = api_post(
        &pool,
        "/portfolio/net-capital-gain/what-if",
        json!({
            "listing_id": 1, "units": "1500", "proceeds": "12000",
            "date": "2025-05-15", "strategy": "harvest_losses"
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(w.tax_year, 2025, "loss claimed in the 2025 income year");
    assert_eq!(w.years[0].year.capital_losses, dec("0"));
    assert_eq!(w.years[1].year.capital_losses, dec("3000"));
    assert_eq!(w.years[1].year.net_capital_gain, dec("0"));
    assert_eq!(w.years[1].year.capital_loss_carried_forward, dec("3000"));

    // Nothing was persisted: the holding is still 4,000 shares at $35,000.
    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert_eq!(holdings.len(), 1);
    assert_eq!(holdings[0].quantity, dec("4000"));
    assert_eq!(holdings[0].total_cost_base, dec("35000"));
}

/// `docs/ato/you-and-your-shares-dividends.md` — "Example 1: payment of dividends" /
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
    assert_eq!(
        y.dividends_assessable,
        dec("900"),
        "unfranked $200 + franked $700"
    );
    assert_eq!(y.franking_credits, dec("300"), "franking credit $300");
    assert_eq!(
        y.dividends_assessable + y.franking_credits,
        dec("1200"),
        "total assessable dividend income $1,200 (the franking-credit gross-up)"
    );
}

/// `docs/ato/you-and-your-shares-dividends.md` — "Example 6: franking credits
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
/// The 45-day holding-period rule and the $5,000 small-shareholder exemption
/// are implemented in `reports::franking`, applied by the tax summary.
#[tokio::test]
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
    assert_eq!(
        y.dividends_assessable,
        dec("13066"),
        "shows the dividend as a franked amount"
    );
    // …but has no entitlement to any part of the $5,600 franking credits
    // (credits > $5,000, so the small-shareholder exemption can't restore them).
    assert_eq!(
        y.franking_credits,
        Decimal::ZERO,
        "no entitlement to the $5,600 franking credits — held under 45 days"
    );
    assert_eq!(
        y.franking_credits_denied,
        dec("5600"),
        "the denied credits are surfaced, not silently dropped"
    );
}

/// `docs/ato/you-and-your-shares-dividends.md` — "Example 7: substantially
/// identical shares" (You and your shares 2025).
///
/// > Jessica holds 10,000 shares in Mimosa Pty Ltd for 12 months. She
/// > purchases an extra 4,000 shares in Mimosa Pty Ltd 10 days before they
/// > became ex-dividend and then sells 4,000 shares 20 days after Mimosa Pty
/// > Ltd shares became ex-dividend. Her total franking credit entitlement for
/// > the income year is more than $5,000. The shares she sells are deemed to
/// > have been held for less than 45 days, based on the last-in first-out
/// > method. Jessica can't claim the franking credits on the 4,000 shares sold.
///
/// The doc fixes only the share counts, so the dividend carries $7,000 of
/// credits (> $5,000): 4,000 of the 14,000 entitled shares fail the test, so
/// $2,000 is denied and $5,000 remains claimable. The sale's CGT parcel
/// allocation deliberately nominates the *old* parcel — the holding-period
/// rule must use LIFO identification regardless of the CGT choice.
#[tokio::test]
async fn you_and_your_shares_example_7_jessica_lifo_identification() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "MIM").await;
    // 10,000 shares held 12 months before the ex-date…
    put_buy(&pool, 1, 1, "2024-03-14", "10000", "5", "0").await;
    // …plus 4,000 bought 10 days before the shares went ex-dividend (14 Mar 2025).
    put_buy(&pool, 2, 1, "2025-03-04", "4000", "8", "0").await;
    api_put(
        &pool,
        "/income/1",
        json!({
            "listing_id": 1,
            "date_paid": "2025-04-08",
            "ex_date": "2025-03-14",
            "franked_amount": "16334",
            "franking_credits": "7000",
            "currency": "AUD",
        }),
    )
    .await;
    // 4,000 sold 20 days after the ex-date, CGT-allocated from the old parcel.
    put_sell(&pool, 3, 1, "2025-04-03", "4000", "8", "0", 1).await;

    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2025);
    assert_eq!(y.dividends_assessable, dec("16334"));
    // LIFO deems the 4,000 sold to be the recently bought parcel (held < 45
    // days at risk): their 4/14 share of the credits is denied.
    assert_eq!(
        y.franking_credits_denied,
        dec("2000"),
        "can't claim the credits on the 4,000 shares sold (LIFO)"
    );
    assert_eq!(
        y.franking_credits,
        dec("5000"),
        "credits on the long-held 10,000 shares remain claimable"
    );
}

/// `docs/ato/cgt-non-assessable-payments.md` — "Example 45: Non-assessable payments"
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
#[tokio::test]
async fn cgt_non_assessable_payments_example_45_rob_return_of_capital() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "RAP").await;
    put_buy(&pool, 1, 1, "1994-07-01", "1500", "5", "0").await;
    // The G1 return of capital, recorded as a corporate action.
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

/// `docs/ato/share-splits-and-consolidations.md` (TD 2000/10) — "Example 1".
///
/// > XYZ Ltd … converts its share capital into 200,000 ordinary shares on
/// > 1 July 1992. … John acquired 2,000 ordinary shares in XYZ Ltd in
/// > September 1984 and 3,000 ordinary shares in XYZ Ltd on 30 April 1988.
/// > Before the conversion, the shares John acquired in 1988 had a cost base
/// > of $1.00 each. … John, however, now has 4,000 ordinary shares with an
/// > acquisition date before 20 September 1985, and 6,000 ordinary shares
/// > with a cost base of $0.50 each with an acquisition date on 30 April 1988.
///
/// (The pre-CGT exemption for John's 1984 parcel is not modelled, and a
/// pre-CGT-dated trade is rejected at write time — see Known limitations —
/// so that parcel is entered with the first post-CGT date, 20 September
/// 1985, as a stand-in: the conversion arithmetic is date-independent, and
/// the assertion on that parcel covers the quantity conversion and preserved
/// acquisition date the determination states.)
#[tokio::test]
async fn td_2000_10_example_1_john_share_split() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "XYZ").await;
    put_buy(&pool, 1, 1, "1985-09-20", "2000", "1", "0").await;
    put_buy(&pool, 2, 1, "1988-04-30", "3000", "1", "0").await;
    // The 2-for-1 conversion (100,000 → 200,000 shares), as a corporate action.
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "ShareSplit",
            "listing_id": 1,
            "date": "1992-07-01",
            "split_new_units": "2",
            "split_old_units": "1",
        }),
    )
    .await;

    let parcels: Vec<crate::reports::open_parcels::OpenParcel> =
        api_get(&pool, "/portfolio/open-parcels").await;
    assert_eq!(parcels.len(), 2);
    // 2,000 → 4,000 shares, acquisition date preserved across the split
    // (the stand-in for the determination's September 1984 date).
    assert_eq!(parcels[0].remaining_quantity, dec("4000"));
    assert_eq!(parcels[0].acquisition_date.to_string(), "1985-09-20");
    // 3,000 → 6,000 shares with a cost base of $0.50 each, still 30 April 1988.
    assert_eq!(parcels[1].remaining_quantity, dec("6000"));
    assert_eq!(parcels[1].acquisition_date.to_string(), "1988-04-30");
    assert_eq!(
        parcels[1].remaining_cost_base,
        dec("3000"),
        "total cost base unchanged: 6,000 × $0.50"
    );

    // No CGT event happens: the conversion itself creates no capital gain.
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert!(years.iter().all(|y| y.net_capital_gain == Decimal::ZERO));
}

/// `docs/ato/share-splits-and-consolidations.md` (TD 2000/10) — "Example 2".
///
/// > If XYZ Ltd in Example 1 decides instead to convert its original share
/// > capital into 50,000 ordinary shares, and all the other facts remain
/// > unchanged, no CGT event happens to John's original shares. In this case,
/// > John would now have 1,000 ordinary shares with an acquisition date before
/// > 20 September 1985, and 1,500 ordinary shares with a cost base of $2.00
/// > each with an acquisition date on 30 April 1988.
///
/// (As in Example 1, John's pre-CGT parcel is entered with the first
/// post-CGT date as a stand-in — a pre-CGT-dated trade is rejected at write
/// time.)
#[tokio::test]
async fn td_2000_10_example_2_john_share_consolidation() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "XYZ").await;
    put_buy(&pool, 1, 1, "1985-09-20", "2000", "1", "0").await;
    put_buy(&pool, 2, 1, "1988-04-30", "3000", "1", "0").await;
    // The 1-for-2 consolidation (100,000 → 50,000 shares).
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "ShareSplit",
            "listing_id": 1,
            "date": "1992-07-01",
            "split_new_units": "1",
            "split_old_units": "2",
        }),
    )
    .await;

    let parcels: Vec<crate::reports::open_parcels::OpenParcel> =
        api_get(&pool, "/portfolio/open-parcels").await;
    assert_eq!(parcels.len(), 2);
    // 2,000 → 1,000 shares, acquisition date preserved.
    assert_eq!(parcels[0].remaining_quantity, dec("1000"));
    assert_eq!(parcels[0].acquisition_date.to_string(), "1985-09-20");
    // 3,000 → 1,500 shares with a cost base of $2.00 each.
    assert_eq!(parcels[1].remaining_quantity, dec("1500"));
    assert_eq!(
        parcels[1].remaining_cost_base,
        dec("3000"),
        "total cost base unchanged: 1,500 × $2.00"
    );

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert!(years.iter().all(|y| y.net_capital_gain == Decimal::ZERO));
}

/// `docs/ato/bonus-shares.md` — "Example 35: Fully paid bonus shares".
///
/// > Chris bought 100 shares in MAC Ltd for $1 each on 1 June 1985. He bought
/// > 300 more shares for $1 each on 27 May 1986. On 15 November 1986, MAC Ltd
/// > issued Chris with 400 bonus shares from its capital profits reserve,
/// > fully paid to $1. […] no part of the value of the bonus shares was
/// > assessed as a dividend.
/// >
/// > The acquisition date of the other 300 bonus shares is 27 May 1986. Their
/// > cost base is worked out by spreading the cost of the 300 shares Chris
/// > bought on that date over both those original shares and the remaining
/// > 300 bonus shares. As the 300 original shares cost $300, the cost base of
/// > each share will now be 50 cents.
///
/// The 400 bonus shares on 400 held are a 1-for-1 issue applied per parcel.
/// The ATO treats the first parcel (1 June 1985) as pre-CGT and exempt; the
/// pre-CGT exemption is not modelled here and a pre-CGT-dated trade is
/// rejected at write time (see Known limitations), so that parcel is entered
/// with the first post-CGT date, 20 September 1985, as a stand-in and this
/// test asserts only the figures the ATO states for the post-CGT parcel —
/// 600 shares, unchanged $300 cost base (50 cents each), acquisition date
/// still 27 May 1986 — plus that the issue itself is no CGT event.
#[tokio::test]
async fn bonus_shares_example_35_chris_fully_paid_bonus_shares() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "MAC").await;
    put_buy(&pool, 1, 1, "1985-09-20", "100", "1", "0").await;
    put_buy(&pool, 2, 1, "1986-05-27", "300", "1", "0").await;
    // The 1-for-1 bonus issue from the capital profits reserve.
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "BonusIssue",
            "listing_id": 1,
            "date": "1986-11-15",
            "bonus_units": "1",
            "bonus_held_units": "1",
        }),
    )
    .await;

    let parcels: Vec<crate::reports::open_parcels::OpenParcel> =
        api_get(&pool, "/portfolio/open-parcels").await;
    assert_eq!(parcels.len(), 2);
    // 100 → 200 shares, acquisition date preserved (the ATO's pre-CGT
    // exemption for this parcel is out of scope; its 1 June 1985 date is
    // entered as the first post-CGT day, the stand-in explained above).
    assert_eq!(parcels[0].remaining_quantity, dec("200"));
    assert_eq!(parcels[0].acquisition_date.to_string(), "1985-09-20");
    // 300 → 600 shares at 50 cents: the $300 cost base is unchanged.
    assert_eq!(parcels[1].remaining_quantity, dec("600"));
    assert_eq!(parcels[1].acquisition_date.to_string(), "1986-05-27");
    assert_eq!(
        parcels[1].remaining_cost_base,
        dec("300"),
        "total cost base unchanged: 600 × $0.50"
    );

    // The bonus issue itself is no CGT event.
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert!(years.iter().all(|y| y.net_capital_gain == Decimal::ZERO));
}

/// `docs/ato/rights-issues.md` (Guide to CGT, QC 64895) — "Example 39: Sale
/// of rights".
///
/// > Shanti owns 2,000 shares in ZAC Ltd. She bought 1,000 shares on
/// > 1 June 1985 and 1,000 shares on 1 December 1996. On 1 July 1998, ZAC Ltd
/// > granted each of its shareholders one right for each four shares owned to
/// > acquire shares in the company for $1.80 each. Shanti therefore received
/// > 500 rights in total. At that time, shares in ZAC Ltd were worth $2. Each
/// > right was therefore worth 20 cents. Shanti decided that she did not wish
/// > to buy any more shares in ZAC Ltd, so she sold all her rights for
/// > 20 cents each […] Only those rights issued for the shares she bought on
/// > 1 December 1996 are subject to CGT. As Shanti did not pay anything for
/// > the rights, she has made a **$50 taxable capital gain** on their sale.
///
/// The pre-CGT half (the $50 received for the rights over the 1 June 1985
/// shares, disregarded as pre-CGT) turns on pre-CGT originals, which are not
/// modelled — so this test enters only the post-CGT 1,000-share parcel and
/// asserts the ATO's figures for it: a $50 capital gain (nil cost base — the
/// rights were free), with the original shares untouched. The rights are
/// taken to have been acquired with the original shares (1 December 1996,
/// > 12 months before the sale), so under current law the gain is
/// discount-eligible (`docs/ato/retail-premiums.md` states the same rule; the
/// example predates the 1999 discount, so the ATO's stated figure is the
/// gross $50).
#[tokio::test]
async fn rights_issues_example_39_shanti_sale_of_rights() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "ZAC").await;
    put_buy(&pool, 1, 1, "1996-12-01", "1000", "2", "0").await;
    // One right per four shares owned, exercise price $1.80, record 1 July 1998.
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "RightsIssue",
            "listing_id": 1,
            "date": "1998-07-01",
            "rights_units": "1",
            "rights_held_units": "4",
            "exercise_price": "1.80",
            "currency": "AUD",
        }),
    )
    .await;
    // Shanti sells the 250 rights her post-CGT shares earned, at 20 cents.
    let sale: Value = api_post(
        &pool,
        "/corporate_actions/1/sell_rights",
        json!({
            "date": "1998-07-15",
            "units": "250",
            "proceeds_per_right": "0.20",
            "allocations": [{ "purchase_trade_id": 1, "units": "250" }],
        }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(sale["units"], "250");

    // "she has made a $50 taxable capital gain on their sale" — nil cost
    // base, proceeds 250 × $0.20 = $50.
    let gains: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(gains.len(), 1);
    assert_eq!(gains[0].source, DisposalSource::RightsSale);
    assert_eq!(gains[0].proceeds, dec("50"));
    assert_eq!(
        gains[0].cost_base,
        dec("0"),
        "Shanti paid nothing for the rights"
    );
    assert_eq!(gains[0].capital_gain_loss, dec("50"));
    // Deemed acquired with the 1 December 1996 shares — > 12 months.
    assert_eq!(gains[0].discount_eligible_gain, dec("50"));

    // Selling the rights does not touch the original shares.
    let parcels: Vec<crate::reports::open_parcels::OpenParcel> =
        api_get(&pool, "/portfolio/open-parcels").await;
    assert_eq!(parcels.len(), 1);
    assert_eq!(parcels[0].remaining_quantity, dec("1000"));
    assert_eq!(parcels[0].remaining_cost_base, dec("2000"));
}

/// `docs/ato/rights-issues.md` (Guide to CGT, QC 64895) — "Example 40: Rights
/// exercised" (building on Example 39's facts).
///
/// > Shanti owns 2,000 shares in ZAC Ltd. She bought 1,000 shares on
/// > 1 June 1985 and 1,000 shares on 1 December 1996. On 1 July 1998, ZAC Ltd
/// > granted each of its shareholders one right for each four shares owned to
/// > acquire shares in the company for $1.80 each. […] She therefore
/// > exercised all 500 rights on 1 August 1998 […] There are no CGT
/// > consequences arising from the exercise of the rights. However, the 500
/// > shares Shanti acquired on 1 August 1998 when she exercised the rights
/// > are subject to CGT and are acquired at the time of the exercise. When
/// > Shanti exercised the rights issued for the shares she bought on
/// > 1 December 1996, the cost base of the 250 shares she acquired is the
/// > amount she paid to exercise each right ($1.80 for each share).
///
/// The pre-CGT half (the rights over the 1 June 1985 shares, whose cost base
/// includes the rights' 20-cent market value) turns on pre-CGT originals,
/// which are not modelled — so this test enters only the post-CGT 1,000-share
/// parcel and asserts the figures the ATO states for it: 250 new shares
/// acquired 1 August 1998 at a $450 cost base ($1.80 each), and no CGT event
/// from the exercise.
#[tokio::test]
async fn rights_issues_example_40_shanti_rights_exercised() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "ZAC").await;
    put_buy(&pool, 1, 1, "1996-12-01", "1000", "2", "0").await;
    // One right per four shares owned, exercise price $1.80, record 1 July 1998.
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "RightsIssue",
            "listing_id": 1,
            "date": "1998-07-01",
            "rights_units": "1",
            "rights_held_units": "4",
            "exercise_price": "1.80",
            "currency": "AUD",
        }),
    )
    .await;
    // Shanti exercises the 250 rights her post-CGT shares earned, on 1 Aug 1998.
    let trade: Trade = api_post(
        &pool,
        "/corporate_actions/1/exercise",
        json!({ "date": "1998-08-01", "units": "250" }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(trade.trade_type, TradeType::Buy);
    assert_eq!(trade.quantity, dec("250"));
    assert_eq!(trade.average_price, dec("1.80"));

    // The 250 shares are a new parcel acquired at the time of exercise, with
    // a cost base of the amount paid to exercise: 250 × $1.80 = $450.
    let parcels: Vec<crate::reports::open_parcels::OpenParcel> =
        api_get(&pool, "/portfolio/open-parcels").await;
    assert_eq!(parcels.len(), 2);
    assert_eq!(
        parcels[0].remaining_quantity,
        dec("1000"),
        "original parcel untouched"
    );
    assert_eq!(parcels[1].acquisition_date.to_string(), "1998-08-01");
    assert_eq!(parcels[1].remaining_quantity, dec("250"));
    assert_eq!(
        parcels[1].remaining_cost_base,
        dec("450"),
        "$1.80 for each share"
    );

    // "There are no CGT consequences arising from the exercise of the rights."
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert!(years.iter().all(|y| y.net_capital_gain == Decimal::ZERO));
}

/// `docs/ato/share-buy-backs.md` (QC 66049) — "Example: off-market buy-back".
///
/// > Ranjini bought 10,000 shares in a company that was not a listed public
/// > company at a cost of $6 per share, including brokerage. A few years
/// > later, the company wrote to its shareholders offering to buy back 10% of
/// > their shares for $9.60 each. The buy-back price included a franked
/// > dividend of $1.40 per share, with each dividend to carry a franking
/// > credit of $0.60. Ranjini applied to participate in the buy-back to sell
/// > 1,000 of her shares. … The market value of the company's shares at the
/// > time of the buy-back, assuming the buy-back had not been proposed, was
/// > $10.20. …
/// > Market value of shares: $10.20 × 1,000 = $10,200
/// > Dividend: $1.40 × 1,000 = $1,400
/// > Capital proceeds: $10,200 − $1,400 = $8,800
/// > Cost base: $6.00 × 1,000 = $6,000
/// > Capital gain (before applying any discount) is $8,800 − $6,000 = $2,800
/// > Ranjini must report her capital gain as well as her dividend of $1,400
/// > and franking credit of $600 in her tax return.
#[tokio::test]
async fn share_buy_backs_example_ranjini_off_market_buy_back() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "RNJ").await;
    // 10,000 shares at $6 each including brokerage, a few years earlier.
    put_buy(&pool, 1, 1, "2021-01-15", "10000", "6", "0").await;
    // The buy-back offer terms, recorded as a corporate action: $9.60 price
    // including a $1.40 franked dividend ($0.60 credit); market value had the
    // buy-back not been proposed $10.20.
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "BuyBack",
            "listing_id": 1,
            "date": "2024-11-30",
            "buyback_price": "9.60",
            "buyback_dividend": "1.40",
            "buyback_franking_credit": "0.60",
            "buyback_market_value": "10.20",
            "currency": "AUD",
        }),
    )
    .await;
    // Ranjini sells 1,000 shares into the buy-back.
    let participation: Value = api_post(
        &pool,
        "/corporate_actions/1/participate",
        json!({
            "date": "2024-11-30",
            "units": "1000",
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "1000" } ],
        }),
        StatusCode::CREATED,
    )
    .await;
    // Capital proceeds per share use the market value (the buy-back price is
    // less than it), excluding the dividend: $10.20 − $1.40 = $8.80.
    assert_eq!(participation["trade"]["average_price"], "8.80");

    // Capital proceeds $8,800 − cost base $6,000 = $2,800 capital gain
    // (before applying any discount; held > 12 months so it is eligible).
    let gains: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(gains.len(), 1);
    assert_eq!(
        gains[0].proceeds,
        dec("8800.00"),
        "capital proceeds: $10,200 − $1,400"
    );
    assert_eq!(
        gains[0].cost_base,
        dec("6000.00"),
        "cost base: $6.00 × 1,000"
    );
    assert_eq!(
        gains[0].capital_gain_loss,
        dec("2800.00"),
        "capital gain before any discount: $8,800 − $6,000"
    );
    assert_eq!(gains[0].discount_eligible_gain, dec("2800.00"));

    // The dividend of $1,400 and franking credit of $600 are reported as
    // income in the same return (paid Nov 2024 → FY2025).
    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1);
    assert_eq!(years[0].tax_year, 2025);
    assert_eq!(
        years[0].dividends_assessable,
        dec("1400.00"),
        "dividend: $1.40 × 1,000"
    );
    assert_eq!(
        years[0].franking_credits,
        dec("600.00"),
        "franking credit: $0.60 × 1,000"
    );

    // Her remaining holding: 9,000 shares at the untouched $6 cost base.
    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert_eq!(holdings.len(), 1);
    assert_eq!(holdings[0].quantity, dec("9000"));
    assert_eq!(holdings[0].total_cost_base, dec("54000.00"));
}

/// `docs/ato/lic-capital-gain-deduction.md` — "Example: Resident individual".
///
/// > Ben, an Australian resident, is a shareholder in XYZ Ltd, a LIC. On
/// > 21 February 2025, Ben received a fully franked dividend from XYZ Ltd of
/// > $70, with an eligible capital gain amount (attributable part) of $50.
/// > Ben includes the following amounts in his 2024–25 tax return:
/// > - Dividends – Franked amount: $70.
/// > - Dividends – Franking credit: $30.
/// > - Dividend deductions: $25 (50% deduction for LIC capital gain).
///
/// The income record carries the **$50 attributable part** the LIC advised —
/// the statement's own figure — and the tax summary's D8 line is the
/// individual's 50% of it, $25 (`Income::lic_capital_gain_deduction`).
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
            "lic_capital_gain_amount": "50",
            "currency": "AUD",
        }),
    )
    .await;

    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2025); // paid Feb 2025 → 2024–25 return
    assert_eq!(
        y.dividends_assessable,
        dec("70"),
        "Dividends – Franked amount: $70"
    );
    assert_eq!(
        y.franking_credits,
        dec("30"),
        "Dividends – Franking credit: $30"
    );
    assert_eq!(
        y.lic_capital_gain_deduction,
        dec("25"),
        "Dividend deductions: $25 (50% deduction for the $50 LIC capital gain amount entered)"
    );
}

/// `docs/ato/demergers.md` (QC 64895) — "Example 30: No pre-CGT interests" and
/// "Example 32: Using the discount method after a demerger (1)".
///
/// > Under the BHP Billiton Ltd demerger of BHP Steel Ltd, shareholders
/// > received one BHP Steel share for every five BHP Billiton shares they
/// > owned at the date of the demerger. Anita owned 280 BHP Billiton shares
/// > (all post-CGT) with a cost base of $2,500 immediately before the
/// > demerger. Under the demerger, Anita received 56 BHP Steel shares.
/// > BHP Billiton advised shareholders to apportion 94.937% of the total
/// > cost base to BHP Billiton shares and 5.063% to BHP Steel shares:
/// > (a) BHP Billiton: 94.937% × $2,500 = $2,373.43
/// > (b) BHP Steel: 5.063% × $2,500 = $126.58
///
/// (The ATO presents cent-rounded figures; the exact apportionment is
/// $2,373.425 and $126.575, summing to the $2,500 step 1 amount — this
/// system never rounds.) Example 32 dates the facts: the demerger happened on
/// 22 July 2002 and the shares were acquired on 15 August 2001, and the BHP
/// Steel shares can use the discount method only when disposed of after
/// 15 August 2002 — more than 12 months after the date the corresponding
/// BHP Billiton shares were acquired, not the demerger date.
#[tokio::test]
async fn demergers_examples_30_32_anita_bhp_billiton_demerger() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "BHP").await;
    put_listing(&pool, 2, "BSL").await;
    // 280 shares with a $2,500 total cost base (280 × $8.50 + $120 costs).
    put_buy(&pool, 1, 1, "2001-08-15", "280", "8.50", "120").await;
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "Demerger",
            "listing_id": 1,
            "date": "2002-07-22",
            "demerger_listing_id": 2,
            "demerger_new_units": "1",
            "demerger_held_units": "5",
            "demerger_cost_base_pct": "5.063",
        }),
    )
    .await;
    let demerge: Value = api_post(
        &pool,
        "/corporate_actions/1/demerge",
        json!({}),
        StatusCode::CREATED,
    )
    .await;
    let demerged_buy_id = demerge["demerged_replacements"][0]["id"].as_i64().unwrap();

    // Step 2: 94.937% of $2,500 stays with the 280 BHP Billiton shares and
    // 5.063% goes to the 56 BHP Steel shares; both keep the 15 Aug 2001
    // acquisition date (steps 3–4 divide these by 280 and 56 — the ATO's
    // $8.48 and $2.26 are those quotients rounded to the cent).
    let mut parcels: Vec<crate::reports::open_parcels::OpenParcel> =
        api_get(&pool, "/portfolio/open-parcels").await;
    parcels.sort_by_key(|p| p.listing_id);
    assert_eq!(parcels.len(), 2);
    assert_eq!(parcels[0].ticker, "BHP");
    assert_eq!(parcels[0].remaining_quantity, dec("280"));
    assert_eq!(
        parcels[0].remaining_cost_base,
        dec("2373.425"),
        "ATO: $2,373.43"
    );
    assert_eq!(parcels[0].acquisition_date.to_string(), "2001-08-15");
    assert_eq!(parcels[1].ticker, "BSL");
    assert_eq!(parcels[1].remaining_quantity, dec("56"));
    assert_eq!(
        parcels[1].remaining_cost_base,
        dec("126.575"),
        "ATO: $126.58"
    );
    assert_eq!(parcels[1].acquisition_date.to_string(), "2001-08-15");

    // The demerger itself: no CGT consequences (the rollover disregards any
    // gain made under the demerger).
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert!(
        years.is_empty(),
        "the demerger is not a CGT event: {years:?}"
    );

    // Example 32: dispose of the BHP Steel shares after 15 August 2002 —
    // under 12 months after the demerger but over 12 months after the BHP
    // Billiton shares were acquired — and the discount method applies.
    put_sell(&pool, 100, 2, "2002-09-01", "56", "5", "0", demerged_buy_id).await;
    let gains: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(gains.len(), 1);
    // 56 × $5 = $280 proceeds − $126.575 cost base = $153.425, all
    // discount-eligible.
    assert_eq!(gains[0].cost_base, dec("126.575"));
    assert_eq!(gains[0].capital_gain_loss, dec("153.425"));
    assert_eq!(gains[0].discount_eligible_gain, dec("153.425"));
    assert_eq!(gains[0].non_discountable_gain, Decimal::ZERO);
}

/// `docs/ato/takeovers-and-scrip-for-scrip.md` (QC 64895) — "Example 27:
/// Partial scrip for scrip rollover".
///
/// > Gunther owns 100 shares in Windsor Ltd, each with a cost base of $9. He
/// > accepts a takeover offer from Regal Ltd, which provides for Gunther to
/// > receive one Regal share plus $10 cash for each share in Windsor.
/// > Gunther receives 100 shares in Regal and $1,000 cash. Just after
/// > Gunther is issued shares in Regal, each share is worth $20.
/// >
/// > $1,000 ÷ $3,000 × $900 = $300 (cost base apportioned to the cash).
/// > Gunther's capital gain: $1,000 (cash) − $300 (cost base) = $700.
/// > Cost base of each of his Regal shares: ($900 − $300) ÷ 100 = $6.
///
/// The ATO doesn't date Gunther's acquisition; held > 12 months here, so the
/// $700 cash-side gain is discount-eligible per the original holding period
/// (the rollover side's combined-period rule).
#[tokio::test]
async fn takeovers_example_27_gunther_partial_scrip_for_scrip_rollover() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "WDR").await;
    put_listing(&pool, 2, "RGL").await;
    // 100 Windsor shares, $9 cost base each = $900.
    put_buy(&pool, 1, 1, "2023-01-15", "100", "9", "0").await;
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "ScripForScrip",
            "listing_id": 1,
            "date": "2024-07-10",
            "scrip_listing_id": 2,
            "scrip_new_units": "1",
            "scrip_old_units": "1",
            "scrip_cash_per_unit": "10",
            "scrip_market_value": "20",
            "scrip_cash_currency": "AUD",
        }),
    )
    .await;
    let _: Value = api_post(
        &pool,
        "/corporate_actions/1/exchange",
        json!({}),
        StatusCode::CREATED,
    )
    .await;

    // The cash side is assessed now: $1,000 proceeds against the $300
    // apportioned cost base.
    let gains: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(gains.len(), 1);
    assert_eq!(gains[0].proceeds, dec("1000"));
    assert_eq!(gains[0].cost_base, dec("300"), "ATO: $300 to the cash");
    assert_eq!(gains[0].capital_gain_loss, dec("700"), "ATO: $700 gain");
    assert_eq!(gains[0].discount_eligible_gain, dec("700"));

    // The rollover carries the rest: 100 Regal shares at $600 total — the
    // ATO's $6 each — with the original acquisition date.
    let parcels: Vec<crate::reports::open_parcels::OpenParcel> =
        api_get(&pool, "/portfolio/open-parcels").await;
    assert_eq!(parcels.len(), 1);
    assert_eq!(parcels[0].ticker, "RGL");
    assert_eq!(parcels[0].remaining_quantity, dec("100"));
    assert_eq!(parcels[0].remaining_cost_base, dec("600"), "ATO: $6 each");
    assert_eq!(parcels[0].acquisition_date.to_string(), "2023-01-15");

    // FY2025's net capital gain: the $700 discount-eligible gain halves.
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    assert_eq!(years[0].tax_year, 2025);
    assert_eq!(years[0].discount_eligible_gains, dec("700"));
    assert_eq!(years[0].cgt_discount, dec("350"));
    assert_eq!(years[0].net_capital_gain, dec("350"));
}

/// `docs/ato/crypto-cgt.md` — "Example: market value of new asset determines
/// old asset's disposal proceeds" (Crypto to crypto exchange or swap, QC 69949).
///
/// > Katrina acquires 100 Coin A for $15,000 on 5 July 2025. Katrina decides
/// > to exchange 20 Coin A for 100 Coin B through a reputable digital asset
/// > exchange on 15 November 2025. Using the exchange rates shown on the
/// > digital asset exchange at the time of the transaction, the market value
/// > of 100 Coin B was $6,000. Therefore, Katrina's capital proceeds are
/// > $6,000 for the disposal of 20 Coin A.
///
/// A crypto-to-crypto swap is entered manually as a Sell at the market-value
/// proceeds plus a Buy of the acquired asset at the same value (README Known
/// limitations). Coin A / Coin B are represented by the seeded BTC / ETH
/// token codes — a Crypto listing's ticker must be a recognised digital
/// token. The 20 disposed Coin A carry 15,000 × 20/100 = $3,000 of cost base,
/// so the swap realises a $3,000 capital gain, held 5 Jul → 15 Nov 2025
/// (under 12 months) → not discount-eligible; the acquired Coin B parcel
/// opens at the $6,000 swap value. Both legs settle same-day (a crypto asset
/// trades on no exchange — no T+n, no holiday calendar).
///
/// The doc's second example (Coin D, proceeds from the *old* asset's market
/// value when the new one has none) is entered identically — which side's
/// market value determined the proceeds happens outside this system — so it
/// is not separately reproduced.
#[tokio::test]
async fn crypto_cgt_example_katrina_coin_swap() {
    let pool = test_pool().await;
    // Coin A and Coin B: exchange-less Crypto listings (no exchange_mic).
    for (id, ticker) in [(1, "BTC"), (2, "ETH")] {
        api_put(
            &pool,
            &format!("/listings/{id}"),
            json!({
                "ticker": ticker,
                "name": ticker,
                "isin": null,
                "security_type": "Crypto",
                "currency": "AUD",
                "amit": false,
            }),
        )
        .await;
    }
    put_buy(&pool, 1, 1, "2025-07-05", "100", "150", "0").await; // 100 Coin A for $15,000
    // The swap on 15 Nov 2025: dispose of 20 Coin A for $6,000 (100 Coin B's
    // market value) and acquire 100 Coin B for the same $6,000.
    put_sell(&pool, 2, 1, "2025-11-15", "20", "300", "0", 1).await;
    put_buy(&pool, 3, 2, "2025-11-15", "100", "60", "0").await;

    // Crypto settles same-day: the auto-populated settlement date is the
    // trade date itself.
    let swap_sell: Trade = api_get(&pool, "/trades/2").await;
    assert_eq!(
        swap_sell.settlement_date, swap_sell.date,
        "crypto settles same-day"
    );

    // Katrina's capital proceeds are $6,000 against a $3,000 cost base.
    let sales: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(sales.len(), 1);
    assert_eq!(
        sales[0].proceeds,
        dec("6000"),
        "capital proceeds are $6,000"
    );
    assert_eq!(sales[0].cost_base, dec("3000"), "20/100 of the $15,000");
    assert_eq!(sales[0].capital_gain_loss, dec("3000"));
    assert_eq!(
        sales[0].non_discountable_gain,
        dec("3000"),
        "held under 12 months"
    );
    assert_eq!(sales[0].discount_eligible_gain, Decimal::ZERO);

    // FY2025-26: the $3,000 gain is assessable in full (no discount).
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    assert_eq!(years[0].tax_year, 2026);
    assert_eq!(years[0].other_gains, dec("3000"));
    assert_eq!(years[0].cgt_discount, Decimal::ZERO);
    assert_eq!(years[0].net_capital_gain, dec("3000"));

    // She now holds 80 Coin A ($12,000 cost base) and 100 Coin B ($6,000).
    let mut holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    holdings.sort_by_key(|h| h.listing_id);
    assert_eq!(holdings.len(), 2);
    assert_eq!(holdings[0].quantity, dec("80"));
    assert_eq!(holdings[0].total_cost_base, dec("12000"));
    assert_eq!(holdings[1].quantity, dec("100"));
    assert_eq!(holdings[1].total_cost_base, dec("6000"));
}

/// `docs/ato/crypto-wrapping.md` (QC 73649) — "Example: crypto asset reward
/// from DeFi platform" (Craig).
///
/// > Craig 'lends' 100 stablecoin tokens valued at $10 per token through the
/// > DeFi platform Compound Finance. The DeFi platform pays a rate of return
/// > of 1% in the form of newly issued stablecoin tokens. … The income amount
/// > Craig declares is $10. The cost base of the newly issued tokens is their
/// > market value at the time Craig acquires them.
///
/// The reward is **ordinary income** at the tokens' receipt-date market value
/// — an `income_type: "OtherIncome"` row, reported at item 24, in no dividend
/// total — and the tokens themselves are a parcel costed at that same value,
/// entered as an ordinary Buy. The same pair records a staking reward or an
/// established-token airdrop (`docs/ato/crypto-staking-airdrops.md`), whose
/// own examples state no figures. The stablecoin is represented by the seeded
/// ETH token code.
#[tokio::test]
async fn crypto_defi_reward_example_craig_stablecoin_tokens() {
    let pool = test_pool().await;
    put_crypto_listing(&pool, 1, "ETH").await;
    // The 100 tokens Craig already held, at $10 each.
    put_buy(&pool, 1, 1, "2025-08-01", "100", "10", "0").await;
    // The 1% reward: $10 of ordinary income, and one token costed at $10.
    api_put(
        &pool,
        "/income/1",
        json!({
            "listing_id": 1,
            "date_paid": "2025-11-30",
            "unfranked_amount": "10",
            "income_type": "OtherIncome",
            "currency": "AUD",
        }),
    )
    .await;
    put_buy(&pool, 2, 1, "2025-11-30", "1", "10", "0").await;

    // "The income amount Craig declares is $10" — at item 24, and nowhere else.
    let summary: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].tax_year, 2026);
    assert_eq!(summary[0].other_income, dec("10"));
    assert_eq!(summary[0].dividends_assessable, Decimal::ZERO);
    assert_eq!(summary[0].franking_credits, Decimal::ZERO);
    assert_eq!(
        summary[0].gross_assessable_investment_income,
        dec("10"),
        "item 24 is prefilled by nothing, so the reward is counted"
    );

    // "The cost base of the newly issued tokens is their market value at the
    // time Craig acquires them" — a $10 parcel of its own, clock from receipt.
    let parcels: Vec<crate::reports::open_parcels::OpenParcel> =
        api_get(&pool, "/portfolio/open-parcels").await;
    let reward = parcels.iter().find(|p| p.trade_id == 2).unwrap();
    assert_eq!(reward.remaining_quantity, dec("1"));
    assert_eq!(reward.remaining_cost_base, dec("10"));
    assert_eq!(reward.acquisition_date, "2025-11-30".parse().unwrap());
}

/// `docs/ato/crypto-chain-splits.md` (QC 69953) — "Example: chain split and
/// sale of new crypto asset" (Alex).
///
/// > Alex held 10 Bitcoin as an investment on 1 August 2017, when Bitcoin Cash
/// > split from Bitcoin. As a result of the chain split, Alex received
/// > 10 Bitcoin Cash, in addition to the 10 Bitcoin previously held. There
/// > were no immediate tax consequences for him. On 2 March 2026, Alex sells
/// > 2 Bitcoin Cash for $1,260. Because the cost base of the Bitcoin Cash is
/// > zero, he makes a total discount capital gain of $1,260 in the 2025–26
/// > income year … he reports the capital gain after discount of $630.
///
/// A chain split needs no entry path of its own: the new asset is a parcel
/// with a **nil cost base**, dated the split, which is an ordinary Buy at a
/// price of zero. The received asset is represented by the seeded ETH token
/// code (a Crypto listing's ticker must be a recognised digital token, and
/// Bitcoin Cash is not seeded) — which side of the split it is changes
/// nothing about the arithmetic.
#[tokio::test]
async fn crypto_chain_split_example_alex_bitcoin_cash() {
    let pool = test_pool().await;
    put_crypto_listing(&pool, 1, "BTC").await; // the original holding
    put_crypto_listing(&pool, 2, "ETH").await; // the asset received in the split
    // 10 Bitcoin held as an investment before the split.
    put_buy(&pool, 1, 1, "2016-05-04", "10", "700", "0").await;
    // 1 August 2017: 10 units of the new asset, nil cost base, no tax event.
    put_buy(&pool, 2, 2, "2017-08-01", "10", "0", "0").await;

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert!(
        years.is_empty(),
        "receiving the new asset is not a CGT event"
    );

    // 2 March 2026: 2 units sold for $1,260 against a nil cost base.
    put_sell(&pool, 3, 2, "2026-03-02", "2", "630", "0", 2).await;

    let sales: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(sales.len(), 1);
    assert_eq!(sales[0].proceeds, dec("1260"));
    assert_eq!(sales[0].cost_base, Decimal::ZERO, "ATO: cost base is zero");
    assert_eq!(
        sales[0].discount_eligible_gain,
        dec("1260"),
        "ATO: a total discount capital gain of $1,260"
    );

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    assert_eq!(years[0].tax_year, 2026);
    assert_eq!(years[0].cgt_discount, dec("630"));
    assert_eq!(
        years[0].net_capital_gain,
        dec("630"),
        "ATO: the capital gain after discount is $630"
    );
}

/// `docs/ato/crypto-chain-splits.md` (QC 69953) — "Example: no continuing
/// rights or relationships" (Ming).
///
/// > Ming held 10 Bitcoin Cash as an investment just before a chain split on
/// > 15 November 2018. Ming had acquired the Bitcoin Cash on 6 April 2018 with
/// > a cost base of $8,300. … A CGT event C2 happens to Ming's original
/// > Bitcoin Cash when the chain split occurred on 15 November 2018. Ming
/// > calculates a capital loss of $8,300, which is equal to the cost base of
/// > his original asset.
///
/// The original asset the community abandoned is closed by the same CGT event
/// C2 the `WorthlessShares` action records for a deregistered company
/// (`worthless_event: "C2Cancellation"`): its recognise operation closes every
/// open parcel at nil proceeds, so the capital loss is the remaining cost
/// base. The two nil-cost-base successors are Alex's case above.
#[tokio::test]
async fn crypto_chain_split_example_ming_abandoned_original() {
    let pool = test_pool().await;
    put_crypto_listing(&pool, 1, "BTC").await;
    // 10 units acquired 6 April 2018 for $8,300.
    put_buy(&pool, 1, 1, "2018-04-06", "10", "830", "0").await;

    // The split of 15 November 2018 leaves no continuation of the original.
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "listing_id": 1,
            "action_type": "WorthlessShares",
            "date": "2018-11-15",
            "worthless_event": "C2Cancellation",
        }),
    )
    .await;
    let _: Value = api_post(
        &pool,
        "/corporate_actions/1/recognise",
        json!({}),
        StatusCode::CREATED,
    )
    .await;

    let sales: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(sales.len(), 1);
    assert_eq!(sales[0].sale_date, "2018-11-15".parse().unwrap());
    assert_eq!(sales[0].proceeds, Decimal::ZERO);
    assert_eq!(
        sales[0].capital_loss,
        dec("8300"),
        "ATO: a capital loss of $8,300, equal to the cost base"
    );

    // FY2019: a loss carries forward, never discounted.
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years[0].tax_year, 2019);
    assert_eq!(years[0].capital_losses, dec("8300"));
    assert_eq!(years[0].net_capital_gain, Decimal::ZERO);
    assert_eq!(years[0].capital_loss_carried_forward, dec("8300"));
    // Nothing happens after it — and the $8,300 is still reported at label 18V
    // in each of those quiet years, until it is used
    // (`reports::net_capital_gain::net_years`).
    for later in years.iter().filter(|y| y.tax_year > 2019) {
        assert_eq!(later.capital_losses, Decimal::ZERO);
        assert_eq!(later.capital_loss_brought_forward, dec("8300"));
        assert_eq!(later.capital_loss_carried_forward, dec("8300"));
    }
}

/// `docs/ato/crypto-staking-airdrops.md` (QC 69950) — "Example: capital gain
/// and CGT discount on initial airdrop token" (Josh).
///
/// > Josh is an eligible account holder of the Cswap protocol and received an
/// > initial allocation of 800 CX tokens on 16 September 2024. Josh doesn't
/// > derive ordinary income or make a capital gain on receipt of the 800 CX.
/// > On 25 May 2026, Josh sold the 800 CX for $4,000. Because the cost base of
/// > the CX tokens was zero, Josh makes a total capital gain of $4,000 …
/// > [and] is also eligible to reduce his total capital gain using the CGT
/// > discount.
///
/// An **initial-allocation** airdrop is not the ordinary-income case: nothing
/// is assessable on receipt and the tokens carry a nil cost base, so the entry
/// is a Buy at a price of zero dated the allocation — the same shape as a
/// chain split's new asset, and *not* the income row an established-token
/// airdrop needs. CX is represented by the seeded BTC token code.
#[tokio::test]
async fn crypto_initial_airdrop_example_josh_cx_tokens() {
    let pool = test_pool().await;
    put_crypto_listing(&pool, 1, "BTC").await;
    put_buy(&pool, 1, 1, "2024-09-16", "800", "0", "0").await;

    // Nothing is assessable on receipt: no income, no capital gain.
    let summary: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert!(summary.is_empty(), "no ordinary income on receipt");
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert!(years.is_empty(), "no capital gain on receipt");

    // Sold 25 May 2026 for $4,000, held more than 12 months.
    put_sell(&pool, 2, 1, "2026-05-25", "800", "5", "0", 1).await;

    let sales: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(sales.len(), 1);
    assert_eq!(sales[0].cost_base, Decimal::ZERO, "ATO: cost base was zero");
    assert_eq!(
        sales[0].discount_eligible_gain,
        dec("4000"),
        "ATO: a total capital gain of $4,000, discount eligible"
    );

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    assert_eq!(years[0].tax_year, 2026);
    assert_eq!(years[0].cgt_discount, dec("2000"));
    assert_eq!(years[0].net_capital_gain, dec("2000"));
}

/// `docs/ato/employee-share-schemes.md` (QC 47628) — "Example: Taxed-upfront
/// scheme – eligible for reduction" (Matt).
///
/// > Core Bank Ltd provides its employee Matt 600 shares under an ESS on
/// > 4 August 2015. The total market value of the shares is $3,600. Matt pays
/// > Core Bank Ltd $1,200 to purchase the shares, acquiring the shares for a
/// > discount of $2,400 ($3,600 less $1,200), reported at label D "Discount from
/// > taxed upfront schemes – eligible for reduction".
///
/// As an eligible taxpayer (adjusted taxable income ≤ $180,000) Matt reduces the
/// discount by the $1,000 concession: the assessable discount is $1,400. His
/// shares' CGT cost base is the $3,600 market value, acquired 4 August 2015 —
/// the cost-base-reset Buy the vesting operation creates.
#[tokio::test]
async fn ess_example_matt_taxed_upfront_eligible_reduction() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "CBL").await; // Core Bank Ltd

    api_put(
        &pool,
        "/ess_statements/1",
        json!({
            "listing_id": 1,
            "taxing_point_date": "2015-08-04",
            "quantity": "600",
            "market_value_per_share": "6", // $3,600 / 600
            "taxed_upfront_eligible": "2400", // label D
            "currency": "AUD",
        }),
    )
    .await;

    // The vesting operation creates the cost-base-reset Buy: 600 shares at the
    // $6 market value, acquired (and settled) on the taxing-point date.
    let vest: Trade = api_post(
        &pool,
        "/ess_statements/1/vest",
        json!({}),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(vest.quantity, dec("600"));
    assert_eq!(vest.average_price, dec("6"));
    assert_eq!(vest.date, "2015-08-04".parse().unwrap());
    assert_eq!(
        vest.deemed_acquisition_date, None,
        "the taxing point is the acquisition date"
    );

    // The assessable ESS discount: $2,400 − $1,000 reduction = $1,400, in
    // FY2016 (acquired Aug 2015), reported separately from dividend income.
    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1);
    let y = &years[0];
    assert_eq!(y.tax_year, 2016);
    assert_eq!(
        y.ess_taxed_upfront_reduction,
        dec("1000"),
        "the $1,000 concession"
    );
    assert_eq!(y.ess_discount_assessable, dec("1400"), "$2,400 − $1,000");
    assert_eq!(y.dividends_assessable, Decimal::ZERO);

    // The CGT cost base is reset to the $3,600 market value (600 × $6).
    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert_eq!(holdings.len(), 1);
    assert_eq!(holdings[0].quantity, dec("600"));
    assert_eq!(holdings[0].total_cost_base, dec("3600"));
}

/// `docs/ato/ess-30-day-rule.md` (QC 23058) — "Example 11: Shares acquired
/// under a tax-deferred scheme and sold within 30 days of the deferred taxing
/// point" (Wyatt).
///
/// > On 23 June 2019 … the deferred taxing point is 23 June 2019. The market
/// > value of the shares on 23 June 2019 is $1,400 … On 20 July 2019, Wyatt
/// > sells the 400 shares … for a total of $1,518. As the sale is within 30
/// > days of the deferred taxing point, the taxing point now becomes
/// > 20 July 2019 … The market value of the shares on 20 July 2019 is the
/// > amount Wyatt received from selling the shares, $1,518 … Therefore, the
/// > discount on the shares is $1,518 … Wyatt must include his discount on his
/// > 2020 tax return, not his 2019 tax return.
///
/// The 30-day rule is not a calculation this system performs — it decides
/// *which statement is the operative one*, which the employer settles by
/// issuing an amended statement (here: the 2019 one withdrawn, a 2020 one
/// issued). What is entered is that amended statement: taxing point
/// 20 July 2019 at the $3.795 per-share sale price. Vesting it resets the cost
/// base to the same $1,518 the sale realises, so the discount lands in FY2020
/// at label F **and there is no separate capital gain** — exactly the ATO's
/// outcome. Entering the superseded 23 June statement instead would book the
/// discount in FY2019 and invent a $118 capital gain (SCENARIOS J-04).
#[tokio::test]
async fn ess_30_day_rule_example_11_wyatt_amended_statement() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "PPL").await; // Pepper Pines Ltd

    // The amended ESS statement: the taxing point moved to the disposal date,
    // the discount re-measured at what Wyatt received ($1,518 / 400 shares).
    api_put(
        &pool,
        "/ess_statements/1",
        json!({
            "listing_id": 1,
            "taxing_point_date": "2019-07-20",
            "quantity": "400",
            "market_value_per_share": "3.795",
            "deferral_discount": "1518", // label F
            "currency": "AUD",
        }),
    )
    .await;
    let vest: Trade = api_post(
        &pool,
        "/ess_statements/1/vest",
        json!({}),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(vest.date, "2019-07-20".parse().unwrap());
    assert_eq!(vest.quantity, dec("400"));

    // The sale, the same day, for the $1,518 that fixed the discount.
    put_sell(&pool, 50, 1, "2019-07-20", "400", "3.795", "0", vest.id).await;

    // "$1,518 at F item 12 … he also writes $1,518 at B item 12" — in FY2020
    // (a July date is the next financial year), and in no other year.
    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1);
    assert_eq!(years[0].tax_year, 2020);
    assert_eq!(years[0].ess_discount_assessable, dec("1518"));
    assert_eq!(
        years[0].ess_taxed_upfront_reduction,
        Decimal::ZERO,
        "the $1,000 reduction is for taxed-upfront eligible schemes only"
    );

    // No separate capital gain: the cost base reset to the same market value
    // the sale realised.
    let sales: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(sales.len(), 1);
    assert_eq!(sales[0].proceeds, dec("1518.000"));
    assert_eq!(sales[0].cost_base, dec("1518.000"));
    assert_eq!(sales[0].capital_gain_loss, Decimal::ZERO);
    assert_eq!(sales[0].capital_loss, Decimal::ZERO);
}

/// `docs/ato/worthless-shares.md` (QC 52234) — "Capital loss when company
/// dissolves" (Dave).
///
/// > On 31 March 2026, the administrators of Company Ltd made a written
/// > declaration that they had reasonable grounds to believe there was no
/// > likelihood that shareholders would receive any distribution. Dave owned
/// > 1,000 Company Ltd shares, acquired in March 2013 for $1.70 each including
/// > brokerage … the reduced cost base of Dave's shares and his capital loss …
/// > is $1,700 — that is, 1,000 multiplied by $1.70.
///
/// Entered as a `WorthlessShares` corporate action (a G3 declaration) whose
/// recognise operation closes the holding at nil proceeds. The realised-gains
/// report shows a $1,700 capital loss (no gain, no discount), and the
/// net-capital-gain report carries it into FY2025/26's loss pool.
#[tokio::test]
async fn worthless_shares_example_dave_capital_loss_on_dissolution() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "CMP").await;
    // 1,000 shares acquired March 2013 for $1.70 each, including brokerage.
    put_buy(&pool, 1, 1, "2013-03-15", "1000", "1.70", "0").await;

    // The administrators' written declaration of worthlessness (CGT event G3).
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "WorthlessShares",
            "listing_id": 1,
            "date": "2026-03-31",
            "worthless_event": "G3Declaration",
        }),
    )
    .await;

    // Recognise the loss: the closing Sell consumes the whole holding at nil
    // proceeds.
    let recognise: Value = api_post(
        &pool,
        "/corporate_actions/1/recognise",
        json!({}),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(recognise["sell"]["quantity"], "1000");

    // The capital loss equals the reduced cost base: 1,000 × $1.70 = $1,700,
    // recognised (not disregarded), and never discounted.
    let sales: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(sales.len(), 1);
    assert_eq!(sales[0].proceeds, Decimal::ZERO);
    assert_eq!(sales[0].cost_base, dec("1700"));
    assert_eq!(sales[0].capital_loss, dec("1700"));
    assert_eq!(sales[0].discount_eligible_gain, Decimal::ZERO);

    // The loss is taken into account for FY2025/26 (year ending 30 June 2026),
    // carried forward as there are no gains to offset.
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    let y = years.iter().find(|y| y.tax_year == 2026).unwrap();
    assert_eq!(y.capital_losses, dec("1700"));
    assert_eq!(y.net_capital_gain, Decimal::ZERO);
    assert_eq!(y.capital_loss_carried_forward, dec("1700"));

    // The holding is fully closed.
    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert!(holdings.iter().all(|h| h.quantity == Decimal::ZERO));
}

/// `docs/ato/inherited-assets-cost-base.md` (QC 66053) — "Example: transfer of
/// an asset from executor (LPR) to beneficiary" (Maria/Antonio), together with
/// the s 115-30 discount-clock rule in
/// `docs/ato/inherited-assets-cgt-discount.md` (QC 69713).
///
/// > Maria died on 13 October 2024 … \[the executor\] transferred the land to
/// > Maria's beneficiary, Antonio, and paid the conveyancing fee of $5,000
/// > upon payment of all debts and tax. … The first element of Antonio's cost
/// > base is Maria's cost base on the date of her death. Antonio can include
/// > the $5,000 the executor spent on the conveyancing in his cost base.
///
/// The example states no figure for Maria's own cost base (and no acquisition
/// date beyond the asset being post-CGT in her hands — her cost base carries
/// over), so the test supplies stand-ins: a $300,000 cost base acquired
/// 1 May 2010. The asserted structure is the ATO's: the parcel's cost base is
/// the deceased's cost base at death **plus the $5,000 LPR conveyancing**,
/// and (s 115-30) the discount clock runs from Maria's acquisition, not the
/// death or transfer. The land is entered as 1 unit, per the property
/// convention above.
#[tokio::test]
async fn inherited_assets_example_maria_antonio_lpr_expenditure() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "LAND").await;

    api_put(
        &pool,
        "/inheritances/1",
        json!({
            "listing_id": 1,
            "quantity": "1",
            "date_of_death": "2024-10-13",
            "cost_base_rule": "DeceasedCostBase",
            "cost_base": "300000",
            "lpr_expenditure": "5000",
            // "upon payment of all debts and tax" — after the death.
            "lpr_expenditure_date": "2025-02-01",
            "deceased_acquisition_date": "2010-05-01",
        }),
    )
    .await;

    // Antonio's parcel: Maria's $300,000 cost base at death + the $5,000
    // conveyancing, acquired (for the 12-month discount clock) when Maria
    // acquired it.
    let parcels: Vec<crate::reports::open_parcels::OpenParcel> =
        api_get(&pool, "/portfolio/open-parcels").await;
    assert_eq!(parcels.len(), 1);
    assert_eq!(parcels[0].original_cost_base, dec("305000"));
    assert_eq!(parcels[0].remaining_quantity, dec("1"));
    assert_eq!(
        parcels[0].acquisition_date,
        chrono::NaiveDate::from_ymd_opt(2010, 5, 1).unwrap()
    );
}

/// `docs/ato/cgt-event-timing.md` (QC 66016) — "Example: contract of sale" (Sue).
///
/// > In June 2024, Sue entered into a contract to sell land she owned. The
/// > contract settled in October 2024. Sue made the capital gain in the
/// > 2023–24 income year (the year she entered into the contract), not the
/// > 2024–25 income year (the year settlement took place).
///
/// A Sell's `date` is the contract date and `settlement_date` is recorded
/// separately, so the FY-keyed reports must bucket the gain by the contract
/// date alone. The example states no prices; stand-ins give a $500 gain
/// (held > 12 months, so it is also discount-eligible — incidental to the
/// timing rule under test). The land is entered as 1 unit, per the property
/// convention above.
#[tokio::test]
async fn cgt_event_timing_example_sue_contract_date_not_settlement() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "LND").await;
    put_buy(&pool, 1, 1, "2022-05-01", "1", "1000", "0").await;

    // Contracted June 2024, settled October 2024.
    api_put(
        &pool,
        "/sells/2",
        json!({
            "date": "2024-06-14",
            "settlement_date": "2024-10-15",
            "listing_id": 1,
            "average_price": "1500",
            "quantity": "1",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "allocations": [
                { "purchase_trade_id": 1, "quantity_allocated": "1" }
            ],
        }),
    )
    .await;

    // The gain belongs to FY2023–24 (the contract year) — and no FY2024–25
    // row exists at all, the settlement date having contributed nothing.
    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1, "one FY only — the contract year");
    assert_eq!(years[0].tax_year, 2024, "FY ending 30 June 2024");
    assert_eq!(years[0].discount_eligible_gains, dec("500"));
}

/// `docs/ato/forex-common-transactions.md` (QC 18322) — "Example: scenario 2"
/// (Lisa), the CGT side.
///
/// > Lisa acquires shares in a US company as a capital investment for a cost
/// > of US$15,000 on 1 July 2004 when the exchange rate is A$1.00 = US$0.50.
/// > The cost base of the shares to Lisa is A$30,000 … On 1 March 2005 Lisa
/// > enters into a contract to sell the shares for US$20,000 when the
/// > exchange rate is A$1.00 = US$0.60. The capital proceeds for the disposal
/// > of the shares on that date is equivalent to A$33,333 … Lisa makes a gain
/// > of A$3,333 on the disposal of the shares ($33,333 − $30,000).
///
/// Cost base and proceeds each translate at their own trade date's rate
/// (s 960-50(6) item 5), never as a US$ gain converted once. The conversion
/// here uses the monthly ATO/RBA rate for each trade month, seeded to the
/// example's rates, and the trades' own `fx_rate` is left at 1 to prove the
/// fallback is not what converts. The ATO's figures are whole-dollar
/// rounded; the system keeps the exact decimals (US$20,000 / 0.60 =
/// A$33,333.33…). Lisa's separate $1,075 FRE 2 forex realisation loss on the
/// settlement window is the forex measures' side, not modelled (see the
/// module header). Held 1 July 2004 → 1 March 2005 (8 months): no discount.
#[tokio::test]
async fn forex_example_lisa_usd_share_cost_base_and_proceeds() {
    let pool = test_pool().await;

    // The example's exchange rates, as the ATO/RBA monthly reference rate
    // (USD per 1 AUD) for each trade's month.
    sqlx::query(
        "INSERT INTO rba_fx_rates (currency, month, rate) VALUES
            ('USD', '2004-07', '0.50'), ('USD', '2005-03', '0.60')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // A US company: a USD listing on the seeded XNYS exchange.
    api_put(
        &pool,
        "/listings/1",
        json!({
            "exchange_mic": "XNYS",
            "ticker": "USC",
            "name": "US Company",
            "isin": null,
            "security_type": "Share",
            "currency": "USD",
            "amit": false,
        }),
    )
    .await;

    // US$15,000 on 1 July 2004: 1,000 shares at US$15.
    api_put(
        &pool,
        "/trades/1",
        json!({
            "trade_type": "Buy",
            "date": "2004-07-01",
            "settlement_date": "2004-07-06",
            "listing_id": 1,
            "average_price": "15",
            "quantity": "1000",
            "currency": "USD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "USD",
            "fx_rate": "1",
        }),
    )
    .await;

    // Sold for US$20,000 by the 1 March 2005 contract; settled 15 March 2005
    // (the example's settlement date — it must not affect the CGT figures).
    api_put(
        &pool,
        "/sells/2",
        json!({
            "date": "2005-03-01",
            "settlement_date": "2005-03-15",
            "listing_id": 1,
            "average_price": "20",
            "quantity": "1000",
            "currency": "USD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "USD",
            "fx_rate": "1",
            "allocations": [
                { "purchase_trade_id": 1, "quantity_allocated": "1000" }
            ],
        }),
    )
    .await;

    let gains: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(gains.len(), 1);
    let g = &gains[0];
    assert_eq!(g.cost_base, dec("30000"), "US$15,000 / 0.50");
    assert_eq!(
        g.proceeds.round_dp(2),
        dec("33333.33"),
        "US$20,000 / 0.60 — the ATO states A$33,333"
    );
    assert_eq!(
        g.capital_gain_loss.round_dp(2),
        dec("3333.33"),
        "the ATO states A$3,333"
    );
    assert_eq!(g.discount_eligible_gain, Decimal::ZERO, "held 8 months");
    assert_eq!(g.capital_loss, Decimal::ZERO);
}

/// `docs/ato/forex-common-transactions.md` (QC 18322) Lisa again — entered
/// with per-trade **spot-rate overrides** instead of seeded monthly rates.
///
/// `docs/ato/forex-average-rates.md` (QC 18020, Examples 5 and 7) permits an
/// average rate only where it reasonably approximates the spot rates at the
/// translation times, and says it is **not** appropriate for a one-off
/// purchase or sale of a large capital asset — the transaction-date spot
/// rate should be used. Here the imported monthly averages deliberately
/// differ from the example's day rates (0.55 and 0.65 vs Lisa's 0.50 and
/// 0.60): each trade's `spot_fx_rate` must win, reproducing the ATO's
/// figures exactly as the monthly-rate variant above does when the months
/// happen to match.
#[tokio::test]
async fn forex_example_lisa_via_spot_rate_overrides() {
    let pool = test_pool().await;

    // Monthly averages that do NOT match the example's day rates: if the
    // monthly rate converted, cost base and proceeds would both be wrong.
    sqlx::query(
        "INSERT INTO rba_fx_rates (currency, month, rate) VALUES
            ('USD', '2004-07', '0.55'), ('USD', '2005-03', '0.65')",
    )
    .execute(&pool)
    .await
    .unwrap();

    api_put(
        &pool,
        "/listings/1",
        json!({
            "exchange_mic": "XNYS",
            "ticker": "USC",
            "name": "US Company",
            "isin": null,
            "security_type": "Share",
            "currency": "USD",
            "amit": false,
        }),
    )
    .await;

    // US$15,000 on 1 July 2004 at the day's rate A$1.00 = US$0.50.
    api_put(
        &pool,
        "/trades/1",
        json!({
            "trade_type": "Buy",
            "date": "2004-07-01",
            "settlement_date": "2004-07-06",
            "listing_id": 1,
            "average_price": "15",
            "quantity": "1000",
            "currency": "USD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "USD",
            "fx_rate": "1",
            "spot_fx_rate": "0.50",
        }),
    )
    .await;

    // US$20,000 by the 1 March 2005 contract at the day's rate US$0.60.
    api_put(
        &pool,
        "/sells/2",
        json!({
            "date": "2005-03-01",
            "settlement_date": "2005-03-15",
            "listing_id": 1,
            "average_price": "20",
            "quantity": "1000",
            "currency": "USD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "USD",
            "fx_rate": "1",
            "spot_fx_rate": "0.60",
            "allocations": [
                { "purchase_trade_id": 1, "quantity_allocated": "1000" }
            ],
        }),
    )
    .await;

    let gains: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(gains.len(), 1);
    let g = &gains[0];
    assert_eq!(g.cost_base, dec("30000"), "US$15,000 / 0.50, not / 0.55");
    assert_eq!(
        g.proceeds.round_dp(2),
        dec("33333.33"),
        "US$20,000 / 0.60, not / 0.65 — the ATO states A$33,333"
    );
    assert_eq!(
        g.capital_gain_loss.round_dp(2),
        dec("3333.33"),
        "the ATO states A$3,333"
    );
}

/// `docs/ato/ess-30-day-rule.md` (QC 23058) — "Example 11" (Wyatt), the
/// 30-day rule.
///
/// > On 20 July 2019, Wyatt sells the 400 shares he acquired under the
/// > tax-deferred scheme, for a total of $1,518. … As the sale is within
/// > 30 days of the deferred taxing point \[23 June 2019\], the taxing point
/// > now becomes 20 July 2019 in accordance with the 30-day rule. … the
/// > discount on the shares is $1,518. Due to the 30-day rule, Wyatt must
/// > include his discount on his 2020 tax return, not his 2019 tax return.
///
/// The 30-day rule is entered as the employer's *amended* ESS statement —
/// taxing point 20 July 2019, market value the $1,518 sale total — never the
/// superseded original (23 June 2019, $1,400), which would book the discount
/// in the wrong FY and a spurious $118 capital gain. Vesting the amended
/// statement resets the cost base to $1,518 at the sale date, so the
/// same-day sale realises exactly nil capital gain alongside the FY2020
/// discount.
#[tokio::test]
async fn ess_30_day_rule_example_wyatt_taxing_point_moves_to_the_sale() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "PPL").await; // Pepper Pines Ltd

    // The amended statement: 400 shares, taxing point 20 July 2019, market
    // value $1,518 total ($3.795 per share), all deferral-scheme discount
    // (label F — no $1,000 taxed-upfront reduction applies).
    api_put(
        &pool,
        "/ess_statements/1",
        json!({
            "listing_id": 1,
            "taxing_point_date": "2019-07-20",
            "quantity": "400",
            "market_value_per_share": "3.795",
            "deferral_discount": "1518",
            "currency": "AUD",
        }),
    )
    .await;

    // Vest at the moved taxing point: the cost-base-reset Buy.
    let vest: Trade = api_post(
        &pool,
        "/ess_statements/1/vest",
        json!({}),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(vest.date, "2019-07-20".parse().unwrap());
    assert_eq!(vest.average_price, dec("3.795"));

    // The on-market sale the same day, for the same $1,518.
    put_sell(&pool, 100, 1, "2019-07-20", "400", "3.795", "0", vest.id).await;

    // The discount lands in FY2019–20 (Wyatt's 2020 return), label F, with
    // no taxed-upfront reduction.
    let years: Vec<TaxYearSummary> = api_get(&pool, "/portfolio/tax-summary").await;
    assert_eq!(years.len(), 1, "no FY2019 income remains once amended");
    assert_eq!(years[0].tax_year, 2020);
    assert_eq!(years[0].ess_discount_assessable, dec("1518"));
    assert_eq!(years[0].ess_taxed_upfront_reduction, Decimal::ZERO);

    // The CGT side: cost base reset to the sale-date market value, so the
    // sale realises exactly nil gain and nil loss.
    let gains: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(gains.len(), 1);
    assert_eq!(gains[0].capital_gain_loss, Decimal::ZERO);
    assert_eq!(gains[0].capital_loss, Decimal::ZERO);
}

/// `docs/ato/personal-investors-guide-managed-fund-distributions.md` —
/// Example 26 (Bob, OZ Investments Fund).
///
/// > The fund gave him a statement showing his distribution included the
/// > following capital gains: $100 calculated using the discount method
/// > (grossed-up amount $200), $75 calculated using the indexation method,
/// > $28 calculated using the 'other' method. … Bob writes the following at
/// > question 18 in his supplementary tax return: $303 at label H, $203 at
/// > label A.
///
/// The statement's discount line is the already-halved figure, grossed up ×2
/// into 18H; with no losses the discount halves it straight back, so 18A is
/// the $203 the statement components sum to. Bob's $105 tax-deferred amount
/// (the fund is not an AMIT) is a CGT event E4 cost-base reduction, entered
/// as a ReturnOfCapital action: cost base $1,200 → $1,095. (The reduced-cost-
/// base side, $1,050 → $945, is not modelled — reduced cost base equals cost
/// base under the elements-1–2 Known limitation.)
#[tokio::test]
async fn pig_managed_funds_example_26_bob_fund_gains_and_tax_deferred() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "OZI").await;
    // Bob's unit holding, entered as one unit carrying the $1,200 cost base.
    put_buy(&pool, 1, 1, "2023-10-01", "1", "1200", "0").await;
    // The fund's statement for 2024–25.
    api_put(
        &pool,
        "/amma_statements/1",
        json!({
            "listing_id": 1,
            "tax_year_end_date": "2025-06-30",
            "date_received": "2025-05-31",
            "units_held": "1",
            "cgt_discount_gains": "100",
            "cgt_indexation_gains": "75",
            "cgt_other_gains": "28",
        }),
    )
    .await;
    // The $105 tax-deferred (non-assessable) amount — CGT event E4, entered
    // as a return of capital of $105 per unit on the distribution date.
    api_put(
        &pool,
        "/corporate_actions/1",
        json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2025-05-31",
            "amount_per_unit": "105",
            "currency": "AUD",
        }),
    )
    .await;

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    assert_eq!(years[0].tax_year, 2025);
    // 18H = grossed-up discount gain + indexation + 'other' = 200 + 103 = 303.
    assert_eq!(years[0].discount_eligible_gains, dec("200"));
    assert_eq!(years[0].other_gains, dec("103"));
    assert_eq!(
        years[0].discount_eligible_gains + years[0].other_gains,
        dec("303"),
        "label 18H: total current year capital gains"
    );
    // No losses: the 50% discount takes the $200 back to $100 → 18A = $203.
    assert_eq!(years[0].capital_losses, Decimal::ZERO);
    assert_eq!(years[0].cgt_discount, dec("100"));
    assert_eq!(
        years[0].net_capital_gain,
        dec("203"),
        "label 18A: net capital gain"
    );

    // The tax-deferred amount is not income or a gain — it reduces the cost
    // base of Bob's units: $1,200 − $105 = $1,095.
    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert_eq!(holdings.len(), 1);
    assert_eq!(holdings[0].total_cost_base, dec("1095"));
}

/// `docs/ato/personal-investors-guide-managed-fund-distributions.md` —
/// Example 27 (Ilena, XYZ Managed Fund): a capital loss of the investor's own
/// against fund-distributed gains.
///
/// > Her distribution included: $65 discounted capital gain, $50 capital gain
/// > calculated using the 'other' method, $40 capital gain calculated using
/// > the indexation method. … Ilena has no other capital gain but made a
/// > capital loss of $100 when she sold some shares during the income year.
/// > … Ilena writes the following at question 18 in her supplementary tax
/// > return: $220 at label H, $60 at label A.
///
/// The worksheet: gross up $65 × 2 = $130; 18H = 130 + 50 + 40 = $220; her
/// own $100 loss goes against the indexation + 'other' gains first ($90),
/// the remaining $10 against the grossed-up discount gain ($130 → $120);
/// 50% discount → $60 at 18A. Only Ilena's own loss enters the netting — the
/// fund's trust-level netting is already inside the statement figures.
#[tokio::test]
async fn pig_managed_funds_example_27_ilena_own_loss_against_fund_gains() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "XYZ").await;
    put_listing(&pool, 2, "SHR").await; // the shares she sells at a loss
    // The fund's statement for 2024–25.
    api_put(
        &pool,
        "/amma_statements/1",
        json!({
            "listing_id": 1,
            "tax_year_end_date": "2025-06-30",
            "date_received": "2025-04-30",
            "units_held": "1",
            "cgt_discount_gains": "65",
            "cgt_other_gains": "50",
            "cgt_indexation_gains": "40",
        }),
    )
    .await;
    // Ilena's own $100 capital loss on some shares sold during the year.
    put_buy(&pool, 1, 2, "2024-08-01", "100", "6", "0").await;
    put_sell(&pool, 2, 2, "2025-03-01", "100", "5", "0", 1).await;

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    assert_eq!(years.len(), 1);
    assert_eq!(years[0].tax_year, 2025);
    // 18H = grossed-up discount gain (65 × 2) + 'other' + indexation = 220.
    assert_eq!(years[0].discount_eligible_gains, dec("130"));
    assert_eq!(years[0].other_gains, dec("90"));
    assert_eq!(
        years[0].discount_eligible_gains + years[0].other_gains,
        dec("220"),
        "label 18H: total current year capital gains"
    );
    // Her own loss: $90 against the non-discountable gains, $10 against the
    // grossed-up discount gain, then the 50% discount → $60 at 18A.
    assert_eq!(years[0].capital_losses, dec("100"));
    assert_eq!(years[0].net_other_gain, Decimal::ZERO);
    assert_eq!(years[0].net_discount_eligible_gain, dec("120"));
    assert_eq!(years[0].cgt_discount, dec("60"));
    assert_eq!(
        years[0].net_capital_gain,
        dec("60"),
        "label 18A: net capital gain"
    );
    assert_eq!(years[0].capital_loss_carried_forward, Decimal::ZERO);
}

/// `docs/ato/personal-investors-guide-managed-fund-distributions.md` —
/// Example 28 (Miriam, Exponential Growth Fund): the AMIT cost base net
/// amount, in both directions.
///
/// > Her units have a cost base of $55 each. The fund attributes $13 of
/// > assessable income per unit … but only pays a cash dividend amount of $3
/// > per unit … resulting in a shortfall AMIT cost base net amount of $10 per
/// > unit [which] is used to increase the cost base … to $65. Alternatively
/// > … the excess of $10 reduces the tax cost base of her units … to $45.
///
/// The AMMA per-unit `cost_base_adjustment` is signed: positive reduces the
/// cost base (excess), negative increases it (shortfall).
#[tokio::test]
async fn pig_managed_funds_example_28_miriam_amit_cost_base_net_amount() {
    let pool = test_pool().await;
    // Two funds carry the example's two alternatives, one unit each at $55.
    put_listing(&pool, 1, "EGF").await;
    put_listing(&pool, 2, "EGF2").await;
    put_buy(&pool, 1, 1, "2024-07-15", "1", "55", "0").await;
    put_buy(&pool, 2, 2, "2024-07-15", "1", "55", "0").await;
    // Shortfall: attribution exceeds cash → the net amount increases the cost
    // base (a negative adjustment).
    api_put(
        &pool,
        "/amma_statements/1",
        json!({
            "listing_id": 1,
            "tax_year_end_date": "2025-06-30",
            "date_received": "2025-07-31",
            "units_held": "1",
            "cost_base_adjustment": "-10",
        }),
    )
    .await;
    api_put(
        &pool,
        "/amit_adjustments/1",
        json!({ "amma_statement_id": 1, "trade_id": 1, "quantity": "1" }),
    )
    .await;
    // Excess: cash exceeds attribution → the net amount reduces the cost base.
    api_put(
        &pool,
        "/amma_statements/2",
        json!({
            "listing_id": 2,
            "tax_year_end_date": "2025-06-30",
            "date_received": "2025-07-31",
            "units_held": "1",
            "cost_base_adjustment": "10",
        }),
    )
    .await;
    api_put(
        &pool,
        "/amit_adjustments/2",
        json!({ "amma_statement_id": 2, "trade_id": 2, "quantity": "1" }),
    )
    .await;

    let holdings: Vec<HoldingOverview> =
        api_post(&pool, "/portfolio/overview", json!({}), StatusCode::OK).await;
    assert_eq!(holdings.len(), 2);
    assert_eq!(
        holdings[0].total_cost_base,
        dec("65"),
        "shortfall: $55 + $10 increase"
    );
    assert_eq!(
        holdings[1].total_cost_base,
        dec("45"),
        "excess: $55 − $10 reduction"
    );
}

/// `docs/ato/capital-gains-question-18.md` (QC 106842) — "Example 1: sale of
/// shares and collectables" through "Example 5: question 18 – label A"
/// (Kathleen), the end-to-end net-capital-gain method.
///
/// > Capital gain on the sale of 1,000 shares for $6 each on 17 December 2025.
/// > Kathleen bought these shares on 17 November 2000 and each has a cost base
/// > of $3 … Capital gain = $6,000 − $3,000 = $3,000 … using the discount
/// > method.
/// > Capital gain on the sale of 130 shares for $8 each on 27 February 2026 …
/// > bought … on 10 October 2025 and each has a cost base of $4 … As the asset
/// > was bought and sold within 12 months, Kathleen must use the 'other'
/// > method … (130 × $8) − (130 × $4) = $520.
/// > … total current year capital gains of $3,520 ($3,000 + $520) … at label H
/// > Capital loss on the sale of 600 shares for $3 each on 25 June 2026 …
/// > reduced cost base of $4 … $2,400 − $1,800 = $600.
/// > … deduct the first $520 of her capital loss from the capital gain
/// > calculated using the 'other' method and … the remaining $80 from the
/// > capital gain calculated using the discount method … totals $2,920.
/// > … unapplied net capital losses from earlier years of $400 … $2,920 −
/// > $400 = $2,520 … $2,520 × 50% = $1,260 … she writes $1,260 at question 18
/// > – label A Net capital gain.
///
/// This is *the* canonical loss-netting order: current-year losses against the
/// non-discountable ('other') gains first, then the earlier-year losses, and
/// only then the 50% discount. The three share legs are entered as three
/// listings — the ATO gives the 130-share and 600-share parcels the same
/// 10 October 2025 acquisition date without saying whether they are the same
/// company, and separate listings keep the parcels unambiguous without
/// changing any figure.
///
/// **Kathleen's jewellery leg is deliberately not entered.** A collectable's
/// capital loss is quarantined — it can only ever reduce a capital gain from
/// another collectable — and this system has one loss pool and no asset-class
/// dimension, so entering the $500 jewellery loss as an ordinary listing would
/// wrongly offset the share gains (a Known limitation in `docs/API.md`). The
/// ATO's label V is therefore her $500 collectables carry-forward, while this
/// test asserts nil: every share-side loss is used up in the same year.
#[tokio::test]
async fn tax_return_18_kathleen_loss_order_then_discount() {
    let pool = test_pool().await;
    // $400 of unapplied net capital losses from earlier income years.
    api_put(
        &pool,
        "/cgt_settings/1",
        json!({ "opening_capital_loss": "400" }),
    )
    .await;

    // Discount-method gain: bought 17 Nov 2000 at $3, sold 17 Dec 2025 at $6.
    put_listing(&pool, 1, "KTHA").await;
    put_buy(&pool, 1, 1, "2000-11-17", "1000", "3", "0").await;
    put_sell(&pool, 10, 1, "2025-12-17", "1000", "6", "0", 1).await;

    // 'Other'-method gain: bought and sold inside 12 months.
    put_listing(&pool, 2, "KTHB").await;
    put_buy(&pool, 2, 2, "2025-10-10", "130", "4", "0").await;
    put_sell(&pool, 11, 2, "2026-02-27", "130", "8", "0", 2).await;

    // The current-year capital loss.
    put_listing(&pool, 3, "KTHC").await;
    put_buy(&pool, 3, 3, "2025-10-10", "600", "4", "0").await;
    put_sell(&pool, 12, 3, "2026-06-25", "600", "3", "0", 3).await;

    let gains: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    let by_sale = |id: i64| gains.iter().find(|g| g.sale_trade_id == id).unwrap();
    assert_eq!(
        by_sale(10).capital_gain_loss,
        dec("3000"),
        "$6,000 − $3,000"
    );
    assert_eq!(
        by_sale(10).discount_eligible_gain,
        dec("3000"),
        "held since 2000 — the discount method"
    );
    assert_eq!(
        by_sale(11).capital_gain_loss,
        dec("520"),
        "(130 × $8) − (130 × $4)"
    );
    assert_eq!(
        by_sale(11).non_discountable_gain,
        dec("520"),
        "bought and sold within 12 months — the 'other' method"
    );
    assert_eq!(by_sale(12).capital_loss, dec("600"), "$2,400 − $1,800");

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    let y = years.iter().find(|y| y.tax_year == 2026).unwrap();
    // Label 18H: total current year capital gains, before losses and discount.
    assert_eq!(y.discount_eligible_gains, dec("3000"));
    assert_eq!(y.other_gains, dec("520"));
    assert_eq!(
        y.discount_eligible_gains + y.other_gains,
        dec("3520"),
        "label 18H: $3,000 + $520"
    );
    // Current-year loss applied 'other'-first: $520 → $0, the remaining $80
    // off the discount-method gain ($3,000 → $2,920).
    assert_eq!(y.capital_losses, dec("600"));
    assert_eq!(
        y.net_other_gain,
        Decimal::ZERO,
        "the $520 'other' gain is fully offset"
    );
    // Then the $400 brought forward: $2,920 − $400 = $2,520.
    assert_eq!(y.capital_loss_brought_forward, dec("400"));
    assert_eq!(
        y.net_discount_eligible_gain,
        dec("2520"),
        "$2,920 − $400, before the discount"
    );
    // Only now the 50% discount.
    assert_eq!(y.cgt_discount, dec("1260"), "$2,520 × 50%");
    assert_eq!(
        y.net_capital_gain,
        dec("1260"),
        "label 18A: net capital gain of $1,260"
    );
    assert_eq!(
        y.capital_loss_carried_forward,
        Decimal::ZERO,
        "every share-side loss is used this year (the ATO's $500 at label V is \
         her quarantined collectables loss, which this system does not model)"
    );
}

/// `docs/ato/personal-investors-guide-managed-fund-distributions.md` —
/// Examples 21–25 (Tim), the C1 step order end to end.
///
/// > *Example 21*: Tim receives a discounted capital gain of $400 → grosses up
/// > to $800 ($400 × 2).
/// > *Example 22*: Tim's fund also distributes a $100 'other'-method gain →
/// > 18H is $900 ($800 + $100).
/// > *Example 23*: Tim has a $200 capital loss selling another CGT asset →
/// > $900 − $200 = $700, applied against the 'other' gain first, leaving the
/// > whole $700 discountable.
/// > *Example 24*: $700 × 50% = $350.
///
/// The same machinery as Bob/Ilena/Miriam below, on a different branch: no
/// indexation component, and a loss that exactly consumes the 'other' gain
/// before spilling into the grossed-up discount gain. The fund distribution is
/// an AMMA statement; the $200 loss is Tim's own disposal of an unrelated
/// asset. The ATO doesn't date the facts — entered in FY2024–25, the guide's
/// own income year.
#[tokio::test]
async fn pig_managed_funds_examples_21_25_tim_gross_up_loss_then_discount() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "TIMF").await;
    put_buy(&pool, 1, 1, "2023-08-01", "1", "1000", "0").await;
    // The fund's statement: a $400 discounted gain and a $100 'other' gain.
    api_put(
        &pool,
        "/amma_statements/1",
        json!({
            "listing_id": 1,
            "tax_year_end_date": "2025-06-30",
            "date_received": "2025-05-31",
            "units_held": "1",
            "cgt_discount_gains": "400",
            "cgt_other_gains": "100",
        }),
    )
    .await;
    // Tim's own $200 capital loss on another CGT asset, same income year.
    put_listing(&pool, 2, "TIMX").await;
    put_buy(&pool, 2, 2, "2024-09-02", "100", "10", "0").await;
    put_sell(&pool, 10, 2, "2025-05-20", "100", "8", "0", 2).await;

    let years: Vec<NetCapitalGainYear> = api_get(&pool, "/portfolio/net-capital-gain").await;
    let y = years.iter().find(|y| y.tax_year == 2025).unwrap();
    // Example 21: the distributed discount gain grosses up ×2.
    assert_eq!(y.discount_eligible_gains, dec("800"), "$400 × 2");
    assert_eq!(y.other_gains, dec("100"));
    // Example 22: label 18H.
    assert_eq!(
        y.discount_eligible_gains + y.other_gains,
        dec("900"),
        "label 18H: $800 + $100"
    );
    // Example 23: the $200 loss takes the 'other' gain first, then $100 off
    // the grossed-up discount gain — leaving the whole $700 discountable.
    assert_eq!(y.capital_losses, dec("200"));
    assert_eq!(y.net_other_gain, Decimal::ZERO);
    assert_eq!(y.net_discount_eligible_gain, dec("700"), "$900 − $200");
    // Example 24 / 25: the discount, and label 18A.
    assert_eq!(y.cgt_discount, dec("350"), "$700 × 50%");
    assert_eq!(
        y.net_capital_gain,
        dec("350"),
        "label 18A: net capital gain of $350"
    );
}

/// `docs/ato/takeovers-and-scrip-for-scrip.md` (QC 64895) — "Example 26:
/// Takeover" (Desiree), a takeover **without** rollover.
///
/// > In October 2000, Desiree bought 500 shares in DEF Ltd. These shares are
/// > currently worth $2 each. Their cost base is $1.50.
/// > XYZ Ltd offers to acquire each share in DEF Ltd for one share in XYZ Ltd
/// > and 75 cents cash. The shares in XYZ Ltd are valued at $1.25 each.
/// > Accepting the offer, Desiree receives 500 shares in XYZ Ltd and $375 cash.
/// > The capital proceeds received for each share in DEF Ltd is $2 ($1.25
/// > market value of each XYZ Ltd share plus 75 cents cash). Therefore, as the
/// > cost base of each DEF Ltd share is $1.50, Desiree will make a capital
/// > gain of 50 cents ($2 − $1.50) on each share, a total of $250.
/// > The cost base of the newly acquired XYZ Ltd shares is the market value of
/// > the shares in DEF Ltd ($2) less the cash amount received ($0.75) which
/// > equals $1.25 each or a total of $625 (500 × $1.25).
///
/// No rollover is chosen (or available), so this is an ordinary disposal at
/// the market value of the consideration — entered manually as a Sell at the
/// $2 market-value-derived price plus a Buy of the new XYZ holding at $1.25,
/// exactly as the crypto-swap example above is entered. The ATO doesn't date
/// the takeover ("currently worth $2"); entered on 15 March 2002, comfortably
/// more than 12 months after the October 2000 purchase, so the gain is
/// discount-eligible.
#[tokio::test]
async fn takeovers_example_26_desiree_takeover_without_rollover() {
    let pool = test_pool().await;
    put_listing(&pool, 1, "DEF").await;
    put_listing(&pool, 2, "XYZ").await;
    // 500 DEF shares bought October 2000 with a $1.50 cost base each.
    put_buy(&pool, 1, 1, "2000-10-15", "500", "1.50", "0").await;
    // The takeover: disposal at $2 per share (a $1.25 XYZ share + 75c cash)…
    put_sell(&pool, 10, 1, "2002-03-15", "500", "2", "0", 1).await;
    // …and the new XYZ parcel at its $1.25 market value.
    put_buy(&pool, 2, 2, "2002-03-15", "500", "1.25", "0").await;

    let gains: Vec<RealisedGainLoss> = api_get(&pool, "/portfolio/realised-gains").await;
    assert_eq!(gains.len(), 1);
    assert_eq!(gains[0].proceeds, dec("1000"), "500 × $2 capital proceeds");
    assert_eq!(gains[0].cost_base, dec("750"), "500 × $1.50");
    assert_eq!(
        gains[0].capital_gain_loss,
        dec("250"),
        "50 cents per share on 500 shares"
    );
    assert_eq!(
        gains[0].discount_eligible_gain,
        dec("250"),
        "held well over 12 months"
    );

    // The replacement holding: 500 XYZ shares at $1.25 = $625, acquired at the
    // takeover (no rollover, so no carried acquisition date).
    let parcels: Vec<crate::reports::open_parcels::OpenParcel> =
        api_get(&pool, "/portfolio/open-parcels").await;
    assert_eq!(parcels.len(), 1);
    assert_eq!(parcels[0].ticker, "XYZ");
    assert_eq!(parcels[0].remaining_quantity, dec("500"));
    assert_eq!(
        parcels[0].remaining_cost_base,
        dec("625"),
        "$1.25 each or a total of $625"
    );
    assert_eq!(parcels[0].acquisition_date.to_string(), "2002-03-15");
}
