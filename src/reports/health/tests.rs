use super::*;
use crate::entities::closing_price::PriceSource;
use crate::entities::closing_price::{self, NonTradingReason};
use crate::entities::corporate_action;
use crate::entities::holding_account;
use crate::entities::inheritance::Inheritance;
use crate::entities::investment_expense::{ExpenseType, InvestmentExpense};
use crate::infra::id::ListingId;
use crate::test_support::{self, ApiClient, dec, test_pool, ymd};
use axum::http::StatusCode;
use chrono::Duration;
use rust_decimal::Decimal;
use std::collections::HashMap;

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

async fn insert_ok_price(pool: &SqlitePool, listing_id: ListingId, date: &str) {
    test_support::closing_price(listing_id.0, date.parse().unwrap())
        .price("10.50")
        .source(PriceSource::Yahoo)
        .fetched_at("2026-07-01T00:00:00Z")
        .insert(pool)
        .await;
}

async fn insert_error_price(pool: &SqlitePool, listing_id: ListingId, date: &str, error: &str) {
    test_support::closing_price(listing_id.0, date.parse().unwrap())
        .source(PriceSource::Yahoo)
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
        "INSERT INTO job_runs (name, started_at, finished_at, status, error) \
             VALUES (?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(finished_at)
    .bind(finished_at)
    .bind(if error.is_none() { "ok" } else { "failed" })
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
    assert!(h.duplicate_price_series.is_empty());
    assert!(h.duplicate_trades.is_empty());
    assert!(h.duplicate_actions.is_empty());
    assert!(h.duplicate_amma_statements.is_empty());
    assert!(h.duplicate_income.is_empty());
    assert!(h.duplicate_interest.is_empty());
    assert!(h.duplicate_expenses.is_empty());
    assert!(h.duplicate_ess_statements.is_empty());
    assert!(h.ess_30_day_rule.is_empty());
    assert!(h.non_trading_day_trades.is_empty());
    assert!(h.nil_proceeds_disposals.is_empty());
}

#[tokio::test]
async fn prices_within_threshold_are_fresh_older_are_stale() {
    let pool = test_pool().await;
    test_support::listing(1).insert(&pool).await;
    // Wed 2026-07-08 → Mon 2026-07-13 is exactly 3 business days: fresh.
    insert_ok_price(&pool, ListingId(1), "2026-07-08").await;
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
    insert_ok_price(&pool, ListingId(1), "2026-07-01").await;
    insert_error_price(&pool, ListingId(1), "2026-07-10", "provider down").await;
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
        ListingId(1),
        "2026-07-01",
        "provider returned no candles for LAR",
    )
    .await;
    insert_error_price(
        &pool,
        ListingId(1),
        "2026-07-02",
        "provider returned no candles for LAR",
    )
    .await;
    insert_error_price(
        &pool,
        ListingId(1),
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
    insert_ok_price(&pool, ListingId(1), "2026-07-01").await;
    insert_error_price(&pool, ListingId(1), "2026-07-05", "err A").await;
    insert_error_price(&pool, ListingId(2), "2026-07-10", "err B").await;

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

/// Store one `job_schedule` row: what a spawned schedule entry writes
/// every iteration.
async fn insert_job_schedule(
    pool: &SqlitePool,
    name: &str,
    cron: &str,
    timezone: Option<&str>,
    next_run_at: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO job_schedule (name, cron, timezone, next_run_at, updated_at) \
             VALUES (?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(cron)
    .bind(timezone)
    .bind(next_run_at.to_rfc3339())
    .bind(next_run_at.to_rfc3339())
    .execute(pool)
    .await
    .unwrap();
}

/// Store one run that started and has not finished — what a run in flight
/// looks like, and equally what a process that died mid-run left behind.
async fn insert_open_job_run(pool: &SqlitePool, name: &str, started_at: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO job_runs (name, started_at, finished_at, status, error) \
             VALUES (?, ?, NULL, 'running', NULL)",
    )
    .bind(name)
    .bind(started_at.to_rfc3339())
    .execute(pool)
    .await
    .unwrap();
}

/// The state every existing database is in the moment it is upgraded: the
/// schedule rows are written by the *running* scheduler, so until a server
/// has started on this schema there are none — and nothing may be reported
/// overdue, because nothing has yet promised to run.
#[tokio::test]
async fn a_database_with_no_stored_schedule_reports_nothing_overdue() {
    let pool = test_pool().await;
    // A job that ran long ago and has no schedule row is not overdue
    // either: the run history alone never says a job is late.
    insert_job_run(&pool, "backup", "2020-01-01T00:00:00Z", None).await;
    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.overdue_jobs.is_empty());
    assert!(h.stalled_jobs.is_empty());
}

/// A job whose scheduler is alive holds an instant in the future, and is
/// not overdue however long ago it last ran.
#[tokio::test]
async fn a_job_whose_schedule_is_still_moving_is_not_overdue() {
    let pool = test_pool().await;
    let now = noon_utc(ymd(2026, 7, 13));
    insert_job_schedule(&pool, "backup", "0 0 * * 0", None, now + Duration::days(4)).await;
    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.overdue_jobs.is_empty());
}

/// The grace margin's boundary, from both sides. The stored instant is
/// only moved on *after* a run returns, so a job legitimately sits a
/// little past its own due time; exactly the margin is still on time, and
/// a second past it is not.
#[tokio::test]
async fn the_overdue_grace_margin_is_exclusive_at_its_boundary() {
    let pool = test_pool().await;
    let now = noon_utc(ymd(2026, 7, 13));
    insert_job_schedule(
        &pool,
        "backup",
        "0 0 * * 0",
        None,
        now - Duration::hours(JOB_OVERDUE_GRACE_HOURS),
    )
    .await;
    let h = db_health(&pool, ymd(2026, 7, 13), now).await.unwrap();
    assert!(
        h.overdue_jobs.is_empty(),
        "exactly the grace margin past due is still on time"
    );

    let h = db_health(&pool, ymd(2026, 7, 13), now + Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!(h.overdue_jobs.len(), 1, "one second later it is overdue");
    assert_eq!(h.overdue_jobs[0].name, "backup");
    assert_eq!(h.overdue_jobs[0].overdue_hours, JOB_OVERDUE_GRACE_HOURS);
}

/// An overdue row names the schedule it is late against — the cron
/// expression as written and the entry's own timezone — so the banner can
/// say which of a job's lines has stopped. `price-import` has three.
#[tokio::test]
async fn an_overdue_job_names_its_schedule_and_how_late_it_is() {
    let pool = test_pool().await;
    let now = noon_utc(ymd(2026, 7, 13));
    insert_job_schedule(
        &pool,
        "price-import",
        "30 17 * * 1-5",
        Some("Australia/Sydney"),
        now - Duration::hours(30),
    )
    .await;
    // A second entry of the same job, still alive: only the dead one is
    // reported.
    insert_job_schedule(
        &pool,
        "price-import",
        "30 17 * * 1-5",
        Some("America/New_York"),
        now + Duration::hours(4),
    )
    .await;
    let h = db_health(&pool, ymd(2026, 7, 13), now).await.unwrap();
    assert_eq!(h.overdue_jobs.len(), 1);
    let row = &h.overdue_jobs[0];
    assert_eq!(row.name, "price-import");
    assert_eq!(row.cron, "30 17 * * 1-5");
    assert_eq!(row.timezone.as_deref(), Some("Australia/Sydney"));
    assert_eq!(row.overdue_hours, 30);
    assert_eq!(
        row.next_run_at,
        (now - Duration::hours(30)).to_rfc3339(),
        "the instant reported is the stored one, verbatim"
    );
}

/// A run open longer than any run of these jobs takes is a process that
/// died mid-run, not a run in flight — the distinction migration 0042
/// deliberately left to this report (SCENARIOS T-11). The boundary is
/// exclusive, like the overdue margin's.
#[tokio::test]
async fn a_run_open_longer_than_any_run_takes_is_reported_stalled() {
    let pool = test_pool().await;
    let now = noon_utc(ymd(2026, 7, 13));
    insert_open_job_run(
        &pool,
        "backup",
        now - Duration::hours(JOB_RUNNING_STALE_HOURS),
    )
    .await;
    let h = db_health(&pool, ymd(2026, 7, 13), now).await.unwrap();
    assert!(
        h.stalled_jobs.is_empty(),
        "a run open exactly the margin is still in flight"
    );

    let h = db_health(&pool, ymd(2026, 7, 13), now + Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!(h.stalled_jobs.len(), 1);
    assert_eq!(h.stalled_jobs[0].name, "backup");
    assert_eq!(h.stalled_jobs[0].running_hours, JOB_RUNNING_STALE_HOURS);
    // …and it is still not a *failure*: nothing failed.
    assert!(h.failed_jobs.is_empty());
}

/// Once a later run has started, the abandoned row is history rather than
/// a live fault — the same latest-run-only rule `failed_jobs` follows.
#[tokio::test]
async fn an_abandoned_run_stops_being_reported_once_the_job_runs_again() {
    let pool = test_pool().await;
    let now = noon_utc(ymd(2026, 7, 13));
    insert_open_job_run(&pool, "backup", now - Duration::days(30)).await;
    let h = db_health(&pool, ymd(2026, 7, 13), now).await.unwrap();
    assert_eq!(h.stalled_jobs.len(), 1);

    insert_job_run(&pool, "backup", "2026-07-13T00:01:00Z", None).await;
    let h = db_health(&pool, ymd(2026, 7, 13), now).await.unwrap();
    assert!(h.stalled_jobs.is_empty());
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
        insert_ok_price(&pool, ListingId(1), day).await;
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

/// SCENARIOS Q-02: once a listing records the date the provider stopped
/// quoting it, both lists go quiet from that date on — the errored rows
/// and the never-fetched days after it are expected, not a to-do, and
/// valuation carries the last close forward instead of blocking. Holes
/// *before* the date are real and stay reported.
#[tokio::test]
async fn an_unpriced_listing_stops_being_nagged_about_from_its_date() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("ATVI").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2026, 7, 6))
        .insert(&pool)
        .await;
    insert_ok_price(&pool, ListingId(1), "2026-07-06").await;
    // A real hole before the delisting, then errored rows after it, then
    // days nothing ever fetched.
    insert_error_price(&pool, ListingId(1), "2026-07-07", "provider down").await;
    for day in ["2026-07-09", "2026-07-10"] {
        insert_error_price(
            &pool,
            ListingId(1),
            day,
            "yahoo fetch for ATVI failed: Not found",
        )
        .await;
    }

    let before = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(before.errored_prices[0].errored_days, 3);
    assert_eq!(before.unpriced_days[0].unpriced_days, 2); // 8 and 13 July

    let marked = test_support::listing(1)
        .ticker("ATVI")
        .unpriced_from(ymd(2026, 7, 9))
        .build();
    crate::entities::listing::db_upsert(&pool, &marked)
        .await
        .unwrap();

    let after = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(
        after.errored_prices.len(),
        1,
        "the 7 July hole predates the delisting and is still real"
    );
    assert_eq!(after.errored_prices[0].errored_days, 1);
    assert_eq!(after.errored_prices[0].latest_errored_date, ymd(2026, 7, 7));
    assert_eq!(after.errored_prices[0].latest_error, "provider down");
    assert_eq!(
        after.unpriced_days[0].unpriced_days, 1,
        "8 July stays a hole; 13 July is inside the unpriced run"
    );
    assert_eq!(after.unpriced_days[0].earliest_date, ymd(2026, 7, 8));
}

/// The mirror (migration 0037): once a listing records the date its
/// provider series begins, both lists go quiet *before* it — nothing
/// there can ever be fetched, and valuation leaves the holding out of
/// those dates' totals instead of waiting. Holes from the date on are
/// real and stay reported.
#[tokio::test]
async fn a_listing_is_not_nagged_about_before_its_series_begins() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2026, 7, 6))
        .insert(&pool)
        .await;
    // Errored rows on the days before the series begins, then a real hole
    // after it (10 July is never fetched).
    for day in ["2026-07-06", "2026-07-07"] {
        insert_error_price(&pool, ListingId(1), day, "yahoo fetch for LAC failed: 400").await;
    }
    insert_ok_price(&pool, ListingId(1), "2026-07-09").await;

    let before = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(before.errored_prices[0].errored_days, 2);
    assert_eq!(before.unpriced_days[0].unpriced_days, 3); // 8, 10 and 13 July

    let marked = test_support::listing(1)
        .ticker("LAC")
        .unpriced_before(ymd(2026, 7, 9))
        .build();
    crate::entities::listing::db_upsert(&pool, &marked)
        .await
        .unwrap();

    let after = health(&pool, ymd(2026, 7, 13)).await;
    assert!(
        after.errored_prices.is_empty(),
        "both errored rows predate the series and are expected, not a to-do: {:?}",
        after.errored_prices
    );
    assert_eq!(
        after.unpriced_days[0].unpriced_days, 2,
        "8 July is before the series begins; 10 and 13 July are real holes"
    );
    assert_eq!(after.unpriced_days[0].earliest_date, ymd(2026, 7, 10));
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
        insert_ok_price(&pool, ListingId(1), day).await;
    }
    insert_error_price(&pool, ListingId(1), "2026-07-08", "provider down").await;

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
        insert_ok_price(&pool, ListingId(1), day).await;
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
        insert_ok_price(&pool, ListingId(1), day).await;
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

/// The crypto counterpart: a UTC candle that is final but not yet
/// published is the one the next price-import run will collect, so it is
/// not a hole either until the provider's publication lag has passed.
#[tokio::test]
async fn a_crypto_candle_not_yet_published_is_not_unpriced() {
    let pool = test_pool().await;
    test_support::listing(1)
        .crypto()
        .ticker("BTC")
        .name("Bitcoin")
        .insert(&pool)
        .await;
    test_support::buy(1, 1)
        .date(ymd(2026, 10, 6))
        .insert(&pool)
        .await;
    for day in ["2026-10-06", "2026-10-07"] {
        insert_ok_price(&pool, ListingId(1), day).await;
    }

    // 06:30 UTC Fri 2026-10-09: Thursday's candle is final, unpublished.
    let before_publication = "2026-10-09T06:30:00Z".parse::<DateTime<Utc>>().unwrap();
    let h = db_health(&pool, ymd(2026, 10, 9), before_publication)
        .await
        .unwrap();
    assert!(h.unpriced_days.is_empty());

    // At noon UTC it is published, and still unstored: a hole.
    let h = health(&pool, ymd(2026, 10, 9)).await;
    assert_eq!(h.unpriced_days.len(), 1);
    assert_eq!(h.unpriced_days[0].latest_date, ymd(2026, 10, 8));
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
/// 2026-06-08) is not a trading day before the move, while the exchange
/// moved to keeps no such holiday, so it is not its own hole.
///
/// The destination is a second **AUD** market rather than the seeded XNYS
/// because a rename across a currency boundary is refused
/// (`RenameError::ExchangeCurrencyMismatch`, SCENARIOS R-01) — the
/// calendar is what this test is about, not the currency.
#[tokio::test]
async fn a_hole_straddling_a_rename_uses_the_calendar_of_the_date() {
    let pool = test_pool().await;
    crate::entities::exchange::db_upsert(
        &pool,
        &crate::entities::exchange::Exchange {
            mic: "CXAX".to_string(),
            name: "Second AUD market".to_string(),
            country: "Australia".to_string(),
            currency: "AUD".into(),
            timezone: "Australia/Sydney".to_string(),
            settlement_days: 2,
            close_time: "16:00".to_string(),
        },
    )
    .await
    .unwrap();
    test_support::listing(1).ticker("OLD").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2026, 6, 5))
        .insert(&pool)
        .await;
    crate::entities::listing_rename::db_rename(
        &pool,
        ListingId(1),
        &crate::entities::listing_rename::RenameBody {
            effective_date: ymd(2026, 6, 10),
            ticker: "NEW".to_string(),
            exchange_mic: Some("CXAX".to_string()),
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
    // Fri 06-05, Tue 06-09, then 06-10..06-12 under the new calendar —
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
        insert_ok_price(&pool, ListingId(1), day).await;
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

async fn insert_roc(pool: &SqlitePool, id: i64, listing_id: ListingId, date: NaiveDate) {
    insert_action(
        pool,
        id,
        listing_id,
        date,
        corporate_action::ActionKind::ReturnOfCapital {
            amount_per_unit: dec("0.50"),
            currency: "AUD".into(),
            record_date: None,
        },
    )
    .await;
}

async fn amit_listing(pool: &SqlitePool, id: i64, ticker: &str) {
    test_support::listing(id)
        .ticker(ticker)
        .amit(true)
        .insert(pool)
        .await;
}

/// An AMMA statement for `year_end`, in `holding_account_id`. Only the
/// grouping key matters here, so the amounts stay at the fixture's zeros.
async fn insert_amma(
    pool: &SqlitePool,
    id: i64,
    listing_id: ListingId,
    year_end: NaiveDate,
    holding_account_id: i64,
) {
    test_support::amma(id, listing_id.0)
        .with(|a| {
            a.tax_year_end_date = year_end;
            a.date_received = year_end + Duration::days(60);
            a.holding_account_id = holding_account_id;
        })
        .insert(pool)
        .await;
}

async fn insert_split(pool: &SqlitePool, id: i64, listing_id: ListingId, date: NaiveDate) {
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
    listing_id: ListingId,
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

/// A demerger the price provider adjusted for, with no stated close to
/// re-base its pre-demerger prices back: the rows are ok, so nothing else
/// in health sees them, and a valuation of those dates is silently the
/// current level (Evan's LAC history, ~2.46x understated). Adding the
/// stated close both fixes the prices and clears the warning.
#[tokio::test]
async fn a_demerger_with_provider_adjusted_prices_and_no_stated_close_is_reported() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    // Two pre-demerger days served long after the demerger…
    for date in ["2023-09-29", "2023-10-02"] {
        test_support::closing_price(1, date.parse().unwrap())
            .price("10.13")
            .source(PriceSource::Yahoo)
            .fetched_at("2026-07-26T07:44:56Z")
            .insert(&pool)
            .await;
    }
    // …one collected before it, which arrived contemporaneous…
    test_support::closing_price(1, "2023-09-28".parse().unwrap())
        .price("24.58")
        .source(PriceSource::Yahoo)
        .fetched_at("2023-09-28T21:00:00Z")
        .insert(&pool)
        .await;
    // …and one after it, which the provider never restated.
    test_support::closing_price(1, "2023-10-04".parse().unwrap())
        .price("11.72")
        .source(PriceSource::Yahoo)
        .fetched_at("2026-07-26T07:44:56Z")
        .insert(&pool)
        .await;
    insert_demerger(&pool, 1, ListingId(1), 2, ymd(2023, 10, 3), None).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.demergers_missing_close.len(), 1);
    let d = &h.demergers_missing_close[0];
    assert_eq!(d.action_id, 1);
    assert_eq!(d.ticker, "LAC");
    assert_eq!(d.demerger_date, ymd(2023, 10, 3));
    assert_eq!(
        d.adjusted_days, 2,
        "only pre-demerger days observed on or after the demerger are suspect"
    );
    assert_eq!(d.earliest_date, ymd(2023, 9, 29));
    assert_eq!(d.latest_date, ymd(2023, 10, 2));
    // No hand-entered row in the span, so the second figure is empty.
    assert_eq!(d.manual_days, 0);
    assert_eq!(d.manual_earliest_date, None);
    assert_eq!(d.manual_latest_date, None);
    // Nothing else in health notices: the rows are ok, not errored.
    assert!(h.errored_prices.is_empty());

    // Stating the close re-bases the prices and clears the alarm.
    insert_demerger(
        &pool,
        1,
        ListingId(1),
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.90")),
    )
    .await;
    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.demergers_missing_close.is_empty());
}

/// The real shape of Evan's LAC history: the pre-demerger span holds both
/// provider-fetched rows served after the demerger **and**, running back
/// years earlier, hand-entered rows — copies of the demerged entity's
/// series, entered to unblock stale snapshots, whose own `reason` says
/// they are "unblocked, not accurate".
///
/// The re-base walk skips manual rows by design, so counting only the
/// fetched half published 260 where the affected span was 635 and made the
/// alarm quietest about the rows already known to be wrong — and put the
/// start of the exposure 18 months late. Both halves are reported, so the
/// banner states what a stated close repairs and what it leaves.
#[tokio::test]
async fn a_demerger_reports_hand_entered_prices_in_the_span_separately() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    // The hand-entered half: the demerged entity's closes, copied in years
    // after the fact to unblock permanently stale snapshots.
    for date in ["2021-03-25", "2022-09-19"] {
        test_support::closing_price(1, date.parse().unwrap())
            .price("4.12")
            .fetched_at("2026-07-28T02:00:00Z")
            .manual(
                "listing 8 (LAR) stored close for the same date",
                "demerger-adjusted, so this period is unblocked, not accurate",
            )
            .insert(&pool)
            .await;
    }
    // The fetched half: what the provider served, after the demerger.
    for date in ["2022-09-20", "2023-10-02"] {
        test_support::closing_price(1, date.parse().unwrap())
            .price("10.13")
            .source(PriceSource::Yahoo)
            .fetched_at("2026-07-26T07:44:56Z")
            .insert(&pool)
            .await;
    }
    insert_demerger(&pool, 1, ListingId(1), 2, ymd(2023, 10, 3), None).await;

    let resp = ApiClient::over(router().with_state(pool.clone()))
        .get("/reports/health")
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    let body: serde_json::Value = resp.json();
    let d = &body["demergers_missing_close"][0];
    // What a stated close re-bases…
    assert_eq!(d["adjusted_days"], 2);
    assert_eq!(d["earliest_date"], "2022-09-20");
    assert_eq!(d["latest_date"], "2023-10-02");
    // …and what it does not, which starts 18 months earlier: the span the
    // operator has to deal with runs from the manual rows, not the fetched
    // ones.
    assert_eq!(d["manual_days"], 2);
    assert_eq!(d["manual_earliest_date"], "2021-03-25");
    assert_eq!(d["manual_latest_date"], "2022-09-19");

    let h = health(&pool, ymd(2026, 7, 13)).await;
    let d = &h.demergers_missing_close[0];
    assert_eq!(
        d.adjusted_days, 2,
        "the fetched figure counts only what the re-base walk touches"
    );
    assert_eq!(d.manual_earliest_date, Some(ymd(2021, 3, 25)));
    assert!(
        d.manual_earliest_date.unwrap() < d.earliest_date,
        "the suspect span starts at the earlier of the two halves"
    );
}

/// A demerger whose head listing has no provider-adjusted price is not a
/// problem: nothing needs re-basing, so no statement is asked for — and a
/// hand-entered pre-demerger row does not make one, since a stated close
/// would not touch it. The manual figure is context on the warning, never
/// a warning of its own.
#[tokio::test]
async fn a_demerger_with_no_adjusted_prices_is_not_reported() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    test_support::closing_price(1, "2023-09-29".parse().unwrap())
        .price("24.58")
        .source(PriceSource::Yahoo)
        .fetched_at("2023-09-29T21:00:00Z")
        .insert(&pool)
        .await;
    test_support::closing_price(1, "2023-09-28".parse().unwrap())
        .price("24.10")
        .fetched_at("2026-07-28T02:00:00Z")
        .manual("the demerged entity's series", "no candle is served")
        .insert(&pool)
        .await;
    insert_demerger(&pool, 1, ListingId(1), 2, ymd(2023, 10, 3), None).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.demergers_missing_close.is_empty());
}

/// A price the *demerged* listing was served by the provider, dated
/// `date` — the evidence that it, not the head, traded before the
/// demerger.
async fn insert_fetched_price(pool: &SqlitePool, listing_id: ListingId, date: &str, price: &str) {
    test_support::closing_price(listing_id.0, date.parse().unwrap())
        .price(price)
        .source(PriceSource::Yahoo)
        .fetched_at("2026-07-26T07:44:56Z")
        .insert(pool)
        .await;
}

/// Evan's LAC demerger as it was recorded for three years, reconstructed
/// as it looks once the borrowed prices are cleared: the head listing is
/// the entity the Arrangement **created** (a new corporation that took the
/// original ticker) and the entity that actually **continued** is recorded
/// as the one demerged from it.
///
/// Nothing else can see this. No tax figure moves — the apportionment
/// percentages attach to the tickers, not to the roles — and no write-time
/// check has anything to test. The only trace is in the price series: the
/// head parcel is held from 2021 on a listing whose provider series begins
/// at the demerger, while the listing it supposedly created has history
/// running back years before it. That inversion is the predicate.
#[tokio::test]
async fn a_demerger_recorded_with_the_created_entity_as_head_is_reported() {
    let pool = test_pool().await;
    // Listing 1 stands for LAC, the new corporation recorded as the head…
    test_support::listing(1)
        .ticker("LAC")
        .unpriced_before(ymd(2023, 10, 3))
        .insert(&pool)
        .await;
    // …listing 2 for the continuing entity, recorded as the demerged one.
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    // The 2021 parcel, sitting on a listing that did not exist in 2021.
    test_support::buy(1, 1)
        .date(ymd(2021, 3, 25))
        .insert(&pool)
        .await;
    // The head's own series begins at the demerger, and not before it.
    insert_fetched_price(&pool, ListingId(1), "2023-10-03", "9.67").await;
    // The continuing entity's series runs back years before the event.
    for date in ["2021-03-25", "2022-09-19", "2023-10-02"] {
        insert_fetched_price(&pool, ListingId(2), date, "16.85").await;
    }
    insert_demerger(&pool, 1, ListingId(1), 2, ymd(2023, 10, 3), None).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.demergers_head_not_continuing.len(), 1);
    let d = &h.demergers_head_not_continuing[0];
    assert_eq!(d.action_id, 1);
    assert_eq!(d.listing_id, 1);
    assert_eq!(d.ticker, "LAC");
    assert_eq!(d.demerger_date, ymd(2023, 10, 3));
    assert_eq!(
        d.head_unpriced_before,
        Some(ymd(2023, 10, 3)),
        "the marker corroborates what the check infers"
    );
    assert_eq!(d.head_first_price_date, Some(ymd(2023, 10, 3)));
    assert_eq!(
        d.head_held_from,
        ymd(2021, 3, 25),
        "the day the model starts claiming a security its own series has not reached"
    );
    assert_eq!(d.demerger_listing_id, 2);
    assert_eq!(d.demerger_ticker, "LAR");
    assert_eq!(d.demerged_priced_days, 3);
    assert_eq!(d.demerged_earliest_date, ymd(2021, 3, 25));
    assert_eq!(d.demerged_latest_date, ymd(2023, 10, 2));

    // Re-modelled with the continuing entity as the head, the inversion is
    // gone: the head now holds the earlier series.
    insert_demerger(&pool, 1, ListingId(2), 1, ymd(2023, 10, 3), None).await;
    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.demergers_head_not_continuing.is_empty());
}

/// The same finding over the HTTP surface the banner reads.
#[tokio::test]
async fn an_inverted_demerger_is_reported_over_http() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2021, 3, 25))
        .insert(&pool)
        .await;
    insert_fetched_price(&pool, ListingId(2), "2021-03-25", "16.85").await;
    insert_demerger(&pool, 1, ListingId(1), 2, ymd(2023, 10, 3), None).await;

    let body: serde_json::Value = ApiClient::over(router().with_state(pool.clone()))
        .get_json("/reports/health")
        .await;
    let d = &body["demergers_head_not_continuing"][0];
    assert_eq!(d["action_id"], 1);
    assert_eq!(d["ticker"], "LAC");
    assert_eq!(d["demerger_date"], "2023-10-03");
    assert_eq!(d["demerger_ticker"], "LAR");
    assert_eq!(d["demerged_priced_days"], 1);
    assert_eq!(d["head_held_from"], "2021-03-25");
    // No marker recorded, and no price of its own at all: both are
    // reported as absent rather than the row being withheld.
    assert_eq!(d["head_unpriced_before"], serde_json::Value::Null);
    assert_eq!(d["head_first_price_date"], serde_json::Value::Null);
}

/// The first false positive the predicate has to survive: a database where
/// **no prices have been collected for anything**. Every head listing has
/// no pre-demerger series there, so an absence-only check would light up
/// on a fresh install. The demerged side's provider evidence is what stops
/// it — nothing collected, nothing reported.
#[tokio::test]
async fn an_uncollected_database_reports_no_inverted_demerger() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2021, 3, 25))
        .insert(&pool)
        .await;
    insert_demerger(&pool, 1, ListingId(1), 2, ymd(2023, 10, 3), None).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.demergers_head_not_continuing.is_empty());
}

/// A demerger recorded the right way round: the head is the entity that
/// traded before the event, the demerged listing's series begins at it.
/// This is the shape every correct demerger has, and it must never be
/// reported.
#[tokio::test]
async fn a_correctly_modelled_demerger_is_not_reported() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAR").insert(&pool).await;
    test_support::listing(2)
        .ticker("LAC")
        .unpriced_before(ymd(2023, 10, 3))
        .insert(&pool)
        .await;
    test_support::buy(1, 1)
        .date(ymd(2021, 3, 25))
        .insert(&pool)
        .await;
    for date in ["2021-03-25", "2023-10-02", "2023-10-03"] {
        insert_fetched_price(&pool, ListingId(1), date, "16.85").await;
    }
    insert_fetched_price(&pool, ListingId(2), "2023-10-03", "9.67").await;
    insert_demerger(&pool, 1, ListingId(1), 2, ymd(2023, 10, 3), None).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.demergers_head_not_continuing.is_empty());
}

/// The second false positive: a demerger of a security whose pre-demerger
/// history genuinely was **never backfilled**. Both listings' series begin
/// at the demerger — an ordinary state, not a defect — so there is no
/// asymmetry to report, however long the head has been held.
#[tokio::test]
async fn a_demerger_whose_history_was_never_backfilled_is_not_reported() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2021, 3, 25))
        .insert(&pool)
        .await;
    insert_fetched_price(&pool, ListingId(1), "2023-10-04", "9.67").await;
    insert_fetched_price(&pool, ListingId(2), "2023-10-04", "6.01").await;
    insert_demerger(&pool, 1, ListingId(1), 2, ymd(2023, 10, 3), None).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.demergers_head_not_continuing.is_empty());
}

/// The third clause: a head listing with no pre-demerger series that was
/// **not held** before the demerger either. Nothing is contradicted — the
/// portfolio never claimed to own the security back there — so a listing
/// whose history simply has not been collected is left alone.
#[tokio::test]
async fn a_head_listing_not_held_before_the_demerger_is_not_reported() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    // Acquired on the demerger date, not before it.
    test_support::buy(1, 1)
        .date(ymd(2023, 10, 3))
        .insert(&pool)
        .await;
    insert_fetched_price(&pool, ListingId(2), "2021-03-25", "16.85").await;
    insert_demerger(&pool, 1, ListingId(1), 2, ymd(2023, 10, 3), None).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.demergers_head_not_continuing.is_empty());
}

/// The live data as it stood **before** the 2026-08-21 cleanup: the head
/// listing's pre-demerger gap had been papered over with prices copied
/// from the other listing. This check is silent there, and says so — a
/// stored ok price before the demerger, however it got there, is an
/// assertion that the security traded then.
///
/// The two checks are complements, not alternatives, and this pins the
/// hand-off: while the borrowed rows are there `duplicate_price_series`
/// names them; once they are cleared this check names the modelling error
/// underneath. Between them the incident is visible at both stages.
#[tokio::test]
async fn borrowed_pre_demerger_prices_are_left_to_the_duplicate_series_check() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2021, 3, 25))
        .insert(&pool)
        .await;
    // Thirty consecutive days of the continuing entity's series, fetched
    // under listing 2 and copied by hand onto listing 1 to unblock the
    // snapshots — exactly what was found on the live database.
    for (date, price) in moving_series(ymd(2023, 8, 1), 30, 1685) {
        test_support::closing_price(2, date)
            .price(&price)
            .source(PriceSource::Yahoo)
            .fetched_at("2026-07-26T07:44:56Z")
            .insert(&pool)
            .await;
        test_support::closing_price(1, date)
            .price(&price)
            .fetched_at("2026-07-28T02:00:00Z")
            .manual(
                "listing 2 (LAR) stored close for the same date",
                "demerger-adjusted, so this period is unblocked, not accurate",
            )
            .insert(&pool)
            .await;
    }
    insert_demerger(&pool, 1, ListingId(1), 2, ymd(2023, 10, 3), None).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(
        h.demergers_head_not_continuing.is_empty(),
        "a stored pre-demerger price, even a borrowed one, is an assertion the head traded then"
    );
    assert_eq!(
        h.duplicate_price_series.len(),
        1,
        "the borrowed rows are what the other check names while they are there"
    );

    // Clearing them — the runbook's own step — hands the incident over.
    sqlx::query("DELETE FROM closing_prices WHERE listing_id = 1")
        .execute(&pool)
        .await
        .unwrap();
    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_price_series.is_empty());
    assert_eq!(h.demergers_head_not_continuing.len(), 1);
    assert_eq!(h.demergers_head_not_continuing[0].ticker, "LAC");
}

/// A moving daily price series: `days` consecutive dates from `start`,
/// the close stepping up a cent a day from `first_cents` — a series that
/// *moves*, which is what the check requires before calling a run of
/// matches a copied series.
fn moving_series(start: NaiveDate, days: i64, first_cents: i64) -> Vec<(NaiveDate, String)> {
    (0..days)
        .map(|n| {
            let cents = first_cents + n;
            (
                start + Duration::days(n),
                format!("{}.{:02}", cents / 100, cents % 100),
            )
        })
        .collect()
}

/// Evan's LAC/LAR shape in miniature: listing 1 carries listing 2's price
/// series, half of it hand-entered (copied, with the provenance saying so)
/// and half fetched under a one-off symbol override that recorded nothing.
/// Every row is `ok`, so no other list here sees any of it.
#[tokio::test]
async fn two_listings_holding_one_price_series_are_reported() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    let series = moving_series(ymd(2021, 3, 25), 40, 412);
    for (index, (date, price)) in series.iter().enumerate() {
        // The head listing's copy: the first 15 days typed in by hand…
        let row = test_support::closing_price(1, *date).price(price);
        if index < 15 {
            row.manual(
                "listing 2 (LAR) stored close for the same date",
                "the provider serves no candle here; unblocked, not accurate",
            )
            .insert(&pool)
            .await;
        } else {
            // …the rest fetched, with no record of the symbol used.
            row.source(PriceSource::Yahoo).insert(&pool).await;
        }
        // The other listing's own series, all fetched.
        test_support::closing_price(2, *date)
            .price(price)
            .source(PriceSource::Yahoo)
            .insert(&pool)
            .await;
    }
    // The day the two series part, and — months later — one genuine
    // coincidence: two securities really can close at the same figure.
    for (listing_id, price) in [(1, "20.00"), (2, "6.00")] {
        test_support::closing_price(listing_id, ymd(2021, 6, 1))
            .price(price)
            .source(PriceSource::Yahoo)
            .insert(&pool)
            .await;
    }
    for listing_id in [1, 2] {
        test_support::closing_price(listing_id, ymd(2021, 7, 1))
            .price("4.12")
            .source(PriceSource::Yahoo)
            .insert(&pool)
            .await;
    }

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_price_series.len(), 1);
    let d = &h.duplicate_price_series[0];
    assert_eq!((d.listing_id.0, d.ticker.as_str()), (1, "LAC"));
    assert_eq!((d.other_listing_id.0, d.other_ticker.as_str()), (2, "LAR"));
    assert_eq!(
        d.identical_days, 40,
        "the run, not the total: the later coincidence is not part of it"
    );
    assert_eq!(d.earliest_date, ymd(2021, 3, 25));
    assert_eq!(d.latest_date, ymd(2021, 3, 25) + Duration::days(39));
    // The two halves, which need different remedies: the hand-entered
    // rows say where they came from, the fetched ones say nothing.
    assert_eq!((d.manual_days, d.fetched_days), (15, 25));
    assert_eq!((d.other_manual_days, d.other_fetched_days), (0, 40));
    // Nothing else in health notices: every row is ok, not errored.
    assert!(h.errored_prices.is_empty());
    assert!(h.unpriced_days.is_empty());
}

/// The near-miss that must stay quiet: two distinct securities coinciding.
/// Here they match on 49 days in all — more than the threshold — but never
/// on 30 in a row, which is the only thing that is not a coincidence.
#[tokio::test]
async fn scattered_identical_closes_and_a_short_run_are_not_reported() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("AAA").insert(&pool).await;
    test_support::listing(2).ticker("BBB").insert(&pool).await;
    // A run of 29 identical days — one short of the threshold.
    for (date, price) in moving_series(ymd(2021, 3, 25), 29, 412) {
        for listing_id in [1, 2] {
            test_support::closing_price(listing_id, date)
                .price(&price)
                .source(PriceSource::Yahoo)
                .insert(&pool)
                .await;
        }
    }
    // Plus 60 later days on which every third close happens to agree: 20
    // more matches, none of them consecutive. The first of these days
    // disagrees, which is what ends the run above — a *disagreement* ends
    // a run, never a gap in the dates, since a day only one listing has a
    // price for is not a comparison at all.
    for (index, (date, price)) in moving_series(ymd(2022, 1, 1), 60, 500)
        .into_iter()
        .enumerate()
    {
        test_support::closing_price(1, date)
            .price(&price)
            .source(PriceSource::Yahoo)
            .insert(&pool)
            .await;
        let other = if index % 3 == 1 {
            price
        } else {
            format!("9{price}")
        };
        test_support::closing_price(2, date)
            .price(&other)
            .source(PriceSource::Yahoo)
            .insert(&pool)
            .await;
    }

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(
        h.duplicate_price_series.is_empty(),
        "49 matching days that never run 30 deep are a coincidence, not one series: {:?}",
        h.duplicate_price_series
    );
}

/// A day only one listing holds a price for is not a comparison: it
/// neither breaks the run nor counts towards it.
#[tokio::test]
async fn a_day_only_one_listing_has_a_price_for_does_not_break_the_run() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    let series = moving_series(ymd(2021, 3, 25), 35, 412);
    let gap = series[17].0;
    for (date, price) in &series {
        test_support::closing_price(1, *date)
            .price(price)
            .source(PriceSource::Yahoo)
            .insert(&pool)
            .await;
        if *date != gap {
            test_support::closing_price(2, *date)
                .price(price)
                .source(PriceSource::Yahoo)
                .insert(&pool)
                .await;
        }
    }

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_price_series.len(), 1);
    let d = &h.duplicate_price_series[0];
    assert_eq!(
        d.identical_days, 34,
        "34 comparisons, the missing day not among them"
    );
    assert_eq!(d.earliest_date, series[0].0);
    assert_eq!(d.latest_date, series[34].0, "the run spans the gap");
}

/// Two instruments pinned to one figure — a pair of stablecoins at 1.00, two
/// funds at a constant unit price — match forever without either series
/// being a copy of the other. A run whose closes never move is not
/// reported, because an alarm nobody can ever clear is worse than none.
#[tokio::test]
async fn two_listings_pinned_to_one_price_are_not_reported() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("USDT").insert(&pool).await;
    test_support::listing(2).ticker("USDC").insert(&pool).await;
    let dates: Vec<NaiveDate> = (0..200)
        .map(|n| ymd(2021, 3, 25) + Duration::days(n))
        .collect();
    for date in &dates {
        for listing_id in [1, 2] {
            test_support::closing_price(listing_id, *date)
                .price("1.00")
                .source(PriceSource::Yahoo)
                .insert(&pool)
                .await;
        }
    }
    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(
        h.duplicate_price_series.is_empty(),
        "200 identical days that never move are two pinned instruments"
    );

    // The moment the shared series *moves* — and both still agree — it is
    // a series, not a peg, and the run is reported.
    for listing_id in [1, 2] {
        test_support::closing_price(listing_id, dates[100])
            .price("1.01")
            .source(PriceSource::Yahoo)
            .insert(&pool)
            .await;
    }
    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_price_series.len(), 1);
    assert_eq!(h.duplicate_price_series[0].identical_days, 200);
}

/// The check owns no marker of its own, like the rest of the `duplicate_*`
/// family: `unpriced_before` does not silence it — that marker says the
/// provider serves nothing before the date, which is a reason to *delete*
/// the borrowed rows, not to accept them. Clearing them is what clears the
/// warning, and it is served over the endpoint the banner polls.
#[tokio::test]
async fn api_the_warning_clears_when_the_borrowed_rows_go_not_when_the_marker_is_set() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("LAC").insert(&pool).await;
    test_support::listing(2).ticker("LAR").insert(&pool).await;
    for (date, price) in moving_series(ymd(2021, 3, 25), 40, 412) {
        for listing_id in [1, 2] {
            test_support::closing_price(listing_id, date)
                .price(&price)
                .source(PriceSource::Yahoo)
                .insert(&pool)
                .await;
        }
    }
    let client = ApiClient::over(router().with_state(pool.clone()));
    let body: serde_json::Value = client.get("/reports/health").await.json();
    let d = &body["duplicate_price_series"][0];
    assert_eq!(d["ticker"], "LAC");
    assert_eq!(d["other_ticker"], "LAR");
    assert_eq!(d["identical_days"], 40);
    assert_eq!(d["earliest_date"], "2021-03-25");
    assert_eq!(d["fetched_days"], 40);
    assert_eq!(d["manual_days"], 0);

    // Declaring the span unpriceable leaves the rows — and the warning.
    test_support::listing(1)
        .ticker("LAC")
        .with(|l| l.unpriced_before = Some(ymd(2021, 6, 1)))
        .insert(&pool)
        .await;
    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(
        h.duplicate_price_series.len(),
        1,
        "the marker explains an absence, not a stored figure that is another security's"
    );

    // Clearing the span is what clears it.
    assert_eq!(
        closing_price::db_clear_unpriced_before(&pool, ListingId(1))
            .await
            .unwrap(),
        closing_price::ClearOutcome::Cleared {
            unpriced_before: ymd(2021, 6, 1),
            deleted: 40,
        }
    );
    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_price_series.is_empty());
}

async fn insert_demerger(
    pool: &SqlitePool,
    id: i64,
    listing_id: ListingId,
    demerged_id: i64,
    date: NaiveDate,
    stated_close: Option<(NaiveDate, &str)>,
) {
    corporate_action::db_upsert(
        pool,
        &corporate_action::CorporateAction {
            id,
            listing_id,
            date,
            kind: corporate_action::ActionKind::Demerger {
                demerger_listing_id: ListingId(demerged_id),
                demerger_new_units: dec("1"),
                demerger_held_units: dec("1"),
                demerger_cost_base_pct: dec("36"),
                demerger_close_date: stated_close.map(|(d, _)| d),
                demerger_close_price: stated_close.map(|(_, p)| dec(p)),
                demerger_close_sourced_from: stated_close
                    .map(|_| "nyse.com daily close".to_string()),
                demerger_close_reason: stated_close
                    .map(|_| "the provider adjusts the pre-demerger series".to_string()),
            },
        },
    )
    .await
    .unwrap();
}

/// A Buy of `listing_id` on `date` carrying `note` as its contract note
/// reference — the shape a re-keyed broker confirmation takes. `None` is
/// what every derived path, and a hand entry that skipped the field,
/// leaves behind.
async fn insert_trade_with_note(
    pool: &SqlitePool,
    id: i64,
    listing_id: ListingId,
    date: NaiveDate,
    note: Option<&str>,
) {
    test_support::buy(id, listing_id.0)
        .date(date)
        .with(|t| t.contract_note_ref = note.map(str::to_string))
        .insert(pool)
        .await;
}

/// SCENARIOS V-09: a whole portfolio's history back-entered in one
/// session, with one contract note keyed twice. Both rows are individually
/// valid — nothing rejects them — and the doubled Buy inflates the holding
/// and its cost base everywhere until the holdings are reconciled against
/// a registry statement. The shared reference is the evidence, so health
/// names it with both ids.
#[tokio::test]
async fn trades_sharing_a_contract_note_reference_are_reported_with_their_ids() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("BHP").insert(&pool).await;
    test_support::listing(2).ticker("CBA").insert(&pool).await;
    // The measured case: the same confirmation entered twice.
    insert_trade_with_note(&pool, 1, ListingId(1), ymd(2026, 3, 10), Some("CN-8891")).await;
    insert_trade_with_note(&pool, 2, ListingId(1), ymd(2026, 3, 10), Some("CN-8891")).await;
    // The same mistake later, on another holding.
    insert_trade_with_note(&pool, 3, ListingId(2), ymd(2026, 6, 1), Some("CN-9002")).await;
    insert_trade_with_note(&pool, 4, ListingId(2), ymd(2026, 6, 1), Some("CN-9002")).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    // Newest first, as on the rest of the family.
    assert_eq!(h.duplicate_trades.len(), 2);
    let june = &h.duplicate_trades[0];
    assert_eq!(june.ticker, "CBA");
    assert_eq!(june.listing_id, 2);
    assert_eq!(june.contract_note_ref, "CN-9002");
    assert_eq!(june.date, ymd(2026, 6, 1));
    assert_eq!(june.trade_count, 2);
    assert_eq!(june.trade_ids, vec![3, 4]);
    let march = &h.duplicate_trades[1];
    assert_eq!(march.ticker, "BHP");
    assert_eq!(march.contract_note_ref, "CN-8891");
    assert_eq!(march.date, ymd(2026, 3, 10));
    assert_eq!(march.trade_ids, vec![1, 2]);
}

/// The check is on the reference, never on the figures: trades that record
/// none are invisible to it however identical they are. That is the honest
/// cost of a list with no false positives — and it is why the same-day,
/// same-price repeats real trading produces (an order filled in equal
/// clips) are not flagged either.
#[tokio::test]
async fn trades_recording_no_contract_note_are_never_duplicates() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("BHP").insert(&pool).await;
    // Identical in every other respect — listing, date, account, price and
    // quantity all match, which is the key `duplicate_income` uses.
    for id in 1..=3 {
        insert_trade_with_note(&pool, id, ListingId(1), ymd(2026, 3, 10), None).await;
    }

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_trades.is_empty());
}

/// A reference that is empty or only whitespace is no reference at all: it
/// is what a form submitted with the field untouched can leave, and
/// grouping on it would flag every such row against every other.
#[tokio::test]
async fn a_blank_or_whitespace_only_reference_never_groups() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("BHP").insert(&pool).await;
    insert_trade_with_note(&pool, 1, ListingId(1), ymd(2026, 3, 10), Some("")).await;
    insert_trade_with_note(&pool, 2, ListingId(1), ymd(2026, 3, 10), Some("")).await;
    insert_trade_with_note(&pool, 3, ListingId(1), ymd(2026, 3, 10), Some("   ")).await;
    insert_trade_with_note(&pool, 4, ListingId(1), ymd(2026, 3, 10), Some("\t")).await;
    // …and a blank one does not group with a null one either.
    insert_trade_with_note(&pool, 5, ListingId(1), ymd(2026, 3, 10), None).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_trades.is_empty());
}

/// Padding is how the same reference pasted twice usually differs, so the
/// comparison is on the trimmed text — and the reported reference is the
/// trimmed one, which is what the broker's records are searched by.
#[tokio::test]
async fn a_reference_pasted_with_surrounding_whitespace_still_groups() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("BHP").insert(&pool).await;
    insert_trade_with_note(&pool, 1, ListingId(1), ymd(2026, 3, 10), Some("CN-8891")).await;
    insert_trade_with_note(&pool, 2, ListingId(1), ymd(2026, 3, 10), Some("  CN-8891 ")).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_trades.len(), 1);
    assert_eq!(h.duplicate_trades[0].contract_note_ref, "CN-8891");
    assert_eq!(h.duplicate_trades[0].trade_ids, vec![1, 2]);
}

/// The listing is part of the key, because one contract note can cover a
/// multi-line order: two securities bought under one confirmation share a
/// reference legitimately. Different references on one listing, and
/// references differing only in case, are likewise two documents.
#[tokio::test]
async fn one_reference_across_two_listings_is_a_multi_line_note_not_a_duplicate() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("BHP").insert(&pool).await;
    test_support::listing(2).ticker("CBA").insert(&pool).await;
    // One note, two lines.
    insert_trade_with_note(&pool, 1, ListingId(1), ymd(2026, 3, 10), Some("CN-8891")).await;
    insert_trade_with_note(&pool, 2, ListingId(2), ymd(2026, 3, 10), Some("CN-8891")).await;
    // Two notes on one listing, same day: an order filled in two clips.
    insert_trade_with_note(&pool, 3, ListingId(1), ymd(2026, 3, 11), Some("CN-8892")).await;
    insert_trade_with_note(&pool, 4, ListingId(1), ymd(2026, 3, 11), Some("CN-8893")).await;
    // Case is part of the reference, not noise to normalise away.
    insert_trade_with_note(&pool, 5, ListingId(1), ymd(2026, 3, 12), Some("cn-8891")).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_trades.is_empty(), "{:?}", h.duplicate_trades);
}

/// The same note re-keyed against the wrong holding account is exactly the
/// double entry this exists for, so the account is **not** part of the
/// key — a per-account key would let the worst version of the mistake
/// through.
#[tokio::test]
async fn one_reference_keyed_into_two_accounts_is_still_a_duplicate() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("BHP").insert(&pool).await;
    crate::entities::holding_account::db_upsert(
        &pool,
        &crate::entities::holding_account::HoldingAccount {
            id: 2,
            name: "Second".to_string(),
        },
    )
    .await
    .unwrap();
    insert_trade_with_note(&pool, 1, ListingId(1), ymd(2026, 3, 10), Some("CN-8891")).await;
    test_support::buy(2, 1)
        .date(ymd(2026, 3, 10))
        .account(2)
        .with(|t| t.contract_note_ref = Some("CN-8891".to_string()))
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_trades.len(), 1);
    assert_eq!(h.duplicate_trades[0].trade_ids, vec![1, 2]);
}

/// Three of a kind is one row counting three, as on the rest of the
/// family.
#[tokio::test]
async fn three_trades_sharing_a_reference_are_one_row_counting_three() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("TRIP").insert(&pool).await;
    for id in 7..=9 {
        insert_trade_with_note(&pool, id, ListingId(1), ymd(2026, 3, 10), Some("CN-8891")).await;
    }

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_trades.len(), 1);
    assert_eq!(h.duplicate_trades[0].trade_count, 3);
    assert_eq!(h.duplicate_trades[0].trade_ids, vec![7, 8, 9]);
}

/// Trades a derived path writes carry no contract note reference at any
/// path — there is no broker confirmation behind a vest, an inherited
/// parcel, a DRP or a corporate action — so they fall out of this check by
/// construction rather than by being excluded by name. Two identical
/// inheritances create two identical parcel Buys: a real duplicate, named
/// by `duplicate_inheritances`, which is where it is fixed.
#[tokio::test]
async fn derived_trades_carry_no_reference_and_are_never_reported_here() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("BHP").insert(&pool).await;
    insert_inheritance(&pool, 1, ListingId(1), 1, "100", "3000").await;
    insert_inheritance(&pool, 2, ListingId(1), 1, "100", "3000").await;
    let derived: Vec<Option<String>> =
        sqlx::query_scalar("SELECT contract_note_ref FROM trades ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(derived, vec![None, None]);

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_trades.is_empty());
    // The duplication is real; it is the inheritance list that owns it.
    assert_eq!(h.duplicate_inheritances.len(), 1);
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
    insert_roc(&pool, 1, ListingId(1), ymd(2026, 3, 10)).await;
    insert_roc(&pool, 2, ListingId(1), ymd(2026, 3, 10)).await;
    insert_split(&pool, 3, ListingId(2), ymd(2026, 6, 1)).await;
    insert_split(&pool, 4, ListingId(2), ymd(2026, 6, 1)).await;

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
    insert_roc(&pool, 1, ListingId(1), ymd(2026, 3, 10)).await;
    insert_roc(&pool, 2, ListingId(2), ymd(2026, 3, 10)).await;
    // Same listing and type, different date.
    insert_roc(&pool, 3, ListingId(1), ymd(2026, 9, 10)).await;
    // Same listing and date, different type.
    insert_split(&pool, 4, ListingId(1), ymd(2026, 3, 10)).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_actions.is_empty());
}

/// Three of a kind is one row, not three: the report answers "this
/// (listing, type, date) is entered N times", listing every id.
#[tokio::test]
async fn three_identical_actions_are_one_row_counting_three() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("TRIP").insert(&pool).await;
    insert_roc(&pool, 7, ListingId(1), ymd(2026, 3, 10)).await;
    insert_roc(&pool, 8, ListingId(1), ymd(2026, 3, 10)).await;
    insert_roc(&pool, 9, ListingId(1), ymd(2026, 3, 10)).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_actions.len(), 1);
    assert_eq!(h.duplicate_actions[0].action_count, 3);
    assert_eq!(h.duplicate_actions[0].action_ids, vec![7, 8, 9]);
}

/// SCENARIOS F-06: an amended AMMA statement entered as a second row
/// instead of over the original. Both are counted by every reader, so the
/// pair is named — newest year first, with the ids ascending.
#[tokio::test]
async fn duplicated_amma_statements_are_reported_with_their_ids() {
    let pool = test_pool().await;
    amit_listing(&pool, 1, "VDHG").await;
    amit_listing(&pool, 2, "HNDQ").await;
    // VDHG FY2025: the original and its amendment.
    insert_amma(&pool, 1, ListingId(1), ymd(2025, 6, 30), 1).await;
    insert_amma(&pool, 2, ListingId(1), ymd(2025, 6, 30), 1).await;
    // HNDQ FY2024: the same mistake a year earlier.
    insert_amma(&pool, 3, ListingId(2), ymd(2024, 6, 30), 1).await;
    insert_amma(&pool, 4, ListingId(2), ymd(2024, 6, 30), 1).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_amma_statements.len(), 2);
    let fy25 = &h.duplicate_amma_statements[0];
    assert_eq!(fy25.ticker, "VDHG");
    assert_eq!(fy25.listing_id, 1);
    assert_eq!(fy25.tax_year, 2025);
    assert_eq!(fy25.holding_account_id, 1);
    assert_eq!(fy25.statement_count, 2);
    assert_eq!(fy25.statement_ids, vec![1, 2]);
    let fy24 = &h.duplicate_amma_statements[1];
    assert_eq!(fy24.ticker, "HNDQ");
    assert_eq!(fy24.tax_year, 2024);
    assert_eq!(fy24.statement_ids, vec![3, 4]);
}

/// The key is all three parts. Two accounts of one fund-year are the
/// legitimate case the warning must stay silent on (SCENARIOS F-03: a
/// registry issues one statement per holder account), and two years or
/// two listings are ordinary entry.
#[tokio::test]
async fn statements_differing_in_listing_year_or_account_are_not_duplicates() {
    let pool = test_pool().await;
    amit_listing(&pool, 1, "VDHG").await;
    amit_listing(&pool, 2, "HNDQ").await;
    crate::entities::holding_account::db_upsert(
        &pool,
        &crate::entities::holding_account::HoldingAccount {
            id: 2,
            name: "Second".to_string(),
        },
    )
    .await
    .unwrap();
    // Same listing and year, different holding account.
    insert_amma(&pool, 1, ListingId(1), ymd(2025, 6, 30), 1).await;
    insert_amma(&pool, 2, ListingId(1), ymd(2025, 6, 30), 2).await;
    // Same listing and account, different year.
    insert_amma(&pool, 3, ListingId(1), ymd(2024, 6, 30), 1).await;
    // Same year and account, different listing.
    insert_amma(&pool, 4, ListingId(2), ymd(2025, 6, 30), 1).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_amma_statements.is_empty());
}

/// Three of a kind is one row counting three, as on the actions side.
#[tokio::test]
async fn three_statements_for_one_fund_year_are_one_row_counting_three() {
    let pool = test_pool().await;
    amit_listing(&pool, 1, "VDHG").await;
    insert_amma(&pool, 7, ListingId(1), ymd(2025, 6, 30), 1).await;
    insert_amma(&pool, 8, ListingId(1), ymd(2025, 6, 30), 1).await;
    insert_amma(&pool, 9, ListingId(1), ymd(2025, 6, 30), 1).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_amma_statements.len(), 1);
    assert_eq!(h.duplicate_amma_statements[0].statement_count, 3);
    assert_eq!(h.duplicate_amma_statements[0].statement_ids, vec![7, 8, 9]);
}

/// A fully franked dividend of `franked` gross, paid on `date` into
/// `account` — the shape a duplicated statement entry takes.
async fn insert_dividend(
    pool: &SqlitePool,
    id: i64,
    listing_id: ListingId,
    date: NaiveDate,
    account: i64,
    franked: &str,
) {
    insert_dividend_amount(pool, id, listing_id, date, account, dec(franked)).await;
}

/// As [`insert_dividend`], taking the franked amount as a `Decimal` so a
/// test can control its scale (`10.0` vs `10.00`).
async fn insert_dividend_amount(
    pool: &SqlitePool,
    id: i64,
    listing_id: ListingId,
    date: NaiveDate,
    account: i64,
    franked: Decimal,
) {
    test_support::income(id, listing_id.0, date)
        .with(|i| {
            i.holding_account_id = account;
            i.franked_amount = franked;
        })
        .insert(pool)
        .await;
}

/// SCENARIOS G-24: a re-submitted form or a re-imported statement leaves
/// two identical dividends, and every reader counts both — twice the
/// dividend income, twice the franking credits. Nothing rejects the pair
/// (two dividends from one company on one day are legitimate in
/// principle), so health names it, with the ids to delete from.
#[tokio::test]
async fn duplicated_income_rows_are_reported_with_their_ids() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("DIVA").insert(&pool).await;
    test_support::listing(2).ticker("DIVB").insert(&pool).await;
    insert_dividend(&pool, 1, ListingId(1), ymd(2026, 3, 10), 1, "70").await;
    insert_dividend(&pool, 2, ListingId(1), ymd(2026, 3, 10), 1, "70").await;
    insert_dividend(&pool, 3, ListingId(2), ymd(2026, 6, 1), 1, "42.50").await;
    insert_dividend(&pool, 4, ListingId(2), ymd(2026, 6, 1), 1, "42.50").await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    // Newest first: June before March, as on the other two lists.
    assert_eq!(h.duplicate_income.len(), 2);
    let june = &h.duplicate_income[0];
    assert_eq!(june.ticker, "DIVB");
    assert_eq!(june.listing_id, 2);
    assert_eq!(june.holding_account_id, 1);
    assert_eq!(june.date_paid, ymd(2026, 6, 1));
    assert_eq!(june.currency, "AUD");
    assert_eq!(june.gross_amount, dec("42.50"));
    assert_eq!(june.income_count, 2);
    assert_eq!(june.income_ids, vec![3, 4]);
    let march = &h.duplicate_income[1];
    assert_eq!(march.ticker, "DIVA");
    assert_eq!(march.date_paid, ymd(2026, 3, 10));
    assert_eq!(march.gross_amount, dec("70"));
    assert_eq!(march.income_ids, vec![1, 2]);
}

/// The key is all four parts. Two payments differing in the amount are the
/// legitimate case the warning must stay silent on — an ordinary and a
/// special dividend paid the same day — and a different listing, account
/// or date is ordinary entry.
#[tokio::test]
async fn income_differing_in_listing_account_date_or_amount_is_not_a_duplicate() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("AAA").insert(&pool).await;
    test_support::listing(2).ticker("BBB").insert(&pool).await;
    crate::entities::holding_account::db_upsert(
        &pool,
        &crate::entities::holding_account::HoldingAccount {
            id: 2,
            name: "Second".to_string(),
        },
    )
    .await
    .unwrap();
    // Same account, date and amount, different listing.
    insert_dividend(&pool, 1, ListingId(1), ymd(2026, 3, 10), 1, "70").await;
    insert_dividend(&pool, 2, ListingId(2), ymd(2026, 3, 10), 1, "70").await;
    // Same listing, date and amount, different holding account.
    insert_dividend(&pool, 3, ListingId(1), ymd(2026, 3, 10), 2, "70").await;
    // Same listing, account and amount, different date.
    insert_dividend(&pool, 4, ListingId(1), ymd(2026, 9, 10), 1, "70").await;
    // Same listing, account and date, different amount: the ordinary +
    // special pair.
    insert_dividend(&pool, 5, ListingId(1), ymd(2026, 3, 10), 1, "12.34").await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_income.is_empty());
}

/// The amounts are compared as decimals, not as the TEXT they are stored
/// as: `70.0` and `70.00` are the same dollars entered twice, however the
/// two clients wrote them.
#[tokio::test]
async fn amounts_equal_in_value_but_not_in_text_are_still_duplicates() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("DIVA").insert(&pool).await;
    insert_dividend_amount(&pool, 1, ListingId(1), ymd(2026, 3, 10), 1, dec("70.0")).await;
    insert_dividend_amount(&pool, 2, ListingId(1), ymd(2026, 3, 10), 1, dec("70.00")).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_income.len(), 1);
    assert_eq!(h.duplicate_income[0].income_ids, vec![1, 2]);
}

/// Three of a kind is one row counting three, as on the other two lists.
#[tokio::test]
async fn three_identical_income_rows_are_one_row_counting_three() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("TRIP").insert(&pool).await;
    insert_dividend(&pool, 7, ListingId(1), ymd(2026, 3, 10), 1, "70").await;
    insert_dividend(&pool, 8, ListingId(1), ymd(2026, 3, 10), 1, "70").await;
    insert_dividend(&pool, 9, ListingId(1), ymd(2026, 3, 10), 1, "70").await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_income.len(), 1);
    assert_eq!(h.duplicate_income[0].income_count, 3);
    assert_eq!(h.duplicate_income[0].income_ids, vec![7, 8, 9]);
}

/// One same-day cluster can hold both a duplicated pair and an unrelated
/// payment: the grouping is per amount fingerprint, not per day, so the
/// third row neither joins the pair nor suppresses it.
#[tokio::test]
async fn a_duplicated_pair_is_reported_beside_a_genuine_second_dividend() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("DIVA").insert(&pool).await;
    insert_dividend(&pool, 1, ListingId(1), ymd(2026, 3, 10), 1, "70").await;
    insert_dividend(&pool, 2, ListingId(1), ymd(2026, 3, 10), 1, "70").await;
    // The special dividend paid the same day.
    insert_dividend(&pool, 3, ListingId(1), ymd(2026, 3, 10), 1, "12.34").await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_income.len(), 1);
    assert_eq!(h.duplicate_income[0].gross_amount, dec("70"));
    assert_eq!(h.duplicate_income[0].income_ids, vec![1, 2]);
}

// The two listing-less sides of the tax summary (SCENARIOS H-01, H-06).

async fn insert_interest(
    pool: &SqlitePool,
    id: i64,
    date: NaiveDate,
    amount: Decimal,
    source: Option<&str>,
) {
    crate::entities::interest_income::db_upsert(
        pool,
        &crate::entities::interest_income::InterestIncome {
            id,
            date_paid: date,
            amount,
            tfn_withholding_tax: Decimal::ZERO,
            foreign_source: false,
            foreign_tax_paid: Decimal::ZERO,
            currency: "AUD".into(),
            source: source.map(str::to_string),
            holding_account_id: None,
        },
    )
    .await
    .unwrap();
}

fn an_expense(
    id: i64,
    date: NaiveDate,
    expense_type: ExpenseType,
    amount: Decimal,
) -> InvestmentExpense {
    InvestmentExpense {
        id,
        date_incurred: date,
        expense_type,
        amount,
        gross_amount: None,
        deductible_percentage: None,
        currency: "AUD".into(),
        description: None,
        listing_id: None,
        holding_account_id: None,
    }
}

async fn insert_expense(pool: &SqlitePool, expense: &InvestmentExpense) {
    crate::entities::investment_expense::db_upsert(pool, expense)
        .await
        .unwrap();
}

/// A term-deposit credit keyed twice doubles the year's gross interest and
/// nothing else notices, so the health report names the pair by id.
#[tokio::test]
async fn duplicated_interest_rows_are_reported_with_their_ids() {
    let pool = test_pool().await;
    insert_interest(&pool, 1, ymd(2026, 3, 10), dec("250"), Some("ANZ savings")).await;
    insert_interest(&pool, 2, ymd(2026, 3, 10), dec("250"), Some("ANZ savings")).await;
    insert_interest(&pool, 3, ymd(2026, 6, 30), dec("500"), Some("Term deposit")).await;
    insert_interest(&pool, 4, ymd(2026, 6, 30), dec("500"), Some("Term deposit")).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    // Newest first, as on every other duplicate list.
    assert_eq!(h.duplicate_interest.len(), 2);
    let june = &h.duplicate_interest[0];
    assert_eq!(june.date_paid, ymd(2026, 6, 30));
    assert_eq!(june.amount, dec("500"));
    assert_eq!(june.currency, "AUD");
    assert_eq!(june.source.as_deref(), Some("Term deposit"));
    assert_eq!(june.holding_account_id, None);
    assert_eq!(june.interest_count, 2);
    assert_eq!(june.interest_ids, vec![3, 4]);
    let march = &h.duplicate_interest[1];
    assert_eq!(march.amount, dec("250"));
    assert_eq!(march.source.as_deref(), Some("ANZ savings"));
    assert_eq!(march.interest_ids, vec![1, 2]);
}

/// Interest has no listing, so `source` and the holding account carry the
/// payer identity: two $250 credits on one day from different accounts are
/// legitimate and stay unflagged, as do rows differing in date, amount, or
/// any withholding figure.
#[tokio::test]
async fn interest_differing_in_any_key_field_is_not_a_duplicate() {
    let pool = test_pool().await;
    crate::entities::holding_account::db_upsert(
        &pool,
        &crate::entities::holding_account::HoldingAccount {
            id: 2,
            name: "Second".to_string(),
        },
    )
    .await
    .unwrap();
    // Same date and amount, different source: two banks, one day.
    insert_interest(&pool, 1, ymd(2026, 3, 10), dec("250"), Some("ANZ savings")).await;
    insert_interest(&pool, 2, ymd(2026, 3, 10), dec("250"), Some("CBA savings")).await;
    // Same source and amount, different date.
    insert_interest(&pool, 3, ymd(2026, 9, 10), dec("250"), Some("ANZ savings")).await;
    // Same source and date, different amount.
    insert_interest(
        &pool,
        4,
        ymd(2026, 3, 10),
        dec("12.34"),
        Some("ANZ savings"),
    )
    .await;
    // Same date, amount and source, different holding account.
    crate::entities::interest_income::db_upsert(
        &pool,
        &crate::entities::interest_income::InterestIncome {
            id: 5,
            date_paid: ymd(2026, 3, 10),
            amount: dec("250"),
            tfn_withholding_tax: Decimal::ZERO,
            foreign_source: false,
            foreign_tax_paid: Decimal::ZERO,
            currency: "AUD".into(),
            source: Some("ANZ savings".to_string()),
            holding_account_id: Some(2),
        },
    )
    .await
    .unwrap();
    // Same everything but the TFN amount withheld — one row was keyed from
    // a statement that showed the withholding, so they are different rows.
    crate::entities::interest_income::db_upsert(
        &pool,
        &crate::entities::interest_income::InterestIncome {
            id: 6,
            date_paid: ymd(2026, 3, 10),
            amount: dec("250"),
            tfn_withholding_tax: dec("117.50"),
            foreign_source: false,
            foreign_tax_paid: Decimal::ZERO,
            currency: "AUD".into(),
            source: Some("ANZ savings".to_string()),
            holding_account_id: None,
        },
    )
    .await
    .unwrap();

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_interest.is_empty());
}

/// Grouped on decimal value, not on the stored TEXT — and three of a kind
/// is one row counting three, as on every other list.
#[tokio::test]
async fn interest_amounts_equal_in_value_but_not_in_text_are_still_duplicates() {
    let pool = test_pool().await;
    insert_interest(&pool, 1, ymd(2026, 3, 10), dec("250.0"), Some("ANZ")).await;
    insert_interest(&pool, 2, ymd(2026, 3, 10), dec("250.00"), Some("ANZ")).await;
    insert_interest(&pool, 3, ymd(2026, 3, 10), dec("250.000"), Some("ANZ")).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_interest.len(), 1);
    assert_eq!(h.duplicate_interest[0].interest_count, 3);
    assert_eq!(h.duplicate_interest[0].interest_ids, vec![1, 2, 3]);
}

/// A re-submitted expense form claims the deduction twice, lowering the
/// year's net assessable investment income by the same amount again.
#[tokio::test]
async fn duplicated_expense_rows_are_reported_with_their_ids() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("FEEA").insert(&pool).await;
    for id in [1, 2] {
        let mut e = an_expense(id, ymd(2026, 3, 10), ExpenseType::AdviceFee, dec("200"));
        e.listing_id = Some(ListingId(1));
        e.description = Some("Annual advice fee".to_string());
        insert_expense(&pool, &e).await;
    }
    for id in [3, 4] {
        insert_expense(
            &pool,
            &an_expense(id, ymd(2026, 6, 1), ExpenseType::LoanInterest, dec("1500")),
        )
        .await;
    }

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_expenses.len(), 2);
    let june = &h.duplicate_expenses[0];
    assert_eq!(june.date_incurred, ymd(2026, 6, 1));
    assert_eq!(june.expense_type, ExpenseType::LoanInterest);
    assert_eq!(june.amount, dec("1500"));
    assert_eq!(june.currency, "AUD");
    // A portfolio-wide expense names no holding.
    assert_eq!(june.listing_id, None);
    assert_eq!(june.ticker, None);
    assert_eq!(june.expense_ids, vec![3, 4]);
    let march = &h.duplicate_expenses[1];
    assert_eq!(march.expense_type, ExpenseType::AdviceFee);
    assert_eq!(march.amount, dec("200"));
    assert_eq!(march.listing_id, Some(ListingId(1)));
    // …and a listing-attributed one is named by ticker, not only by id.
    assert_eq!(march.ticker.as_deref(), Some("FEEA"));
    assert_eq!(march.description.as_deref(), Some("Annual advice fee"));
    assert_eq!(march.expense_count, 2);
    assert_eq!(march.expense_ids, vec![1, 2]);
}

/// Everything that identifies the expense is in the key: two advice fees of
/// the same amount on one day against different listings — or of different
/// types, amounts, descriptions, or apportionment provenance — are ordinary
/// entry, not a double claim.
#[tokio::test]
async fn expenses_differing_in_any_key_field_are_not_duplicates() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("AAA").insert(&pool).await;
    test_support::listing(2).ticker("BBB").insert(&pool).await;
    let base = an_expense(0, ymd(2026, 3, 10), ExpenseType::AdviceFee, dec("200"));
    // Same date, type and amount, different listing.
    let mut a = base.clone();
    a.id = 1;
    a.listing_id = Some(ListingId(1));
    insert_expense(&pool, &a).await;
    let mut b = base.clone();
    b.id = 2;
    b.listing_id = Some(ListingId(2));
    insert_expense(&pool, &b).await;
    // Same listing-less row, different type.
    let mut c = base.clone();
    c.id = 3;
    c.expense_type = ExpenseType::ManagementFee;
    insert_expense(&pool, &c).await;
    // …different amount.
    let mut d = base.clone();
    d.id = 4;
    d.amount = dec("12.34");
    insert_expense(&pool, &d).await;
    // …different description: one invoice per quarter, keyed the same day.
    let mut e = base.clone();
    e.id = 5;
    e.description = Some("Q1".to_string());
    insert_expense(&pool, &e).await;
    let mut f = base.clone();
    f.id = 6;
    f.description = Some("Q2".to_string());
    insert_expense(&pool, &f).await;
    // …and one carrying the apportionment provenance the other lacks.
    let mut g = base.clone();
    g.id = 7;
    g.gross_amount = Some(dec("400"));
    g.deductible_percentage = Some(dec("50"));
    insert_expense(&pool, &g).await;
    // …plus one on another date entirely.
    let mut i = base.clone();
    i.id = 8;
    i.date_incurred = ymd(2026, 9, 10);
    insert_expense(&pool, &i).await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_expenses.is_empty());
}

// The employee-share-scheme side (SCENARIOS J-11).

/// A statement that vests `quantity` shares worth `market_value` each, all
/// of it a deferral-scheme (label F) discount — the RSU shape.
async fn insert_ess(
    pool: &SqlitePool,
    id: i64,
    listing_id: ListingId,
    taxing_point: NaiveDate,
    quantity: &str,
    market_value: &str,
) {
    test_support::ess_statement(id, listing_id.0, taxing_point)
        .with(|s| {
            s.quantity = dec(quantity);
            s.market_value_per_share = dec(market_value);
            s.deferral_discount = dec(quantity) * dec(market_value);
        })
        .insert(pool)
        .await;
}

/// The same statement entered twice — the accident the 30-day rule makes
/// likely, an amended employer statement keyed as a new row — doubles the
/// year's Item 12 discount and vests the parcel twice. Reported with both
/// ids, newest taxing point first.
#[tokio::test]
async fn duplicated_ess_statements_are_reported_with_their_ids() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("EMPA").insert(&pool).await;
    test_support::listing(2).ticker("EMPB").insert(&pool).await;
    insert_ess(&pool, 1, ListingId(1), ymd(2026, 3, 10), "100", "10").await;
    insert_ess(&pool, 2, ListingId(1), ymd(2026, 3, 10), "100", "10").await;
    insert_ess(&pool, 3, ListingId(2), ymd(2026, 6, 1), "50", "4.25").await;
    insert_ess(&pool, 4, ListingId(2), ymd(2026, 6, 1), "50", "4.25").await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    // Newest first, as on the other duplicate lists.
    assert_eq!(h.duplicate_ess_statements.len(), 2);
    let june = &h.duplicate_ess_statements[0];
    assert_eq!(june.ticker, "EMPB");
    assert_eq!(june.listing_id, 2);
    assert_eq!(june.holding_account_id, 1);
    assert_eq!(june.taxing_point_date, ymd(2026, 6, 1));
    assert_eq!(june.currency, "AUD");
    assert_eq!(june.quantity, dec("50"));
    assert_eq!(june.discount_total, dec("212.50"));
    assert_eq!(june.statement_count, 2);
    assert_eq!(june.statement_ids, vec![3, 4]);
    let march = &h.duplicate_ess_statements[1];
    assert_eq!(march.ticker, "EMPA");
    assert_eq!(march.taxing_point_date, ymd(2026, 3, 10));
    assert_eq!(march.discount_total, dec("1000"));
    assert_eq!(march.statement_ids, vec![1, 2]);
}

/// The legitimate case must stay silent: two vests on one date from
/// different grants are ordinary, and so is a different listing, account or
/// taxing point. The figures are part of the key for exactly that reason —
/// a different quantity or a different discount is a second grant.
#[tokio::test]
async fn ess_statements_differing_in_any_key_field_are_not_duplicates() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("AAA").insert(&pool).await;
    test_support::listing(2).ticker("BBB").insert(&pool).await;
    crate::entities::holding_account::db_upsert(
        &pool,
        &crate::entities::holding_account::HoldingAccount {
            id: 2,
            name: "Second".to_string(),
        },
    )
    .await
    .unwrap();
    insert_ess(&pool, 1, ListingId(1), ymd(2026, 3, 10), "100", "10").await;
    // Same account, taxing point and figures, different listing.
    insert_ess(&pool, 2, ListingId(2), ymd(2026, 3, 10), "100", "10").await;
    // Same listing, taxing point and figures, different holding account.
    test_support::ess_statement(3, 1, ymd(2026, 3, 10))
        .with(|s| {
            s.holding_account_id = 2;
            s.quantity = dec("100");
            s.market_value_per_share = dec("10");
            s.deferral_discount = dec("1000");
        })
        .insert(&pool)
        .await;
    // Same listing, account and figures, different taxing point.
    insert_ess(&pool, 4, ListingId(1), ymd(2026, 1, 10), "100", "10").await;
    // Same listing, account and taxing point, different quantity: a second
    // tranche vesting the same day.
    insert_ess(&pool, 5, ListingId(1), ymd(2026, 3, 10), "40", "10").await;
    // …and one of the same size whose discount differs (shares part-paid
    // for under the plan), which is likewise a second grant.
    test_support::ess_statement(6, 1, ymd(2026, 3, 10))
        .with(|s| {
            s.quantity = dec("100");
            s.market_value_per_share = dec("10");
            s.deferral_discount = dec("600");
        })
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(h.duplicate_ess_statements.is_empty());
}

/// The figures are compared as decimals, not as the TEXT they are stored
/// as: `1000.0` and `1000.00` are one grant entered twice.
#[tokio::test]
async fn ess_figures_equal_in_value_but_not_in_text_are_still_duplicates() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("EMPA").insert(&pool).await;
    insert_ess(&pool, 1, ListingId(1), ymd(2026, 3, 10), "100.0", "10.0").await;
    insert_ess(&pool, 2, ListingId(1), ymd(2026, 3, 10), "100.00", "10.00").await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_ess_statements.len(), 1);
    assert_eq!(h.duplicate_ess_statements[0].statement_ids, vec![1, 2]);
}

/// One same-day cluster can hold both a duplicated pair and an unrelated
/// grant: the grouping is per figure fingerprint, not per taxing point, so
/// the third statement neither joins the pair nor suppresses it. The pair
/// is still reported once it has been vested — `vest_trade_id` is derived,
/// not part of the key.
#[tokio::test]
async fn a_duplicated_ess_pair_is_reported_beside_a_second_grant_and_after_vesting() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("EMPA").insert(&pool).await;
    insert_ess(&pool, 1, ListingId(1), ymd(2026, 3, 10), "100", "10").await;
    insert_ess(&pool, 2, ListingId(1), ymd(2026, 3, 10), "100", "10").await;
    // The second tranche vesting the same day.
    insert_ess(&pool, 3, ListingId(1), ymd(2026, 3, 10), "40", "10").await;
    crate::entities::ess_vest::db_vest(&pool, 1).await.unwrap();

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_ess_statements.len(), 1);
    assert_eq!(h.duplicate_ess_statements[0].quantity, dec("100"));
    assert_eq!(h.duplicate_ess_statements[0].statement_ids, vec![1, 2]);
}

// The ESS 30-day rule (SCENARIOS J-04).

/// Enters a 400-share statement worth $3.50 each (a $1,400 deferral
/// discount — Example 11's figures) and vests it, answering the vest Buy a
/// sale allocates from. Every vest a test needs is created **before** its
/// sells: the vest Buy's id is assigned as max+1, so a sell inserted in
/// between would have the next vest land on top of it.
async fn ess_vest(
    pool: &SqlitePool,
    statement_id: i64,
    listing_id: ListingId,
    taxing_point: NaiveDate,
) -> i64 {
    test_support::ess_statement(statement_id, listing_id.0, taxing_point)
        .with(|s| {
            s.quantity = dec("400");
            s.market_value_per_share = dec("3.50");
            s.deferral_discount = dec("1400");
        })
        .insert(pool)
        .await;
    crate::entities::ess_vest::db_vest(pool, statement_id)
        .await
        .unwrap()
        .id
}

/// Sells `units` of a vest parcel on `sale_date`, allocating them to it.
async fn sell_vest_parcel(
    pool: &SqlitePool,
    sale_id: i64,
    listing_id: ListingId,
    vest_trade_id: i64,
    sale_date: NaiveDate,
    units: Decimal,
) {
    sell_vest_parcel_in_account(
        pool,
        sale_id,
        listing_id,
        vest_trade_id,
        sale_date,
        units,
        1,
    )
    .await;
}

/// [`sell_vest_parcel`] in a named holding account — a Sell must sit in the
/// account holding the parcel it allocates.
async fn sell_vest_parcel_in_account(
    pool: &SqlitePool,
    sale_id: i64,
    listing_id: ListingId,
    vest_trade_id: i64,
    sale_date: NaiveDate,
    units: Decimal,
    account: i64,
) {
    test_support::sell(sale_id, listing_id.0)
        .date(sale_date)
        .qty(units)
        .price(dec("3.795"))
        .account(account)
        .insert(pool)
        .await;
    test_support::allocate(pool, sale_id, sale_id, vest_trade_id, units).await;
}

/// Example 11 itself, entered the way a user naturally would — the
/// employer's *original* statement, then the sale 29 days later (the ATO's
/// 20 July is a Saturday, so the sale sits on the Monday). Both the
/// discount and its year are wrong, and a phantom capital gain appears;
/// the alert names the sale, the statement it draws on, and the two years
/// involved.
#[tokio::test]
async fn a_sale_inside_the_thirty_day_window_is_flagged_with_both_years() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("PEPP").insert(&pool).await;
    let vest = ess_vest(&pool, 1, ListingId(1), ymd(2019, 6, 23)).await;
    sell_vest_parcel(&pool, 101, ListingId(1), vest, ymd(2019, 7, 22), dec("400")).await;

    let h = health(&pool, ymd(2019, 8, 1)).await;
    assert_eq!(h.ess_30_day_rule.len(), 1);
    let alert = &h.ess_30_day_rule[0];
    assert_eq!(alert.sale_trade_id, 101);
    assert_eq!(alert.ticker, "PEPP");
    assert_eq!(alert.sale_date, ymd(2019, 7, 22));
    assert_eq!(alert.units_sold, dec("400"));
    assert_eq!(alert.ess_statement_id, 1);
    assert_eq!(alert.taxing_point_date, ymd(2019, 6, 23));
    assert_eq!(alert.days_after, 29);
    assert_eq!(alert.currency, "AUD");
    assert_eq!(alert.statement_discount, dec("1400"));
    // The whole point of the rule: the discount is assessed in FY2019 as
    // entered, but belongs in FY2020 — two different returns.
    assert_eq!(alert.statement_tax_year, 2019);
    assert_eq!(alert.disposal_tax_year, 2020);
}

/// The window is 30 days **after** the taxing point, inclusive — ITAA 1997
/// s 83A-115(3). Day 30 is inside it, day 31 is an ordinary post-vest sale
/// whose gain is a real capital gain.
#[tokio::test]
async fn the_window_includes_day_thirty_and_excludes_day_thirty_one() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("DAY30").insert(&pool).await;
    test_support::listing(2).ticker("DAY31").insert(&pool).await;
    let taxing_point = ymd(2026, 3, 10);
    let day30 = ess_vest(&pool, 1, ListingId(1), taxing_point).await;
    let day31 = ess_vest(&pool, 2, ListingId(2), taxing_point).await;
    sell_vest_parcel(&pool, 101, ListingId(1), day30, ymd(2026, 4, 9), dec("400")).await;
    sell_vest_parcel(
        &pool,
        102,
        ListingId(2),
        day31,
        ymd(2026, 4, 10),
        dec("400"),
    )
    .await;

    let h = health(&pool, ymd(2026, 5, 1)).await;
    assert_eq!(h.ess_30_day_rule.len(), 1);
    assert_eq!(h.ess_30_day_rule[0].ticker, "DAY30");
    assert_eq!(h.ess_30_day_rule[0].days_after, 30);
    // Both sides of the boundary fall in one financial year here, which is
    // the ordinary case — the rule still applies, it just moves no return.
    assert_eq!(h.ess_30_day_rule[0].statement_tax_year, 2026);
    assert_eq!(h.ess_30_day_rule[0].disposal_tax_year, 2026);
}

/// The corrected entry must not nag: with the **amended** statement (taxing
/// point = the disposal date) the rule's effect is a no-op, and that is
/// exactly what `ato_examples::ess_30_day_rule_example_11_wyatt_amended_statement`
/// enters. A sale of a non-ESS parcel is likewise none of this check's
/// business.
#[tokio::test]
async fn a_same_day_sale_and_a_non_ess_parcel_are_not_flagged() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("PEPP").insert(&pool).await;
    test_support::listing(2).ticker("ORD").insert(&pool).await;
    // The amended statement: taxing point *is* the disposal date.
    let vest = ess_vest(&pool, 1, ListingId(1), ymd(2019, 7, 22)).await;
    sell_vest_parcel(&pool, 101, ListingId(1), vest, ymd(2019, 7, 22), dec("400")).await;
    // An ordinary Buy sold the next day — no ESS statement behind it.
    test_support::buy(50, 2)
        .date(ymd(2019, 7, 22))
        .qty(dec("400"))
        .insert(&pool)
        .await;
    test_support::sell(51, 2)
        .date(ymd(2019, 7, 23))
        .qty(dec("400"))
        .insert(&pool)
        .await;
    test_support::allocate(&pool, 51, 51, 50, dec("400")).await;

    let h = health(&pool, ymd(2019, 8, 1)).await;
    assert!(h.ess_30_day_rule.is_empty());
}

/// One Sell drawing on two vest parcels inside their own windows is two
/// alerts: each statement would be amended separately, so each is named.
/// A partial disposal reports the units actually allocated, not the vest.
#[tokio::test]
async fn each_vest_parcel_a_sale_draws_on_is_named_separately() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("EMPA").insert(&pool).await;
    let march2 = ess_vest(&pool, 1, ListingId(1), ymd(2026, 3, 2)).await;
    let march10 = ess_vest(&pool, 2, ListingId(1), ymd(2026, 3, 10)).await;
    test_support::sell(101, 1)
        .date(ymd(2026, 3, 20))
        .qty(dec("500"))
        .insert(&pool)
        .await;
    test_support::allocate(&pool, 1, 101, march2, dec("400")).await;
    test_support::allocate(&pool, 2, 101, march10, dec("100")).await;

    let h = health(&pool, ymd(2026, 5, 1)).await;
    assert_eq!(h.ess_30_day_rule.len(), 2);
    // Both sales are the same day, so the tie-break is the statement id.
    assert_eq!(h.ess_30_day_rule[0].ess_statement_id, 1);
    assert_eq!(h.ess_30_day_rule[0].days_after, 18);
    assert_eq!(h.ess_30_day_rule[0].units_sold, dec("400"));
    assert_eq!(h.ess_30_day_rule[1].ess_statement_id, 2);
    assert_eq!(h.ess_30_day_rule[1].days_after, 10);
    assert_eq!(h.ess_30_day_rule[1].units_sold, dec("100"));
}

/// SCENARIOS N-08: moving vested shares out of the employer's plan account
/// into the holder's own broker account inside the window is **not** a
/// disposal — the same beneficial owner holds the same interests, no CGT
/// event arises, and s 83A-130 does not reach it either
/// (`docs/ato/ess-takeovers-and-restructures.md`). It is also the move
/// `entities::transfer` exists for, so flagging it nagged about the
/// ordinary use of the feature and invited an amended return over a change
/// of custody. A real sale in the same window still fires.
#[tokio::test]
async fn a_holding_account_transfer_inside_the_window_is_not_a_disposal() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("ICE").insert(&pool).await;
    holding_account::db_upsert(
        &pool,
        &holding_account::HoldingAccount {
            id: 2,
            name: "ICE Employee Plan".to_string(),
        },
    )
    .await
    .unwrap();
    let taxing_point = ymd(2024, 3, 1);
    let vest = ess_vest(&pool, 1, ListingId(1), taxing_point).await;
    let group = crate::entities::transfer::db_transfer(
        &pool,
        1,
        &crate::entities::transfer::TransferBody {
            listing_id: ListingId(1),
            date: ymd(2024, 3, 11),
            from_account_id: 1,
            to_account_id: 2,
            allocations: vec![crate::entities::sell::AllocationInput {
                purchase_trade_id: vest,
                quantity_allocated: dec("400"),
            }],
            fee_allocations: Vec::new(),
            fee_market_price: None,
            fee_fx_rate: None,
        },
    )
    .await
    .unwrap();

    let h = health(&pool, ymd(2024, 4, 1)).await;
    assert!(
        h.ess_30_day_rule.is_empty(),
        "a transfer between the taxpayer's own accounts is not a disposal: {:?}",
        h.ess_30_day_rule
    );

    // A real sale of those same units after the move is a disposal, and is
    // reported as an ordinary one — reached through the transfer chain,
    // since the transferred-in parcel carries no `ess_statement_id` of its
    // own (SCENARIOS N-08).
    let transferred_in = group.transfer_ins[0].id;
    sell_vest_parcel_in_account(
        &pool,
        101,
        ListingId(1),
        transferred_in,
        ymd(2024, 3, 20),
        dec("400"),
        2,
    )
    .await;
    let h = health(&pool, ymd(2024, 4, 1)).await;
    assert_eq!(h.ess_30_day_rule.len(), 1);
    assert_eq!(h.ess_30_day_rule[0].sale_trade_id, 101);
    assert_eq!(h.ess_30_day_rule[0].days_after, 19);
    assert_eq!(h.ess_30_day_rule[0].disposal_kind, EssDisposalKind::Sale);
    assert_eq!(h.ess_30_day_rule[0].vest_trade_id, vest);
}

/// A scrip-for-scrip exchange's closing Sell inside the window stays on the
/// list, labelled: ITAA 1997 s 83A-130(2) normally treats the replacement
/// interests as a continuation (so the taxing point does not move), but that
/// rests on (4)/(9) facts this system does not record, and (5) makes a
/// partial rollover's cash a disposal to that extent — advisory, not
/// silence.
#[tokio::test]
async fn a_scrip_exchange_closing_sell_is_flagged_as_a_takeover_or_restructure() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("OLD").insert(&pool).await;
    test_support::listing(2).ticker("NEW").insert(&pool).await;
    let taxing_point = ymd(2024, 3, 1);
    let vest = ess_vest(&pool, 1, ListingId(1), taxing_point).await;
    corporate_action::db_upsert(
        &pool,
        &corporate_action::CorporateAction {
            id: 1,
            listing_id: ListingId(1),
            date: ymd(2024, 3, 11),
            kind: corporate_action::ActionKind::ScripForScrip {
                scrip_listing_id: ListingId(2),
                scrip_new_units: Decimal::ONE,
                scrip_old_units: Decimal::ONE,
                scrip_cash_per_unit: None,
                scrip_market_value: None,
                scrip_cash_currency: None,
            },
        },
    )
    .await
    .unwrap();
    crate::entities::scrip_exchange::db_exchange(&pool, 1)
        .await
        .unwrap();

    let h = health(&pool, ymd(2024, 4, 1)).await;
    assert_eq!(h.ess_30_day_rule.len(), 1);
    let alert = &h.ess_30_day_rule[0];
    assert_eq!(alert.vest_trade_id, vest);
    assert_eq!(alert.days_after, 10);
    assert_eq!(alert.disposal_kind, EssDisposalKind::TakeoverOrRestructure);
}

// The deceased-estate side (SCENARIOS K-09).

/// An inheritance of `quantity` units at a `cost_base`, all from the one
/// 2015 acquisition and 2024 death.
async fn insert_inheritance(
    pool: &SqlitePool,
    id: i64,
    listing_id: ListingId,
    account: i64,
    quantity: &str,
    cost_base: &str,
) {
    crate::entities::inheritance::db_upsert(
        pool,
        &Inheritance {
            id,
            listing_id,
            holding_account_id: account,
            quantity: dec(quantity),
            date_of_death: ymd(2024, 3, 1),
            cost_base_rule: crate::entities::inheritance::CostBaseRule::DeceasedCostBase,
            cost_base: dec(cost_base),
            lpr_expenditure: Decimal::ZERO,
            lpr_expenditure_date: None,
            deceased_acquisition_date: Some(ymd(2015, 5, 5)),
            currency: "AUD".into(),
            fx_rate: Decimal::ONE,
        },
    )
    .await
    .unwrap();
}

/// One inherited parcel entered twice: each row creates its own Buy, so
/// the holding and its cost base are doubled with nothing else to notice.
#[tokio::test]
async fn one_inherited_parcel_entered_twice_is_reported() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("INHA").insert(&pool).await;
    insert_inheritance(&pool, 1, ListingId(1), 1, "100", "3000").await;
    insert_inheritance(&pool, 2, ListingId(1), 1, "100", "3000").await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.duplicate_inheritances.len(), 1);
    let d = &h.duplicate_inheritances[0];
    assert_eq!(d.ticker, "INHA");
    assert_eq!(d.date_of_death, ymd(2024, 3, 1));
    assert_eq!(d.quantity, dec("100"));
    assert_eq!(d.cost_base_total, dec("3000"));
    assert_eq!(d.inheritance_count, 2);
    assert_eq!(d.inheritance_ids, vec![1, 2]);
}

/// Two inheritances from one death that are not one entered twice: a
/// different share of the estate, a different account, a different listing
/// — and a different cost-base *rule*, which is a contradiction the
/// warning does show.
#[tokio::test]
async fn inheritances_from_one_death_that_differ_are_not_reported() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("INHA").insert(&pool).await;
    test_support::listing(2).ticker("INHB").insert(&pool).await;
    crate::entities::holding_account::db_upsert(
        &pool,
        &crate::entities::holding_account::HoldingAccount {
            id: 2,
            name: "Broker".to_string(),
        },
    )
    .await
    .unwrap();

    insert_inheritance(&pool, 1, ListingId(1), 1, "100", "3000").await;
    // A second parcel of the same listing from the same death, in a
    // different quantity — a part interest recorded in stages.
    insert_inheritance(&pool, 2, ListingId(1), 1, "60", "1800").await;
    // The same figures in another holding account, and on another listing.
    insert_inheritance(&pool, 3, ListingId(1), 2, "100", "3000").await;
    insert_inheritance(&pool, 4, ListingId(2), 1, "100", "3000").await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(
        h.duplicate_inheritances.is_empty(),
        "{:?}",
        h.duplicate_inheritances
    );
}

/// SCENARIOS R-01: a listing whose `currency` is not the currency its own
/// exchange quotes in. The rename path refuses to create it, but a plain
/// `PUT /listings/:id` compares the two columns nowhere, so a listing can
/// be entered this way directly — and once it has history the freeze makes
/// it permanent. Health names it wherever it came from.
#[tokio::test]
async fn a_listing_whose_currency_is_not_its_exchanges_is_reported() {
    let pool = test_pool().await;
    // Entered by a plain PUT: an AUD-quoting market holding a USD listing.
    test_support::listing(1)
        .ticker("CBA")
        .mic("XASX")
        .currency("USD")
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert_eq!(h.exchange_currency_mismatches.len(), 1);
    let m = &h.exchange_currency_mismatches[0];
    assert_eq!(m.listing_id, 1);
    assert_eq!(m.ticker, "CBA");
    assert_eq!(m.currency, "USD");
    assert_eq!(m.exchange_mic, "XASX");
    assert_eq!(m.exchange_currency, "AUD");
}

/// A listing quoting the money its exchange does is silent, and so is a
/// Crypto listing — it has no exchange for its currency to disagree with,
/// which is the whole point of the exchange-less shape rather than a hole
/// in the check.
#[tokio::test]
async fn matching_and_exchange_less_listings_are_not_reported() {
    let pool = test_pool().await;
    test_support::listing(1)
        .ticker("BHP")
        .mic("XASX")
        .currency("AUD")
        .insert(&pool)
        .await;
    test_support::listing(2)
        .ticker("MSFT")
        .mic("XNYS")
        .currency("USD")
        .insert(&pool)
        .await;
    // Exchange-less, and deliberately *not* denominated in the token: a
    // crypto holding is recorded in the fiat it is priced in.
    test_support::listing(3)
        .ticker("BTC")
        .security_type(crate::entities::listing::SecurityType::Crypto)
        .currency("AUD")
        .with(|l| l.exchange_mic = None)
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 7, 13)).await;
    assert!(
        h.exchange_currency_mismatches.is_empty(),
        "{:?}",
        h.exchange_currency_mismatches
    );
}

/// Served over the endpoint the banner polls, since that is the only way
/// the warning reaches anyone.
#[tokio::test]
async fn api_exchange_currency_mismatch_is_served_to_the_banner() {
    let pool = test_pool().await;
    test_support::listing(1)
        .ticker("CBA")
        .mic("XNYS")
        .currency("AUD")
        .insert(&pool)
        .await;
    let body: serde_json::Value = ApiClient::over(router().with_state(pool))
        .get("/reports/health")
        .await
        .json();
    let m = &body["exchange_currency_mismatches"][0];
    assert_eq!(m["ticker"], "CBA");
    assert_eq!(m["currency"], "AUD");
    assert_eq!(m["exchange_mic"], "XNYS");
    assert_eq!(m["exchange_currency"], "USD");
}

// Trades dated on a day the exchange was shut (SCENARIOS S-08).

/// SCENARIOS S-08: a derived write path is exempt from the write-time
/// refusal `PUT /trades/:id` and `PUT /sells/:id` apply — an ESS vest is
/// dated its statement's taxing point, which the scheme sets and which
/// need not be a market day — so the row *is* written, and this alert is
/// what surfaces it. The Saturday vest Buy is listed with its reason, its
/// exchange, and the path that wrote it; the ordinary Buy on the Monday
/// after is not.
#[tokio::test]
async fn a_vest_dated_on_a_saturday_is_written_and_then_flagged() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("PEPP").insert(&pool).await;
    // Saturday 2026-03-14 — refused on `PUT /trades/:id`, but the vest
    // INSERTs its Buy directly and is accepted.
    let vest = ess_vest(&pool, 1, ListingId(1), ymd(2026, 3, 14)).await;
    // Monday 2026-03-16 on a second listing: an ordinary trading day.
    test_support::listing(2).ticker("ORD").insert(&pool).await;
    test_support::buy(50, 2)
        .date(ymd(2026, 3, 16))
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 4, 1)).await;
    assert_eq!(h.non_trading_day_trades.len(), 1);
    let alert = &h.non_trading_day_trades[0];
    assert_eq!(alert.trade_id, vest);
    assert_eq!(alert.trade_type, "Buy");
    assert_eq!(alert.ticker, "PEPP");
    assert_eq!(alert.date, ymd(2026, 3, 14));
    assert_eq!(alert.exchange_mic, "XASX");
    assert_eq!(alert.reason, NonTradingReason::Weekend);
    assert_eq!(alert.source, "an ESS vest");
}

/// A seeded exchange holiday reads as a holiday rather than a weekend, an
/// exchange-less (Crypto) listing is never flagged at all — it trades
/// every day — and a listing whose calendar has no seeded holidays in the
/// year is judged on weekends alone.
#[tokio::test]
async fn a_holiday_a_crypto_saturday_and_an_unseeded_year_are_told_apart() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("PEPP").insert(&pool).await;
    test_support::listing(2)
        .crypto()
        .ticker("ETH")
        .name("Ether")
        .insert(&pool)
        .await;
    // Good Friday 2026-04-03, a seeded XASX holiday, reached through the
    // vest's exempt INSERT.
    let vest = ess_vest(&pool, 1, ListingId(1), ymd(2026, 4, 3)).await;
    // The same Saturday on a crypto listing: `PUT /trades/:id` accepts it,
    // and so does the alert.
    test_support::buy(60, 2)
        .date(ymd(2026, 3, 14))
        .insert(&pool)
        .await;
    // 2018 has no seeded calendar at all: a weekday there is clean.
    test_support::buy(61, 1)
        .date(ymd(2018, 12, 25))
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 5, 1)).await;
    assert_eq!(
        h.non_trading_day_trades
            .iter()
            .map(|a| a.trade_id)
            .collect::<Vec<_>>(),
        vec![vest],
        "only the seeded-holiday vest is flagged"
    );
    assert_eq!(
        h.non_trading_day_trades[0].reason,
        NonTradingReason::Holiday
    );
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
// Disposals recorded at nil proceeds (SCENARIOS AA-03).

/// A gift entered as what was actually *received* — nothing — is accepted
/// in full and raises a capital loss the size of the whole cost base. The
/// market-value substitution rule says the proceeds are the asset's market
/// value instead, so the shape is named here; a Sell that records what it
/// realised is not.
#[tokio::test]
async fn a_sell_at_nil_proceeds_is_flagged_and_a_priced_one_is_not() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("GIFT").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 2))
        .qty(dec("1000"))
        .price(dec("20"))
        .insert(&pool)
        .await;
    // The gift out, entered at the nil consideration received.
    test_support::sell(2, 1)
        .date(ymd(2025, 5, 1))
        .qty(dec("1000"))
        .price(Decimal::ZERO)
        .insert(&pool)
        .await;
    // An ordinary sale of the same listing, which must not be flagged.
    test_support::sell(3, 1)
        .date(ymd(2025, 5, 2))
        .qty(dec("100"))
        .price(dec("25"))
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2025, 6, 1)).await;
    assert_eq!(h.nil_proceeds_disposals.len(), 1);
    let d = &h.nil_proceeds_disposals[0];
    assert_eq!(d.kind, NilProceedsKind::Sell);
    assert_eq!(d.record_id, 2);
    assert_eq!(d.listing_id, 1);
    assert_eq!(d.ticker, "GIFT");
    assert_eq!(d.date, ymd(2025, 5, 1));
    assert_eq!(d.quantity, dec("1000"));
    assert_eq!(d.currency, "AUD");
    // The loss is the parcel's adjusted cost base, which this flag does
    // not compute — and market value on the day, the figure actually
    // wanted, is not a stored fact at all.
    assert_eq!(d.rights_cost, None);
}

/// The price is what is tested, not the netted proceeds: a real price
/// whose brokerage happens to cancel it exactly is arithmetic, not a
/// nil-consideration disposal, and saying the market-value substitution
/// rule is about it would be untrue.
#[tokio::test]
async fn brokerage_cancelling_the_proceeds_is_not_a_nil_proceeds_disposal() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("EVEN").insert(&pool).await;
    test_support::buy(1, 1).qty(dec("100")).insert(&pool).await;
    // 100 × $1 = $100 of proceeds, $100 of brokerage: nil net, real price.
    test_support::sell(2, 1)
        .date(ymd(2025, 5, 1))
        .qty(dec("100"))
        .price(Decimal::ONE)
        .brokerage(dec("100"))
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2025, 6, 1)).await;
    assert!(h.nil_proceeds_disposals.is_empty());
}

/// The exclusion that matters most: a worthless-shares recognise closes
/// the holding at nil proceeds **by definition** — that is what
/// recognising a G3/C2 loss is — so it must never be flagged. Driven
/// through the real operation, since it is the one nil-proceeds Sell the
/// system writes itself.
#[tokio::test]
async fn a_worthless_shares_recognise_is_never_flagged() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("DEAD").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 2))
        .qty(dec("1000"))
        .price(dec("2"))
        .insert(&pool)
        .await;
    corporate_action::db_upsert(
        &pool,
        &corporate_action::CorporateAction {
            id: 10,
            listing_id: ListingId(1),
            date: ymd(2025, 3, 31),
            kind: corporate_action::ActionKind::WorthlessShares {
                worthless_event: corporate_action::WorthlessEvent::G3Declaration,
            },
        },
    )
    .await
    .unwrap();
    let recognised = crate::entities::worthless::db_recognise(&pool, 10)
        .await
        .unwrap();
    assert_eq!(recognised.sell.average_price, Decimal::ZERO);

    let h = health(&pool, ymd(2025, 6, 1)).await;
    assert!(
        h.nil_proceeds_disposals.is_empty(),
        "a recognise closes the holding at nil proceeds by definition: {:?}",
        h.nil_proceeds_disposals
    );
}

/// Every other operation-written closing Sell is excluded too — a scrip
/// exchange, a demerger, a transfer out, a buy-back participation, and the
/// transfer's crypto network-fee disposal, which carries its link on the
/// *transfer* rather than on the trade row. Each is stamped directly here:
/// what is under test is the exclusion predicate
/// (`trade::operation_written_sql`), not the operations, which have their
/// own tests.
#[tokio::test]
async fn operation_written_closing_sells_are_never_flagged() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("OPS").insert(&pool).await;
    test_support::buy(1, 1)
        .qty(dec("10000"))
        .insert(&pool)
        .await;
    let columns = [
        "buyback_action_id",
        "scrip_action_id",
        "demerger_action_id",
        "transfer_id",
        "worthless_action_id",
    ];
    // The Sells: five to stamp, plus the network-fee disposal the transfer
    // links to instead of stamping.
    for id in 100..=104 {
        test_support::sell(id, 1)
            .date(ymd(2025, 5, 1))
            .qty(dec("100"))
            .price(Decimal::ZERO)
            .insert(&pool)
            .await;
    }
    test_support::sell(200, 1)
        .date(ymd(2025, 5, 1))
        .qty(dec("1"))
        .price(Decimal::ZERO)
        .insert(&pool)
        .await;

    // The records the provenance columns point at. Which action or
    // transfer is irrelevant to the predicate — it asks only whether the
    // column is set — but the foreign keys are real.
    corporate_action::db_upsert(
        &pool,
        &corporate_action::CorporateAction {
            id: 1,
            listing_id: ListingId(1),
            date: ymd(2025, 2, 1),
            kind: corporate_action::ActionKind::RightsIssue {
                rights_units: Decimal::ONE,
                rights_held_units: Decimal::from(4),
                exercise_price: dec("1.80"),
                currency: "AUD".into(),
                renounceable: true,
            },
        },
    )
    .await
    .unwrap();
    holding_account::db_upsert(
        &pool,
        &holding_account::HoldingAccount {
            id: 2,
            name: "Broker".to_string(),
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO transfers (id, listing_id, date, from_account_id, to_account_id, \
                                    fee_sale_trade_id) \
             VALUES (1, 1, '2025-05-01', 1, 2, 200)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Stamped directly: what is under test is the exclusion predicate, not
    // the operations, each of which has its own tests.
    for (i, column) in columns.iter().enumerate() {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE trades SET {column} = 1 WHERE id = ?"
        )))
        .bind(100 + i as i64)
        .execute(&pool)
        .await
        .unwrap();
    }

    let h = health(&pool, ymd(2025, 6, 1)).await;
    assert!(
        h.nil_proceeds_disposals.is_empty(),
        "no operation-written Sell is a user entry: {:?}",
        h.nil_proceeds_disposals
    );
}

/// Rights: a **free** right that lapses is the nil/nil non-event
/// `docs/API.md` describes — nil proceeds against a nil cost base
/// fabricates nothing, and it is what every ordinary lapse looks like — so
/// only a right that was **paid for** is listed, where the nil proceeds
/// turn its cost into a capital loss.
#[tokio::test]
async fn only_a_paid_for_right_disposed_of_at_nil_is_flagged() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("RTS").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 2))
        .qty(dec("4000"))
        .insert(&pool)
        .await;
    corporate_action::db_upsert(
        &pool,
        &corporate_action::CorporateAction {
            id: 10,
            listing_id: ListingId(1),
            date: ymd(2025, 2, 1),
            kind: corporate_action::ActionKind::RightsIssue {
                rights_units: Decimal::ONE,
                rights_held_units: Decimal::from(4),
                exercise_price: dec("1.80"),
                currency: "AUD".into(),
                renounceable: true,
            },
        },
    )
    .await
    .unwrap();
    let lapse = |date: NaiveDate, units: &str, cost: Option<Decimal>| {
        crate::entities::rights_sale::SellRightsBody {
            date,
            units: units.parse().unwrap(),
            proceeds_per_right: Some(Decimal::ZERO),
            rights_cost: cost,
            fx_rate: None,
            holding_account_id: 1,
            allocations: vec![crate::entities::rights_sale::AllocationInput {
                purchase_trade_id: 1,
                units: units.parse().unwrap(),
            }],
        }
    };
    // A free right left to lapse…
    crate::entities::rights_sale::db_sell_rights(&pool, 10, &lapse(ymd(2025, 3, 1), "400", None))
        .await
        .unwrap();
    // …and one that cost $80 on-market, lapsing worthless.
    let paid = crate::entities::rights_sale::db_sell_rights(
        &pool,
        10,
        &lapse(ymd(2025, 3, 2), "400", Some(dec("80"))),
    )
    .await
    .unwrap();

    let h = health(&pool, ymd(2025, 6, 1)).await;
    assert_eq!(h.nil_proceeds_disposals.len(), 1);
    let d = &h.nil_proceeds_disposals[0];
    assert_eq!(d.kind, NilProceedsKind::RightsSale);
    assert_eq!(d.record_id, paid.id);
    assert_eq!(d.ticker, "RTS");
    assert_eq!(d.date, ymd(2025, 3, 2));
    assert_eq!(d.quantity, dec("400"));
    assert_eq!(d.rights_cost, Some(dec("80")));
}

/// Served over the endpoint the banner polls, newest disposal first —
/// which is the only way the flag reaches anyone.
#[tokio::test]
async fn api_nil_proceeds_disposals_are_served_to_the_banner() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("GIFT").insert(&pool).await;
    test_support::buy(1, 1).qty(dec("1000")).insert(&pool).await;
    test_support::sell(2, 1)
        .date(ymd(2025, 5, 1))
        .qty(dec("400"))
        .price(Decimal::ZERO)
        .insert(&pool)
        .await;
    test_support::sell(3, 1)
        .date(ymd(2025, 6, 2))
        .qty(dec("600"))
        .price(Decimal::ZERO)
        .insert(&pool)
        .await;

    let body: serde_json::Value = ApiClient::over(router().with_state(pool))
        .get("/reports/health")
        .await
        .json();
    let list = body["nil_proceeds_disposals"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["record_id"], 3);
    assert_eq!(list[0]["kind"], "Sell");
    assert_eq!(list[0]["date"], "2025-06-02");
    assert_eq!(list[0]["quantity"], "600");
    assert_eq!(list[0]["rights_cost"], serde_json::Value::Null);
    assert_eq!(list[1]["record_id"], 2);
}

// ---- distribution calendar ------------------------------------------
//
// The two alerts are the only external evidence the health report has
// about the *books*: every other check compares recorded facts against
// each other, so a distribution nobody entered is invisible to all of
// them.

/// Store one calendar event directly — the shape the `distribution-import`
/// job writes, without the job or a provider.
async fn insert_event(
    pool: &SqlitePool,
    listing_id: ListingId,
    ex_date: NaiveDate,
    amount_per_unit: &str,
    fetched_at: &str,
) {
    insert_event_in(
        pool,
        listing_id,
        ex_date,
        amount_per_unit,
        "AUD",
        fetched_at,
    )
    .await;
}

/// [`insert_event`] in a nominated quote currency — the calendar stores
/// the listing's, never AUD.
async fn insert_event_in(
    pool: &SqlitePool,
    listing_id: ListingId,
    ex_date: NaiveDate,
    amount_per_unit: &str,
    currency: &str,
    fetched_at: &str,
) {
    sqlx::query(
        "INSERT INTO distribution_events \
                 (listing_id, ex_date, amount_per_unit, currency, source, fetched_symbol, \
                  fetched_at) \
             VALUES (?, ?, ?, ?, 'yahoo', 'T1.AX', ?)",
    )
    .bind(listing_id)
    .bind(ex_date)
    .bind(amount_per_unit)
    .bind(currency)
    .bind(fetched_at)
    .execute(pool)
    .await
    .unwrap();
}

/// One listing with a 100-unit parcel bought well before the ex-date.
async fn holding_before_the_ex_date(pool: &SqlitePool) {
    test_support::listing(1).ticker("ABC").insert(pool).await;
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(pool)
        .await;
}

#[tokio::test]
async fn a_known_distribution_with_no_income_row_is_reported() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "0.726547",
        "2026-08-27T00:00:00Z",
    )
    .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert_eq!(h.missing_dividend_entries.len(), 1);
    let m = &h.missing_dividend_entries[0];
    assert_eq!(m.ticker, "ABC");
    assert_eq!(m.ex_date, ymd(2024, 7, 1));
    assert_eq!(m.units_held, dec("100"));
    // The expected gross is the per-unit figure times the units — not an
    // eligibility flag, the figure the missing row would carry.
    assert_eq!(m.expected_amount, dec("72.6547"));
    assert_eq!(m.currency, "AUD");
    assert!(h.dividend_amount_mismatches.is_empty());
}

#[tokio::test]
async fn an_entered_distribution_clears_the_alert() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "0.726547",
        "2026-08-27T00:00:00Z",
    )
    .await;
    // Paid 15 days after the ex-date, carrying no ex_date of its own —
    // the ordinary shape, and the one the window exists for.
    test_support::income(1, 1, ymd(2024, 7, 16))
        .with(|i| i.unfranked_amount = dec("72.65"))
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert!(h.missing_dividend_entries.is_empty());
    assert!(
        h.dividend_amount_mismatches.is_empty(),
        "72.65 against 72.6547 is agreement, not a mismatch"
    );
}

#[tokio::test]
async fn a_distribution_entered_with_the_wrong_gross_is_reported() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "0.726547",
        "2026-08-27T00:00:00Z",
    )
    .await;
    // A transposed figure: 726.55 for 72.65.
    test_support::income(1, 1, ymd(2024, 7, 16))
        .with(|i| i.unfranked_amount = dec("726.55"))
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert!(
        h.missing_dividend_entries.is_empty(),
        "the row matched — it is the amount that is wrong, not the entry that is absent"
    );
    assert_eq!(h.dividend_amount_mismatches.len(), 1);
    let m = &h.dividend_amount_mismatches[0];
    assert_eq!(m.income_id, 1);
    assert_eq!(m.expected_amount, dec("72.6547"));
    assert_eq!(m.recorded_amount, dec("726.55"));
    assert_eq!(m.amount_difference, dec("653.8953"));
    assert_eq!(m.date_paid, ymd(2024, 7, 16));
}

#[tokio::test]
async fn a_holding_bought_on_the_ex_date_is_not_entitled_and_raises_nothing() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("ABC").insert(&pool).await;
    // Bought *on* the ex-date: the security traded that day without this
    // distribution attached, so there is nothing missing.
    test_support::buy(1, 1)
        .date(ymd(2024, 7, 1))
        .qty(dec("100"))
        .insert(&pool)
        .await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "0.726547",
        "2026-08-27T00:00:00Z",
    )
    .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert!(h.missing_dividend_entries.is_empty());

    // The control: one day earlier is the last cum-dividend day, and that
    // holding *is* entitled — so the silence above is the entitlement
    // rule, not a walk that fails to see the parcel at all.
    sqlx::query("UPDATE trades SET date = '2024-06-30' WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert_eq!(h.missing_dividend_entries.len(), 1);
}

#[tokio::test]
async fn each_holding_account_is_asked_separately() {
    let pool = test_pool().await;
    test_support::listing(1).ticker("ABC").insert(&pool).await;
    holding_account::db_upsert(
        &pool,
        &holding_account::HoldingAccount {
            id: 2,
            name: "Second".to_string(),
        },
    )
    .await
    .unwrap();
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;
    test_support::buy(2, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("40"))
        .account(2)
        .insert(&pool)
        .await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "1",
        "2026-08-27T00:00:00Z",
    )
    .await;
    // Only the first account's distribution was entered.
    test_support::income(1, 1, ymd(2024, 7, 16))
        .with(|i| i.unfranked_amount = dec("100"))
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert_eq!(h.missing_dividend_entries.len(), 1);
    let m = &h.missing_dividend_entries[0];
    assert_eq!(m.holding_account_id, 2);
    assert_eq!(m.holding_account, "Second");
    assert_eq!(m.units_held, dec("40"));
    assert_eq!(m.expected_amount, dec("40"));
    assert!(h.dividend_amount_mismatches.is_empty());
}

#[tokio::test]
async fn one_income_row_is_claimed_by_one_event_only() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    // Two events 30 days apart — inside one another's match windows, which
    // is what an unclaimed-row match would fall for.
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "1",
        "2026-08-27T00:00:00Z",
    )
    .await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 7, 31),
        "1",
        "2026-08-27T00:00:00Z",
    )
    .await;
    test_support::income(1, 1, ymd(2024, 8, 5))
        .with(|i| i.unfranked_amount = dec("100"))
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    // The row is nearest 31 July, so that one is matched and 1 July is the
    // one reported missing — exactly one of the two, never both or neither.
    assert_eq!(h.missing_dividend_entries.len(), 1);
    assert_eq!(h.missing_dividend_entries[0].ex_date, ymd(2024, 7, 1));
    assert!(h.dividend_amount_mismatches.is_empty());
}

/// The regression the real portfolio found: a row whose recorded `ex_date`
/// is a **different label for the same distribution** (a record date, which
/// is what the live database's rows carry) must still match.
///
/// This was a false positive on both halves at once — the event reported
/// missing *and* the row left unclaimed — because `ex_date` was a filter
/// rather than an anchor. 34 of Evan's recorded distributions read as
/// missing against a calendar that agreed with every one of them to the
/// cent.
#[tokio::test]
async fn a_row_whose_recorded_ex_date_is_a_few_days_out_still_matches() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2025, 4, 1),
        "1",
        "2026-08-27T00:00:00Z",
    )
    .await;
    // VDHG's real shape: period ending 31 March, true ex-date 1 April, and
    // the row records 2025-04-06 — five days out across an Easter weekend.
    test_support::income(1, 1, ymd(2025, 4, 16))
        .with(|i| {
            i.ex_date = Some(ymd(2025, 4, 6));
            i.entitlement_date = Some(ymd(2025, 3, 31));
            i.trust_income = true;
            i.unfranked_amount = dec("100");
        })
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert!(
        h.missing_dividend_entries.is_empty(),
        "the distribution is recorded, under a differently-labelled date: {:?}",
        h.missing_dividend_entries
    );
    assert!(h.dividend_amount_mismatches.is_empty());
}

#[tokio::test]
async fn a_row_carrying_its_own_ex_date_matches_on_it_exactly() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "1",
        "2026-08-27T00:00:00Z",
    )
    .await;
    // The payment date is far outside the window, but the row states the
    // ex-date itself — the same fact the calendar stores.
    test_support::income(1, 1, ymd(2025, 3, 1))
        .with(|i| {
            i.ex_date = Some(ymd(2024, 7, 1));
            i.unfranked_amount = dec("100");
        })
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert!(h.missing_dividend_entries.is_empty());
    assert!(h.dividend_amount_mismatches.is_empty());
}

#[tokio::test]
async fn a_split_between_the_ex_date_and_the_fetch_does_not_skew_the_expected_amount() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    // A 10-for-1 split after the distribution. The provider restates its
    // whole dividend history into the current basis, so the stored figure
    // is a tenth of what was declared — and the units it multiplies must
    // be in that same basis, or the product is out by the ratio.
    insert_action(
        &pool,
        1,
        ListingId(1),
        ymd(2025, 1, 15),
        corporate_action::ActionKind::ShareSplit {
            split_new_units: dec("10"),
            split_old_units: dec("1"),
        },
    )
    .await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "0.0726547",
        "2026-08-27T00:00:00Z",
    )
    .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert_eq!(h.missing_dividend_entries.len(), 1);
    let m = &h.missing_dividend_entries[0];
    assert_eq!(
        m.units_held,
        dec("1000"),
        "100 units in the post-split basis"
    );
    assert_eq!(
        m.expected_amount,
        dec("72.6547"),
        "the same dollars either side of the split — a total is basis-independent"
    );
}

/// The registry's own DRP surplus contradicts the feed, and wins.
///
/// The real portfolio's only surviving alert: the provider's stream
/// carried a VDHG event on 2 November 2020, but the September advice
/// carried $50.4362 forward and the December advice brought exactly
/// $50.4362 back. A distribution in that gap would have consumed the
/// surplus through the DRP, so there was none, and there was never a row
/// to enter.
#[tokio::test]
async fn a_drp_surplus_arriving_intact_disproves_a_distribution_in_the_gap() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    // Two reinvested distributions either side of the disputed event, the
    // second bringing forward exactly what the first carried on.
    reinvested(&pool, 2, 1, ymd(2020, 10, 16), "0", "50.4362").await;
    reinvested(&pool, 3, 2, ymd(2021, 1, 19), "50.4362", "38.7177").await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2020, 11, 2),
        "0.038237",
        "2026-08-27T00:00:00Z",
    )
    .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert!(
        h.missing_dividend_entries.is_empty(),
        "the registry's own surplus says nothing was distributed there: {:?}",
        h.missing_dividend_entries
    );

    // The control, and it is the whole point: break the chain — the
    // surplus arrives *changed*, as it would if something had been
    // distributed and reinvested in between — and the alert fires again.
    sqlx::query("UPDATE trades SET residual_brought_forward = '12.3456' WHERE id = 3")
        .execute(&pool)
        .await
        .unwrap();
    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert_eq!(h.missing_dividend_entries.len(), 1);
    assert_eq!(h.missing_dividend_entries[0].ex_date, ymd(2020, 11, 2));
}

/// The control that matters: a genuinely missing **reinvested**
/// distribution must still be reported.
///
/// This is where the veto is most dangerous, and a first cut of it was
/// wrong. A reinvestment lands on its own distribution's payment date, so
/// the pair bracketing that distribution's ex-date is (previous payment,
/// *its own* payment) — and those two always chain, because the later
/// one's brought-forward is the earlier one's carried-forward before this
/// distribution was added to it. Read naively, the chain "closes" across
/// every DRP distribution, and the alert would be suppressed exactly when
/// it was right.
#[tokio::test]
async fn a_missing_reinvested_distribution_is_still_reported() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    // VDHG's real chain: September reinvested on 16 October carrying
    // $50.4362 on, December reinvested on 19 January bringing it back.
    reinvested(&pool, 2, 1, ymd(2020, 10, 16), "0", "50.4362").await;
    reinvested(&pool, 3, 2, ymd(2021, 1, 19), "50.4362", "38.7177").await;
    // The December distribution itself: ex 4 January, paid 19 January —
    // and its income row is the one that was never entered.
    sqlx::query("DELETE FROM income WHERE id = 2")
        .execute(&pool)
        .await
        .unwrap();
    insert_event(
        &pool,
        ListingId(1),
        ymd(2021, 1, 4),
        "0.993767",
        "2026-08-27T00:00:00Z",
    )
    .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert_eq!(
        h.missing_dividend_entries.len(),
        1,
        "the 19 January payment is this distribution's own reinvestment, so the chain \
             across 4 January proves nothing"
    );
    assert_eq!(h.missing_dividend_entries[0].ex_date, ymd(2021, 1, 4));
}

/// A nil surplus proves nothing, so it must not suppress anything.
///
/// An account whose residual handling is `PayOut` carries nothing by
/// design: its chain is `0 → 0` across every gap, and reading that as
/// evidence would silently switch the alert off for the whole holding.
#[tokio::test]
async fn a_nil_drp_surplus_is_not_evidence_and_suppresses_nothing() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    reinvested(&pool, 2, 1, ymd(2020, 10, 16), "0", "0").await;
    reinvested(&pool, 3, 2, ymd(2021, 1, 19), "0", "0").await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2020, 11, 2),
        "0.038237",
        "2026-08-27T00:00:00Z",
    )
    .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert_eq!(h.missing_dividend_entries.len(), 1);
}

/// A DRP reinvestment and the income row that funded it, with the
/// registry's brought/carried surplus as the advice states them.
async fn reinvested(
    pool: &SqlitePool,
    trade_id: i64,
    income_id: i64,
    date: NaiveDate,
    brought: &str,
    carried: &str,
) {
    test_support::drp(trade_id, 1)
        .date(date)
        .qty(dec("10"))
        .with(|t| {
            t.residual_brought_forward = dec(brought);
            t.residual_carried_forward = dec(carried);
        })
        .insert(pool)
        .await;
    // The funding distribution. Its `reinvestment_trade_id` is deliberately
    // not set: that link is server-owned provenance the reinvest operation
    // writes, and the chain is read off the DRP trades themselves.
    test_support::income(income_id, 1, date)
        .with(|i| {
            i.unfranked_amount = dec("100");
            i.trust_income = true;
        })
        .insert(pool)
        .await;
}

/// The listing's own record is what says when a distribution is due: two
/// entered distributions paid 15 and 19 days after going ex put the next
/// one due 19 days after its ex-date — the longest gap, not the typical
/// one — and it is a note until that day passes, a problem the day after.
#[tokio::test]
async fn a_distribution_not_yet_due_is_awaiting_payment_until_its_listings_longest_gap() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    for (id, ex, paid) in [
        (1, ymd(2025, 7, 1), ymd(2025, 7, 16)),
        (2, ymd(2025, 10, 1), ymd(2025, 10, 20)),
    ] {
        insert_event(&pool, ListingId(1), ex, "0.5", "2026-10-02T00:00:00Z").await;
        test_support::income(id, 1, paid)
            .with(|i| i.unfranked_amount = dec("50"))
            .insert(&pool)
            .await;
    }
    insert_event(
        &pool,
        ListingId(1),
        ymd(2026, 10, 1),
        "0.5",
        "2026-10-02T00:00:00Z",
    )
    .await;

    let h = health(&pool, ymd(2026, 10, 9)).await;
    assert_eq!(h.missing_dividend_entries.len(), 1);
    let m = &h.missing_dividend_entries[0];
    assert_eq!(m.ex_date, ymd(2026, 10, 1));
    assert_eq!(m.expected_payment_by, ymd(2026, 10, 20));
    assert!(m.awaiting_payment);

    let on_the_day = health(&pool, ymd(2026, 10, 20)).await;
    assert!(
        on_the_day.missing_dividend_entries[0].awaiting_payment,
        "the due date itself is still within the expected gap"
    );

    let after = health(&pool, ymd(2026, 10, 21)).await;
    assert_eq!(after.missing_dividend_entries.len(), 1);
    assert!(!after.missing_dividend_entries[0].awaiting_payment);
}

/// A listing with nothing entered to learn from falls back to the far
/// edge of the matching window, so it is never called late before its
/// payment could even have been recognised as its own.
#[tokio::test]
async fn a_listing_with_no_payment_history_is_due_at_the_end_of_the_match_window() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2026, 10, 1),
        "0.5",
        "2026-10-02T00:00:00Z",
    )
    .await;

    let h = health(&pool, ymd(2026, 10, 9)).await;
    let m = &h.missing_dividend_entries[0];
    assert_eq!(
        m.expected_payment_by,
        ymd(2026, 10, 1) + Duration::days(DIVIDEND_MATCH_AFTER_DAYS)
    );
    assert!(m.awaiting_payment);
}

/// A listing's learned gap wins; a listing with none gets the window.
#[test]
fn the_expected_payment_lag_falls_back_to_the_match_window() {
    let lags = HashMap::from([(ListingId(1), 19)]);
    assert_eq!(expected_payment_lag_days(&lags, ListingId(1)), 19);
    assert_eq!(
        expected_payment_lag_days(&lags, ListingId(2)),
        DIVIDEND_MATCH_AFTER_DAYS
    );
}

#[tokio::test]
async fn an_empty_calendar_raises_nothing() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    test_support::income(1, 1, ymd(2024, 7, 16))
        .with(|i| i.unfranked_amount = dec("72.65"))
        .insert(&pool)
        .await;

    // A database whose import job has never run knows of no ex-date, and
    // must therefore say nothing at all — the alerts are evidence about
    // the books, never the absence of evidence.
    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert!(h.missing_dividend_entries.is_empty());
    assert!(h.dividend_amount_mismatches.is_empty());
}

/// The gross comparison is refused across currencies rather than made
/// wrong. `income.currency` defaults to `'AUD'` and nothing in the income
/// write path ties it to the listing's, so a USD-quoted holding entered in
/// AUD is entirely ordinary — and subtracting one gross from the other
/// would alert forever on the exchange rate.
#[tokio::test]
async fn a_gross_in_another_currency_is_not_compared_against_the_calendar() {
    let pool = test_pool().await;
    test_support::listing(1)
        .ticker("ABC")
        .currency("USD")
        .insert(&pool)
        .await;
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .currency("USD")
        .insert(&pool)
        .await;
    insert_event_in(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "0.726547",
        "USD",
        "2026-08-27T00:00:00Z",
    )
    .await;
    // USD 72.6547 of distribution, entered as its AUD equivalent.
    test_support::income(1, 1, ymd(2024, 7, 16))
        .with(|i| {
            i.unfranked_amount = dec("110.00");
            i.currency = "AUD".into();
        })
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert!(
        h.missing_dividend_entries.is_empty(),
        "the row is still this distribution's record, so it is claimed"
    );
    assert!(
        h.dividend_amount_mismatches.is_empty(),
        "AUD 110.00 minus USD 72.6547 is not a number this alert may report"
    );
}

/// The control for the currency guard: the same divergence *within* one
/// currency is still reported, so the guard suppresses a comparison that
/// cannot be made rather than the alert itself.
#[tokio::test]
async fn the_same_divergence_inside_one_currency_is_still_reported() {
    let pool = test_pool().await;
    test_support::listing(1)
        .ticker("ABC")
        .currency("USD")
        .insert(&pool)
        .await;
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .currency("USD")
        .insert(&pool)
        .await;
    insert_event_in(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "0.726547",
        "USD",
        "2026-08-27T00:00:00Z",
    )
    .await;
    test_support::income(1, 1, ymd(2024, 7, 16))
        .with(|i| {
            i.unfranked_amount = dec("110.00");
            i.currency = "USD".into();
        })
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert_eq!(h.dividend_amount_mismatches.len(), 1);
    let m = &h.dividend_amount_mismatches[0];
    assert_eq!(m.currency, "USD");
    assert_eq!(m.recorded_amount, dec("110.00"));
    assert_eq!(m.expected_amount, dec("72.6547"));
}

/// A monthly payer's income rows must go to the month they belong to.
///
/// A fund paying ~15 days after going ex puts February's payment 14 days
/// *before* the March ex-date and March's own 15 days after it — so on
/// absolute distance the previous month's row was the nearer anchor and
/// took the match outright. The gross then compared was February's, and
/// the alert reported a mismatch that was really a mis-assignment.
///
/// February's own event is absent from the calendar here, which is what
/// leaves its row unclaimed for March to steal: Yahoo serves 8 of HNDQ's
/// 12 periods, so a recorded distribution the calendar never knew about is
/// an ordinary thing to meet.
#[tokio::test]
async fn a_monthly_payer_does_not_match_the_previous_month_s_payment() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 3, 1),
        "0.20",
        "2026-08-27T00:00:00Z",
    )
    .await;

    // Both payments carry only `date_paid` — the anchor 13 of the 47 rows
    // in the live database have, and the one the window has to carry the
    // matching on.
    test_support::income(1, 1, ymd(2024, 2, 16))
        .with(|i| i.unfranked_amount = dec("10.00"))
        .insert(&pool)
        .await;
    test_support::income(2, 1, ymd(2024, 3, 16))
        .with(|i| i.unfranked_amount = dec("20.00"))
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert!(
        h.dividend_amount_mismatches.is_empty(),
        "March's event is March's payment, exactly: {:?}",
        h.dividend_amount_mismatches
    );
    assert!(h.missing_dividend_entries.is_empty());
}

/// The control for the `date_paid` rule: an **entitlement** anchor before
/// the ex-date still matches, which is the whole reason
/// [`DIVIDEND_MATCH_BEFORE_DAYS`] reaches backwards at all.
#[tokio::test]
async fn an_entitlement_date_before_the_ex_date_still_matches() {
    let pool = test_pool().await;
    holding_before_the_ex_date(&pool).await;
    insert_event(
        &pool,
        ListingId(1),
        ymd(2024, 7, 1),
        "0.20",
        "2026-08-27T00:00:00Z",
    )
    .await;
    // The distribution period ends 30 June and the security goes ex on
    // 1 July; the cash lands far outside the window, so the entitlement
    // date is the only anchor that can carry this match.
    test_support::income(1, 1, ymd(2024, 12, 1))
        .with(|i| {
            i.unfranked_amount = dec("20.00");
            // An entitlement date is a trust row's fact, so the row has to
            // be one — which is also the shape that carries this anchor.
            i.trust_income = true;
            i.entitlement_date = Some(ymd(2024, 6, 30));
        })
        .insert(&pool)
        .await;

    let h = health(&pool, ymd(2026, 8, 27)).await;
    assert!(h.missing_dividend_entries.is_empty());
    assert!(h.dividend_amount_mismatches.is_empty());
}
