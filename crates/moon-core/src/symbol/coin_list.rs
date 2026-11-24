//! A coin list as the core keeps it, and the one-token edits every surface makes on it: the
//! terminal's coin menu and the bot's Control section.
//!
//! Two lists, two separator sets. The core-wide blacklist (Settings → Main → Black List) is
//! comma-separated: MoonBot documents it as "comma-separated without spaces" and MoonProto's
//! `parse_coin_blacklist_text` splits it on commas only, so [`contains`] / [`add`] / [`remove`] /
//! [`edit`] split on commas and nothing else — `ANC GALA` there is one entry the core does not
//! associate with either coin. A strategy's `CoinsBlackList` takes names "separated by a space,
//! comma+space or comma without space" (moonbot.pro, filters tab), the set
//! [`crate::symbol::split_coin_list`] owns for every strategy list field, so the `strategy_*`
//! functions recognise entries with that splitter: `ANC GALA` there is two coins (#1448).
//!
//! Matching is deliberately LITERAL in both — an ASCII-case-insensitive compare of trimmed tokens,
//! not `symbol::coin_match_key` — and this is measured rather than assumed: MoonProto's
//! `rebuild_market_blacklisted_cfg` compares each entry with the market's own `market_currency`
//! by `same_text_ascii`, an exact case-insensitive match with no folding. So the token to write
//! and to compare is the core's spelling (`BTC_RP`, `1kBONKPERP`), what `OrderRow::coin` and
//! `MarketLabel::coin` carry. Folding here would claim "already listed" about an entry the core
//! does not associate with the market.

use std::ops::Range;

use crate::symbol::{coin_list_separator, split_coin_list};

/// Whether the core-wide, comma-separated `text` lists `coin`.
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

/// Byte ranges of a strategy list's entries inside `text`, in list order.
///
/// The entries come from [`split_coin_list`] itself rather than a second copy of its separator
/// set, so this module and the Strategies counters cannot drift apart on what the field holds.
/// Every entry is a subslice of `text` (the splitter only splits and trims), so its offset is the
/// distance between the two start pointers.
pub fn strategy_entries(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let origin = text.as_ptr() as usize;
    split_coin_list(text).map(move |entry| {
        let start = entry.as_ptr() as usize - origin;
        start..start + entry.len()
    })
}

/// Whether a strategy coin list (`CoinsBlackList`, `CoinsWhiteList`) lists `coin`.
///
/// Entries are split like [`split_coin_list`] — spaces, commas, semicolons, line breaks — and
/// each is compared literally, as [`contains`] does. An empty `coin` is never listed: the splitter
/// yields no empty entry.
pub fn strategy_contains(text: &str, coin: &str) -> bool {
    split_coin_list(text).any(|entry| entry.eq_ignore_ascii_case(coin))
}

/// A strategy coin list with `coin` appended, unless already listed.
///
/// The new entry follows the field's own separator ([`coin_list_separator`]) so a one-coin edit
/// does not re-spell the rest of the line. A field of one entry or none has no separator of its
/// own yet and gets the comma MoonBot itself writes (`A,B,C`) — the same text [`add`] produced
/// before strategy lists had their own editor. Trailing separators are dropped before appending,
/// as [`add`] drops a trailing comma.
pub fn strategy_add(text: &str, coin: &str) -> String {
    if strategy_contains(text, coin) {
        return text.to_string();
    }
    let base = text
        .trim()
        .trim_end_matches([',', ';', ' ', '\t', '\n', '\r']);
    if base.is_empty() {
        return coin.to_string();
    }
    let sep = if split_coin_list(base).nth(1).is_some() {
        coin_list_separator(base)
    } else {
        ","
    };
    format!("{base}{sep}{coin}")
}

/// A strategy coin list without `coin`, every other entry and separator kept verbatim.
///
/// Only the matched entries' bytes go, each together with the separator run before it (after it,
/// for the first entry), so `ANC GALA` lifts GALA to `ANC` rather than `ANC ` — and the text
/// before the first entry and after the last one stays as written. A coin that is not listed
/// leaves the exact bytes unchanged; lifting the only entry leaves an empty field.
pub fn strategy_remove(text: &str, coin: &str) -> String {
    if !strategy_contains(text, coin) {
        return text.to_string();
    }
    let entries: Vec<Range<usize>> = strategy_entries(text).collect();
    let kept: Vec<usize> = (0..entries.len())
        .filter(|&ix| !text[entries[ix].clone()].eq_ignore_ascii_case(coin))
        .collect();
    let (Some(first), Some(last)) = (entries.first(), entries.last()) else {
        return text.to_string();
    };
    if kept.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..first.start]);
    for (pos, &ix) in kept.iter().enumerate() {
        if pos > 0 {
            out.push_str(&text[entries[ix - 1].end..entries[ix].start]);
        }
        out.push_str(&text[entries[ix].clone()]);
    }
    out.push_str(&text[last.end..]);
    out
}

/// [`strategy_remove`] when `lift`, else [`strategy_add`].
pub fn strategy_edit(text: &str, coin: &str, lift: bool) -> String {
    if lift {
        strategy_remove(text, coin)
    } else {
        strategy_add(text, coin)
    }
}

#[cfg(test)]
mod tests;
