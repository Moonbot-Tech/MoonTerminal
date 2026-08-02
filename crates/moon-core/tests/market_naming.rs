//! Market naming, checked against real market names rather than invented ones.
//!
//! The fixture in `tests/data/markets.txt` holds one entry per distinct name shape per exchange,
//! taken from the catalogs of the cores connected to this workspace. The oracles here are
//! independent of how [`moon_core::symbol::parse`] is written:
//!
//! - a name must actually come APART — a coin that still contains the quote means the rules did
//!   not recognize the exchange's spelling, which is the defect that showed `BEAT-USDT-SWAP` in
//!   the Orders table;
//! - a split name must SPELL BACK to itself through `market_names_for`, which catches a wrong
//!   split and a wrong spelling table with one assertion;
//! - the exchange-aware path and the shape-recognition path must agree on the coin, because the
//!   Log panel only ever has the latter.

use moon_core::symbol::parse::{market_names_for, split_market};
use moon_core::symbol::Exchange;

/// `(exchange ordinal, market name)` rows of the fixture.
fn fixture() -> Vec<(u8, String)> {
    let text = include_str!("data/markets.txt");
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (code, market) = line
                .split_once('\t')
                .unwrap_or_else(|| panic!("fixture row is not `code<TAB>market`: {line:?}"));
            (
                code.parse()
                    .unwrap_or_else(|_| panic!("bad ordinal: {code}")),
                market.to_string(),
            )
        })
        .collect()
}

/// Names that carry no coin or no reversible spelling, excluded from the round trip by shape:
/// a Hyperliquid spot index is an opaque number, and a HIP-3 or slash name is spelled by its
/// venue, not by the pair rules.
fn round_trips(market: &str) -> bool {
    !market.starts_with('@') && !market.contains(':') && !market.contains('/')
}

#[test]
fn fixture_is_not_empty() {
    let rows = fixture();
    assert!(rows.len() > 40, "fixture shrank to {} rows", rows.len());
}

/// No coin may still contain a separator or the quote it was supposed to shed.
#[test]
fn every_name_comes_apart() {
    for (code, market) in fixture() {
        let ex = Exchange::from_code(code);
        let parts = split_market(&market, ex);
        assert!(
            !parts.base.is_empty(),
            "{market} on {ex:?} produced an empty coin"
        );
        if market.starts_with('@') {
            continue;
        }
        assert!(
            !parts.base.contains(['-', ':', '/']),
            "{market} on {ex:?} left a separator in the coin: {}",
            parts.base
        );
        // An underscore survives only in a coin the core itself spells that way; no exchange here
        // lists one, so any underscore left in the coin is an unrecognized contract tail.
        assert!(
            !parts.base.contains('_'),
            "{market} on {ex:?} left an underscore in the coin: {}",
            parts.base
        );
        assert!(
            parts.base.len() < market.len() || parts.quote.is_empty(),
            "{market} on {ex:?} claims a quote but kept the whole name as the coin"
        );
    }
}

/// Split, then spell back: the pair rules must reproduce the exchange's own name.
#[test]
fn split_names_spell_back() {
    for (code, market) in fixture() {
        let ex = Exchange::from_code(code);
        let parts = split_market(&market, ex);
        if parts.quote.is_empty() || parts.contract.is_some() || !round_trips(&market) {
            continue;
        }
        let spellings: Vec<String> = market_names_for(parts.base, parts.quote, ex).collect();
        assert!(
            spellings.iter().any(|name| name == &market),
            "{market} on {ex:?} split to {}/{} but spells back as {spellings:?}",
            parts.base,
            parts.quote
        );
    }
}

/// Shape recognition is what a log line and an offline report row get; it must reach the same coin
/// as the exchange's own rules for every name a core actually lists.
#[test]
fn shape_recognition_agrees_with_the_exchange() {
    for (code, market) in fixture() {
        let ex = Exchange::from_code(code);
        assert_eq!(
            split_market(&market, ex).base,
            split_market(&market, Exchange::Unknown).base,
            "{market}: {ex:?} and shape recognition disagree on the coin"
        );
    }
}

/// Pricing a wallet spells a pair and looks it up, so every exchange must produce at least one
/// name that PARSES BACK to the pair it was asked for. Reading the spelling back is the oracle;
/// asserting that the output merely mentions the coin would pass for any `format!` at all.
#[test]
fn every_exchange_spells_a_usdt_pair_that_parses_back() {
    for code in [2u8, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15] {
        let ex = Exchange::from_code(code);
        let names: Vec<String> = market_names_for("BTC", "USDT", ex).collect();
        assert!(!names.is_empty(), "{ex:?} spells no BTC/USDT market");
        let round_trips = names.iter().any(|name| {
            let parts = split_market(name, ex);
            // The coin must survive exactly. The quote may legitimately differ from the request:
            // a Hyperliquid name carries none, and Binance COIN-M has no USDT market at all — its
            // contracts are quoted in USD, which is what makes them answer a USDT request.
            let quote_ok = parts.quote.is_empty() || moon_core::symbol::is_usd_stable(parts.quote);
            parts.base == "BTC" && quote_ok
        });
        assert!(
            round_trips,
            "{ex:?} spells BTC/USDT as {names:?}, none of which parses back to the coin BTC \
             against a USD-equivalent quote"
        );
    }
}

/// A COIN-M contract is quoted in USD whatever quote was requested, so it must not answer a
/// request for a different quote with a USD price.
#[test]
fn coin_m_spelling_only_answers_usd_requests() {
    let usdt: Vec<String> = market_names_for("BTC", "USDT", Exchange::Unknown).collect();
    assert!(usdt.iter().any(|n| n == "BTCUSD_PERP"));
    let btc: Vec<String> = market_names_for("ETH", "BTC", Exchange::Unknown).collect();
    assert!(
        !btc.iter().any(|n| n.contains("USD")),
        "a BTC-quoted request must not be answered by a USD contract: {btc:?}"
    );
}
