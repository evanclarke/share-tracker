//! Manually entered prices — a day the provider cannot serve, priced by hand
//! with the provenance that makes the figure auditable later.

use super::*;

async fn put_json(
    app: &ApiClient,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, axum::body::Bytes) {
    let resp = app.put(uri, &body).await;
    let status = resp.status;
    let bytes = resp.body.clone();
    (status, bytes)
}

fn manual_body(price: &str) -> serde_json::Value {
    serde_json::json!({
        "price": price,
        "sourced_from": "asx.com.au closing report",
        "reason": "provider serves no candle since the delisting",
    })
}

/// A day the provider cannot serve is priced by hand, and the row records
/// both halves of its provenance — where the figure came from and why it
/// had to be entered — with the provider slot moved to `manual`.
#[tokio::test]
async fn api_manual_price_stores_the_price_with_its_provenance() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, bytes) =
        put_json(&app, "/closing_prices/1/2026-06-04", manual_body("62.48")).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(bytes.is_empty());

    let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.price, Some("62.48".parse().unwrap()));
    assert_eq!(row.status, PriceStatus::Ok);
    assert_eq!(row.origin, PriceOrigin::Manual);
    assert_eq!(row.source, "manual");
    assert_eq!(
        row.sourced_from.as_deref(),
        Some("asx.com.au closing report")
    );
    assert_eq!(
        row.reason.as_deref(),
        Some("provider serves no candle since the delisting")
    );
    assert!(row.error.is_none());
}

/// A manual price is read by valuation exactly like a fetched one: it is
/// the way a date the provider blocked forever starts producing snapshots.
#[tokio::test]
async fn manual_price_unblocks_valuation_of_an_errored_day() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    store_errored(&pool, ymd(2026, 6, 4)).await;
    let now = utc(2026, 6, 8, 9, 0);

    let blocked = crate::reports::valuation::stored_valuations(&pool, ymd(2026, 6, 4), now)
        .await
        .unwrap_err();
    assert!(
        blocked.to_string().contains("errored"),
        "setup: the day is blocked — {blocked}"
    );

    let app = full_router(pool.clone(), StubFetcher::default());
    let (status, bytes) =
        put_json(&app, "/closing_prices/1/2026-06-04", manual_body("62.48")).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{:?}", bytes);

    let valuations = crate::reports::valuation::stored_valuations(&pool, ymd(2026, 6, 4), now)
        .await
        .unwrap();
    assert_eq!(valuations.valuations.len(), 1);
    assert_eq!(
        valuations.valuations[0].native_price,
        "62.48".parse().unwrap()
    );
    assert_eq!(valuations.valuations[0].aud_price, "62.48".parse().unwrap());
}

/// Both provenance fields are required, and whitespace does not satisfy
/// them: a hand-entered figure with no sourcing or reason is exactly the
/// unauditable row the columns exist to prevent.
#[tokio::test]
async fn api_manual_price_requires_both_provenance_fields() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let app = full_router(pool.clone(), StubFetcher::default());

    for (sourced_from, reason, expected) in [
        ("   ", "provider has no candle", "sourced_from is required"),
        ("asx.com.au", "  ", "reason is required"),
    ] {
        let body = serde_json::json!({
            "price": "62.48", "sourced_from": sourced_from, "reason": reason,
        });
        let (status, bytes) = put_json(&app, "/closing_prices/1/2026-06-04", body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8_lossy(&bytes);
        assert!(msg.contains(expected), "names the missing field: {msg}");
    }
    assert!(
        db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .is_none(),
        "nothing is stored for a rejected entry"
    );
}

/// A price that can never exist is refused rather than stored: zero or
/// negative is a typo, not a close.
#[tokio::test]
async fn api_manual_price_rejects_a_non_positive_price() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let app = full_router(pool.clone(), StubFetcher::default());

    for price in ["0", "-1.50"] {
        let (status, bytes) =
            put_json(&app, "/closing_prices/1/2026-06-04", manual_body(price)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{price}");
        let msg = String::from_utf8_lossy(&bytes);
        assert!(msg.contains("must be positive"), "{msg}");
    }
}

/// The same trading-day gate as a fetch: valuation only ever reads a
/// trading day whose close is final, so a manual price on any other date
/// would be a row nothing could use.
#[tokio::test]
async fn api_manual_price_rejects_non_trading_days_and_unfinished_closes() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let app = full_router(pool.clone(), StubFetcher::default());

    // 2026-06-06 is a Saturday.
    let (status, bytes) =
        put_json(&app, "/closing_prices/1/2026-06-06", manual_body("62.48")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        String::from_utf8_lossy(&bytes).contains("not a trading day"),
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    // A date whose close cannot have happened yet.
    let future = (Utc::now() + Duration::days(30)).date_naive();
    let (status, bytes) = put_json(
        &app,
        &format!("/closing_prices/1/{future}"),
        manual_body("62.48"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        String::from_utf8_lossy(&bytes).contains("not final yet"),
        "{}",
        String::from_utf8_lossy(&bytes)
    );
}

/// SCENARIOS L-15. A crypto asset trades every calendar day, so the
/// trading-day gate that refuses a weekend price on an exchange listing
/// must let the same Saturday through for an exchange-less one — otherwise
/// the way out of a day the provider has no candle for (a hand-entered
/// price) would be closed on two days in every seven.
#[tokio::test]
async fn api_manual_price_accepts_a_weekend_day_for_crypto_only() {
    let pool = test_pool().await;
    insert_crypto_listing(&pool, 1, "BTC").await;
    insert_listing(&pool, 2, "BHP", "XASX", "AUD").await;
    let app = full_router(pool.clone(), StubFetcher::default());

    // 2026-06-06 is a Saturday: a trading day for BTC, not for the ASX.
    let (status, bytes) =
        put_json(&app, "/closing_prices/1/2026-06-06", manual_body("91000")).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let stored = db_get_one(&pool, 1, ymd(2026, 6, 6))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.price, Some(Decimal::from(91000)));

    let (status, bytes) =
        put_json(&app, "/closing_prices/2/2026-06-06", manual_body("62.48")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        String::from_utf8_lossy(&bytes).contains("not a trading day"),
        "{}",
        String::from_utf8_lossy(&bytes)
    );
}

#[tokio::test]
async fn api_manual_price_unknown_listing_is_404() {
    let pool = test_pool().await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, _) = put_json(&app, "/closing_prices/9/2026-06-04", manual_body("62.48")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The provider never takes a hand-priced day back: an explicit re-fetch
/// is refused, so a deliberate correction cannot be lost to a stray click
/// — and the refusal quotes the reason so the user sees why it exists.
#[tokio::test]
async fn api_fetch_refuses_to_replace_a_manual_price() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    seed_manual_price(&pool, 1, ymd(2026, 6, 4), "62.48").await;
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 4), "99.99", "AUD");
    let app = full_router(pool.clone(), stub);

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/fetch",
        serde_json::json!({ "listing_id": 1, "price_date": "2026-06-04" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let msg = String::from_utf8_lossy(&bytes);
    assert!(msg.contains("entered manually"), "{msg}");
    assert!(
        msg.contains("provider serves no candle"),
        "quotes why: {msg}"
    );

    let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.price, Some("62.48".parse().unwrap()), "untouched");
    assert_eq!(row.origin, PriceOrigin::Manual);
}

/// Nor is a manual price deletable — it is an ok row, so the same rule
/// that stops a fetched price being deleted applies, and the message
/// points at the only way to change it.
#[tokio::test]
async fn api_delete_rejects_a_manual_price() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    seed_manual_price(&pool, 1, ymd(2026, 6, 4), "62.48").await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let (status, bytes) = delete_req(&app, "/closing_prices/1/2026-06-04").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let msg = String::from_utf8_lossy(&bytes);
    assert!(msg.contains("enter another manual price"), "{msg}");
    assert!(
        db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .is_some()
    );
}

/// Neither the scheduled run nor a backfill over the range clobbers a
/// hand-entered price: both skip every date already stored ok, which a
/// manual row is.
#[tokio::test]
async fn collection_and_backfill_leave_a_manual_price_alone() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    seed_manual_price(&pool, 1, ymd(2026, 6, 4), "62.48").await;

    let week = asx_lookback_window();
    let mut stub = StubFetcher::default();
    for &d in &week {
        stub = stub.with_close(1, d, "99.99", "AUD");
    }
    run_collection(&pool, &stub, friday_evening_sydney())
        .await
        .unwrap();
    let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.price, Some("62.48".parse().unwrap()), "not re-fetched");
    assert_eq!(row.origin, PriceOrigin::Manual);
    // The other days of the window were collected normally.
    let other = db_get_one(&pool, 1, ymd(2026, 6, 5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(other.origin, PriceOrigin::Fetched);

    let app = full_router(pool.clone(), StubFetcher::default());
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-05" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{:?}", bytes);
    let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.price, Some("62.48".parse().unwrap()), "still manual");
    assert_eq!(row.origin, PriceOrigin::Manual);
}

/// Correcting a manual price keeps the superseded one: the upsert is an
/// UPDATE, so the audit trail (0021) holds the old figure *and* the
/// sourcing and reason given for it. Without that, re-entering a price
/// would quietly destroy the record of why the first one was entered —
/// which is what made auditing this table worth the surrogate key.
#[tokio::test]
async fn revising_a_manual_price_retains_the_superseded_provenance() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let first = serde_json::json!({
        "price": "62.48",
        "sourced_from": "asx.com.au closing report",
        "reason": "provider serves no candle since the delisting",
    });
    let (status, _) = put_json(&app, "/closing_prices/1/2026-06-04", first).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let stored = db_get_one(&pool, 1, ymd(2026, 6, 4))
        .await
        .unwrap()
        .unwrap();

    let corrected = serde_json::json!({
        "price": "64.28",
        "sourced_from": "the registry's own statement",
        "reason": "the first entry transposed two digits",
    });
    let (status, _) = put_json(&app, "/closing_prices/1/2026-06-04", corrected).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The row keeps its identity across the correction — one audit trail,
    // not two rows.
    let now = db_get_one(&pool, 1, ymd(2026, 6, 4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(now.id, stored.id, "the surrogate key survives an upsert");
    assert_eq!(now.price, Some("64.28".parse().unwrap()));

    let history = crate::reports::row_history::db_row_history(&pool, "closing_prices", now.id)
        .await
        .unwrap();
    assert_eq!(history.len(), 1, "one recorded prior version");
    let prior = &history[0];
    assert_eq!(prior["operation"], "UPDATE");
    assert_eq!(prior["price"], "62.48");
    assert_eq!(prior["sourced_from"], "asx.com.au closing report");
    assert_eq!(
        prior["reason"],
        "provider serves no candle since the delisting"
    );
}
