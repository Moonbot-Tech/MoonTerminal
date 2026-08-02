//! `MarketLabel`: how a market is named to the user.
//!
//! Every catalog value here was READ OFF a live core (`market_currency` / `base_currency` dumped
//! from the connected exchanges), not invented — the whole point of this type is that the core's
//! spelling is not derivable from the market name.

use super::{pick_market_for_coin, MarketLabel};
use crate::symbol::Exchange;

/// A catalog-sourced label, as `market_labels` builds one.
fn catalog(coin: &str, quote: &str) -> MarketLabel {
    MarketLabel {
        coin: coin.to_string(),
        quote: quote.to_string(),
        contract: None,
    }
}

/// The token is what the core matches its coin lists against, so it keeps its contract tail; the
/// coin COLUMN shows the coin. Conflating the two is what made a news click stop finding a market.
#[test]
fn token_keeps_its_contract_and_display_drops_it() {
    let perp = catalog("AAVE_RP", "");
    assert_eq!(perp.coin, "AAVE_RP");
    assert_eq!(perp.display_coin(), "AAVE");

    let dated = catalog("BNB_0925", "");
    assert_eq!(dated.display_coin(), "BNB");

    // A Bybit USDC perpetual is one token: the core reports `1kBONKPERP`, tail and all.
    let bybit = catalog("1kBONKPERP", "USDC");
    assert_eq!(bybit.display_coin(), "1kBONKPERP");
}

/// Matching goes through the shared key, so the bare coin a news item or a coin list names finds
/// the contract-qualified market the core reports.
#[test]
fn match_key_connects_a_bare_coin_to_its_contract() {
    assert_eq!(catalog("AAVE_RP", "").match_key(), "AAVE");
    assert_eq!(catalog("BNB_0925", "").match_key(), "BNB");
    assert_eq!(catalog("SOL", "USDT").match_key(), "SOL");
    // The Bybit fold is part of the coin's name, not a contract, so it survives the key.
    assert_eq!(catalog("1kBONKPERP", "USDC").match_key(), "1KBONKPERP");
}

/// A dated contract must not share a caption with its perpetual — one chart per instrument.
#[test]
fn pair_keeps_an_expiry_and_hides_the_perpetual() {
    assert_eq!(catalog("SOL", "USDT").pair(), "SOL-USDT");
    assert_eq!(catalog("BEAT", "USDT").pair(), "BEAT-USDT");
    // `_RP` is the perpetual marker; every market on a futures connection is one.
    assert_eq!(catalog("AAVE_RP", "USD").pair(), "AAVE-USD");
    assert_eq!(catalog("BNB_0925", "USD").pair(), "BNB-USD-0925");
    // No quote anywhere: the coin alone, never an invented pair.
    assert_eq!(catalog("HFUN", "").pair(), "HFUN");
}

/// Without a catalog the label falls back to the NAME, and the expiry has to survive that path
/// too — it is the only thing distinguishing two Bybit contracts of one pair.
#[test]
fn name_fallback_carries_the_expiry() {
    let dated = MarketLabel::from_name("BTCUSDT-07AUG26", Exchange::Bybit);
    assert_eq!(dated.coin, "BTC");
    assert_eq!(dated.pair(), "BTC-USDT-07AUG26");

    let perp = MarketLabel::from_name("BEAT-USDT-SWAP", Exchange::Okx);
    assert_eq!(perp.pair(), "BEAT-USDT");

    // A spot index carries no coin at all; the fallback must not invent one.
    let index = MarketLabel::from_name("@206", Exchange::Hyperliquid);
    assert_eq!(index.coin, "@206");
    assert_eq!(index.quote, "");
}

/// `(market name, its catalog label)` as the caller builds it from `market_labels`.
fn listed(name: &str, coin: &str, quote: &str) -> (String, MarketLabel) {
    (name.to_string(), catalog(coin, quote))
}

/// The reported case: the report stores the core's folded token while the market is spelled in
/// full, so a name-based comparison finds nothing and the coin opens an empty chart.
#[test]
fn a_folded_token_finds_its_market() {
    let universe = [
        listed("1000RATSUSDT", "1kRATS", "USDT"),
        listed("BTCUSDT", "BTC", "USDT"),
    ];
    assert_eq!(
        pick_market_for_coin(&universe, "1kRATS"),
        Some("1000RATSUSDT")
    );
    // The market's own name is NOT the coin, and must not be matched as one.
    assert_eq!(pick_market_for_coin(&universe, "1000RATS"), None);
}

/// A bare coin reaches the contract-qualified market through the folded key.
#[test]
fn a_bare_coin_reaches_a_contract_market() {
    let universe = [listed("AAVEUSD_PERP", "AAVE_RP", "USD")];
    assert_eq!(
        pick_market_for_coin(&universe, "AAVE"),
        Some("AAVEUSD_PERP")
    );
    assert_eq!(
        pick_market_for_coin(&universe, "AAVE_RP"),
        Some("AAVEUSD_PERP")
    );
}

/// A coin names an instrument family, so the undated contract wins however the search ranked them.
#[test]
fn the_undated_contract_wins() {
    let universe = [
        listed("BTCUSD_260925", "BTC_0925", "USD"),
        listed("BTCUSD_PERP", "BTC_RP", "USD"),
    ];
    assert_eq!(pick_market_for_coin(&universe, "BTC"), Some("BTCUSD_PERP"));
    // Asking for one expiry by its exact token still gets that expiry.
    assert_eq!(
        pick_market_for_coin(&universe, "BTC_0925"),
        Some("BTCUSD_260925")
    );
}

/// A coin no candidate carries yields nothing rather than the first row, which would open a chart
/// on a market the trade never happened on.
#[test]
fn an_absent_coin_picks_nothing() {
    let universe = [listed("BTCUSDT", "BTC", "USDT")];
    assert_eq!(pick_market_for_coin(&universe, "ETH"), None);
    assert_eq!(pick_market_for_coin(&[], "BTC"), None);
}

/// The empty label a caller gets for an unknown core must render as nothing, not as a stray dash.
#[test]
fn an_unresolved_label_renders_empty() {
    let empty = MarketLabel::default();
    assert_eq!(empty.pair(), "");
    assert_eq!(empty.display_coin(), "");
}
