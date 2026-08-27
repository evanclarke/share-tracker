//! Market context: which day a price belongs to, and the as-at identity —
//! the symbol and the exchange calendar both follow the date, not today.

use super::*;

#[tokio::test]
async fn db_close_time_gates_same_day_collection() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();

    // Friday 15:00 Sydney (05:00 UTC): before the 16:00 close → Thursday.
    let before_close = utc(2026, 6, 5, 5, 0);
    assert_eq!(
        market.latest_complete_trading_day(before_close).unwrap(),
        Some(ymd(2026, 6, 4))
    );
    // Friday 18:00 Sydney: after the close → Friday itself.
    assert_eq!(
        market
            .latest_complete_trading_day(friday_evening_sydney())
            .unwrap(),
        Some(ymd(2026, 6, 5))
    );
}

#[tokio::test]
async fn db_weekends_and_holidays_walk_back_to_a_trading_day() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    // Sunday 18:00 Sydney → Friday (weekend skipped).
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        market
            .latest_complete_trading_day(utc(2026, 6, 7, 8, 0))
            .unwrap(),
        Some(ymd(2026, 6, 5))
    );

    // With Friday seeded as a holiday, the walk lands on Thursday.
    exchange_holiday::db_upsert(
        &pool,
        &exchange_holiday::ExchangeHoliday {
            id: 0,
            mic: "XASX".to_string(),
            holiday_date: ymd(2026, 6, 5),
            name: "Test Holiday".to_string(),
        },
    )
    .await
    .unwrap();
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        market
            .latest_complete_trading_day(utc(2026, 6, 7, 8, 0))
            .unwrap(),
        Some(ymd(2026, 6, 4))
    );
}

#[tokio::test]
async fn db_crypto_cutoff_is_utc_midnight_with_no_holiday_calendar() {
    let pool = test_pool().await;
    insert_crypto_listing(&pool, 1, "BTC").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    // Sunday 01:30 UTC: Saturday's UTC candle is complete — weekends and
    // holiday calendars don't apply to a continuously-trading asset.
    assert_eq!(
        market
            .latest_complete_trading_day(utc(2026, 6, 7, 1, 30))
            .unwrap(),
        Some(ymd(2026, 6, 6))
    );
}

/// The prompting case (LAAC → LAR): a fetch of a date *before* the rename
/// asks the provider for the symbol the security was actually quoted
/// under then, with no `symbol` override supplied by the caller.
#[tokio::test]
async fn db_yahoo_symbol_resolves_as_at_the_date_across_a_rename() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
    rename_listing(&pool, 1, ymd(2025, 1, 27), "LAR", None).await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    assert_eq!(yahoo_symbol(&market, ymd(2025, 1, 26)).unwrap(), "LAAC");
    // The effective date itself is already the new identity.
    assert_eq!(yahoo_symbol(&market, ymd(2025, 1, 27)).unwrap(), "LAR");
    assert_eq!(yahoo_symbol(&market, ymd(2025, 6, 1)).unwrap(), "LAR");
    // A live quote is always a question about today.
    assert_eq!(yahoo_symbol_now(&market).unwrap(), "LAR");
}

/// An exchange change moves the derived suffix too: the same security is
/// `OLD.AX` before it moved and plain `NEW` after.
#[tokio::test]
async fn db_yahoo_symbol_follows_the_exchange_in_force_on_the_date() {
    let pool = test_pool().await;
    // Quoted in USD from the start, so the move to the NYSE crosses no
    // currency boundary (a rename that did is refused — SCENARIOS R-01);
    // the symbol the *date* resolves to is what this test is about.
    insert_listing(&pool, 1, "OLD", "XASX", "USD").await;
    rename_listing(&pool, 1, ymd(2025, 3, 10), "NEW", Some("XNYS")).await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    assert_eq!(yahoo_symbol(&market, ymd(2025, 3, 7)).unwrap(), "OLD.AX");
    assert_eq!(yahoo_symbol(&market, ymd(2025, 3, 10)).unwrap(), "NEW");
}

/// `listings.price_symbol` is the *current* provider spelling, so it must
/// not be applied to a pre-rename date — an override that matched the new
/// ticker would otherwise silently re-label the old identity's history.
#[tokio::test]
async fn db_price_symbol_applies_to_the_current_identity_only() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
    rename_listing(&pool, 1, ymd(2025, 1, 27), "LAR", None).await;
    sqlx::query("UPDATE listings SET price_symbol = 'LAR-CURRENT' WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    assert_eq!(yahoo_symbol(&market, ymd(2025, 1, 26)).unwrap(), "LAAC");
    assert_eq!(
        yahoo_symbol(&market, ymd(2025, 2, 3)).unwrap(),
        "LAR-CURRENT"
    );
}

/// A trading-day question about a pre-rename date is answered by the
/// exchange that was actually open then. 2025-01-27 is Australia Day
/// (an ASX holiday, seeded) and an ordinary NYSE trading day, so the two
/// calendars disagree on exactly that date.
#[tokio::test]
async fn db_trading_days_follow_the_exchange_calendar_in_force_then() {
    let pool = test_pool().await;
    // USD from the start, for the reason
    // `db_yahoo_symbol_follows_the_exchange_in_force_on_the_date` gives:
    // the calendars are what this test is about, not the currency.
    insert_listing(&pool, 1, "OLD", "XASX", "USD").await;
    exchange_holiday::db_upsert(
        &pool,
        &exchange_holiday::ExchangeHoliday {
            id: 0,
            mic: "XASX".to_string(),
            holiday_date: ymd(2025, 1, 27),
            name: "Australia Day".to_string(),
        },
    )
    .await
    .unwrap();
    rename_listing(&pool, 1, ymd(2025, 6, 2), "NEW", Some("XNYS")).await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    // Before the move the ASX calendar applies, so the holiday is closed
    // and valuation falls back to the previous trading day.
    assert_eq!(
        market.latest_trading_day_on_or_before(ymd(2025, 1, 27)),
        Some(ymd(2025, 1, 24))
    );
    // After the move, NYSE's calendar — which has no such holiday — is
    // what a date is tested against.
    assert_eq!(
        market.latest_trading_day_on_or_before(ymd(2025, 6, 3)),
        Some(ymd(2025, 6, 3))
    );
}

/// A fetch range straddling a rename is one call per identity, each under
/// the symbol quoted over its own span — never one call for the lot under
/// today's symbol.
#[tokio::test]
async fn db_fetch_straddling_a_rename_calls_the_provider_once_per_identity() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
    rename_listing(&pool, 1, ymd(2026, 6, 3), "LAR", None).await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let days = [
        ymd(2026, 6, 1),
        ymd(2026, 6, 2),
        ymd(2026, 6, 3),
        ymd(2026, 6, 4),
    ];
    let mut stub = StubFetcher::default();
    for &d in &days {
        stub = stub.with_close(1, d, "2.77", "USD");
    }
    let (ok, errored) = fetch_and_store(&pool, &stub, &market, &days).await.unwrap();
    assert_eq!((ok, errored), (4, 0));

    assert_eq!(
        stub.calls(),
        vec![
            (1, ymd(2026, 6, 1), ymd(2026, 6, 2)),
            (1, ymd(2026, 6, 3), ymd(2026, 6, 4)),
        ],
        "the range splits at the effective date"
    );
    assert_eq!(stub.symbols(), vec!["LAAC".to_string(), "LAR".to_string()]);
}

/// A wholly pre-rename backfill is self-healing: the operator supplies no
/// `symbol`, and the old one is read off the rename chain.
#[tokio::test]
async fn api_backfill_before_a_rename_uses_the_old_symbol_without_an_override() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
    rename_listing(&pool, 1, ymd(2026, 6, 3), "LAR", None).await;

    let stub = Arc::new(StubFetcher::default().with_close(1, ymd(2026, 6, 1), "2.77", "USD"));
    let shared: SharedFetcher = stub.clone();
    let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(shared)));
    let (status, bytes) = post_json(
        &app,
        "/closing_prices/backfill",
        serde_json::json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-01" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    assert_eq!(stub.symbols(), vec!["LAAC".to_string()]);
    let row = db_get_one(&pool, 1, ymd(2026, 6, 1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, PriceStatus::Ok);
    assert_eq!(row.price, Some("2.77".parse().unwrap()));
}

/// The zero-candle message is judged per segment, so it names the symbol
/// that actually came back empty rather than today's — and the segment
/// that *did* return candles still stores its ok rows.
#[tokio::test]
async fn db_a_dead_segment_errors_alone_and_names_its_own_symbol() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
    rename_listing(&pool, 1, ymd(2026, 6, 3), "LAR", None).await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    // Only the post-rename day has a candle; the old symbol serves none.
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 3), "2.77", "USD");
    let days = [ymd(2026, 6, 2), ymd(2026, 6, 3)];
    let (ok, errored) = fetch_and_store(&pool, &stub, &market, &days).await.unwrap();
    assert_eq!((ok, errored), (1, 1));

    let dead = db_get_one(&pool, 1, ymd(2026, 6, 2))
        .await
        .unwrap()
        .unwrap();
    let msg = dead.error.unwrap();
    assert!(
        msg.contains("LAAC"),
        "names the dead segment's symbol: {msg}"
    );
    assert!(!msg.contains("LAR"), "not the current symbol: {msg}");
    let good = db_get_one(&pool, 1, ymd(2026, 6, 3))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(good.status, PriceStatus::Ok);
    // Advice for a pre-rename span: price_symbol cannot reach it.
    assert!(
        msg.contains("backfill this range with an explicit symbol"),
        "names the remedy that applies to a pre-rename span: {msg}"
    );
    assert!(!msg.contains("set price_symbol"), "{msg}");
}

/// The dead-symbol diagnostic has to fire on the **error** path too:
/// Yahoo answers a retired ticker with 400/"Not found", not with an empty
/// 200, so the pre-rename half of a straddling backfill used to store a
/// bare HTTP string with nothing saying why it failed (SCENARIOS R-06).
#[tokio::test]
async fn db_a_no_such_symbol_failure_is_diagnosed_not_stored_bare() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "FB", "XNYS", "USD").await;
    rename_listing(&pool, 1, ymd(2022, 6, 9), "META", None).await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::failing_no_such_symbol(
        "yahoo fetch for FB failed: Unexpected response \
             status: 400 at https://query2.finance.yahoo.com/v8/finance/chart/FB",
    );
    let days = [ymd(2022, 6, 8)];
    let (ok, errored) = fetch_and_store(&pool, &stub, &market, &days).await.unwrap();
    assert_eq!((ok, errored), (0, 1));

    let row = db_get_one(&pool, 1, ymd(2022, 6, 8))
        .await
        .unwrap()
        .unwrap();
    let msg = row.error.unwrap();
    assert!(
        msg.contains("status: 400"),
        "keeps the provider's own words: {msg}"
    );
    assert!(msg.contains("FB"), "names the symbol that failed: {msg}");
    assert!(
        msg.contains("may be wrong, renamed, or delisted"),
        "says why it may have failed: {msg}"
    );
    assert_eq!(row.fetched_symbol.as_deref(), Some("FB"));
}

/// ...and only on that path. A 5xx, a rate limit or a dropped connection
/// is not evidence the symbol is wrong, so the row keeps the provider's
/// error and gains no diagnosis it cannot support.
#[tokio::test]
async fn db_a_transient_failure_is_never_diagnosed_as_a_dead_symbol() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "FB", "XNYS", "USD").await;
    rename_listing(&pool, 1, ymd(2022, 6, 9), "META", None).await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::failing("yahoo fetch for FB failed: Server error 503 at https://q");
    let days = [ymd(2022, 6, 8)];
    let (ok, errored) = fetch_and_store(&pool, &stub, &market, &days).await.unwrap();
    assert_eq!((ok, errored), (0, 1));

    let msg = db_get_one(&pool, 1, ymd(2022, 6, 8))
        .await
        .unwrap()
        .unwrap()
        .error
        .unwrap();
    assert_eq!(
        msg, "yahoo fetch for FB failed: Server error 503 at https://q",
        "the provider's error, unembellished"
    );
    assert!(!msg.contains("renamed"), "{msg}");
    assert!(!msg.contains("price_symbol"), "{msg}");
}

/// The advice is span-aware. For a date in the listing's **current**
/// identity, `price_symbol` is consulted, so it is worth naming.
#[tokio::test]
async fn db_dead_symbol_advice_names_price_symbol_for_a_current_span() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "FB", "XNYS", "USD").await;
    rename_listing(&pool, 1, ymd(2022, 6, 9), "META", None).await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::failing_no_such_symbol("yahoo fetch for META failed: Not found");
    let days = [ymd(2026, 6, 2)];
    fetch_and_store(&pool, &stub, &market, &days).await.unwrap();

    let msg = db_get_one(&pool, 1, ymd(2026, 6, 2))
        .await
        .unwrap()
        .unwrap()
        .error
        .unwrap();
    assert!(msg.contains("set price_symbol on the listing"), "{msg}");
}

/// ...and for a **pre-rename** date it is not: `yahoo_symbol_for` applies
/// `price_symbol` to the current identity only, so setting it could not
/// fix this fetch. Only the backfill `symbol` override can.
#[tokio::test]
async fn db_dead_symbol_advice_for_an_earlier_span_names_the_backfill_override() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "FB", "XNYS", "USD").await;
    rename_listing(&pool, 1, ymd(2022, 6, 9), "META", None).await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::failing_no_such_symbol("yahoo fetch for FB failed: Not found");
    let days = [ymd(2022, 6, 8)];
    fetch_and_store(&pool, &stub, &market, &days).await.unwrap();

    let msg = db_get_one(&pool, 1, ymd(2022, 6, 8))
        .await
        .unwrap()
        .unwrap()
        .error
        .unwrap();
    assert!(
        msg.contains("backfill this range with an explicit symbol"),
        "{msg}"
    );
    assert!(
        !msg.contains("set price_symbol"),
        "advice that cannot reach a pre-rename span: {msg}"
    );
}

/// The dead-symbol verdict is read off the provider's **typed** error, so
/// it cannot drift with the crate's or Yahoo's wording — and it is narrow:
/// only a positive "no such series" answer counts, never an outage.
#[test]
fn yahoo_classifies_only_a_no_such_series_answer_as_a_dead_symbol() {
    let url = || "https://query2.finance.yahoo.com/v8/finance/chart/FB".to_string();
    let dead = [
        yfinance_rs::YfError::NotFound { url: url() },
        yfinance_rs::YfError::Status {
            status: 400,
            url: url(),
        },
    ];
    for error in dead {
        assert!(
            matches!(
                classify_yahoo_failure("FB", error),
                FetchError::NoSuchSymbol(_)
            ),
            "a provider answer of no-such-series"
        );
    }
    let transient = [
        yfinance_rs::YfError::ServerError {
            status: 503,
            url: url(),
        },
        yfinance_rs::YfError::RateLimited { url: url() },
        yfinance_rs::YfError::Status {
            status: 403,
            url: url(),
        },
        yfinance_rs::YfError::Auth("no crumb".to_string()),
    ];
    for error in transient {
        let classified = classify_yahoo_failure("FB", error);
        assert!(
            matches!(classified, FetchError::Other(_)),
            "not evidence about the symbol: {}",
            classified.message()
        );
    }
}
