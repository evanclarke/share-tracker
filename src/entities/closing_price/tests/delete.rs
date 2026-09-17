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

/// The delete guard reads the row and the listing's marker on the same
/// connection, inside the same write transaction, that the delete runs on — so
/// a concurrent manual price write (a `PUT`, or a re-fetch) cannot land in a
/// window between the decision and the statement and be removed by it.
///
/// The interleave driven here is exactly that window, made deterministic
/// rather than sampled: a manual ok price for the same (listing, date) is
/// written on a connection holding an open `BEGIN IMMEDIATE`, so it is
/// invisible to a pool reader — the state the old guard read the errored row
/// in — and the handler's own `write_tx` cannot begin until it commits. Read
/// on the pool and deleted in a separate unguarded statement (2026-09-17
/// review), the old handler approved the delete while the ok write was
/// uncommitted, then the `DELETE` waited for the lock and removed the ok row
/// once it was released: this test's `422` and surviving row are what that
/// path failed. The grace period before the commit is what guarantees the
/// handler's reads happened first under that old shape; under the fix the
/// handler is blocked at its first statement either way, so it cannot make
/// the test flaky.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_cannot_race_a_concurrent_manual_price_write() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::race_pool(&dir).await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    store_errored(&pool, ymd(2026, 6, 2)).await;
    let app = full_router(pool.clone(), StubFetcher::default());

    // The concurrent manual write, in flight: an ok price for the very
    // (listing, date) the delete is about, stored on a transaction left
    // uncommitted so a pool reader still sees the errored row.
    let mut tx = crate::infra::db::write_tx(&pool).await.unwrap();
    let manual = crate::test_support::closing_price(1, ymd(2026, 6, 2))
        .price("62.48")
        .manual("asx.com.au closing report", "concurrent manual entry")
        .build();
    db_store(&mut *tx, &manual).await.unwrap();

    let running = Arc::new(AtomicBool::new(false));
    let deleting = {
        let app = app.clone();
        let running = running.clone();
        tokio::spawn(async move {
            running.store(true, Ordering::SeqCst);
            delete_req(&app, "/closing_prices/1/2026-06-02").await
        })
    };
    // A condition, not a guessed duration: the handler is under way. Then a
    // grace period in which it does whatever it can — holding the write lock,
    // that is its guard reads at most, and it can never reach the delete.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !running.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "the delete never started"
        );
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !deleting.is_finished(),
        "the delete waits for the write lock rather than deciding on a stale read"
    );

    tx.commit().await.unwrap();
    let (status, bytes) = deleting.await.expect("the delete task does not panic");
    let msg = String::from_utf8_lossy(&bytes);
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the guard sees the committed ok price: {msg}"
    );
    assert!(
        msg.contains("enter another manual price to replace it"),
        "and names the manual row's replacement: {msg}"
    );
    let row = db_get_one(&pool, 1, ymd(2026, 6, 2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.status,
        PriceStatus::Ok,
        "the price the concurrent write stored is still there"
    );
}
