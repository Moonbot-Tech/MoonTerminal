//! Which core an arbitrage line points at.
//!
//! Explicit imports: the chart parent re-exports `gpui::*`, whose own `test` shadows the built-in
//! attribute and makes `#[test]` expand recursively.

use moon_core::feed::ExchangeId;
use moon_core::venue::CoreVenue;

use super::venue_matches;

fn venue(code: u8, dex: &str) -> CoreVenue {
    CoreVenue {
        id: ExchangeId::with_dex(code, dex),
        dex: dex.to_string(),
        reported: String::new(),
    }
}

/// An ordinary exchange is matched by its platform ordinal, which the arbitrage code copies.
#[test]
fn an_exchange_matches_the_core_on_the_same_platform() {
    assert!(venue_matches(&venue(4, ""), 4, ""));
    assert!(!venue_matches(&venue(3, ""), 4, ""));
}

/// A Hyperliquid deployer shares the futures ordinal with every other deployer, so only its DEX
/// name identifies it — and a plain Hyperliquid core is NOT one of them.
#[test]
fn a_deployer_matches_by_its_dex_name() {
    assert!(venue_matches(&venue(13, "hyna"), 51, "hyna"));
    assert!(!venue_matches(&venue(13, "para"), 51, "hyna"));
    assert!(
        !venue_matches(&venue(13, ""), 51, "hyna"),
        "plain Hyperliquid futures is not the hyna deployer"
    );
}

/// The other direction of the same rule: a core WITH a DEX is not the exchange the ordinal names,
/// or every deployer would answer for Hyperliquid futures itself.
#[test]
fn an_exchange_does_not_match_a_core_with_a_dex() {
    assert!(!venue_matches(&venue(13, "xyz"), 13, ""));
    assert!(venue_matches(&venue(13, ""), 13, ""));
}
