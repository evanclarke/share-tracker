//! Provider-agnostic fetcher behaviour: price cleaning, and the quote cache.

use super::*;

#[test]
fn clean_price_strips_float_noise_and_keeps_tiny_prices() {
    let cases = [
        ("62.4799995422363", "62.48"), // 62.48f32 — the live BHP.AX shape
        ("99545.3515625", "99545.35"), // 99545.35f32 — the live BTC-AUD shape
        ("141.5", "141.5"),
        ("0.000012345678", "0.00001234568"), // sub-$1: significance starts at the 1
    ];
    for (input, expected) in cases {
        assert_eq!(
            clean_price(input.parse().unwrap()),
            expected.parse::<Decimal>().unwrap(),
            "clean_price({input})"
        );
    }
}

/// The window is what the cache is for: a second valuation inside it is
/// answered without the provider being asked again.
#[tokio::test]
async fn a_quote_inside_the_window_is_answered_without_asking_again() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_listing(&pool, 2, "WBC", "XASX", "AUD").await;
    let as_of = utc(2026, 6, 5, 6, 30);
    let inner = Arc::new(
        StubFetcher::default()
            .with_quote(1, "62.48", "AUD", as_of)
            .with_quote(2, "30", "AUD", as_of),
    );
    let cache = CachingFetcher::new(inner.clone(), std::time::Duration::from_secs(300));

    let first = fetch_live_aud_prices(&pool, &cache, &[1, 2]).await.unwrap();
    let second = fetch_live_aud_prices(&pool, &cache, &[1, 2]).await.unwrap();

    assert_eq!(
        inner.quote_batches(),
        vec![vec![1, 2]],
        "the second valuation asked the provider nothing"
    );
    // And answered with the same figures, not an empty or defaulted row.
    for prices in [&first, &second] {
        assert_eq!(
            prices[&1].as_ref().unwrap().aud_price,
            "62.48".parse::<Decimal>().unwrap()
        );
        assert_eq!(prices[&2].as_ref().unwrap().aud_price, Decimal::from(30));
    }
    // The provider's own quote timestamp survives the cache: a served
    // row states when the price was *observed*, never when it was served.
    assert_eq!(second[&1].as_ref().unwrap().as_of, as_of.to_rfc3339());
}

/// Past the window the provider is asked again — the cache is a window,
/// not a memo. A zero TTL is the same code path with the window shut, and
/// is how a caller turns the cache off.
#[tokio::test]
async fn a_quote_past_the_window_is_fetched_again() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let as_of = utc(2026, 6, 5, 6, 30);
    let inner = Arc::new(StubFetcher::default().with_quote(1, "62.48", "AUD", as_of));
    let cache = CachingFetcher::new(inner.clone(), std::time::Duration::ZERO);

    fetch_live_aud_prices(&pool, &cache, &[1]).await.unwrap();
    fetch_live_aud_prices(&pool, &cache, &[1]).await.unwrap();

    assert_eq!(
        inner.quote_batches(),
        vec![vec![1], vec![1]],
        "an expired entry is re-fetched"
    );
}

/// Only the listings the cache cannot answer reach the provider, and they
/// go as one batch — the caching and the batching compose rather than one
/// undoing the other. The answers must still land on the right listings,
/// which is what the interleaving here is for.
#[tokio::test]
async fn only_the_misses_are_asked_for_and_they_go_as_one_batch() {
    let pool = test_pool().await;
    for id in 1..=4 {
        insert_listing(&pool, id, &format!("T{id}"), "XASX", "AUD").await;
    }
    let as_of = utc(2026, 6, 5, 6, 30);
    let inner = Arc::new(
        StubFetcher::default()
            .with_quote(1, "10", "AUD", as_of)
            .with_quote(2, "20", "AUD", as_of)
            .with_quote(3, "30", "AUD", as_of)
            .with_quote(4, "40", "AUD", as_of),
    );
    let cache = CachingFetcher::new(inner.clone(), std::time::Duration::from_secs(300));

    // Warm 2 and 4, so the next call's misses are 1 and 3 — interleaved,
    // so a positional slip would show up as swapped prices.
    fetch_live_aud_prices(&pool, &cache, &[2, 4]).await.unwrap();
    let all = fetch_live_aud_prices(&pool, &cache, &[1, 2, 3, 4])
        .await
        .unwrap();

    assert_eq!(
        inner.quote_batches(),
        vec![vec![2, 4], vec![1, 3]],
        "one batch carrying only the two listings not already known"
    );
    for (id, expected) in [(1, 10), (2, 20), (3, 30), (4, 40)] {
        assert_eq!(
            all[&id].as_ref().unwrap().aud_price,
            Decimal::from(expected),
            "listing {id} kept its own price across the cache/fetch mix"
        );
    }
}

/// A failure is never remembered: an outage or a rate limit must be
/// retried on the next request, not pinned for the whole window. The
/// recovery is the point — the second call here succeeds.
#[tokio::test]
async fn a_failed_quote_is_not_remembered() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let as_of = utc(2026, 6, 5, 6, 30);
    // Quotes listing 2 only, so listing 1 fails.
    let down = Arc::new(StubFetcher::default());
    let cache = CachingFetcher::new(down, std::time::Duration::from_secs(300));
    let failed = fetch_live_aud_prices(&pool, &cache, &[1]).await.unwrap();
    assert!(failed[&1].is_err());

    // A fresh cache over a working provider stands in for the outage
    // ending; what matters is that nothing was pinned by the failure.
    let up = Arc::new(StubFetcher::default().with_quote(1, "62.48", "AUD", as_of));
    let cache = CachingFetcher::new(up.clone(), std::time::Duration::from_secs(300));
    fetch_live_aud_prices(&pool, &cache, &[1]).await.unwrap();
    let recovered = fetch_live_aud_prices(&pool, &cache, &[1]).await.unwrap();
    assert_eq!(
        recovered[&1].as_ref().unwrap().aud_price,
        "62.48".parse::<Decimal>().unwrap()
    );
    assert_eq!(
        up.quote_batches(),
        vec![vec![1]],
        "the recovered quote is then cached like any other"
    );
}

/// Price *history* is never cached — it is already persisted in
/// `closing_prices`, and the `price-import` job that collects it must
/// reach the provider every time it runs.
#[tokio::test]
async fn history_fetches_are_not_cached() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let day = ymd(2026, 6, 5);
    let inner = Arc::new(StubFetcher::default().with_close(1, day, "62.48", "AUD"));
    let cache = CachingFetcher::new(inner.clone(), std::time::Duration::from_secs(300));
    let market = load_market(&pool, 1).await.unwrap().unwrap();

    cache.daily_closes(&market, day, day).await.unwrap();
    cache.daily_closes(&market, day, day).await.unwrap();

    assert_eq!(
        inner.calls(),
        vec![(1, day, day), (1, day, day)],
        "both history fetches reached the provider"
    );
}
