//! Catalog-first quote routing without live MoonClient fixtures.

use super::*;

/// Guessing from opaque or misleading names would select a tape in the wrong quote currency.
#[test]
fn catalog_quote_wins_over_market_spelling() {
    let overrides = HashMap::from([("USDT".into(), 1), ("USDC".into(), 2), ("USDH".into(), 4)]);
    for (market, quote, expected) in [
        ("BTCUSDT", "usdc ", 2),
        ("@254", "USDH", 4),
        ("@334", "USDC", 2),
        ("BTCPERP", "USDC", 2),
    ] {
        assert_eq!(
            market_provider(
                Some(3),
                Exchange::Unknown,
                Some(&overrides),
                market,
                Some(quote)
            ),
            Some(expected),
            "{market}"
        );
    }
}

/// Missing catalog quotes must retain name fallback; an entirely unknown quote stays home.
#[test]
fn missing_catalog_quote_uses_name_then_own_provider() {
    let overrides = HashMap::from([("USDT".into(), 1)]);
    for catalog_quote in [None, Some(""), Some("  ")] {
        assert_eq!(
            market_provider(
                Some(3),
                Exchange::Unknown,
                Some(&overrides),
                "BTCUSDT",
                catalog_quote
            ),
            Some(1)
        );
        assert_eq!(
            market_provider(
                Some(3),
                Exchange::Unknown,
                Some(&overrides),
                "@254",
                catalog_quote
            ),
            Some(3)
        );
    }
    assert_eq!(quote_provider(None, Some(&overrides), "USDT"), None);
}

/// A currency-rate lookup must choose its USDT tape even without a spellable market name.
#[test]
fn known_quote_routes_without_a_market_name() {
    let overrides = HashMap::from([("USDT".into(), 1)]);
    assert_eq!(quote_provider(Some(3), Some(&overrides), "USDT"), Some(1));
    assert_eq!(quote_provider(Some(3), Some(&overrides), "USDH"), Some(3));
}
