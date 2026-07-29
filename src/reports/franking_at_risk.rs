//! Franking at-risk foresight: *why* franking credits disappear, and what a
//! contemplated sale would put at risk — before it happens.
//!
//! The tax summary applies the 45-day (90 for preference shares) at-risk
//! holding-period rule silently: credits just come out lower. This report
//! explains each denial — the dividend, its qualification window, and the
//! entitled/disqualified units from the same walk
//! (`franking::HoldingWalks::test`) the tax summary denies by — and a
//! what-if mode injects a contemplated Sell into that walk so disposals can
//! be timed to keep the credits (selling after a dividend's window end can
//! no longer disqualify it). See `docs/ato/you-and-your-shares-dividends.md`.

use crate::infra::fx::FxRates;
use crate::infra::http::ApiError;
use crate::reports::franking;
use axum::{Json, Router, extract::State, routing::get, routing::post};
use chrono::{Duration, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// A dividend whose entitled shares fail the holding-period walk — its
/// credits are denied, unless the year's small-shareholder exemption shields
/// them (`status` says which; `credits_denied` is what the tax summary
/// actually excludes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrankingAtRiskAlert {
    /// The `income` row id.
    pub income_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    /// Financial year the credits count for (calendar year of its 30 June end).
    pub tax_year: i32,
    pub date_paid: NaiveDate,
    /// Ex-dividend date (the payment date when no ex-date was recorded).
    pub ex_date: NaiveDate,
    /// 45, or 90 for preference shares — the at-risk days required.
    pub required_days: i64,
    /// The qualification window runs from `ex_date` to here. A disposal
    /// inside it with fewer than `required_days` at-risk days (acquisition
    /// and disposal day excluded) disqualifies; one after it cannot.
    pub window_end: NaiveDate,
    pub entitled_units: Decimal,
    /// Entitled units disposed of (LIFO identification) short of the
    /// required at-risk days.
    pub disqualified_units: Decimal,
    /// Attached franking credits, AUD.
    pub credits_attached: Decimal,
    /// The disqualified share of the credits per the walk, AUD.
    pub credits_at_risk: Decimal,
    /// What the tax summary actually denies: `credits_at_risk`, or zero when
    /// the small-shareholder exemption applies.
    pub credits_denied: Decimal,
    /// `denied`, or `exempt_small_shareholder` (the year's attached credits
    /// are under A$5,000, so the rule doesn't apply).
    pub status: String,
}

/// A contemplated sale to test against every franked dividend of the listing:
/// would selling `units` on `sale_date` disqualify credits?
#[derive(Debug, Deserialize)]
pub struct WhatIfRequest {
    pub listing_id: i64,
    pub sale_date: NaiveDate,
    pub units: Decimal,
}

/// A dividend whose credits the contemplated sale would (further) disqualify.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrankingWhatIfAlert {
    pub income_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub tax_year: i32,
    pub ex_date: NaiveDate,
    pub required_days: i64,
    /// Selling after this date can no longer disqualify this dividend — the
    /// lever for timing the disposal.
    pub window_end: NaiveDate,
    pub credits_attached: Decimal,
    pub disqualified_units_now: Decimal,
    pub disqualified_units_after_sale: Decimal,
    /// The additional credits the sale would put at risk (AUD): the walk's
    /// denial with the sale minus the denial without it.
    pub additional_credits_at_risk: Decimal,
    /// `denied` (the year's attached credits reach the A$5,000 threshold) or
    /// `exempt_small_shareholder` (currently shielded — but the exemption is
    /// year-wide and can lapse as more credits arrive).
    pub status: String,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/reports/franking_at_risk", get(report))
        .route("/reports/franking_at_risk/what-if", post(what_if))
}

/// Tickers for the alerts to carry (the required at-risk days come from the
/// loaded walks themselves).
async fn listing_tickers(
    conn: &mut sqlx::SqliteConnection,
) -> Result<HashMap<i64, String>, sqlx::Error> {
    let rows = sqlx::query("SELECT id, ticker FROM listings")
        .fetch_all(conn)
        .await?;
    rows.iter()
        .map(|r| Ok((r.try_get("id")?, r.try_get("ticker")?)))
        .collect()
}

/// Every franked dividend whose entitled shares fail the holding-period walk,
/// with the failing window and the denied amount. The candidates and per-year
/// attached totals come from the loader shared with the tax summary
/// (`franking::db_franked_dividends`), and the denial arithmetic is the same
/// `HoldingPeriodTest::denied` — so a row here with status `denied` is
/// exactly what the tax summary's `franking_credits_denied` carries. The walk
/// inputs load on the same read transaction as the dividends, and every
/// dividend's walk reuses that one load.
pub async fn db_franking_at_risk(
    pool: &SqlitePool,
) -> Result<Vec<FrankingAtRiskAlert>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let fx = FxRates::load(&mut *tx).await?;
    let (dividends, attached_by_year) = franking::db_franked_dividends(&mut tx, &fx).await?;
    let tickers = listing_tickers(&mut tx).await?;
    let walks = franking::HoldingWalks::load(&mut tx).await?;
    tx.commit().await?;

    let mut alerts = Vec::new();
    for div in &dividends {
        let test = walks.test(div.listing_id, div.ex_date);
        if test.disqualified_units.is_zero() {
            continue;
        }
        let ticker = tickers.get(&div.listing_id).cloned().unwrap_or_default();
        let days = walks.required_days(div.listing_id);
        let at_risk = test.denied(div.credits_aud);
        let exempt = attached_by_year[&div.tax_year] < franking::small_shareholder_threshold_aud();
        alerts.push(FrankingAtRiskAlert {
            income_id: div.income_id,
            listing_id: div.listing_id,
            ticker,
            tax_year: div.tax_year,
            date_paid: div.date_paid,
            ex_date: div.ex_date,
            required_days: days,
            window_end: div.ex_date + Duration::days(days),
            entitled_units: test.entitled_units,
            disqualified_units: test.disqualified_units,
            credits_attached: div.credits_aud,
            credits_at_risk: at_risk,
            credits_denied: if exempt { Decimal::ZERO } else { at_risk },
            status: if exempt {
                "exempt_small_shareholder"
            } else {
                "denied"
            }
            .to_string(),
        });
    }
    alerts.sort_by_key(|a| (a.ex_date, a.income_id));
    Ok(alerts)
}

/// Run every franked dividend of the listing through the holding-period walk
/// with the contemplated Sell injected, and report each one whose denial
/// would grow — the credits the sale would cost, and the window end to wait
/// out instead.
pub async fn db_franking_what_if(
    pool: &SqlitePool,
    req: &WhatIfRequest,
) -> Result<Vec<FrankingWhatIfAlert>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let fx = FxRates::load(&mut *tx).await?;
    let (dividends, attached_by_year) = franking::db_franked_dividends(&mut tx, &fx).await?;
    let tickers = listing_tickers(&mut tx).await?;
    let walks = franking::HoldingWalks::load(&mut tx).await?;
    tx.commit().await?;

    let mut alerts = Vec::new();
    for div in dividends.iter().filter(|d| d.listing_id == req.listing_id) {
        let base = walks.test(div.listing_id, div.ex_date);
        let with_sale = walks.test_with_sale(
            div.listing_id,
            div.ex_date,
            Some((req.sale_date, req.units)),
        );
        let additional = with_sale.denied(div.credits_aud) - base.denied(div.credits_aud);
        if additional <= Decimal::ZERO {
            continue;
        }
        let ticker = tickers.get(&div.listing_id).cloned().unwrap_or_default();
        let days = walks.required_days(div.listing_id);
        let exempt = attached_by_year[&div.tax_year] < franking::small_shareholder_threshold_aud();
        alerts.push(FrankingWhatIfAlert {
            income_id: div.income_id,
            listing_id: div.listing_id,
            ticker,
            tax_year: div.tax_year,
            ex_date: div.ex_date,
            required_days: days,
            window_end: div.ex_date + Duration::days(days),
            credits_attached: div.credits_aud,
            disqualified_units_now: base.disqualified_units,
            disqualified_units_after_sale: with_sale.disqualified_units,
            additional_credits_at_risk: additional,
            status: if exempt {
                "exempt_small_shareholder"
            } else {
                "denied"
            }
            .to_string(),
        });
    }
    alerts.sort_by_key(|a| (a.ex_date, a.income_id));
    Ok(alerts)
}

async fn report(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<FrankingAtRiskAlert>>, ApiError> {
    db_franking_at_risk(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn what_if(
    State(pool): State<SqlitePool>,
    Json(req): Json<WhatIfRequest>,
) -> Result<Json<Vec<FrankingWhatIfAlert>>, ApiError> {
    if req.units <= Decimal::ZERO {
        return Err(ApiError::Unprocessable(
            "units must be positive".to_string(),
        ));
    }
    db_franking_what_if(&pool, &req)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reports::tax_summary;
    use crate::test_support::{self, ApiClient, test_pool, ymd};
    use axum::http::StatusCode;

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .insert(pool)
            .await;
    }

    async fn insert_buy(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate, qty: i64) {
        test_support::buy(id, listing_id)
            .date(date)
            .qty(Decimal::from(qty))
            .price(Decimal::ONE)
            .insert(pool)
            .await;
    }

    async fn insert_sell(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate, qty: i64) {
        test_support::sell(id, listing_id)
            .date(date)
            .qty(Decimal::from(qty))
            .price(Decimal::ONE)
            .insert(pool)
            .await;
    }

    async fn insert_dividend(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        paid: NaiveDate,
        ex: NaiveDate,
        credits: i64,
    ) {
        test_support::income(id, listing_id, paid)
            .with(|i| {
                i.ex_date = Some(ex);
                i.franking_credits = Decimal::from(credits);
            })
            .insert(pool)
            .await;
    }

    /// Matthew's shape (docs/ato/you-and-your-shares-dividends.md, Example 6):
    /// the whole holding sold 39 at-risk days after acquisition, inside the
    /// qualification window — every credit denied, with the failing window
    /// surfaced.
    #[tokio::test]
    async fn db_denied_dividend_lists_failing_window_and_amounts() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "TLS").await;
        insert_buy(&pool, 1, 1, ymd(2025, 3, 1), 1000).await;
        insert_sell(&pool, 2, 1, ymd(2025, 4, 10), 1000).await;
        insert_dividend(&pool, 1, 1, ymd(2025, 3, 28), ymd(2025, 3, 14), 5600).await;

        let alerts = db_franking_at_risk(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.income_id, 1);
        assert_eq!(a.ticker, "TLS");
        assert_eq!(a.tax_year, 2025);
        assert_eq!(a.ex_date, ymd(2025, 3, 14));
        assert_eq!(a.required_days, 45);
        assert_eq!(a.window_end, ymd(2025, 4, 28));
        assert_eq!(a.entitled_units, Decimal::from(1000));
        assert_eq!(a.disqualified_units, Decimal::from(1000));
        assert_eq!(a.credits_attached, Decimal::from(5600));
        assert_eq!(a.credits_at_risk, Decimal::from(5600));
        assert_eq!(a.credits_denied, Decimal::from(5600));
        assert_eq!(a.status, "denied");
    }

    /// A dividend whose shares pass the walk produces no row — an empty
    /// report means every credit is safe.
    #[tokio::test]
    async fn db_qualified_dividend_is_not_listed() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "TLS").await;
        insert_buy(&pool, 1, 1, ymd(2024, 3, 1), 1000).await;
        insert_dividend(&pool, 1, 1, ymd(2025, 3, 28), ymd(2025, 3, 14), 5600).await;
        assert!(db_franking_at_risk(&pool).await.unwrap().is_empty());
    }

    /// Under A$5,000 of attached credits in the year the small-shareholder
    /// exemption shields the failing walk: the row still explains the failure
    /// but shows nothing denied — matching the tax summary.
    #[tokio::test]
    async fn db_small_shareholder_exemption_flagged_but_not_denied() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "TLS").await;
        insert_buy(&pool, 1, 1, ymd(2025, 3, 1), 1000).await;
        insert_sell(&pool, 2, 1, ymd(2025, 4, 10), 1000).await;
        insert_dividend(&pool, 1, 1, ymd(2025, 3, 28), ymd(2025, 3, 14), 4990).await;

        let alerts = db_franking_at_risk(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.status, "exempt_small_shareholder");
        assert_eq!(a.credits_at_risk, Decimal::from(4990));
        assert_eq!(a.credits_denied, Decimal::ZERO);

        let summary = tax_summary::db_tax_summary(&pool).await.unwrap();
        let y2025 = summary.iter().find(|s| s.tax_year == 2025).unwrap();
        assert_eq!(y2025.franking_credits_denied, Decimal::ZERO);
    }

    /// The report's denied amounts are the tax summary's denial, per year —
    /// Jessica's partial-LIFO shape (Example 7): 4,000 of 14,000 entitled
    /// units disqualified, so 2,000 of the 7,000 credits are denied by both.
    #[tokio::test]
    async fn db_denied_amounts_match_the_tax_summary() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "JES").await;
        insert_buy(&pool, 1, 1, ymd(2024, 3, 14), 10000).await;
        insert_buy(&pool, 2, 1, ymd(2025, 3, 4), 4000).await;
        insert_sell(&pool, 3, 1, ymd(2025, 4, 3), 4000).await;
        insert_dividend(&pool, 1, 1, ymd(2025, 3, 28), ymd(2025, 3, 14), 7000).await;

        let alerts = db_franking_at_risk(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].disqualified_units, Decimal::from(4000));
        assert_eq!(alerts[0].credits_denied, Decimal::from(2000));
        assert_eq!(alerts[0].status, "denied");

        let summary = tax_summary::db_tax_summary(&pool).await.unwrap();
        let y2025 = summary.iter().find(|s| s.tax_year == 2025).unwrap();
        assert_eq!(y2025.franking_credits_denied, alerts[0].credits_denied);
        assert_eq!(y2025.franking_credits, Decimal::from(5000));
    }

    /// What-if: Jessica's shape *before* the sale — holding 10,000 old +
    /// 4,000 recent units, contemplating selling 4,000 twenty days after the
    /// ex-date. The walk with the hypothetical Sell shows the 2,000 credits
    /// it would cost and the window end to wait for; nothing is written.
    #[tokio::test]
    async fn db_what_if_shows_credits_a_contemplated_sale_would_cost() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "JES").await;
        insert_buy(&pool, 1, 1, ymd(2024, 3, 14), 10000).await;
        insert_buy(&pool, 2, 1, ymd(2025, 3, 4), 4000).await;
        insert_dividend(&pool, 1, 1, ymd(2025, 3, 28), ymd(2025, 3, 14), 7000).await;

        let req = WhatIfRequest {
            listing_id: 1,
            sale_date: ymd(2025, 4, 3),
            units: Decimal::from(4000),
        };
        let alerts = db_franking_what_if(&pool, &req).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.income_id, 1);
        assert_eq!(a.disqualified_units_now, Decimal::ZERO);
        assert_eq!(a.disqualified_units_after_sale, Decimal::from(4000));
        assert_eq!(a.additional_credits_at_risk, Decimal::from(2000));
        assert_eq!(a.window_end, ymd(2025, 4, 28));
        assert_eq!(a.status, "denied");

        // Nothing was written: the real walk still shows no denial.
        assert!(db_franking_at_risk(&pool).await.unwrap().is_empty());
    }

    /// Selling after the qualification window ends cannot disqualify the
    /// dividend — the timing lever the report exists to surface.
    #[tokio::test]
    async fn db_what_if_sale_after_window_end_is_safe() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "JES").await;
        insert_buy(&pool, 1, 1, ymd(2024, 3, 14), 10000).await;
        insert_buy(&pool, 2, 1, ymd(2025, 3, 4), 4000).await;
        insert_dividend(&pool, 1, 1, ymd(2025, 3, 28), ymd(2025, 3, 14), 7000).await;

        let req = WhatIfRequest {
            listing_id: 1,
            // The day after window_end (2025-04-28).
            sale_date: ymd(2025, 4, 29),
            units: Decimal::from(4000),
        };
        assert!(db_franking_what_if(&pool, &req).await.unwrap().is_empty());
    }

    /// The what-if only tests the requested listing's dividends.
    #[tokio::test]
    async fn db_what_if_ignores_other_listings() {
        let pool = test_pool().await;
        for (listing_id, ticker) in [(1, "AAA"), (2, "BBB")] {
            insert_listing(&pool, listing_id, ticker).await;
            let base = (listing_id - 1) * 10;
            insert_buy(&pool, base + 1, listing_id, ymd(2025, 3, 4), 4000).await;
            insert_dividend(
                &pool,
                listing_id,
                listing_id,
                ymd(2025, 3, 28),
                ymd(2025, 3, 14),
                7000,
            )
            .await;
        }
        let req = WhatIfRequest {
            listing_id: 1,
            sale_date: ymd(2025, 4, 3),
            units: Decimal::from(4000),
        };
        let alerts = db_franking_what_if(&pool, &req).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].listing_id, 1);
    }

    /// Preference shares carry the 90-day requirement into the row.
    #[tokio::test]
    async fn db_preference_listing_reports_90_day_window() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("PRF")
            .preference(true)
            .insert(&pool)
            .await;
        insert_buy(&pool, 1, 1, ymd(2025, 3, 4), 100).await;
        insert_sell(&pool, 2, 1, ymd(2025, 5, 13), 100).await; // 69 at-risk days
        insert_dividend(&pool, 1, 1, ymd(2025, 3, 28), ymd(2025, 3, 14), 6000).await;

        let alerts = db_franking_at_risk(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].required_days, 90);
        assert_eq!(alerts[0].window_end, ymd(2025, 3, 14) + Duration::days(90));
    }

    #[tokio::test]
    async fn api_get_franking_at_risk() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "TLS").await;
        insert_buy(&pool, 1, 1, ymd(2025, 3, 1), 1000).await;
        insert_sell(&pool, 2, 1, ymd(2025, 4, 10), 1000).await;
        insert_dividend(&pool, 1, 1, ymd(2025, 3, 28), ymd(2025, 3, 14), 5600).await;

        let resp = ApiClient::over(router().with_state(pool))
            .get("/reports/franking_at_risk")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let alerts: Vec<FrankingAtRiskAlert> = resp.json();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].status, "denied");
    }

    #[tokio::test]
    async fn api_post_what_if_and_validation() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "JES").await;
        insert_buy(&pool, 1, 1, ymd(2025, 3, 4), 4000).await;
        insert_dividend(&pool, 1, 1, ymd(2025, 3, 28), ymd(2025, 3, 14), 7000).await;

        let post = |body: String| {
            let client = ApiClient::over(router().with_state(pool.clone()));
            async move {
                client
                    .post_raw("/reports/franking_at_risk/what-if", body.as_ref())
                    .await
            }
        };

        let resp =
            post(r#"{"listing_id": 1, "sale_date": "2025-04-03", "units": "4000"}"#.to_string())
                .await;
        assert_eq!(resp.status, StatusCode::OK);
        let alerts: Vec<FrankingWhatIfAlert> = resp.json();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].additional_credits_at_risk, Decimal::from(7000));

        // Non-positive units rejected.
        let resp =
            post(r#"{"listing_id": 1, "sale_date": "2025-04-03", "units": "0"}"#.to_string()).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);

        // A missing required field is a deserialization failure (422).
        let resp = post(r#"{"listing_id": 1}"#.to_string()).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}
