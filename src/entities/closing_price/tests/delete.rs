//! Deleting a stored row, and the audit trail it leaves.

use super::*;

/// An errored row for a day that can never have a price (here: before the
/// security's first trading day) is deletable, which is the only way to
/// stop `reports::health` reporting it forever.
#[tokio::test]
async fn api_delete_removes_an_errored_row() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "HNDQ", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    store_errored(&pool, ymd(2026, 6, 2)).await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, bytes) = delete_req(&app, "/closing_prices/1/2026-06-02").await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(bytes.is_empty());
    assert!(
        db_get_one(&pool, 1, ymd(2026, 6, 2))
            .await
            .unwrap()
            .is_none()
    );
    // The health report's standing alarm is cleared with it.
    let health = crate::reports::health::db_health(&pool, ymd(2026, 6, 3), Utc::now())
        .await
        .unwrap();
    assert!(health.errored_prices.is_empty());
}

/// An ok row is never deletable: real price data is replaced by a
/// re-fetch, so the endpoint cannot punch a hole in a valued series.
#[tokio::test]
async fn api_delete_rejects_an_ok_row() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 2), "62.48", "AUD");
    fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 2)])
        .await
        .unwrap();
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, bytes) = delete_req(&app, "/closing_prices/1/2026-06-02").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let msg = String::from_utf8_lossy(&bytes);
    assert!(msg.contains("re-fetch it"), "points at the fix: {msg}");
    let row = db_get_one(&pool, 1, ymd(2026, 6, 2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, PriceStatus::Ok, "the price is still stored");
}

/// Discarding an errored row is recorded too — the trail keeps the
/// acknowledgement that a day was written off, and the message it carried.
#[tokio::test]
async fn discarding_an_errored_row_is_recorded_in_the_audit_trail() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    store_errored(&pool, ymd(2026, 6, 2)).await;
    let row = db_get_one(&pool, 1, ymd(2026, 6, 2))
        .await
        .unwrap()
        .unwrap();
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, _) = delete_req(&app, "/closing_prices/1/2026-06-02").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let history = crate::reports::row_history::db_row_history(&pool, "closing_prices", row.id)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["operation"], "DELETE");
    assert_eq!(history[0]["status"], "error");
    assert!(
        history[0]["error"].as_str().is_some_and(|e| !e.is_empty()),
        "the failure the day was written off for is kept"
    );
}

#[tokio::test]
async fn api_delete_unknown_row_is_404() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, _) = delete_req(&app, "/closing_prices/1/2026-06-02").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
