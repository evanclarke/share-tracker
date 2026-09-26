//! The fetch, backfill and list endpoints, and the provider symbol every
//! stored row records alongside its figure.

use super::*;

#[tokio::test]
async fn api_backfill_fetches_only_missing_trading_days() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    // Week of Mon 2026-06-01 .. Fri 2026-06-05; Wednesday already stored ok.
    let pre = StubFetcher::default().with_close(1, ymd(2026, 6, 3), "64.91", "AUD");
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    fetch_and_store(&pool, &pre, &market, &[ymd(2026, 6, 3)])
        .await
        .unwrap();

    let fetcher = StubFetcher::default()
        .with_close(1, ymd(2026, 6, 1), "62.48", "AUD")
        .with_close(1, ymd(2026, 6, 2), "63.37", "AUD")
        .with_close(1, ymd(2026, 6, 4), "62.80", "AUD")
        .with_close(1, ymd(2026, 6, 5), "61.24", "AUD");
    let app = full_router(pool.clone(), fetcher);

    // Sat..Sat range: weekends are not trading days, Wednesday is skipped.
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({ "listing_id": 1, "from": "2026-05-30", "to": "2026-06-06" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let summary: BackfillSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.trading_days, 5);
    assert_eq!(summary.already_stored, 1);
    assert_eq!(summary.fetched_ok, 4);
    assert_eq!(summary.errored, 0);

    let rows = db_list(&pool, Some(1), None, None, None).await.unwrap();
    assert_eq!(rows.len(), 5);
    assert!(rows.iter().all(|r| r.status == PriceStatus::Ok));
    // Wednesday kept its original fetch (the stub reports the provider's own
    // source both ways, but the pre-stored price is unchanged).
    let wed = db_get_one(&pool, 1, ymd(2026, 6, 3))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wed.price, Some("64.91".parse().unwrap()));
}

/// The backfill body's optional `symbol` reaches the fetcher as a one-off
/// override — recovering a pre-rename date range under the old symbol
/// without touching `listings.price_symbol`.
#[tokio::test]
async fn api_backfill_symbol_override_reaches_the_fetcher() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAR", "XNYS", "USD").await;
    insert_buy(&pool, 1, 1, "100").await;

    let fetcher = Arc::new(StubFetcher::default().with_close(1, ymd(2026, 6, 1), "10", "USD"));
    let shared: SharedFetcher = fetcher.clone();
    let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(shared)));

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({
            "listing_id": 1, "from": "2026-06-01", "to": "2026-06-01",
            "symbol": "LAAC-OLD"
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(fetcher.symbols(), vec!["LAAC-OLD".to_string()]);
    // The listing's own stored symbol is untouched by the one-off override.
    assert_eq!(
        listing::db_get(&pool, 1)
            .await
            .unwrap()
            .unwrap()
            .price_symbol,
        None
    );
}

/// Every fetched row records the provider symbol it was fetched under —
/// on an ordinary fetch too, not only an overridden one, so the stored
/// answer to "what symbol produced this row?" is never a null that has to
/// be interpreted (migration 0038).
#[tokio::test]
async fn db_a_fetched_row_records_the_symbol_it_was_fetched_under() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 4), "62.80", "AUD");

    fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 4)])
        .await
        .unwrap();

    let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.fetched_symbol.as_deref(), Some("BHP.AX"));
}

/// A failed fetch records the symbol it was *attempted* under: the symbol
/// is as much of the provenance of a failure as of a price, and a wrong
/// one is the usual reason for the failure.
#[tokio::test]
async fn db_a_failed_fetch_records_the_symbol_it_was_attempted_under() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();

    fetch_and_store(&pool, &StubFetcher::default(), &market, &[ymd(2026, 6, 4)])
        .await
        .unwrap();

    let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, PriceStatus::Error);
    assert_eq!(row.fetched_symbol.as_deref(), Some("LAAC"));
}

/// A range straddling a rename is fetched under one symbol per identity,
/// and each stored row records *its own* segment's symbol — not one
/// symbol for the lot.
#[tokio::test]
async fn db_each_row_records_its_own_segments_symbol_across_a_rename() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
    rename_listing(&pool, 1, ymd(2026, 6, 3), "LAR", None).await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let days = [ymd(2026, 6, 2), ymd(2026, 6, 4)];
    let mut stub = StubFetcher::default();
    for &d in &days {
        stub = stub.with_close(1, d, "2.77", "USD");
    }
    fetch_and_store(&pool, &stub, &market, &days).await.unwrap();

    let mut symbols = Vec::new();
    for &d in &days {
        symbols.push(
            db_get_one(&pool, 1, d)
                .await
                .unwrap()
                .unwrap()
                .fetched_symbol,
        );
    }
    assert_eq!(
        symbols,
        vec![Some("LAAC".to_string()), Some("LAR".to_string())]
    );
}

/// The incident this column exists for (TODO "LAC's whole pre-demerger
/// price history is LAR's series"): a backfill run with the one-off
/// `symbol` override stored 260 rows of another security's series under
/// the listing's own id, and nothing recorded which symbol produced them.
/// Now every such row names it on its face — and a later re-fetch under
/// the ordinary symbol *replaces* the record rather than leaving the row
/// asserting a symbol it no longer came from.
#[tokio::test]
async fn api_backfill_records_the_overriding_symbol_on_every_stored_row() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
    insert_buy(&pool, 1, 1, "100").await;
    let days = [ymd(2026, 6, 1), ymd(2026, 6, 2)];
    let mut stub = StubFetcher::default();
    for &d in &days {
        stub = stub.with_close(1, d, "10", "USD");
    }
    let app = full_router(pool.clone(), stub);

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({
            "listing_id": 1, "from": "2026-06-01", "to": "2026-06-02",
            "symbol": "LAAC"
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    for &d in &days {
        let row = db_get_one(&pool, 1, d).await.unwrap().unwrap();
        assert_eq!(
            row.fetched_symbol.as_deref(),
            Some("LAAC"),
            "the row names the symbol that produced it, not the listing's own"
        );
        assert_ne!(row.fetched_symbol.as_deref(), Some("LAC"));
    }

    // Re-fetching without the override moves the record with the figure:
    // a row must never keep the symbol of a write it is no longer from.
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/fetch",
        serde_json::json!({ "listing_id": 1, "price_date": "2026-06-01" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let row = db_get_one(&pool, 1, days[0]).await.unwrap().unwrap();
    assert_eq!(row.fetched_symbol.as_deref(), Some("LAC"));
}

/// The recorded symbol is served by `GET /closing_prices` — the column is
/// provenance for a person to read, so it has to reach the list the
/// Closing Prices screen renders, not just the row.
#[tokio::test]
async fn api_list_serves_the_symbol_a_row_was_fetched_under() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
    crate::test_support::closing_price(1, ymd(2026, 6, 1))
        .fetched_symbol("LAAC")
        .insert(&pool)
        .await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let rows: Vec<serde_json::Value> = app.get_json("/closing_prices").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["fetched_symbol"], "LAAC");
}

/// A hand-entered price is fetched under no symbol at all, so it records
/// none — the column is CHECK-paired with the origin (0038), the way
/// `sourced_from`/`reason` are paired the other way round.
#[tokio::test]
async fn api_a_manual_price_records_no_fetched_symbol() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let app = full_router(pool.clone(), StubFetcher::default());

    let resp = app
        .put(
            "/closing_prices/1/2026-06-04",
            &serde_json::json!({
                "price": "62.48",
                "sourced_from": "asx.com.au closing report",
                "reason": "provider serves no candle for the day"
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::CREATED);

    let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.origin, PriceOrigin::Manual);
    assert_eq!(row.fetched_symbol, None);
}

/// The cheap cross-check on top of recording the symbol: whatever symbol
/// the provider was asked for, the currency it answers in must be the
/// listing's. A mismatch is an errored row for the day — the same
/// treatment as any other provider failure, so the wrong figure is never
/// stored and the reason is on the record — and the row still names the
/// overriding symbol that produced it.
#[tokio::test]
async fn api_backfill_under_an_override_stores_a_currency_mismatch_as_an_error() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
    insert_buy(&pool, 1, 1, "100").await;
    // The override reaches a security quoted in another currency — the
    // clearest evidence a symbol names a different security altogether.
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 1), "10", "AUD");
    let app = full_router(pool.clone(), stub);

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({
            "listing_id": 1, "from": "2026-06-01", "to": "2026-06-01",
            "symbol": "LAAC.AX"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let summary: BackfillSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!((summary.fetched_ok, summary.errored), (0, 1));

    let row = db_get_one(&pool, 1, ymd(2026, 6, 1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, PriceStatus::Error);
    assert_eq!(row.price, None, "no figure is stored from a foreign series");
    let msg = row.error.unwrap();
    assert!(msg.contains("currency mismatch"), "{msg}");
    assert!(msg.contains("AUD") && msg.contains("USD"), "{msg}");
    assert_eq!(row.fetched_symbol.as_deref(), Some("LAAC.AX"));
}

#[tokio::test]
async fn api_backfill_records_missing_candles_as_errors() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    // Provider has Thu+Fri but nothing for Mon-Wed (e.g. an unseeded
    // historical holiday or missing provider data) — those days must be
    // recorded as errored rows, never silently missing.
    let fetcher = StubFetcher::default()
        .with_close(1, ymd(2026, 6, 4), "62.80", "AUD")
        .with_close(1, ymd(2026, 6, 5), "61.24", "AUD");
    let app = full_router(pool.clone(), fetcher);

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-05" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let summary: BackfillSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.fetched_ok, 2);
    assert_eq!(summary.errored, 3);

    let row = db_get_one(&pool, 1, ymd(2026, 6, 2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, PriceStatus::Error);
    assert!(row.error.as_deref().unwrap().contains("no candle"));
}

/// A provider call that returns *zero* candles across the whole
/// requested window (as opposed to a data gap on one date among others)
/// is the classic wrong/renamed/delisted-symbol case — every date's
/// errored row names the symbol and points at the fix, instead of the
/// generic per-day message that's indistinguishable from a transient
/// outage.
#[tokio::test]
async fn fetch_and_store_names_the_symbol_when_the_whole_window_returns_no_candles() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
    insert_buy(&pool, 1, 1, "100").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();

    // Fetcher has no data for this listing at all — Ok(vec![]).
    let empty = StubFetcher::default();
    let dates = [ymd(2026, 6, 1), ymd(2026, 6, 2)];
    let (ok, errored) = fetch_and_store(&pool, &empty, &market, &dates)
        .await
        .unwrap();
    assert_eq!(ok, 0);
    assert_eq!(errored, 2);

    for date in dates {
        let row = db_get_one(&pool, 1, date).await.unwrap().unwrap();
        assert_eq!(row.status, PriceStatus::Error);
        let msg = row.error.unwrap();
        assert!(msg.contains("LAAC"), "names the symbol: {msg}");
        assert!(msg.contains("renamed"), "points at the cause: {msg}");
        // A single-identity listing, so this window *is* the current
        // span and price_symbol is a remedy that can reach it.
        assert!(
            msg.contains("set price_symbol on the listing"),
            "points at the fix: {msg}"
        );
    }
}

#[tokio::test]
async fn api_backfill_unknown_listing_404_and_bad_range_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let app = full_router(pool, StubFetcher::default());

    let (status, _) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({ "listing_id": 99, "from": "2026-06-01", "to": "2026-06-05" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({ "listing_id": 1, "from": "2026-06-05", "to": "2026-06-01" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn api_fetch_answers_201_with_the_created_row() {
    // `POST /closing_prices/fetch` returns the row it fetched and stored, so
    // it answers `201 Created` — the same "returns the created row" signal
    // every other such POST gives (REST API audit 2026-09-24), not the `200`
    // it used to.
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    // The stub serves Thu 2026-06-04 and nothing for Fri 2026-06-05.
    let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 4), "62.80", "AUD");
    let app = full_router(pool.clone(), fetcher);

    let (status, bytes) = post_json(
        &app,
        "/closing_prices/fetch",
        serde_json::json!({ "listing_id": 1, "price_date": "2026-06-04" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let row: ClosingPrice = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(row.status, PriceStatus::Ok);
    assert_eq!(row.listing_id, 1);
    assert_eq!(row.price_date, ymd(2026, 6, 4));
    assert_eq!(row.price, Some("62.80".parse().unwrap()));
    // The body is the stored row, not a copy: the same id the list serves.
    let stored = db_get_one(&pool, 1, ymd(2026, 6, 4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.id, stored.id);
    assert_eq!(row.origin, stored.origin);

    // An errored fetch is still a created row, so it is still `201` — the
    // status says a row was stored, and the row's own `status` says whether
    // it carries a price.
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/fetch",
        serde_json::json!({ "listing_id": 1, "price_date": "2026-06-05" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let row: ClosingPrice = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(row.status, PriceStatus::Error);
    assert_eq!(row.price, None);
}

#[tokio::test]
async fn api_fetch_replaces_errored_row_and_returns_it() {
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

    let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "62.48", "AUD");
    let app = full_router(pool.clone(), fetcher);
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/fetch",
        serde_json::json!({ "listing_id": 1, "price_date": "2026-06-05" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let row: ClosingPrice = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(row.status, PriceStatus::Ok);
    assert_eq!(row.price, Some("62.48".parse().unwrap()));

    let stored = db_get_one(&pool, 1, ymd(2026, 6, 5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.status,
        PriceStatus::Ok,
        "the errored row was replaced"
    );
}

#[tokio::test]
async fn api_fetch_rejects_incomplete_and_non_trading_days() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let app = full_router(pool, StubFetcher::default());

    // Far future: the close cannot be final.
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/fetch",
        serde_json::json!({ "listing_id": 1, "price_date": "2099-01-04" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(String::from_utf8_lossy(&bytes).contains("not final"));

    // A Saturday well in the past: not a trading day.
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/fetch",
        serde_json::json!({ "listing_id": 1, "price_date": "2024-01-06" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(String::from_utf8_lossy(&bytes).contains("not a trading day"));

    // Unknown listing.
    let (status, _) = post_json(
        &app,
        "/closing_prices/fetch",
        serde_json::json!({ "listing_id": 99, "price_date": "2024-01-05" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_list_filters_by_listing_and_date_range_including_errors() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_listing(&pool, 2, "ICE", "XNYS", "USD").await;
    let market1 = load_market(&pool, 1).await.unwrap().unwrap();
    let market2 = load_market(&pool, 2).await.unwrap().unwrap();
    let ok = StubFetcher::default()
        .with_close(1, ymd(2026, 6, 4), "62.80", "AUD")
        .with_close(1, ymd(2026, 6, 5), "61.24", "AUD");
    fetch_and_store(&pool, &ok, &market1, &[ymd(2026, 6, 4), ymd(2026, 6, 5)])
        .await
        .unwrap();
    fetch_and_store(
        &pool,
        &StubFetcher::failing("down"),
        &market2,
        &[ymd(2026, 6, 5)],
    )
    .await
    .unwrap();

    let app = full_router(pool, StubFetcher::default());
    let get = |uri: &str| {
        let app = app.clone();
        let uri = uri.to_string();
        async move {
            let resp = app.get(uri).await;
            assert_eq!(resp.status, StatusCode::OK);
            let bytes = resp.body.clone();
            serde_json::from_slice::<Vec<ClosingPrice>>(&bytes).unwrap()
        }
    };

    assert_eq!(
        get("/closing_prices").await.len(),
        3,
        "errored rows are listed too"
    );
    assert_eq!(get("/closing_prices?listing_id=1").await.len(), 2);
    let one_day = get("/closing_prices?from=2026-06-05&to=2026-06-05").await;
    assert_eq!(one_day.len(), 2);
    assert!(one_day.iter().any(|r| r.status == PriceStatus::Error));
}

/// REST API audit 2026-09-24, item B7. The list interleaves errored rows
/// (`status: "error"`, `price` null) with ok ones, so a client computing a
/// valuation had to filter them out itself. `?status=ok` is that clean series
/// in one call; `?status=error` is the fetch-failure list; omitting it is every
/// row, which is the unchanged default the Closing Prices screen reads. The
/// filter ANDs with the existing listing/date bounds, matches nothing as an
/// empty `200` array, and a value the typed enum cannot read is refused `400`
/// naming the parameter (never a silently-ignored unknown status).
#[tokio::test]
async fn api_list_filters_by_status() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_listing(&pool, 2, "ICE", "XNYS", "USD").await;
    async fn seed(pool: &SqlitePool, listing_id: i64, date: NaiveDate, price: &str, errored: bool) {
        let row = crate::test_support::closing_price(listing_id, date).price(price);
        if errored {
            row.errored("no candle for the day").insert(pool).await;
        } else {
            row.insert(pool).await;
        }
    }
    // A mix across two listings and two days: four ok rows and three errored.
    seed(&pool, 1, ymd(2026, 6, 4), "62.80", false).await;
    seed(&pool, 1, ymd(2026, 6, 5), "61.24", false).await;
    seed(&pool, 1, ymd(2026, 5, 1), "60.00", true).await;
    seed(&pool, 1, ymd(2026, 5, 2), "60.00", true).await;
    seed(&pool, 2, ymd(2026, 6, 5), "99.99", false).await;
    seed(&pool, 2, ymd(2026, 5, 1), "99.00", false).await;
    seed(&pool, 2, ymd(2026, 5, 2), "99.00", true).await;

    let app = full_router(pool.clone(), StubFetcher::default());
    let get = |uri: &str| {
        let app = app.clone();
        let uri = uri.to_string();
        async move {
            let resp = app.get(uri.clone()).await;
            assert_eq!(resp.status, StatusCode::OK, "{uri}");
            let bytes = resp.body.clone();
            serde_json::from_slice::<Vec<ClosingPrice>>(&bytes).unwrap()
        }
    };

    // Omitted: every row, errored ones included — the default is unchanged.
    let all = get("/closing_prices").await;
    assert_eq!(all.len(), 7, "the default still lists every row");
    assert_eq!(
        all.iter()
            .filter(|r| r.status == PriceStatus::Error)
            .count(),
        3
    );

    // The "one call" the item is about: only the clean figures.
    let ok = get("/closing_prices?status=ok").await;
    assert_eq!(ok.len(), 4);
    assert!(
        ok.iter().all(|r| r.status == PriceStatus::Ok),
        "no errored row survives ?status=ok"
    );
    assert!(
        ok.iter().all(|r| r.price.is_some()),
        "every ok row carries a price, so a valuation needs no client-side filter"
    );

    // …and the fetch-failure list.
    let errored = get("/closing_prices?status=error").await;
    assert_eq!(errored.len(), 3);
    assert!(errored.iter().all(|r| r.status == PriceStatus::Error));
    assert!(errored.iter().all(|r| r.price.is_none()));

    // Every filter ANDs together.
    assert_eq!(get("/closing_prices?status=ok&listing_id=1").await.len(), 2);
    let bounded = get("/closing_prices?status=ok&listing_id=1&from=2026-06-05&to=2026-06-05").await;
    assert_eq!(bounded.len(), 1);
    assert_eq!(bounded[0].listing_id, 1);
    assert_eq!(bounded[0].price_date, ymd(2026, 6, 5));

    // A filter matching nothing is an empty `200`, never a `404`.
    assert!(
        get("/closing_prices?status=error&listing_id=2&from=2026-06-04&to=2026-06-05")
            .await
            .is_empty()
    );

    // A value the typed enum cannot read is refused `400` naming the
    // parameter — never silently read as "no filter".
    let resp = app.get("/closing_prices?status=maybe").await;
    let (status, body) = resp.status_and_body();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.contains("status"),
        "the refusal names the parameter: {body}"
    );
}
