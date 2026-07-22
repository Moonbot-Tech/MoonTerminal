//! Market-symbol display utilities. A core connects using a single quote currency
//! (USDT/USDC/…), and the UI displays the coin WITHOUT that suffix: `ADAUSDT` → `ADA`.

/// Known quote currencies whose suffixes are stripped. Ordered by length (longest first),
/// so `FDUSD`/`USDC` match before `USD`.
const QUOTES: [&str; 9] = [
    "FDUSD", "TUSD", "USDC", "BUSD", "USDT", "USD", "BTC", "ETH", "BNB",
];

/// Recognizes a known quote suffix in any market name, ASCII-case-insensitively.
/// `BTCUSDT` → `USDT`; returns the canonical uppercase quote, or an empty string if unrecognized.
pub fn resolve_quote(market: &str) -> String {
    let up = market.to_ascii_uppercase();
    QUOTES
        .iter()
        .find(|q| up.ends_with(*q) && up.len() > q.len())
        .map(|q| q.to_string())
        .unwrap_or_default()
}

/// Whether the currency is a USD stablecoin (its USD rate is approximately 1).
/// This list mirrors `feed::assets`.
pub fn is_usd_stable(currency: &str) -> bool {
    matches!(
        currency.to_ascii_uppercase().as_str(),
        "USDT" | "USDC" | "BUSD" | "USD" | "FDUSD" | "TUSD" | "DAI" | "USDP"
    )
}

/// Returns the base coin by stripping `quote` from the end of `sym` when it matches.
/// If `quote` is empty or does not match, returns the symbol unchanged.
///
/// Gate and similar exchanges separate the symbol with an underscore (`VANRY_USDT`,
/// `1INCH_USDT`). Stripping `USDT` leaves a trailing separator as in `VANRY_`, so also
/// strip `_`, `-`, or `/`; otherwise tables would display the token as `VANRY_`.
pub fn base_symbol<'a>(sym: &'a str, quote: &str) -> &'a str {
    if !quote.is_empty() && sym.len() > quote.len() && sym.to_ascii_uppercase().ends_with(quote) {
        let base = &sym[..sym.len() - quote.len()];
        base.trim_end_matches(|c| c == '_' || c == '-' || c == '/')
    } else {
        sym
    }
}

/// Full ticker for a chart label: `BTCUSDT` → `BTC-USDT`. HIP-3 (`xyz:BIRD`) → `BIRD`
/// because the DEX prefix and implicit USDC quote are hidden. If the quote is unrecognized,
/// displays the coin without its DEX prefix.
pub fn display_pair(market: &str) -> String {
    let after_dex = strip_dex(market);
    let quote = resolve_quote(after_dex);
    if quote.is_empty() {
        return coin_of_market(market).to_string();
    }
    format!("{}-{}", base_symbol(after_dex, &quote), quote)
}

/// A Hyperliquid HIP-3 market whose name carries a `dex_name:coin` DEX prefix (`xyz:BIRD`).
/// A colon occurs in market names ONLY for HIP-3; regular exchanges send an empty `dex_name`
/// without a colon.
pub fn is_hip3(market: &str) -> bool {
    market.contains(':')
}

/// The DEX name of a HIP-3 market (`xyz:BIRD` → `Some("xyz")`), or `None` otherwise.
pub fn dex_of_market(market: &str) -> Option<&str> {
    market.split_once(':').map(|(dex, _)| dex)
}

/// The part of the market name AFTER its DEX prefix: `xyz:BIRD` → `BIRD`,
/// `ADAUSDT` → `ADAUSDT`.
fn strip_dex(market: &str) -> &str {
    market.rsplit(':').next().unwrap_or(market)
}

/// The CANONICAL market coin for blacklists, matching, and ticker display: strips the DEX
/// prefix, then the quote suffix. `xyz:BIRD` → `BIRD`, `BIRDUSDC` → `BIRD`,
/// `ADAUSDT` → `ADA`.
///
/// IMPORTANT: this is NOT a market key. Subscribing, opening, and data lookup require the full
/// market name (`xyz:BIRD`). Search may accept a base-coin query, but returns full market keys.
/// `coin_of_market` only answers "what is this coin called?"; the server compares blacklists
/// against this unprefixed `market_currency`.
pub fn coin_of_market(market: &str) -> &str {
    let after_dex = strip_dex(market);
    base_symbol(after_dex, &resolve_quote(after_dex))
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
