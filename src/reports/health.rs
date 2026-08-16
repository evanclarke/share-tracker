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
//!   `reports::valuation` refuses to value a date with an errored price);
//! - every listing with a held day whose price was never even attempted —
//!   the missing-row counterpart of the errored list (see
//!   [`UnpricedListing`]);
//! - every (listing, action type, date) carrying more than one corporate
//!   action — the double-entry that silently compounds (see
//!   [`DuplicateAction`]).
//!
//! A database with no prices or FX rates at all reports `stale = false` for
//! that series: nothing has decayed — a fresh install shows no banner, and a
//! price/FX import that breaks before ever succeeding surfaces through
//! `failed_jobs` (and the Jobs page) instead.

use crate::entities::closing_price::{self, HeldTimeline};
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::{BTreeSet, HashSet};

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

/// A listing with a held day whose price was never stored at all — the
/// missing-row counterpart of [`ErroredPriceListing`]. An errored fetch at
/// least leaves a row to find; a day nobody ever asked for is silent and
/// permanent: it only shows up as a snapshot stuck stale, and by the time it
/// is noticed the provider may no longer serve that far back.
///
/// It happens whenever a trade is entered later than the price-import job's
/// lookback window on a listing not otherwise held — a batch of statements
/// entered years after the fact — so nothing ever attempted those days.
///
/// A day is unpriced when it is exactly what `reports::valuation` would ask
/// for and not find: the listing was held on some calendar date, that date's
/// valuation day (`Market::latest_trading_day_on_or_before`) has no
/// `closing_prices` row, and that day's close is already final. A day whose
/// row is errored belongs to `errored_prices` instead — the two lists
/// partition the problem. Close it with `POST /closing_prices/backfill`, or a
/// manual price for a day the provider can never serve.
#[derive(Debug, Serialize, Deserialize)]
pub struct UnpricedListing {
    pub listing_id: i64,
    pub ticker: String,
    /// Count of distinct valuation days with no stored row.
    pub unpriced_days: i64,
    pub earliest_date: NaiveDate,
    pub latest_date: NaiveDate,
}

/// More than one corporate action of the same type, on the same listing and
/// date. Two such rows are two independent events to every reader — the
/// cost-base pipeline sums both `ReturnOfCapital` reductions and multiplies
/// both `ShareSplit` ratios (SCENARIOS E-03, E-15) — so a re-submitted form or
/// a re-imported statement restates every cost base and quantity of the
/// listing with nothing to show for it.
///
/// Deliberately a **warning, not a constraint**: a genuine same-day pair
/// exists in principle (two tranches of one capital return), so the pair stays
/// enterable and this names it for the user to judge.
#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicateAction {
    pub listing_id: i64,
    pub ticker: String,
    pub action_type: String,
    pub date: NaiveDate,
    /// How many actions share this (listing, type, date) — always ≥ 2.
    pub action_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be found and
    /// deleted without a search.
    pub action_ids: Vec<i64>,
}

/// The grouped row behind [`DuplicateAction`]: SQLite returns the ids as one
/// `GROUP_CONCAT` string, split into the public struct's `Vec<i64>` by
/// [`db_duplicate_actions`].
#[derive(sqlx::FromRow)]
struct DuplicateActionRow {
    listing_id: i64,
    ticker: String,
    action_type: String,
    date: NaiveDate,
    action_count: i64,
    action_ids: String,
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
    /// Listings with a held day that has no stored price row at all, oldest
    /// hole first — the oldest is the least recoverable, since a provider
    /// stops serving history long before it stops serving last week.
    pub unpriced_days: Vec<UnpricedListing>,
    /// Every (listing, action type, date) carrying more than one corporate
    /// action, newest first. Empty when no two actions of a type share a
    /// listing and date.
    pub duplicate_actions: Vec<DuplicateAction>,
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

/// Held days with no stored closing-price row at all, per listing (see
/// [`UnpricedListing`]).
///
/// Deliberately shaped as the exact question `reports::valuation` asks, so
/// there are no false positives: for every calendar date a listing was held,
/// the valuation day it resolves to must have a row. Days whose close is not
/// final yet (today's, an unsettled crypto candle) are out of scope — the
/// walk stops at each market's `latest_complete_trading_day`.
///
/// One holdings load and one stored-date query per listing, then an in-memory
/// walk: six years of history per listing is thousands of dates, so a per-day
/// round trip is not an option (the same pre-loading pattern as `FxRates` and
/// `RenameHistory`).
///
/// Not on the read transaction its caller uses: `load_market` is pool-based,
/// and this check tolerates a concurrent write far better than a financial
/// aggregation would — a hole is a hole whichever snapshot it is seen in.
async fn db_unpriced_days(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<Vec<UnpricedListing>, sqlx::Error> {
    let timeline = HeldTimeline::load(pool).await?;
    let mut listings = Vec::new();
    for listing_id in timeline.listing_ids() {
        let Some(market) = closing_price::load_market(pool, listing_id).await? else {
            continue;
        };
        // A calendar so misconfigured it has no trading day in the past year
        // has nothing this check can say about it; the price-import job fails
        // loudly on the same listing.
        let Some(final_day) = market
            .latest_complete_trading_day(now)
            .map_err(sqlx::Error::Protocol)?
        else {
            continue;
        };
        let spans = timeline.held_spans(listing_id, final_day);
        if spans.is_empty() {
            continue;
        }
        // Every stored date, ok or errored: an errored day is *not* unpriced
        // — it is reported by `errored_prices`.
        let stored: HashSet<NaiveDate> =
            sqlx::query_scalar("SELECT price_date FROM closing_prices WHERE listing_id = ?")
                .bind(listing_id)
                .fetch_all(pool)
                .await?
                .into_iter()
                .collect();

        // Distinct valuation days, not calendar days: a weekend and the
        // Friday it values at are one hole, not three.
        let mut missing: BTreeSet<NaiveDate> = BTreeSet::new();
        for (from, to) in spans {
            let mut date = from;
            while date <= to {
                if let Some(valuation_day) = market.latest_trading_day_on_or_before(date)
                    && !stored.contains(&valuation_day)
                {
                    missing.insert(valuation_day);
                }
                date += Duration::days(1);
            }
        }
        let (Some(&earliest_date), Some(&latest_date)) = (missing.first(), missing.last()) else {
            continue;
        };
        listings.push(UnpricedListing {
            listing_id,
            ticker: market.listing.ticker.clone(),
            unpriced_days: missing.len() as i64,
            earliest_date,
            latest_date,
        });
    }
    // Oldest hole first: the least recoverable reads first.
    listings.sort_by_key(|row| (row.earliest_date, row.listing_id));
    Ok(listings)
}

/// Corporate actions sharing a (listing, type, date) — see [`DuplicateAction`].
///
/// Grouped in SQL and read on the caller's transaction: it is one small
/// aggregate over `corporate_actions`, not a per-listing walk like
/// [`db_unpriced_days`].
async fn db_duplicate_actions(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateAction>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DuplicateActionRow>(
        "SELECT ca.listing_id AS listing_id, l.ticker AS ticker, \
                ca.action_type AS action_type, ca.date AS date, \
                COUNT(*) AS action_count, GROUP_CONCAT(ca.id) AS action_ids \
         FROM corporate_actions ca JOIN listings l ON l.id = ca.listing_id \
         GROUP BY ca.listing_id, ca.action_type, ca.date \
         HAVING COUNT(*) > 1 \
         ORDER BY ca.date DESC, l.ticker, ca.action_type",
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            // GROUP_CONCAT's order is unspecified, so sort rather than trust it.
            let mut action_ids = row
                .action_ids
                .split(',')
                .map(|id| {
                    id.parse::<i64>().map_err(|e| {
                        sqlx::Error::Decode(format!("corporate action id {id}: {e}").into())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            action_ids.sort_unstable();
            Ok(DuplicateAction {
                listing_id: row.listing_id,
                ticker: row.ticker,
                action_type: row.action_type,
                date: row.date,
                action_count: row.action_count,
                action_ids,
            })
        })
        .collect()
}

/// Read the freshness facts on one snapshot. `today` and `now` are parameters
/// so tests can pin the staleness thresholds and the "close is final yet"
/// cut-off to fixed dates.
pub async fn db_health(
    pool: &SqlitePool,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Result<HealthReport, sqlx::Error> {
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
    let duplicate_actions = db_duplicate_actions(&mut tx).await?;
    tx.commit().await?;
    let unpriced_days = db_unpriced_days(pool, now).await?;

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
        unpriced_days,
        duplicate_actions,
    })
}

async fn report(State(pool): State<SqlitePool>) -> Result<Json<HealthReport>, ApiError> {
    let today = chrono::Local::now().date_naive();
    db_health(&pool, today, Utc::now())
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
    use crate::entities::corporate_action;
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};
    use axum::http::StatusCode;

    /// The report as at `today`, read late enough in the day (22:00 Sydney /
    /// noon UTC) that `today`'s ASX close is final. Tests that care about the
    /// "not final yet" boundary call `db_health` directly with their own
    /// `now`.
    async fn health(pool: &SqlitePool, today: NaiveDate) -> HealthReport {
        db_health(pool, today, noon_utc(today)).await.unwrap()
    }

    fn noon_utc(date: NaiveDate) -> DateTime<Utc> {
        date.and_hms_opt(12, 0, 0).expect("valid time").and_utc()
    }

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
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.latest_price_date, None);
        assert!(!h.prices_stale);
        assert_eq!(h.latest_fx_month, None);
        assert!(!h.fx_stale);
        assert!(h.failed_jobs.is_empty());
        assert!(h.errored_prices.is_empty());
        assert!(h.unpriced_days.is_empty());
        assert!(h.duplicate_actions.is_empty());
    }

    #[tokio::test]
    async fn prices_within_threshold_are_fresh_older_are_stale() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        // Wed 2026-07-08 → Mon 2026-07-13 is exactly 3 business days: fresh.
        insert_ok_price(&pool, 1, "2026-07-08").await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.latest_price_date, Some(ymd(2026, 7, 8)));
        assert!(!h.prices_stale);

        // One business day further out (Tue 2026-07-14) crosses the threshold.
        let h = health(&pool, ymd(2026, 7, 14)).await;
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
        let h = health(&pool, ymd(2026, 7, 13)).await;
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

        let h = health(&pool, ymd(2026, 7, 13)).await;
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

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.errored_prices.len(), 2);
        assert_eq!(h.errored_prices[0].ticker, "B"); // newest error first
        assert_eq!(h.errored_prices[1].ticker, "A");
    }

    #[tokio::test]
    async fn fx_fresh_with_previous_month_stale_when_older() {
        let pool = test_pool().await;
        // June is the month before July 2026: fresh.
        insert_fx_month(&pool, "2026-06").await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.latest_fx_month.as_deref(), Some("2026-06"));
        assert!(!h.fx_stale);

        // Come September with nothing newer imported, June is stale.
        let h = health(&pool, ymd(2026, 9, 1)).await;
        assert!(h.fx_stale);
    }

    #[tokio::test]
    async fn fx_current_month_is_fresh() {
        let pool = test_pool().await;
        insert_fx_month(&pool, "2026-07").await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
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
        let h = health(&pool, ymd(2026, 7, 13)).await;
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

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.failed_jobs.len(), 1);
        assert_eq!(h.failed_jobs[0].name, "backup");
    }

    /// The case the errored list cannot catch: a held day nobody ever
    /// fetched, so there is no row to find. Wed 2026-07-08 is a trading day
    /// inside the held span with no stored row.
    #[tokio::test]
    async fn a_held_day_with_no_stored_row_is_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("BHP").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        for day in [
            "2026-07-06",
            "2026-07-07",
            "2026-07-09",
            "2026-07-10",
            "2026-07-13",
        ] {
            insert_ok_price(&pool, 1, day).await;
        }

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.unpriced_days.len(), 1);
        let row = &h.unpriced_days[0];
        assert_eq!(row.listing_id, 1);
        assert_eq!(row.ticker, "BHP");
        assert_eq!(row.unpriced_days, 1);
        assert_eq!(row.earliest_date, ymd(2026, 7, 8));
        assert_eq!(row.latest_date, ymd(2026, 7, 8));
    }

    /// The two lists partition the problem: a day whose fetch failed has a
    /// row, so it is `errored_prices`' to report, never `unpriced_days`'.
    #[tokio::test]
    async fn an_errored_day_is_not_reported_as_unpriced() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("BHP").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        for day in [
            "2026-07-06",
            "2026-07-07",
            "2026-07-09",
            "2026-07-10",
            "2026-07-13",
        ] {
            insert_ok_price(&pool, 1, day).await;
        }
        insert_error_price(&pool, 1, "2026-07-08", "provider down").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.unpriced_days.is_empty());
        assert_eq!(h.errored_prices.len(), 1);
    }

    /// A day the market was shut is not a hole: the weekend and the ASX's
    /// King's Birthday (Mon 2026-06-08) all value at Fri 2026-06-05, which is
    /// priced.
    #[tokio::test]
    async fn non_trading_days_are_not_unpriced() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 6, 5))
            .insert(&pool)
            .await;
        for day in ["2026-06-05", "2026-06-09", "2026-06-10"] {
            insert_ok_price(&pool, 1, day).await;
        }

        let h = health(&pool, ymd(2026, 6, 10)).await;
        assert!(h.unpriced_days.is_empty());
    }

    /// Today's close is not final until the exchange closes, so the day the
    /// price-import job has yet to collect is not reported as a hole — it
    /// becomes one only once the close has passed and nothing was stored.
    #[tokio::test]
    async fn a_close_that_is_not_final_yet_is_not_unpriced() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        for day in [
            "2026-07-06",
            "2026-07-07",
            "2026-07-08",
            "2026-07-09",
            "2026-07-10",
        ] {
            insert_ok_price(&pool, 1, day).await;
        }

        // 11:00 Sydney on Mon 2026-07-13: the ASX has not closed yet.
        let before_close = "2026-07-13T01:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let h = db_health(&pool, ymd(2026, 7, 13), before_close)
            .await
            .unwrap();
        assert!(h.unpriced_days.is_empty());

        // After the close, the still-unstored day is a hole.
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.unpriced_days.len(), 1);
        assert_eq!(h.unpriced_days[0].latest_date, ymd(2026, 7, 13));
    }

    /// Nothing is held after the last unit is sold, so the span ends there —
    /// a listing sold out of the portfolio must not report every day since as
    /// a hole.
    #[tokio::test]
    async fn a_fully_sold_listing_is_not_reported_after_its_sale() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2026, 7, 8))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("100")).await;

        // Nothing was ever priced, so the whole held span is a hole: Mon
        // 2026-07-06 and Tue 07-07, and nothing from the sale date onward.
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.unpriced_days.len(), 1);
        assert_eq!(h.unpriced_days[0].unpriced_days, 2);
        assert_eq!(h.unpriced_days[0].earliest_date, ymd(2026, 7, 6));
        assert_eq!(h.unpriced_days[0].latest_date, ymd(2026, 7, 7));
    }

    /// A hole spanning a ticker/exchange change is walked on the calendar
    /// that was in force at each date: the ASX's King's Birthday (Mon
    /// 2026-06-08) is not a trading day before the move to the NYSE, whose
    /// calendar has no such holiday, so it is not its own hole.
    #[tokio::test]
    async fn a_hole_straddling_a_rename_uses_the_calendar_of_the_date() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("OLD").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 6, 5))
            .insert(&pool)
            .await;
        crate::entities::listing_rename::db_rename(
            &pool,
            1,
            &crate::entities::listing_rename::RenameBody {
                effective_date: ymd(2026, 6, 10),
                ticker: "NEW".to_string(),
                exchange_mic: Some("XNYS".to_string()),
                name: None,
                price_symbol: None,
                note: None,
            },
        )
        .await
        .unwrap();

        // 17:00 New York on Fri 2026-06-12: that day's close is final.
        let now = "2026-06-12T21:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let h = db_health(&pool, ymd(2026, 6, 12), now).await.unwrap();
        assert_eq!(h.unpriced_days.len(), 1);
        let row = &h.unpriced_days[0];
        assert_eq!(row.ticker, "NEW");
        // Fri 06-05, Tue 06-09, then 06-10..06-12 under the NYSE calendar —
        // the ASX holiday of Mon 06-08 values at Fri 06-05 and is not a sixth.
        assert_eq!(row.unpriced_days, 5);
        assert_eq!(row.earliest_date, ymd(2026, 6, 5));
        assert_eq!(row.latest_date, ymd(2026, 6, 12));
    }

    #[tokio::test]
    async fn a_fully_priced_database_reports_no_unpriced_days() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        for day in [
            "2026-07-06",
            "2026-07-07",
            "2026-07-08",
            "2026-07-09",
            "2026-07-10",
            "2026-07-13",
        ] {
            insert_ok_price(&pool, 1, day).await;
        }

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.unpriced_days.is_empty());
    }

    /// Oldest hole first: the further back it goes the less likely the
    /// provider will still serve it, so it is the one to act on.
    #[tokio::test]
    async fn unpriced_listings_are_ordered_oldest_hole_first() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("RECENT")
            .insert(&pool)
            .await;
        test_support::listing(2).ticker("OLD").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        test_support::buy(2, 2)
            .date(ymd(2026, 7, 2))
            .insert(&pool)
            .await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.unpriced_days.len(), 2);
        assert_eq!(h.unpriced_days[0].ticker, "OLD");
        assert_eq!(h.unpriced_days[0].earliest_date, ymd(2026, 7, 2));
        assert_eq!(h.unpriced_days[1].ticker, "RECENT");
    }

    async fn insert_roc(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate) {
        insert_action(
            pool,
            id,
            listing_id,
            date,
            corporate_action::ActionKind::ReturnOfCapital {
                amount_per_unit: dec("0.50"),
                currency: "AUD".to_string(),
                record_date: None,
            },
        )
        .await;
    }

    async fn insert_split(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate) {
        insert_action(
            pool,
            id,
            listing_id,
            date,
            corporate_action::ActionKind::ShareSplit {
                split_new_units: dec("2"),
                split_old_units: dec("1"),
            },
        )
        .await;
    }

    async fn insert_action(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        kind: corporate_action::ActionKind,
    ) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id,
                date,
                kind,
            },
        )
        .await
        .unwrap();
    }

    /// SCENARIOS E-03 / E-15: a re-submitted form or a re-imported statement
    /// leaves two identical actions, and the cost-base pipeline reads them as
    /// two events — the return of capital reduces twice, the split multiplies
    /// twice. Nothing rejects the pair (a genuine same-day pair exists in
    /// principle), so health names it, with the ids to delete from.
    #[tokio::test]
    async fn duplicated_corporate_actions_are_reported_with_their_ids() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("ROCC").insert(&pool).await;
        test_support::listing(2).ticker("SPLT").insert(&pool).await;
        insert_roc(&pool, 1, 1, ymd(2026, 3, 10)).await;
        insert_roc(&pool, 2, 1, ymd(2026, 3, 10)).await;
        insert_split(&pool, 3, 2, ymd(2026, 6, 1)).await;
        insert_split(&pool, 4, 2, ymd(2026, 6, 1)).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        // Newest first: the split (June) before the capital return (March).
        assert_eq!(h.duplicate_actions.len(), 2);
        let split = &h.duplicate_actions[0];
        assert_eq!(split.ticker, "SPLT");
        assert_eq!(split.listing_id, 2);
        assert_eq!(split.action_type, "ShareSplit");
        assert_eq!(split.date, ymd(2026, 6, 1));
        assert_eq!(split.action_count, 2);
        assert_eq!(split.action_ids, vec![3, 4]);
        let roc = &h.duplicate_actions[1];
        assert_eq!(roc.ticker, "ROCC");
        assert_eq!(roc.action_type, "ReturnOfCapital");
        assert_eq!(roc.date, ymd(2026, 3, 10));
        assert_eq!(roc.action_ids, vec![1, 2]);
    }

    /// The warning is per (listing, action type, date): actions that differ in
    /// any of the three are ordinary independent events, however close
    /// together they fall.
    #[tokio::test]
    async fn actions_differing_in_listing_type_or_date_are_not_duplicates() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("AAA").insert(&pool).await;
        test_support::listing(2).ticker("BBB").insert(&pool).await;
        // Same type and date, different listing.
        insert_roc(&pool, 1, 1, ymd(2026, 3, 10)).await;
        insert_roc(&pool, 2, 2, ymd(2026, 3, 10)).await;
        // Same listing and type, different date.
        insert_roc(&pool, 3, 1, ymd(2026, 9, 10)).await;
        // Same listing and date, different type.
        insert_split(&pool, 4, 1, ymd(2026, 3, 10)).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.duplicate_actions.is_empty());
    }

    /// Three of a kind is one row, not three: the report answers "this
    /// (listing, type, date) is entered N times", listing every id.
    #[tokio::test]
    async fn three_identical_actions_are_one_row_counting_three() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("TRIP").insert(&pool).await;
        insert_roc(&pool, 7, 1, ymd(2026, 3, 10)).await;
        insert_roc(&pool, 8, 1, ymd(2026, 3, 10)).await;
        insert_roc(&pool, 9, 1, ymd(2026, 3, 10)).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_actions.len(), 1);
        assert_eq!(h.duplicate_actions[0].action_count, 3);
        assert_eq!(h.duplicate_actions[0].action_ids, vec![7, 8, 9]);
    }

    #[tokio::test]
    async fn api_get_health() {
        let pool = test_pool().await;
        insert_job_run(&pool, "backup", "2026-07-13T00:00:00Z", Some("disk full")).await;
        let resp = ApiClient::over(router().with_state(pool))
            .get("/reports/health")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let h: HealthReport = resp.json();
        assert_eq!(h.failed_jobs.len(), 1);
        assert_eq!(h.failed_jobs[0].name, "backup");
    }
}
