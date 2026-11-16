//! Which spelling one core is sent. Each case names the edit that would send a core a token its
//! coin lists never match.

use moon_core::market::MarketLabel;

use super::{CoreName, pick_core_name};

/// A catalog row as the core labels it.
fn row(market: &str, coin: &str, canonic: &str, quote: &str) -> (String, MarketLabel) {
    (
        market.to_string(),
        MarketLabel {
            coin: coin.to_string(),
            canonic: canonic.to_string(),
            quote: quote.to_string(),
            contract: None,
        },
    )
}

fn name(market: &str, coin: &str) -> Option<CoreName> {
    Some(CoreName {
        market: market.to_string(),
        coin: coin.to_string(),
    })
}

/// A click on Bybit's `1kBONK` must reach a Binance core as `1000BONK`.
///
/// Mutation: send the clicked token to every core, as the menu did. The Binance core would store
/// `1kBONK`, a coin it does not have, and the ban would never apply.
#[test]
fn a_multiplier_coin_takes_the_target_cores_own_spelling() {
    let binance = [
        row("1000BONKUSDT", "1000BONK", "BONK", "USDT"),
        row("BONKUSDC", "BONK", "BONK", "USDC"),
    ];
    assert_eq!(
        pick_core_name(&binance, "BONK", "1000BONKPERP", "USDT", "USDC", None),
        name("1000BONKUSDT", "1000BONK")
    );
}

/// A coin that merely shares letters with the clicked one is not that coin.
///
/// Mutation: accept the best search hit when no identity matches. `PEPE` would ban `PEPECOIN`.
#[test]
fn a_lookalike_coin_is_not_taken() {
    let catalog = [row("PEPECOINUSDT", "PEPECOIN", "PEPECOIN", "USDT")];
    assert_eq!(
        pick_core_name(&catalog, "PEPE", "PEPEUSDT", "USDT", "USDT", None),
        None
    );
}

/// A click with no catalog identity still reaches a core that spells the coin the same way, and
/// only such a core.
///
/// Mutation: drop the token fallback. A second core of the same exchange — the one case the old
/// send-the-clicked-token rule got right — would fall out of "N cores" whenever the clicked market
/// had left the catalog. Mutation the other way: accept any search hit as the fallback, and
/// `1kBONK` would be written into a core that only lists `1000BONK`.
#[test]
fn a_token_without_a_catalog_identity_reaches_only_cores_that_spell_it_alike() {
    let bybit = [row("1000BONKPERP", "1kBONK", "BONK", "USDC")];
    assert_eq!(
        pick_core_name(&bybit, "1KBONK", "", "USDC", "", Some("1kBONK")),
        name("1000BONKPERP", "1kBONK")
    );
    let binance = [row("1000BONKUSDT", "1000BONK", "BONK", "USDT")];
    assert_eq!(
        pick_core_name(&binance, "1KBONK", "", "USDT", "", Some("1kBONK")),
        None
    );
}

/// Without the clicked market on the target core, the perpetual is the coin's one name.
///
/// Mutation: take the first identity match in catalog order. The quarterly `AAVE_0925` sorts ahead
/// of `AAVE_RP` and the ban would sit on a contract that expires.
#[test]
fn a_perpetual_is_chosen_over_an_expiry() {
    let coin_m = [
        row("AAVEUSD_260925", "AAVE_0925", "AAVE", "USD"),
        row("AAVEUSD_PERP", "AAVE_RP", "AAVE", "USD"),
    ];
    assert_eq!(
        pick_core_name(&coin_m, "AAVE", "AAVEUSDT", "USD", "USDT", None),
        name("AAVEUSD_PERP", "AAVE_RP")
    );
}

/// Two cores of one exchange: the market that was clicked is the market that is banned.
///
/// Mutation: drop the same-market rule and always prefer the target core's traded quote. A ban
/// clicked on `BTCUSDC` would be sent to the neighbouring core as `BTCUSDT`.
#[test]
fn the_clicked_market_wins_where_the_core_lists_it() {
    let binance = [
        row("BTCUSDT", "BTC", "BTC", "USDT"),
        row("BTCUSDC", "BTC", "BTC", "USDC"),
    ];
    assert_eq!(
        pick_core_name(&binance, "BTC", "BTCUSDC", "USDT", "USDC", None),
        name("BTCUSDC", "BTC")
    );
}

/// The quote the core trades in decides among its markets of one coin.
///
/// Mutation: pass the clicked market's quote even when the core states its own. A USDC account
/// would be handed the USDT market of a coin clicked on a USDT chart elsewhere.
#[test]
fn the_cores_own_quote_decides_between_its_markets() {
    let bybit = [
        row("SOLUSDT", "SOL", "SOL", "USDT"),
        row("SOLUSDC", "SOL", "SOL", "USDC"),
    ];
    assert_eq!(
        pick_core_name(&bybit, "SOL", "SOL-USDT-SWAP", "USDC", "USDT", None),
        name("SOLUSDC", "SOL")
    );
    // A core that states no quote falls back to the clicked market's.
    assert_eq!(
        pick_core_name(&bybit, "SOL", "SOL-USDT-SWAP", "", "USDT", None),
        name("SOLUSDT", "SOL")
    );
}
