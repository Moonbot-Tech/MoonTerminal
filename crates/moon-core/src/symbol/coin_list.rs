//! A coin list as the core keeps it — a comma-separated text, the core's own `CoinsBlackList` and
//! the core-wide blacklist alike — and the one-token edits every surface makes on it: the
//! terminal's coin menu and the bot's Control section.
//!
//! Matching is deliberately LITERAL — an ASCII-case-insensitive compare of trimmed tokens, not
//! `symbol::coin_match_key` — and this is measured rather than assumed: MoonProto's
//! `rebuild_market_blacklisted_cfg` compares each entry with the market's own `market_currency`
//! by `same_text_ascii`, an exact case-insensitive match with no folding. So the token to write
//! and to compare is the core's spelling (`BTC_RP`, `1kBONKPERP`), what `OrderRow::coin` and
//! `MarketLabel::coin` carry. Folding here would claim "already listed" about an entry the core
//! does not associate with the market.

/// Whether `text` lists `coin`.
pub fn contains(text: &str, coin: &str) -> bool {
    text.split(',').any(|s| s.trim().eq_ignore_ascii_case(coin))
}

/// `text` with `coin` appended, unless already listed; an empty list becomes the token alone.
pub fn add(text: &str, coin: &str) -> String {
    if contains(text, coin) {
        return text.to_string();
    }
    let base = text.trim().trim_end_matches(',').trim_end();
    if base.is_empty() {
        coin.to_string()
    } else {
        format!("{base},{coin}")
    }
}

/// `text` without `coin`, every other entry kept verbatim — order, inner spacing, and spellings
/// this does not recognise. A token that is not listed leaves the exact bytes unchanged.
pub fn remove(text: &str, coin: &str) -> String {
    if !contains(text, coin) {
        return text.to_string();
    }
    text.split(',')
        .filter(|s| !s.trim().eq_ignore_ascii_case(coin))
        .collect::<Vec<_>>()
        .join(",")
}

/// [`remove`] when `lift`, else [`add`].
pub fn edit(text: &str, coin: &str, lift: bool) -> String {
    if lift {
        remove(text, coin)
    } else {
        add(text, coin)
    }
}

#[cfg(test)]
mod tests;
