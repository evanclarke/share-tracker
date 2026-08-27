//! DB-level and API-level tests for the distribution calendar.

use super::test_support::DistributionStub;
use super::*;
use crate::test_support::{ApiClient, allocate, buy, dec, listing, sell, test_pool, ymd};

fn client(pool: &SqlitePool) -> ApiClient {
    ApiClient::over(router().with_state(pool.clone()))
}

/// A stub that answers for `listing_id` with one event, and the job run that
/// stores it.
async fn refresh(
    pool: &SqlitePool,
    fetcher: &dyn DistributionFetcher,
) -> Result<Option<String>, String> {
    run_refresh(
        pool,
        fetcher,
        ymd(2026, 8, 27).and_hms_opt(0, 0, 0).unwrap().and_utc(),
    )
    .await
}

#[tokio::test]
async fn a_refresh_stores_the_provider_events_for_a_held_listing() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let stub = DistributionStub::default()
        .with_event(1, ymd(2024, 7, 1), dec("0.726547"), "AUD")
        .with_event(1, ymd(2025, 1, 2), dec("0.018741"), "AUD");
    assert_eq!(refresh(&pool, &stub).await, Ok(None));

    let stored = db_list(&pool).await.unwrap();
    assert_eq!(stored.len(), 2);
    // Newest ex-date first, and every provenance column recorded.
    assert_eq!(stored[0].ex_date, ymd(2025, 1, 2));
    assert_eq!(stored[0].amount_per_unit, dec("0.018741"));
    assert_eq!(stored[0].currency, "AUD");
    assert_eq!(stored[0].source, "stub");
    assert_eq!(stored[0].fetched_symbol, "T1");
    assert!(stored[0].fetched_at.starts_with("2026-08-27T"));
    assert_eq!(stored[1].ex_date, ymd(2024, 7, 1));
}

#[tokio::test]
async fn a_listing_never_held_is_not_fetched_at_all() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    // No trade: the listing exists but was never held, so there is no span to
    // ask the provider about and no distribution anyone could have missed.
    let stub = DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("1"), "AUD");
    assert_eq!(refresh(&pool, &stub).await, Ok(None));
    assert!(db_list(&pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_window_is_the_held_span_so_an_event_outside_it_is_not_stored() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;
    sell(2, 1)
        .date(ymd(2024, 9, 30))
        .qty(dec("100"))
        .insert(&pool)
        .await;
    // The holding timeline is built from parcel allocations, not from a Sell's
    // own quantity — so the sale only closes the parcel once it is allocated.
    allocate(&pool, 1, 2, 1, dec("100")).await;

    let stub = DistributionStub::default()
        // Before the parcel was acquired, and after it was sold out.
        .with_event(1, ymd(2023, 7, 1), dec("1"), "AUD")
        .with_event(1, ymd(2024, 7, 1), dec("1"), "AUD")
        .with_event(1, ymd(2025, 7, 1), dec("1"), "AUD");
    refresh(&pool, &stub).await.unwrap();

    let stored = db_list(&pool).await.unwrap();
    assert_eq!(
        stored.iter().map(|e| e.ex_date).collect::<Vec<_>>(),
        vec![ymd(2024, 7, 1)]
    );
}

#[tokio::test]
async fn a_re_fetch_updates_the_amount_in_place_and_keeps_the_row_id() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    refresh(
        &pool,
        &DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.70"), "AUD"),
    )
    .await
    .unwrap();
    let first = db_list(&pool).await.unwrap();
    assert_eq!(first.len(), 1);

    refresh(
        &pool,
        &DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.726547"), "AUD"),
    )
    .await
    .unwrap();
    let second = db_list(&pool).await.unwrap();
    assert_eq!(second.len(), 1, "the natural key is (listing, ex_date)");
    assert_eq!(second[0].amount_per_unit, dec("0.726547"));
    assert_eq!(
        second[0].id, first[0].id,
        "the surrogate id survives, so the row keeps its own audit trail"
    );

    // …and the trail records what it said before.
    let trail: Vec<(String, String)> = sqlx::query_as(
        "SELECT operation, old_row FROM row_history \
         WHERE table_name = 'distribution_events' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].0, "UPDATE");
    assert!(trail[0].1.contains("0.70"));
}

#[tokio::test]
async fn a_refresh_never_deletes_an_event_the_provider_stops_serving() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    refresh(
        &pool,
        &DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.70"), "AUD"),
    )
    .await
    .unwrap();
    // The provider now knows of nothing at all — an outage that answered
    // rather than failed, or a history it silently truncated.
    refresh(&pool, &DistributionStub::default()).await.unwrap();
    assert_eq!(db_list(&pool).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_provider_failure_fails_the_run_rather_than_reading_as_no_distribution() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let error = refresh(&pool, &DistributionStub::failing("upstream 503"))
        .await
        .unwrap_err();
    assert!(error.contains("T1 (1)"), "{error}");
    assert!(error.contains("upstream 503"), "{error}");
    assert!(db_list(&pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_undatable_event_qualifies_the_run_instead_of_vanishing() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let stub = DistributionStub::default()
        .with_event(1, ymd(2024, 7, 1), dec("0.70"), "AUD")
        .with_undatable(1, ymd(2025, 1, 1));
    let note = refresh(&pool, &stub).await.unwrap().expect("a note");
    assert!(note.contains("could not be placed"), "{note}");
    assert!(note.contains("2025-01-01"), "{note}");
    // The run still succeeded and still stored what it could.
    assert_eq!(db_list(&pool).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_currency_the_listing_does_not_trade_in_is_refused() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await; // AUD
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let stub = DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.70"), "USD");
    let error = refresh(&pool, &stub).await.unwrap_err();
    assert!(error.contains("USD"), "{error}");
    assert!(error.contains("AUD"), "{error}");
    assert!(
        db_list(&pool).await.unwrap().is_empty(),
        "the listing's whole set rolls back rather than landing half-stored"
    );
}

#[tokio::test]
async fn a_non_positive_amount_is_refused() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let stub = DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0"), "AUD");
    let error = refresh(&pool, &stub).await.unwrap_err();
    assert!(error.contains("not a positive amount"), "{error}");
}

#[tokio::test]
async fn the_list_and_get_routes_serve_the_stored_calendar() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;
    refresh(
        &pool,
        &DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.726547"), "AUD"),
    )
    .await
    .unwrap();

    let api = client(&pool);
    let listed: Vec<DistributionEvent> = api.get_json("/distribution_events").await;
    assert_eq!(listed.len(), 1);
    let one: DistributionEvent = api
        .get_json(&format!("/distribution_events/{}", listed[0].id))
        .await;
    assert_eq!(one.ex_date, ymd(2024, 7, 1));
    assert_eq!(one.amount_per_unit, dec("0.726547"));

    // A `GET` of a missing row is the house's bare 404 — only a DELETE names
    // the noun, and this entity has no DELETE (the calendar is provider-owned).
    api.get("/distribution_events/9999")
        .await
        .expect_status(axum::http::StatusCode::NOT_FOUND);
}
