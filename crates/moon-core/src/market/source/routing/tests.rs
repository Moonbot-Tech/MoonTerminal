//! Catalog-first quote routing without live MoonClient fixtures.

use super::*;

/// Dropping cross-quote routing during a scan would make the screener disagree with chart reads.
#[test]
fn screener_catalog_routing_matches_per_market_reads() {
    let source = super::super::MarketDataSource::new(crate::market::MarketStore::shared(0.0));
    source.set_provider_map(&HashMap::from([(1, 1), (3, 3)]));
    source
        .set_market_provider_overrides(&HashMap::from([(3, HashMap::from([("USDT".into(), 1)]))]));
    let (provider, exchange, overrides) = source.inner.read().unwrap().market_routing(3);
    // A successful writer proves the scan's copied routing state retains no source guard.
    assert!(source.inner.try_write().is_ok());
    for (name, quote, expected) in [("BTCUSDT", "USDT", 1), ("XALPHAUSDC", "USDC", 3)] {
        let routed = market_provider(provider, exchange, overrides.as_ref(), name, Some(quote));
        assert_eq!(routed, Some(expected));
        assert_eq!(routed, source.provider_of_market(3, name));
    }
}

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
