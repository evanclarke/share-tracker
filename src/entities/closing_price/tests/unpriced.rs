//! The `unpriced_from` / `unpriced_before` markers: what collection, fetch,
//! backfill and delete each do inside a span the provider does not serve, and
//! clearing a superseded span wholesale.

use super::*;

/// Mark an already-inserted listing as unpriced before `before`: the
/// provider's series for it begins then, so every stored row earlier than
/// it is superseded by the listing's own declaration.
async fn mark_unpriced_before(
    pool: &SqlitePool,
    id: i64,
    ticker: &str,
    before: NaiveDate,
) -> listing::Listing {
    let marked = crate::test_support::listing(id)
        .ticker(ticker)
        .name(ticker)
        .security_type(listing::SecurityType::Share)
        .unpriced_before(before)
        .build();
    listing::db_upsert(pool, &marked).await.unwrap();
    marked
}

/// The one relaxation of the ok-row rule, and the case it exists for: a
/// span the listing itself declares unpriceable, stored from another
/// security's series. Valuation excludes the holding from those dates
/// rather than pricing it, so no stored figure was ever valued at these
/// rows and deleting them punches no hole — whichever way they arrived.
#[tokio::test]
async fn api_delete_removes_an_ok_row_the_unpriced_before_marker_supersedes() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    crate::test_support::closing_price(1, ymd(2026, 6, 2))
        .price("10.13")
        .insert(&pool)
        .await;
    seed_manual_price(&pool, 1, ymd(2026, 6, 3), "9.87").await;
    mark_unpriced_before(&pool, 1, "LAC", ymd(2026, 6, 4)).await;
    let app = full_router(pool.clone(), StubFetcher::default());

    for date in ["2026-06-02", "2026-06-03"] {
        let (status, bytes) = delete_req(&app, &format!("/closing_prices/1/{date}")).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{date}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    assert!(
        db_get_one(&pool, 1, ymd(2026, 6, 2))
            .await
            .unwrap()
            .is_none(),
        "the fetched row is gone"
    );
    assert!(
        db_get_one(&pool, 1, ymd(2026, 6, 3))
            .await
            .unwrap()
            .is_none(),
        "the hand-entered row goes the same way — origin decides nothing here"
    );
}

/// The relaxation stops exactly at the marker: a row on the day the
/// series begins, or after it, is an ordinary priced day again and the
/// original refusal stands word for word.
#[tokio::test]
async fn api_delete_still_rejects_an_ok_row_on_or_after_unpriced_before() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
    crate::test_support::closing_price(1, ymd(2026, 6, 4))
        .price("10.13")
        .insert(&pool)
        .await;
    crate::test_support::closing_price(1, ymd(2026, 6, 5))
        .price("10.50")
        .insert(&pool)
        .await;
    mark_unpriced_before(&pool, 1, "LAC", ymd(2026, 6, 4)).await;
    let app = full_router(pool.clone(), StubFetcher::default());

    for date in ["2026-06-04", "2026-06-05"] {
        let (status, bytes) = delete_req(&app, &format!("/closing_prices/1/{date}")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{date}");
        let msg = String::from_utf8_lossy(&bytes);
        assert!(msg.contains("is ok, not errored"), "{date}: {msg}");
        assert!(msg.contains("re-fetch it"), "{date}: {msg}");
        assert!(
            db_get_one(&pool, 1, date.parse().unwrap())
                .await
                .unwrap()
                .is_some(),
            "{date} is still stored"
        );
    }
}

/// The two markers are **not** symmetric, and this is why the relaxation
/// is only at one end. A date on or after `unpriced_from` *is* valued —
/// `reports::valuation` carries the last stored ok close forward into it
/// — so deleting a row there could remove the very figure being carried.
/// The refusal stands.
#[tokio::test]
async fn api_delete_still_rejects_an_ok_row_inside_an_unpriced_from_run() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SUSP", "XASX", "AUD").await;
    crate::test_support::closing_price(1, ymd(2026, 6, 2))
        .price("3.10")
        .insert(&pool)
        .await;
    seed_manual_price(&pool, 1, ymd(2026, 6, 4), "2.95").await;
    let marked = crate::test_support::listing(1)
        .ticker("SUSP")
        .name("SUSP")
        .security_type(listing::SecurityType::Share)
        .unpriced_from(ymd(2026, 6, 3))
        .build();
    listing::db_upsert(&pool, &marked).await.unwrap();
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, bytes) = delete_req(&app, "/closing_prices/1/2026-06-04").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        String::from_utf8_lossy(&bytes).contains("is ok, not errored"),
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    // …and that row really is what a valuation of the unpriced run reads.
    assert_eq!(
        db_latest_ok_price_on_or_before(&pool, 1, ymd(2026, 6, 10), None)
            .await
            .unwrap(),
        Some((ymd(2026, 6, 4), "2.95".parse().unwrap())),
        "the refused row is the carried-forward figure"
    );
}

/// Nothing is destroyed: a superseded row's figure and the provenance
/// that says what it was land in the audit trail, which is the property
/// the whole cleanup rests on.
#[tokio::test]
async fn deleting_a_superseded_price_is_recorded_in_the_audit_trail() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
    seed_manual_price(&pool, 1, ymd(2026, 6, 3), "9.87").await;
    let row = db_get_one(&pool, 1, ymd(2026, 6, 3))
        .await
        .unwrap()
        .unwrap();
    mark_unpriced_before(&pool, 1, "LAC", ymd(2026, 6, 4)).await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, _) = delete_req(&app, "/closing_prices/1/2026-06-03").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let history = crate::reports::row_history::db_row_history(&pool, "closing_prices", row.id)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["operation"], "DELETE");
    assert_eq!(history[0]["price"], "9.87");
    assert_eq!(history[0]["sourced_from"], "asx.com.au closing report");
    assert_eq!(history[0]["reason"], "provider serves no candle");
}

/// The bulk form: hundreds of borrowed days are not a runbook one DELETE
/// at a time. The span is the listing's own marker — never a caller's
/// date range — so it clears exactly what the declaration supersedes,
/// leaves the priced days alone, says how many rows went, and is safe to
/// run again.
#[tokio::test]
async fn api_clear_unpriced_before_clears_exactly_the_superseded_span() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
    insert_listing(&pool, 2, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    // Listing 1: two borrowed ok rows and an errored one before the
    // marker, one real row on the day the series begins.
    crate::test_support::closing_price(1, ymd(2026, 6, 1))
        .price("10.13")
        .insert(&pool)
        .await;
    seed_manual_price(&pool, 1, ymd(2026, 6, 2), "9.87").await;
    crate::test_support::closing_price(1, ymd(2026, 6, 3))
        .errored("no candle")
        .insert(&pool)
        .await;
    crate::test_support::closing_price(1, ymd(2026, 6, 4))
        .price("24.90")
        .insert(&pool)
        .await;
    // Another listing's row on a date inside the span stays put: the
    // marker is listing 1's declaration and nobody else's.
    crate::test_support::closing_price(2, ymd(2026, 6, 1))
        .price("62.48")
        .insert(&pool)
        .await;
    mark_unpriced_before(&pool, 1, "LAC", ymd(2026, 6, 4)).await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/clear_unpriced_before",
        serde_json::json!({ "listing_id": 1 }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let summary: ClearSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.listing_id, 1);
    assert_eq!(summary.unpriced_before, ymd(2026, 6, 4));
    assert_eq!(summary.deleted, 3, "both ok rows and the errored one");

    for gone in [ymd(2026, 6, 1), ymd(2026, 6, 2), ymd(2026, 6, 3)] {
        assert!(
            db_get_one(&pool, 1, gone).await.unwrap().is_none(),
            "{gone} was superseded"
        );
    }
    assert!(
        db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .is_some(),
        "the day the series begins is a real price"
    );
    assert!(
        db_get_one(&pool, 2, ymd(2026, 6, 1))
            .await
            .unwrap()
            .is_some(),
        "another listing's prices are not in this listing's span"
    );

    // Idempotent: re-running clears nothing and says so.
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/clear_unpriced_before",
        serde_json::json!({ "listing_id": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let again: ClearSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(again.deleted, 0);
}

/// Without a marker there is no superseded span, so there is nothing this
/// endpoint may clear — it must never become a bulk-delete of real price
/// history. An unknown listing is the ordinary 404.
#[tokio::test]
async fn api_clear_unpriced_before_is_refused_without_a_marker() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    crate::test_support::closing_price(1, ymd(2026, 6, 2))
        .price("62.48")
        .insert(&pool)
        .await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/clear_unpriced_before",
        serde_json::json!({ "listing_id": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let msg = String::from_utf8_lossy(&bytes);
    assert!(msg.contains("BHP has no unpriced_before"), "{msg}");
    assert!(
        db_get_one(&pool, 1, ymd(2026, 6, 2))
            .await
            .unwrap()
            .is_some(),
        "nothing was cleared"
    );

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/clear_unpriced_before",
        serde_json::json!({ "listing_id": 99 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(String::from_utf8_lossy(&bytes), "no such listing");
}

/// The audit trail is per row, not per statement: the `AFTER DELETE`
/// trigger fires once for each row of the multi-row DELETE, so a cleared
/// span leaves every figure and every `reason` recoverable — including
/// the note explaining what the borrowed prices were.
#[tokio::test]
async fn clearing_a_span_records_every_row_in_the_audit_trail() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
    crate::test_support::closing_price(1, ymd(2026, 6, 1))
        .price("10.13")
        .insert(&pool)
        .await;
    seed_manual_price(&pool, 1, ymd(2026, 6, 2), "9.87").await;
    let fetched = db_get_one(&pool, 1, ymd(2026, 6, 1))
        .await
        .unwrap()
        .unwrap();
    let manual = db_get_one(&pool, 1, ymd(2026, 6, 2))
        .await
        .unwrap()
        .unwrap();
    mark_unpriced_before(&pool, 1, "LAC", ymd(2026, 6, 4)).await;

    let cleared = db_clear_unpriced_before(&pool, 1).await.unwrap();
    assert_eq!(
        cleared,
        ClearOutcome::Cleared {
            unpriced_before: ymd(2026, 6, 4),
            deleted: 2,
        }
    );

    let history = crate::reports::row_history::db_row_history(&pool, "closing_prices", fetched.id)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["operation"], "DELETE");
    assert_eq!(history[0]["price"], "10.13");

    let history = crate::reports::row_history::db_row_history(&pool, "closing_prices", manual.id)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["operation"], "DELETE");
    assert_eq!(history[0]["price"], "9.87");
    assert_eq!(history[0]["reason"], "provider serves no candle");
}

/// Clearing the span cannot break the other marker's write-time pairing:
/// `unpriced_from` needs a stored ok price *before* it to carry forward,
/// and that check only ever looks at rows on or after `unpriced_before` —
/// exactly the rows the clear leaves alone.
#[tokio::test]
async fn clearing_a_span_leaves_the_carry_forward_price_and_its_rule_intact() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
    crate::test_support::closing_price(1, ymd(2026, 6, 1))
        .price("10.13")
        .insert(&pool)
        .await;
    crate::test_support::closing_price(1, ymd(2026, 6, 4))
        .price("24.90")
        .insert(&pool)
        .await;
    let marked = crate::test_support::listing(1)
        .ticker("LAC")
        .name("LAC")
        .security_type(listing::SecurityType::Share)
        .unpriced_before(ymd(2026, 6, 4))
        .unpriced_from(ymd(2026, 6, 5))
        .build();
    listing::db_upsert(&pool, &marked).await.unwrap();

    db_clear_unpriced_before(&pool, 1).await.unwrap();

    assert_eq!(
        db_latest_ok_price_on_or_before(&pool, 1, ymd(2026, 6, 9), Some(ymd(2026, 6, 4)))
            .await
            .unwrap(),
        Some((ymd(2026, 6, 4), "24.90".parse().unwrap())),
        "the figure the unpriced run carries forward is untouched"
    );
    // …and the pairing still accepts a re-save of the listing.
    listing::db_upsert(&pool, &marked).await.unwrap();
}

/// SCENARIOS Q-02: a listing marked `unpriced_from` is not fetched from
/// that date on — every call would only store another errored row, fail
/// the job, and nag from health forever. The days *before* it are still
/// collected.
#[tokio::test]
async fn collection_skips_a_listing_from_its_unpriced_from_date() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "ATVI", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    for &d in &asx_lookback_window() {
        if d < ymd(2026, 6, 3) {
            seed_ok_price(&pool, 1, d).await;
        }
    }
    let marked = crate::test_support::listing(1)
        .ticker("ATVI")
        .name("ATVI")
        .mic("XASX")
        .security_type(listing::SecurityType::Share)
        .unpriced_from(ymd(2026, 6, 3))
        .build();
    listing::db_upsert(&pool, &marked).await.unwrap();

    let fetcher = StubFetcher::default();
    run_collection(&pool, &fetcher, friday_evening_sydney())
        .await
        .unwrap();
    assert!(
        fetcher.calls().is_empty(),
        "nothing left to fetch before the date, nothing fetched after it: {:?}",
        fetcher.calls()
    );
    assert!(
        db_get_one(&pool, 1, ymd(2026, 6, 5))
            .await
            .unwrap()
            .is_none(),
        "no errored row is stored for a day the provider cannot serve"
    );
}

/// The explicit paths refuse the same dates: a single re-fetch is `422`
/// naming the marker, and a backfill crossing it fills the priced part
/// and stops.
#[tokio::test]
async fn api_fetch_and_backfill_stop_at_unpriced_from() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "ATVI", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    seed_ok_price(&pool, 1, ymd(2026, 6, 1)).await;
    let marked = crate::test_support::listing(1)
        .ticker("ATVI")
        .name("ATVI")
        .mic("XASX")
        .security_type(listing::SecurityType::Share)
        .unpriced_from(ymd(2026, 6, 3))
        .build();
    listing::db_upsert(&pool, &marked).await.unwrap();

    let mut stub = StubFetcher::default();
    for d in [ymd(2026, 6, 2), ymd(2026, 6, 3), ymd(2026, 6, 4)] {
        stub = stub.with_close(1, d, "94.42", "AUD");
    }
    let app = full_router(pool.clone(), stub);

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/fetch",
        serde_json::json!({ "listing_id": 1, "price_date": "2026-06-04" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let msg = String::from_utf8_lossy(&bytes);
    assert!(msg.contains("unpriced from 2026-06-03"), "{msg}");

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-04" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let summary: BackfillSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.trading_days, 2, "1 and 2 June, not 3 or 4");
    assert_eq!(summary.already_stored, 1);
    assert_eq!(summary.fetched_ok, 1);
    assert_eq!(summary.errored, 0);
    assert!(
        db_get_one(&pool, 1, ymd(2026, 6, 3))
            .await
            .unwrap()
            .is_none()
    );

    // A range wholly inside the unpriced run is refused outright.
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({ "listing_id": 1, "from": "2026-06-03", "to": "2026-06-04" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        String::from_utf8_lossy(&bytes).contains("unpriced from 2026-06-03"),
        "{}",
        String::from_utf8_lossy(&bytes)
    );
}

/// Migration 0037, the mirror: a listing marked `unpriced_before` is not
/// fetched *earlier* than that date — the provider's series has not begun
/// and every call would only store an errored row. The days from it on
/// are still collected.
#[tokio::test]
async fn collection_skips_a_listing_before_its_unpriced_before_date() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    let marked = crate::test_support::listing(1)
        .ticker("LAC")
        .name("LAC")
        .mic("XASX")
        .security_type(listing::SecurityType::Share)
        .unpriced_before(ymd(2026, 6, 4))
        .build();
    listing::db_upsert(&pool, &marked).await.unwrap();

    let mut stub = StubFetcher::default();
    for &d in &asx_lookback_window() {
        stub = stub.with_close(1, d, "24.90", "AUD");
    }
    run_collection(&pool, &stub, friday_evening_sydney())
        .await
        .unwrap();

    for &d in &asx_lookback_window() {
        let stored = db_get_one(&pool, 1, d).await.unwrap();
        if d < ymd(2026, 6, 4) {
            assert!(
                stored.is_none(),
                "nothing is fetched or stored before the series begins ({d})"
            );
        } else {
            assert!(stored.is_some(), "the days from it on are collected ({d})");
        }
    }
}

/// The explicit paths refuse the same days: a single fetch before the
/// date is `422` naming the marker, and a backfill crossing it starts at
/// the date instead of storing a run of errored rows.
#[tokio::test]
async fn api_fetch_and_backfill_start_at_unpriced_before() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    let marked = crate::test_support::listing(1)
        .ticker("LAC")
        .name("LAC")
        .mic("XASX")
        .security_type(listing::SecurityType::Share)
        .unpriced_before(ymd(2026, 6, 4))
        .build();
    listing::db_upsert(&pool, &marked).await.unwrap();

    let mut stub = StubFetcher::default();
    for d in [
        ymd(2026, 6, 2),
        ymd(2026, 6, 3),
        ymd(2026, 6, 4),
        ymd(2026, 6, 5),
    ] {
        stub = stub.with_close(1, d, "24.90", "AUD");
    }
    let app = full_router(pool.clone(), stub);

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/fetch",
        serde_json::json!({ "listing_id": 1, "price_date": "2026-06-03" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let msg = String::from_utf8_lossy(&bytes);
    assert!(msg.contains("unpriced before 2026-06-04"), "{msg}");

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({ "listing_id": 1, "from": "2026-06-02", "to": "2026-06-05" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let summary: BackfillSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.trading_days, 2, "4 and 5 June, not 2 or 3");
    assert_eq!(summary.fetched_ok, 2);
    assert_eq!(summary.errored, 0);
    assert!(
        db_get_one(&pool, 1, ymd(2026, 6, 3))
            .await
            .unwrap()
            .is_none()
    );

    // A range wholly before the date is refused outright.
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-03" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        String::from_utf8_lossy(&bytes).contains("unpriced before 2026-06-04"),
        "{}",
        String::from_utf8_lossy(&bytes)
    );
}
