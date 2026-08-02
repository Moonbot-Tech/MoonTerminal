//! THE market-naming module: one place that answers "what coin is this market, and how is it
//! spelled?", for every exchange.
//!
//! A core connects using a single quote currency (USDT/USDC/…) and the UI displays the coin
//! WITHOUT it: `ADAUSDT` → `ADA`. Each exchange spells that differently — `BTC_USDT` on Gate,
//! `BTC-USDT-SWAP` on OKX, `BTCUSD_PERP` on Binance COIN-M — so the rules are per exchange, in
//! [`parse`], dispatched by [`Exchange`]. Nothing outside this module may hand-roll a quote list
//! or a name split; two spellings of the rule is how one panel starts disagreeing with another
//! about which coin an order is on.
//!
//! The exchange is often known at the call site (a panel has the core, and the core has its
//! `ExchangeCode`); pass it through [`Exchange::from_code`] and use the `*_on` variants. Where it
//! is not — a log line, a report row read before any core connected — [`Exchange::Unknown`]
//! recognizes the shape instead.

mod exchange;
pub mod parse;

pub use exchange::Exchange;

use parse::split_market;

/// Recognizes a known quote suffix in any market name, ASCII-case-insensitively.
/// `BTCUSDT` → `USDT`; returns the canonical uppercase quote, or an empty string if unrecognized.
pub fn resolve_quote(market: &str) -> String {
    resolve_quote_on(market, Exchange::Unknown)
}

/// [`resolve_quote`] for a market whose exchange is known.
pub fn resolve_quote_on(market: &str, ex: Exchange) -> String {
    split_market(market, ex).quote.to_ascii_uppercase()
}

/// Whether the currency is a USD stablecoin (its USD rate is approximately 1).
/// This list mirrors `feed::assets`.
pub fn is_usd_stable(currency: &str) -> bool {
    matches!(
        currency.to_ascii_uppercase().as_str(),
        "USDT" | "USDC" | "BUSD" | "USD" | "FDUSD" | "TUSD" | "DAI" | "USDP"
    )
}

/// Full ticker for a chart label: `BTCUSDT` → `BTC-USDT`. HIP-3 (`xyz:BIRD`) → `BIRD`
/// because the DEX prefix and implicit USDC quote are hidden. If the quote is unrecognized,
/// displays the coin without its DEX prefix.
///
/// A dated contract keeps its date (`BTCUSDT-07AUG26` → `BTC-USDT-07AUG26`): two expiries of the
/// same pair are different instruments and must not share a label. A perpetual has no tail, so
/// `BTC-USDT-SWAP` and `BTCUSDT` both read `BTC-USDT`.
pub fn display_pair(market: &str) -> String {
    let parts = split_market(market, Exchange::Unknown);
    if parts.quote.is_empty() {
        return parts.base.to_string();
    }
    let pair = format!("{}-{}", parts.base, parts.quote.to_ascii_uppercase());
    match parts.contract {
        Some(contract) => format!("{pair}-{contract}"),
        None => pair,
    }
}

/// The CANONICAL market coin for blacklists, matching, and ticker display: strips the DEX
/// prefix, then the quote suffix. `xyz:BIRD` → `BIRD`, `BIRDUSDC` → `BIRD`,
/// `ADAUSDT` → `ADA`, `BEAT-USDT-SWAP` → `BEAT`.
///
/// IMPORTANT: this is NOT a market key. Subscribing, opening, and data lookup require the full
/// market name (`xyz:BIRD`). Search may accept a base-coin query, but returns full market keys.
/// `coin_of_market` only answers "what is this coin called?"; the server compares blacklists
/// against this unprefixed `market_currency`.
///
/// It answers from the NAME alone. Where a core's market catalog is at hand, its `market_currency`
/// is the better answer and the only one that reproduces the core's own foldings
/// (`1000BONKPERP` → `1kBONKPERP`); see [`parse`].
pub fn coin_of_market(market: &str) -> &str {
    coin_of_market_on(market, Exchange::Unknown)
}

/// [`coin_of_market`] for a market whose exchange is known.
pub fn coin_of_market_on(market: &str, ex: Exchange) -> &str {
    split_market(market, ex).base
}

/// The token a strategy's coin list (`CoinsBlackList` / `CoinsWhiteList`) uses for
/// a coin, derived from the coin name as the REPORT writes it.
///
/// The report names a coin together with its contract (`BTC_RP` perpetual,
/// `LTC_1230` quarterly), while the strategy lists hold bare tokens (`ANC, GALA`,
/// `mith,tribe`). Without stripping the tail no futures coin would ever match its
/// own list. Case is left alone — the comparison is case-insensitive anyway, and
/// the report legitimately holds `kFLOKI` / `10kSATS`.
///
/// Only KNOWN tails are stripped (`_RP` and a 4-digit `_MMDD`), so a coin genuinely
/// named `FOO_BAR` — or `FOO_2` — stays itself instead of collapsing to `FOO`.
pub fn strip_contract_suffix(coin: &str) -> &str {
    let Some((base, suffix)) = coin.rsplit_once('_') else {
        return coin;
    };
    if base.is_empty() {
        return coin;
    }
    let known = suffix.eq_ignore_ascii_case("RP")
        || (suffix.len() == 4 && suffix.bytes().all(|b| b.is_ascii_digit()));
    if known {
        base
    } else {
        coin
    }
}

/// THE comparison key for "is this coin the one that list names?".
///
/// Both sides of a strategy coin list go through this: the report's name (which carries a
/// contract) and the list's own token (bare, in whatever case the user typed). Callers must
/// not hand-roll the comparison — two spellings of the rule is how one screen starts
/// disagreeing with another about whether a coin is blacklisted.
pub fn coin_match_key(coin: &str) -> String {
    strip_contract_suffix(coin.trim()).to_ascii_uppercase()
}

/// A strategy coin list (`"ANC, GALA"` / `"mith,tribe"`) as DISTINCT match keys.
///
/// The separator set and the key both live here so the terminal's coin table, its
/// counters and the per-strategy column cannot each decide for themselves what a list
/// contains — they already have to agree on the number they print.
pub fn parse_coin_list(text: &str) -> std::collections::HashSet<String> {
    split_coin_list(text).map(coin_match_key).collect()
}

/// The list's entries AS WRITTEN — same splitting as [`parse_coin_list`], without the fold.
///
/// Matching a coin needs the folded token; REPRODUCING the field needs the original text.
/// `BTC, BTC_0626, BTC_0925` are three entries that all match the coin `BTC`, so anything
/// that presents the folded form as the field's value silently drops two of them.
pub fn split_coin_list(text: &str) -> impl Iterator<Item = &str> {
    // Brackets and quotes count as separators too: the field is plain text in the strategy,
    // but a core that ever spells it as a JSON array would otherwise yield tokens like
    // `["BTC` that match no coin and inflate every count built on this.
    text.split([',', ';', ' ', '\n', '\r', '\t', '[', ']', '"', '\''])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests;
