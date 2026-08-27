//! On-demand live valuation: latest quote per listing, converted to AUD.

use super::*;

#[tokio::test]
async fn live_aud_prices_converts_quote_currency_and_carries_as_of() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_listing(&pool, 2, "ICE", "XNYS", "USD").await;
    // 2 USD per AUD for June 2026 → US$141.50 = A$70.75.
    sqlx::query("INSERT INTO rba_fx_rates (currency, month, rate) VALUES ('USD', '2026-06', '2')")
        .execute(&pool)
        .await
        .unwrap();
    let as_of = utc(2026, 6, 5, 6, 30);
    let fetcher = StubFetcher::default()
        .with_quote(1, "62.48", "AUD", as_of)
        .with_quote(2, "141.50", "USD", as_of);

    let prices = fetch_live_aud_prices(&pool, &fetcher, &[1, 2])
        .await
        .unwrap();
    let bhp = prices[&1].as_ref().unwrap();
    assert_eq!(bhp.aud_price, "62.48".parse::<Decimal>().unwrap());
    assert_eq!(bhp.as_of, as_of.to_rfc3339());
    let ice = prices[&2].as_ref().unwrap();
    assert_eq!(ice.aud_price, "70.75".parse::<Decimal>().unwrap());
}

#[tokio::test]
async fn live_aud_prices_surface_failures_instead_of_zeroing() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await; // provider down
    insert_listing(&pool, 2, "ICE", "XNYS", "USD").await; // currency mismatch
    insert_listing(&pool, 3, "VAS", "XASX", "USD").await; // no ATO rate for the quote month

    // Listing 1: blanket failure.
    let down = fetch_live_aud_prices(&pool, &StubFetcher::failing("provider down"), &[1])
        .await
        .unwrap();
    assert!(down[&1].as_ref().unwrap_err().contains("provider down"));

    let as_of = utc(2026, 6, 5, 6, 30);
    // Listing 2: provider quotes AUD for a USD listing.
    let mismatch = StubFetcher::default().with_quote(2, "141.50", "AUD", as_of);
    let m = fetch_live_aud_prices(&pool, &mismatch, &[2]).await.unwrap();
    assert!(m[&2].as_ref().unwrap_err().contains("currency mismatch"));

    // Listing 3: USD quote but no ATO rate imported for the quote month.
    let unconvertible = StubFetcher::default().with_quote(3, "10.00", "USD", as_of);
    let u = fetch_live_aud_prices(&pool, &unconvertible, &[3])
        .await
        .unwrap();
    assert!(u[&3].as_ref().unwrap_err().contains("no ATO FX rate"));
}

/// Live valuation is a whole-portfolio question, so the provider is asked
/// **once** — one batch carrying every held listing, not one round trip
/// per holding. This is the point of `PriceFetcher::latest_quotes`: the
/// old per-listing loop made the Portfolio Overview screen's load time
/// grow with the portfolio (~500 ms of provider latency per holding).
#[tokio::test]
async fn live_valuation_asks_the_price_source_once_for_the_whole_portfolio() {
    let pool = test_pool().await;
    let as_of = utc(2026, 6, 5, 6, 30);
    let mut fetcher = StubFetcher::default();
    for id in 1..=4 {
        insert_listing(&pool, id, &format!("T{id}"), "XASX", "AUD").await;
        fetcher = fetcher.with_quote(id, "10", "AUD", as_of);
    }

    let prices = fetch_live_aud_prices(&pool, &fetcher, &[1, 2, 3, 4])
        .await
        .unwrap();

    assert_eq!(prices.len(), 4, "every listing is valued");
    assert_eq!(
        fetcher.quote_batches(),
        vec![vec![1, 2, 3, 4]],
        "one call carrying all four listings, not four calls"
    );
}

/// A batched answer is matched back to the listing it belongs to, and a
/// listing that never reached the provider (deleted since the holdings
/// were read) still gets its own reason. Mis-assigning one holding's
/// price to another would be a silent valuation error, so the pairing is
/// pinned rather than assumed.
#[tokio::test]
async fn a_batched_answer_is_paired_back_to_each_listing() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_listing(&pool, 2, "WBC", "XASX", "AUD").await;
    insert_listing(&pool, 3, "CBA", "XASX", "AUD").await;
    let as_of = utc(2026, 6, 5, 6, 30);
    // Listing 2 is deliberately unquoted: its failure must stay its own.
    let fetcher = StubFetcher::default()
        .with_quote(1, "62.48", "AUD", as_of)
        .with_quote(3, "180.10", "AUD", as_of);

    // Listing 99 does not exist at all.
    let prices = fetch_live_aud_prices(&pool, &fetcher, &[1, 2, 3, 99])
        .await
        .unwrap();

    assert_eq!(
        prices[&1].as_ref().unwrap().aud_price,
        "62.48".parse::<Decimal>().unwrap()
    );
    assert!(prices[&2].as_ref().unwrap_err().contains("listing 2"));
    assert_eq!(
        prices[&3].as_ref().unwrap().aud_price,
        "180.10".parse::<Decimal>().unwrap()
    );
    assert!(
        prices[&99]
            .as_ref()
            .unwrap_err()
            .contains("no longer exists")
    );
    assert_eq!(
        fetcher.quote_batches(),
        vec![vec![1, 2, 3]],
        "the missing listing never occupies a slot in the request"
    );
}

/// The trait's default `latest_quotes` — what a fetcher that cannot batch
/// inherits by implementing nothing. It must still answer positionally
/// and in full, since `fetch_live_aud_prices` pairs by position.
/// `QuoteStub` deliberately does not override it, so this exercises the
/// default body.
#[tokio::test]
async fn the_default_batch_is_the_per_market_loop_answered_positionally() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_listing(&pool, 2, "WBC", "XASX", "AUD").await;
    let as_of = utc(2026, 6, 5, 6, 30);
    let fetcher = test_support::QuoteStub::default().with_quote(2, "30", "AUD", as_of);
    let markets = [
        load_market(&pool, 1).await.unwrap().unwrap(),
        load_market(&pool, 2).await.unwrap().unwrap(),
    ];
    let borrowed: Vec<&Market> = markets.iter().collect();

    let quotes = fetcher.latest_quotes(&borrowed).await;

    assert_eq!(quotes.len(), 2, "one result per market, always");
    assert!(quotes[0].as_ref().unwrap_err().contains("listing 1"));
    assert_eq!(
        quotes[1].as_ref().unwrap().price,
        Decimal::from(30),
        "the second market's own quote, in its own slot"
    );
}

/// Yahoo answers a multi-symbol request in an order of its own and simply
/// omits a symbol it cannot serve, so `yahoo_quote_named` pairs by symbol.
/// Pairing by position instead would hand one holding another's price —
/// wrong figures with nothing to show for it — which is why this is
/// pinned here rather than left to the provider's habits.
#[test]
fn a_yahoo_batch_is_read_by_symbol_not_by_position() {
    let as_of = utc(2026, 6, 5, 6, 30);
    // Answered in the reverse of the order asked, and missing VGS.AX.
    let quotes = vec![
        yahoo_quote("WBC.AX", "30.10", as_of),
        yahoo_quote("BHP.AX", "62.48", as_of),
    ];

    assert_eq!(
        yahoo_quote_named(&quotes, "BHP.AX").unwrap().price,
        "62.48".parse::<Decimal>().unwrap()
    );
    assert_eq!(
        yahoo_quote_named(&quotes, "WBC.AX").unwrap().price,
        "30.10".parse::<Decimal>().unwrap()
    );
    // The symbol the provider dropped is its own failure, not another
    // listing's price.
    assert!(
        yahoo_quote_named(&quotes, "VGS.AX")
            .unwrap_err()
            .contains("no quote for VGS.AX")
    );
    // The crate canonicalises to uppercase; matching survives either way.
    assert!(yahoo_quote_named(&quotes, "bhp.ax").is_ok());
}

#[tokio::test]
async fn resolve_live_prices_skips_overridden_and_respects_the_flag() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_listing(&pool, 2, "WBC", "XASX", "AUD").await;
    let as_of = utc(2026, 6, 5, 6, 30);
    let fetcher = StubFetcher::default()
        .with_quote(1, "62.48", "AUD", as_of)
        .with_quote(2, "30", "AUD", as_of);

    // live = false → nothing fetched.
    let off = resolve_live_prices(&pool, Some(&fetcher), false, &HashMap::new(), [1, 2])
        .await
        .unwrap();
    assert!(off.is_empty());

    // live = true, listing 1 overridden → only listing 2 is fetched.
    let overrides = HashMap::from([(1i64, "99".parse::<Decimal>().unwrap())]);
    let on = resolve_live_prices(&pool, Some(&fetcher), true, &overrides, [1, 2])
        .await
        .unwrap();
    assert!(!on.contains_key(&1), "overridden listing is never fetched");
    assert_eq!(on[&2].as_ref().unwrap().aud_price, Decimal::from(30));

    // live = true with no fetcher → each listing marked unavailable.
    let none = resolve_live_prices(&pool, None, true, &HashMap::new(), [1])
        .await
        .unwrap();
    assert!(none[&1].as_ref().unwrap_err().contains("unavailable"));
}
