//! The scheduled `price-import` collection job.

use super::*;

/// Store an errored row directly (as an earlier failed run would have).
async fn seed_errored_price(pool: &SqlitePool, listing_id: i64, date: NaiveDate, msg: &str) {
    crate::test_support::closing_price(listing_id, date)
        .source("stub")
        .fetched_at("2026-06-03T08:00:00Z")
        .errored(msg)
        .insert(pool)
        .await;
}

#[tokio::test]
async fn collection_stores_price_per_held_listing_and_skips_non_held() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_listing(&pool, 2, "IDLE", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    // The earlier window days are already stored ok; only Friday is new.
    for &d in asx_lookback_window().iter().rev().skip(1) {
        seed_ok_price(&pool, 1, d).await;
    }
    let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "62.48", "AUD");

    run_collection(&pool, &fetcher, friday_evening_sydney())
        .await
        .unwrap();

    let row = db_get_one(&pool, 1, ymd(2026, 6, 5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.price, Some("62.48".parse().unwrap()));
    assert_eq!(row.status, PriceStatus::Ok);
    assert_eq!(row.source, "stub");
    assert!(row.error.is_none());
    let rows = db_list(&pool, Some(2), None, None).await.unwrap();
    assert!(rows.is_empty(), "the non-held listing is not collected");
    assert_eq!(
        fetcher.calls(),
        vec![(1, ymd(2026, 6, 5), ymd(2026, 6, 5))],
        "only the missing day is fetched"
    );
}

#[tokio::test]
async fn collection_skips_days_already_stored_ok() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    let mut fetcher = StubFetcher::default();
    for &d in &asx_lookback_window() {
        fetcher = fetcher.with_close(1, d, "62.48", "AUD");
    }
    run_collection(&pool, &fetcher, friday_evening_sydney())
        .await
        .unwrap();
    assert_eq!(
        fetcher.calls().len(),
        1,
        "one provider call spans the window"
    );
    assert_eq!(db_list(&pool, None, None, None).await.unwrap().len(), 10);

    // A second run (same evening) finds every window day ok: no re-fetch.
    run_collection(&pool, &fetcher, friday_evening_sydney())
        .await
        .unwrap();
    assert_eq!(fetcher.calls().len(), 1, "no second provider call");
    assert_eq!(db_list(&pool, None, None, None).await.unwrap().len(), 10);
}

/// The lookback self-heals: a day stored errored (and a day missed
/// outright) is re-attempted by the next run — with the days already ok
/// never re-fetched — so the daily runs are each other's retries.
#[tokio::test]
async fn collection_backfills_missing_and_errored_days_in_the_lookback() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    // Window state: Wed errored, Thu missing, Fri missing; the rest ok.
    for &d in &asx_lookback_window()[..7] {
        seed_ok_price(&pool, 1, d).await;
    }
    seed_errored_price(&pool, 1, ymd(2026, 6, 3), "provider down").await;

    let fetcher = StubFetcher::default()
        .with_close(1, ymd(2026, 6, 3), "64.91", "AUD")
        .with_close(1, ymd(2026, 6, 4), "63.10", "AUD")
        .with_close(1, ymd(2026, 6, 5), "62.48", "AUD");
    run_collection(&pool, &fetcher, friday_evening_sydney())
        .await
        .unwrap();

    // One call spanning exactly the days that needed work.
    assert_eq!(fetcher.calls(), vec![(1, ymd(2026, 6, 3), ymd(2026, 6, 5))]);
    for (d, price) in [
        (ymd(2026, 6, 3), "64.91"),
        (ymd(2026, 6, 4), "63.10"),
        (ymd(2026, 6, 5), "62.48"),
    ] {
        let row = db_get_one(&pool, 1, d).await.unwrap().unwrap();
        assert_eq!(row.status, PriceStatus::Ok, "{d}");
        assert_eq!(row.price, Some(price.parse().unwrap()), "{d}");
    }
}

#[tokio::test]
async fn collection_failure_stores_errored_rows_and_fails_the_job() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    let fetcher = StubFetcher::failing("provider down");

    let err = run_collection(&pool, &fetcher, friday_evening_sydney())
        .await
        .unwrap_err();
    assert!(err.contains("BHP"), "job error names the listing: {err}");

    let rows = db_list(&pool, None, None, None).await.unwrap();
    assert_eq!(
        rows.len(),
        asx_lookback_window().len(),
        "every attempted window day is recorded, never silently missing"
    );
    assert!(rows.iter().all(|r| r.status == PriceStatus::Error));
    assert!(rows.iter().all(|r| r.price.is_none()));
    assert!(rows[0].error.as_deref().unwrap().contains("provider down"));
}

#[tokio::test]
async fn collection_replaces_errored_rows_once_the_provider_recovers() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    run_collection(
        &pool,
        &StubFetcher::failing("down"),
        friday_evening_sydney(),
    )
    .await
    .unwrap_err();

    let mut fetcher = StubFetcher::default();
    for &d in &asx_lookback_window() {
        fetcher = fetcher.with_close(1, d, "62.48", "AUD");
    }
    run_collection(&pool, &fetcher, friday_evening_sydney())
        .await
        .unwrap();

    let rows = db_list(&pool, None, None, None).await.unwrap();
    assert_eq!(rows.len(), asx_lookback_window().len());
    assert!(rows.iter().all(|r| r.status == PriceStatus::Ok));
    assert!(rows.iter().all(|r| r.error.is_none()));
}

#[tokio::test]
async fn collection_records_currency_mismatch_as_error() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "ICE", "XNYS", "USD").await;
    insert_buy(&pool, 1, 1, "10").await;
    // Only Friday is missing; the provider quotes AUD for a USD listing —
    // wrong symbol mapping; the price must not be stored as if it were USD.
    for &d in asx_lookback_window().iter().rev().skip(1) {
        seed_ok_price(&pool, 1, d).await;
    }
    let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "141.50", "AUD");

    // 21:00 UTC Friday = 17:00 New York, after the close.
    run_collection(&pool, &fetcher, utc(2026, 6, 5, 21, 0))
        .await
        .unwrap_err();
    let row = db_get_one(&pool, 1, ymd(2026, 6, 5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, PriceStatus::Error);
    assert!(row.error.as_deref().unwrap().contains("currency mismatch"));
}

#[tokio::test]
async fn collection_crypto_collected_daily_at_utc_cutoff() {
    let pool = test_pool().await;
    insert_crypto_listing(&pool, 1, "BTC").await;
    insert_buy(&pool, 1, 1, "0.5").await;
    // Crypto trades every day: the lookback is the COLLECTION_LOOKBACK_DAYS
    // calendar days ending Saturday 2026-06-06; all but Saturday are
    // already stored ok.
    for i in 1..COLLECTION_LOOKBACK_DAYS {
        seed_ok_price(&pool, 1, ymd(2026, 6, 6) - Duration::days(i)).await;
    }
    let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 6), "86378.35", "AUD");

    // Sunday 01:30 UTC: Saturday 2026-06-06 is a complete crypto day.
    run_collection(&pool, &fetcher, utc(2026, 6, 7, 1, 30))
        .await
        .unwrap();
    let row = db_get_one(&pool, 1, ymd(2026, 6, 6))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, PriceStatus::Ok);
    assert_eq!(row.price, Some("86378.35".parse().unwrap()));
    assert_eq!(fetcher.calls(), vec![(1, ymd(2026, 6, 6), ymd(2026, 6, 6))]);
}
