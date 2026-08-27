//! The Yahoo symbol mapping.

use super::*;

#[test]
fn yahoo_symbols_cover_asx_us_and_crypto() {
    let mk = |mic: Option<&str>, ticker: &str, ccy: &str| {
        let b = crate::test_support::listing(1)
            .ticker(ticker)
            .name(ticker)
            .currency(ccy);
        let listing = match mic {
            Some(m) => b.mic(m).security_type(listing::SecurityType::Share),
            None => b.crypto(),
        }
        .build();
        Market::unrenamed(listing, None, HashSet::new())
    };
    let d = ymd(2024, 6, 3);
    assert_eq!(
        yahoo_symbol(&mk(Some("XASX"), "BHP", "AUD"), d).unwrap(),
        "BHP.AX"
    );
    assert_eq!(
        yahoo_symbol(&mk(Some("XNYS"), "ICE", "USD"), d).unwrap(),
        "ICE"
    );
    assert_eq!(yahoo_symbol(&mk(None, "BTC", "AUD"), d).unwrap(), "BTC-AUD");
    assert!(
        yahoo_symbol(&mk(Some("XLON"), "BARC", "GBP"), d)
            .unwrap_err()
            .contains("XLON")
    );
}

/// `listings.price_symbol` overrides the derived mapping (a symbol the
/// provider spells differently, or an exchange with no mapping at all).
#[test]
fn yahoo_symbol_prefers_the_listings_stored_price_symbol_override() {
    let mut market = Market::unrenamed(
        crate::test_support::listing(1)
            .mic("XLON")
            .security_type(listing::SecurityType::Share)
            .ticker("BARC")
            .build(),
        None,
        HashSet::new(),
    );
    let d = ymd(2024, 6, 3);
    // XLON has no derived mapping, so without an override it errors...
    assert!(yahoo_symbol(&market, d).is_err());
    // ...but a stored price_symbol resolves it.
    market.listing.price_symbol = Some("BARC.L".to_string());
    assert_eq!(yahoo_symbol(&market, d).unwrap(), "BARC.L");
}

/// A one-off `symbol_override` (backfill's `symbol` param) wins over even
/// a stored `price_symbol` — it's for a single deliberate fetch, e.g.
/// recovering pre-rename history under the old symbol.
#[test]
fn yahoo_symbol_override_wins_over_the_stored_price_symbol() {
    let mut market = Market::unrenamed(
        crate::test_support::listing(1)
            .mic("XNYS")
            .security_type(listing::SecurityType::Share)
            .ticker("LAR")
            .price_symbol("LAR-CURRENT")
            .build(),
        None,
        HashSet::new(),
    );
    market.symbol_override = Some("LAAC-OLD".to_string());
    assert_eq!(yahoo_symbol(&market, ymd(2024, 6, 3)).unwrap(), "LAAC-OLD");
}
