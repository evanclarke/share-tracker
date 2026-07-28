//! Health / data-freshness report: the one read the web UI's cross-view
//! banner polls. Surfaces, in a single read transaction:
//!
//! - the latest stored ok closing-price date and whether it is stale
//!   (older than [`PRICE_STALE_BUSINESS_DAYS`] business days — a coarse
//!   Mon–Fri count, deliberately ignoring per-exchange holiday calendars:
//!   this is a freshness alarm across every exchange and crypto, not a
//!   settlement calculation);
//! - the latest imported RBA FX rate month and whether it is stale (the RBA
//!   publishes month M's F11 rates shortly after M ends, so anything older
//!   than the previous calendar month means the weekly import has stopped
//!   landing new months);
//! - every job whose most recent recorded run failed;
//! - every listing with at least one errored closing-price row (a wrong,
//!   renamed, or delisted provider symbol otherwise only shows up
//!   indirectly, as a missing snapshot from the errored date onward —
//!   `reports::valuation` refuses to value a date with an errored price).
//!
//! A database with no prices or FX rates at all reports `stale = false` for
//! that series: nothing has decayed — a fresh install shows no banner, and a
//! price/FX import that breaks before ever succeeding surfaces through
//! `failed_jobs` (and the Jobs page) instead.

use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::{Datelike, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// Prices are stale once the latest ok closing price is more than this many
/// business days (Mon–Fri) old. The price-import job runs every weekday, so a
/// healthy database is at most 1–2 business days behind; 3 leaves headroom for
/// a long exchange-holiday weekend without a false alarm.
pub const PRICE_STALE_BUSINESS_DAYS: i64 = 3;

/// A job whose most recent recorded run failed.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct FailedJob {
    pub name: String,
    pub finished_at: String,
    pub error: Option<String>,
}

/// A listing with one or more errored closing-price rows: a stuck symbol
/// (wrong, renamed, or delisted) would otherwise only show up indirectly, as
/// a missing snapshot from the errored date onward (`reports::valuation`
/// refuses to value a date with an errored price). Re-fetch it via
/// `POST /closing_prices/backfill` (or `/fetch` for a single date) once the
/// underlying symbol issue — see `latest_error` — is fixed (e.g. set
/// `listings.price_symbol`).
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ErroredPriceListing {
    pub listing_id: i64,
    pub ticker: String,
    /// Count of errored rows for this listing (any date, not just recent).
    pub errored_days: i64,
    pub latest_errored_date: NaiveDate,
    pub latest_error: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthReport {
    /// Latest `closing_prices` date stored with status ok, across every
    /// listing; `None` when no price has ever been stored.
    pub latest_price_date: Option<NaiveDate>,
    pub prices_stale: bool,
    /// Latest `rba_fx_rates` month (`YYYY-MM`); `None` when none imported yet.
    pub latest_fx_month: Option<String>,
    pub fx_stale: bool,
    pub failed_jobs: Vec<FailedJob>,
    /// Listings with at least one errored closing-price row, newest error
    /// first. Empty when every stored price is ok.
    pub errored_prices: Vec<ErroredPriceListing>,
}

/// Business days (Mon–Fri) strictly after `from`, up to and including `today`.
/// Zero when `today <= from`.
fn business_days_since(from: NaiveDate, today: NaiveDate) -> i64 {
    let mut days = 0;
    let mut d = from;
    while d < today {
        d = d.succ_opt().expect("date within chrono range");
        if !matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
            days += 1;
        }
    }
    days
}

/// The calendar month before `today`'s, as the `YYYY-MM` key `rba_fx_rates`
/// uses. `YYYY-MM` strings compare correctly lexicographically.
fn previous_month(today: NaiveDate) -> String {
    let (year, month) = if today.month() == 1 {
        (today.year() - 1, 12)
    } else {
        (today.year(), today.month() - 1)
    };
    format!("{year:04}-{month:02}")
}

/// Read the three freshness facts on one snapshot. `today` is a parameter so
/// tests can pin the staleness thresholds to fixed dates.
pub async fn db_health(pool: &SqlitePool, today: NaiveDate) -> Result<HealthReport, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let latest_price_date: Option<NaiveDate> =
        sqlx::query_scalar("SELECT MAX(price_date) FROM closing_prices WHERE status = 'ok'")
            .fetch_one(&mut *tx)
            .await?;
    let latest_fx_month: Option<String> = sqlx::query_scalar("SELECT MAX(month) FROM rba_fx_rates")
        .fetch_one(&mut *tx)
        .await?;
    let failed_jobs = sqlx::query_as::<_, FailedJob>(
        "SELECT name, finished_at, error FROM job_runs r \
         WHERE id = (SELECT MAX(id) FROM job_runs WHERE name = r.name) AND success = 0 \
         ORDER BY name",
    )
    .fetch_all(&mut *tx)
    .await?;
    let errored_prices = sqlx::query_as::<_, ErroredPriceListing>(
        "SELECT cp.listing_id AS listing_id, l.ticker AS ticker, \
                COUNT(*) AS errored_days, MAX(cp.price_date) AS latest_errored_date, \
                (SELECT cp2.error FROM closing_prices cp2 \
                 WHERE cp2.listing_id = cp.listing_id AND cp2.status = 'error' \
                 ORDER BY cp2.price_date DESC LIMIT 1) AS latest_error \
         FROM closing_prices cp JOIN listings l ON l.id = cp.listing_id \
         WHERE cp.status = 'error' \
         GROUP BY cp.listing_id \
         ORDER BY latest_errored_date DESC",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let prices_stale = latest_price_date
        .is_some_and(|d| business_days_since(d, today) > PRICE_STALE_BUSINESS_DAYS);
    let fx_stale = latest_fx_month
        .as_deref()
        .is_some_and(|m| m < previous_month(today).as_str());
    Ok(HealthReport {
        latest_price_date,
        prices_stale,
        latest_fx_month,
        fx_stale,
        failed_jobs,
        errored_prices,
    })
}

async fn report(State(pool): State<SqlitePool>) -> Result<Json<HealthReport>, ApiError> {
    let today = chrono::Local::now().date_naive();
    db_health(&pool, today)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/health", get(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, test_pool, ymd};
    use axum::http::StatusCode;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn insert_ok_price(pool: &SqlitePool, listing_id: i64, date: &str) {
        test_support::closing_price(listing_id, date.parse().unwrap())
            .price("10.50")
            .source("yahoo")
            .fetched_at("2026-07-01T00:00:00Z")
            .insert(pool)
            .await;
    }

    async fn insert_error_price(pool: &SqlitePool, listing_id: i64, date: &str, error: &str) {
        test_support::closing_price(listing_id, date.parse().unwrap())
            .source("yahoo")
            .fetched_at("2026-07-01T00:00:00Z")
            .errored(error)
            .insert(pool)
            .await;
    }

    async fn insert_fx_month(pool: &SqlitePool, month: &str) {
        sqlx::query("INSERT INTO rba_fx_rates (currency, month, rate) VALUES ('USD', ?, '0.66')")
            .bind(month)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn insert_job_run(pool: &SqlitePool, name: &str, finished_at: &str, error: Option<&str>) {
        sqlx::query(
            "INSERT INTO job_runs (name, started_at, finished_at, success, error) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(finished_at)
        .bind(finished_at)
        .bind(error.is_none())
        .bind(error)
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn business_days_skip_weekends() {
        // Fri 2026-07-10 → Mon 2026-07-13 is one business day…
        assert_eq!(business_days_since(ymd(2026, 7, 10), ymd(2026, 7, 13)), 1);
        // …Mon → Fri of the same week is four…
        assert_eq!(business_days_since(ymd(2026, 7, 6), ymd(2026, 7, 10)), 4);
        // …and the same day (or a future `from`) is zero.
        assert_eq!(business_days_since(ymd(2026, 7, 13), ymd(2026, 7, 13)), 0);
        assert_eq!(business_days_since(ymd(2026, 7, 14), ymd(2026, 7, 13)), 0);
    }

    #[test]
    fn previous_month_wraps_the_year() {
        assert_eq!(previous_month(ymd(2026, 7, 13)), "2026-06");
        assert_eq!(previous_month(ymd(2026, 1, 5)), "2025-12");
    }

    #[tokio::test]
    async fn empty_database_reports_nothing_stale() {
        // A fresh install has nothing to have gone stale: no banner. A price
        // import that breaks before ever succeeding shows via failed_jobs.
        let pool = test_pool().await;
        let h = db_health(&pool, ymd(2026, 7, 13)).await.unwrap();
        assert_eq!(h.latest_price_date, None);
        assert!(!h.prices_stale);
        assert_eq!(h.latest_fx_month, None);
        assert!(!h.fx_stale);
        assert!(h.failed_jobs.is_empty());
        assert!(h.errored_prices.is_empty());
    }

    #[tokio::test]
    async fn prices_within_threshold_are_fresh_older_are_stale() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        // Wed 2026-07-08 → Mon 2026-07-13 is exactly 3 business days: fresh.
        insert_ok_price(&pool, 1, "2026-07-08").await;
        let h = db_health(&pool, ymd(2026, 7, 13)).await.unwrap();
        assert_eq!(h.latest_price_date, Some(ymd(2026, 7, 8)));
        assert!(!h.prices_stale);

        // One business day further out (Tue 2026-07-14) crosses the threshold.
        let h = db_health(&pool, ymd(2026, 7, 14)).await.unwrap();
        assert!(h.prices_stale);
    }

    #[tokio::test]
    async fn only_ok_prices_count_towards_freshness() {
        // An errored fetch stores a row but no usable price — a run of errored
        // days must not make the data look fresh.
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        insert_ok_price(&pool, 1, "2026-07-01").await;
        insert_error_price(&pool, 1, "2026-07-10", "provider down").await;
        let h = db_health(&pool, ymd(2026, 7, 13)).await.unwrap();
        assert_eq!(h.latest_price_date, Some(ymd(2026, 7, 1)));
        assert!(h.prices_stale);
    }

    /// A listing with errored closing-price rows is surfaced by ticker (not
    /// raw id), with the count and the most recent error message — the
    /// surface that stops a stuck symbol (renamed/delisted) from only
    /// showing up indirectly as a missing snapshot.
    #[tokio::test]
    async fn errored_price_listing_is_surfaced_with_ticker_count_and_latest_error() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAR").insert(&pool).await;
        insert_error_price(
            &pool,
            1,
            "2026-07-01",
            "provider returned no candles for LAR",
        )
        .await;
        insert_error_price(
            &pool,
            1,
            "2026-07-02",
            "provider returned no candles for LAR",
        )
        .await;
        insert_error_price(
            &pool,
            1,
            "2026-07-03",
            "provider returned no candles for LAR (latest)",
        )
        .await;

        let h = db_health(&pool, ymd(2026, 7, 13)).await.unwrap();
        assert_eq!(h.errored_prices.len(), 1);
        let row = &h.errored_prices[0];
        assert_eq!(row.listing_id, 1);
        assert_eq!(row.ticker, "LAR");
        assert_eq!(row.errored_days, 3);
        assert_eq!(row.latest_errored_date, ymd(2026, 7, 3));
        assert_eq!(
            row.latest_error,
            "provider returned no candles for LAR (latest)"
        );
    }

    /// Multiple affected listings are each their own row, newest error first
    /// — and an ok price for the same listing doesn't hide its errors.
    #[tokio::test]
    async fn errored_prices_are_grouped_per_listing_and_ordered_newest_first() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("A").insert(&pool).await;
        test_support::listing(2).ticker("B").insert(&pool).await;
        insert_ok_price(&pool, 1, "2026-07-01").await;
        insert_error_price(&pool, 1, "2026-07-05", "err A").await;
        insert_error_price(&pool, 2, "2026-07-10", "err B").await;

        let h = db_health(&pool, ymd(2026, 7, 13)).await.unwrap();
        assert_eq!(h.errored_prices.len(), 2);
        assert_eq!(h.errored_prices[0].ticker, "B"); // newest error first
        assert_eq!(h.errored_prices[1].ticker, "A");
    }

    #[tokio::test]
    async fn fx_fresh_with_previous_month_stale_when_older() {
        let pool = test_pool().await;
        // June is the month before July 2026: fresh.
        insert_fx_month(&pool, "2026-06").await;
        let h = db_health(&pool, ymd(2026, 7, 13)).await.unwrap();
        assert_eq!(h.latest_fx_month.as_deref(), Some("2026-06"));
        assert!(!h.fx_stale);

        // Come September with nothing newer imported, June is stale.
        let h = db_health(&pool, ymd(2026, 9, 1)).await.unwrap();
        assert!(h.fx_stale);
    }

    #[tokio::test]
    async fn fx_current_month_is_fresh() {
        let pool = test_pool().await;
        insert_fx_month(&pool, "2026-07").await;
        let h = db_health(&pool, ymd(2026, 7, 13)).await.unwrap();
        assert!(!h.fx_stale);
    }

    #[tokio::test]
    async fn job_whose_latest_run_failed_is_surfaced() {
        let pool = test_pool().await;
        insert_job_run(
            &pool,
            "price-import",
            "2026-07-12T07:00:00Z",
            Some("yahoo 403"),
        )
        .await;
        let h = db_health(&pool, ymd(2026, 7, 13)).await.unwrap();
        assert_eq!(h.failed_jobs.len(), 1);
        assert_eq!(h.failed_jobs[0].name, "price-import");
        assert_eq!(h.failed_jobs[0].error.as_deref(), Some("yahoo 403"));
    }

    #[tokio::test]
    async fn job_that_recovered_is_not_surfaced() {
        // Only the *latest* run per job counts: a failure followed by a
        // success is recovered, not failing.
        let pool = test_pool().await;
        insert_job_run(
            &pool,
            "price-import",
            "2026-07-12T07:00:00Z",
            Some("yahoo 403"),
        )
        .await;
        insert_job_run(&pool, "price-import", "2026-07-13T07:00:00Z", None).await;
        // And the reverse — success then failure — is failing.
        insert_job_run(&pool, "backup", "2026-07-12T00:00:00Z", None).await;
        insert_job_run(&pool, "backup", "2026-07-13T00:00:00Z", Some("disk full")).await;

        let h = db_health(&pool, ymd(2026, 7, 13)).await.unwrap();
        assert_eq!(h.failed_jobs.len(), 1);
        assert_eq!(h.failed_jobs[0].name, "backup");
    }

    #[tokio::test]
    async fn api_get_health() {
        let pool = test_pool().await;
        insert_job_run(&pool, "backup", "2026-07-13T00:00:00Z", Some("disk full")).await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/reports/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let h: HealthReport = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(h.failed_jobs.len(), 1);
        assert_eq!(h.failed_jobs[0].name, "backup");
    }
}
