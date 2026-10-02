//! Edits of the distribution's lists as pure functions: what one chip action does to a row's two
//! lists, how "Distribute" deals the market out, and how a target list is written back into a
//! strategy's field without disturbing what it does not change.
//!
//! Everything here works on coin MATCH KEYS (see `moon_core::symbol::coin_match_key`); only
//! [`rewrite_list`] touches the field's own text, and it keeps every entry it does not drop
//! exactly as written — `BTC_0626` stays `BTC_0626`, the field's separator stays its own.

use std::collections::HashSet;

use moon_core::symbol::{coin_list_separator, coin_match_key, split_coin_list};

/// One row's lists as ordered match keys.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::strategies) struct RowLists {
    pub(in crate::strategies) white: Vec<String>,
    pub(in crate::strategies) black: Vec<String>,
}

/// What a chip's menu asks of its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChipAction {
    /// Drop the coin from the whitelist.
    RemoveWhite,
    /// Drop the coin from the blacklist.
    RemoveBlack,
    /// Put the coin on the whitelist too (a blacklist chip's "Copy to WL").
    AddWhite,
    /// Put the coin on the blacklist too (a whitelist chip's "Copy to BL").
    AddBlack,
}

/// Apply one chip action to a row's lists.
///
/// Args:
///     lists: The row's current lists, as match keys.
///     coin: The chip's match key.
///     action: What to do with it.
///
/// Returns:
///     The new lists; an addition goes to the END, so the field changes by one entry.
pub(super) fn apply_action(lists: &RowLists, coin: &str, action: ChipAction) -> RowLists {
    let without = |list: &[String]| -> Vec<String> {
        list.iter()
            .filter(|c| c.as_str() != coin)
            .cloned()
            .collect()
    };
    let with = |list: &[String]| -> Vec<String> {
        let mut out = list.to_vec();
        if !out.iter().any(|c| c == coin) {
            out.push(coin.to_string());
        }
        out
    };
    match action {
        ChipAction::RemoveWhite => RowLists {
            white: without(&lists.white),
            black: lists.black.clone(),
        },
        ChipAction::RemoveBlack => RowLists {
            white: lists.white.clone(),
            black: without(&lists.black),
        },
        ChipAction::AddWhite => RowLists {
            white: with(&lists.white),
            black: lists.black.clone(),
        },
        ChipAction::AddBlack => RowLists {
            white: lists.white.clone(),
            black: with(&lists.black),
        },
    }
}

/// Put a coin back on ONE list as the core has it there: on it when the core's list holds it,
/// off it otherwise. The other list is left alone — a copy made there stays until it is removed
/// from there.
///
/// Args:
///     lists: The strategy's lists as they will be, as match keys.
///     live: Its lists as the core stores them.
///     coin: The chip's match key.
///     white: Whether the chip sits on the whitelist (else the blacklist).
pub(super) fn restore(lists: &RowLists, live: &RowLists, coin: &str, white: bool) -> RowLists {
    let set = |list: &[String], keep: bool| -> Vec<String> {
        let mut out: Vec<String> = list
            .iter()
            .filter(|c| c.as_str() != coin)
            .cloned()
            .collect();
        if keep {
            out.push(coin.to_string());
        }
        out
    };
    match white {
        true => RowLists {
            white: set(&lists.white, live.white.iter().any(|c| c == coin)),
            black: lists.black.clone(),
        },
        false => RowLists {
            white: lists.white.clone(),
            black: set(&lists.black, live.black.iter().any(|c| c == coin)),
        },
    }
}

/// The two switches of "Distribute".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DistributeOptions {
    /// The first row keeps no whitelist and blacklists every other row's part: it trades the
    /// free coins of its own part and whatever the exchange lists later.
    pub(super) first_blacklists: bool,
    /// Leave the coins of the common blacklist out of the parts. Off, every coin is dealt, so a
    /// coin later taken off the blacklist trades where its part put it.
    pub(super) skip_common_black: bool,
}

/// Deal the market out between the rows.
///
/// The coins that will trade (`free`) and the coins of the common blacklist (`held`, dealt only
/// when `skip_common_black` is off) are dealt APART, each in consecutive name-ordered runs of
/// equal size (see [`deal`]), and a row's part is its free run plus its held run. So every row
/// trades the same number of coins (±1) and holds about as many entries (±1). The free coins'
/// remainder goes one each to the top rows (02.10: 740 over 6 is 124, 124, 123, 123, 123, 123);
/// the held coins' remainder continues from the next row. The rows' old blacklists are pooled into
/// one common blacklist that every row receives. A row's blacklisted coin that ANOTHER row
/// whitelists is not pooled, whichever way the switches stand: it is a previous distribution's
/// split (the first row of "BL on the first" holds every other part), not a blacklist anyone
/// chose — pooling it would blacklist each row's own new part. A coin a row both whitelists and
/// blacklists itself stays blacklisted, unless another row whitelists it as well.
///
/// Args:
///     rows: Every row's current lists, in distribution order.
///     universe: The coins to deal, as match keys, sorted.
///     options: The two switches.
///
/// Returns:
///     The new lists per row, in the same order; empty when there are no rows.
pub(super) fn distribute(
    rows: &[RowLists],
    universe: &[String],
    options: DistributeOptions,
) -> Vec<RowLists> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut common: Vec<String> = Vec::new();
    for (ix, row) in rows.iter().enumerate() {
        let others_white: HashSet<&String> = rows
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != ix)
            .flat_map(|(_, r)| &r.white)
            .collect();
        common.extend(
            row.black
                .iter()
                .filter(|c| !others_white.contains(c))
                .cloned(),
        );
    }
    common.sort_unstable();
    common.dedup();

    // The coins that will trade and the coins the common blacklist holds are dealt APART, each
    // evenly: dealt as one alphabetical run, a part starting at "A" caught five blacklisted coins
    // and another none, so the rows traded 117 to 122 coins instead of the same number.
    let blocked: HashSet<&String> = common.iter().collect();
    let free: Vec<&String> = universe.iter().filter(|c| !blocked.contains(c)).collect();
    let held: Vec<&String> = match options.skip_common_black {
        true => Vec::new(),
        false => universe.iter().filter(|c| blocked.contains(c)).collect(),
    };
    let n = rows.len();
    let free_parts = deal(&free, n, 0);
    // The held coins' remainder starts where the free coins' remainder stopped, so a row takes
    // an extra coin from both only when the two remainders together exceed the row count — and
    // then every row has at least one, so the sizes still differ by one at most.
    let held_parts = deal(&held, n, free.len() % n);
    let parts: Vec<Vec<String>> = free_parts
        .into_iter()
        .zip(held_parts)
        .map(|(mut part, extra)| {
            part.extend(extra);
            part.sort_unstable();
            part
        })
        .collect();

    if options.first_blacklists {
        // The common blacklist first, then the other rows' parts — dealt coins that the common
        // list already holds are not repeated.
        let mut black = common.clone();
        let seen: HashSet<String> = common.iter().cloned().collect();
        black.extend(
            parts[1..]
                .iter()
                .flatten()
                .filter(|c| !seen.contains(*c))
                .cloned(),
        );
        let mut out = vec![RowLists {
            white: Vec::new(),
            black,
        }];
        out.extend(parts.into_iter().skip(1).map(|white| RowLists {
            white,
            black: common.clone(),
        }));
        out
    } else {
        parts
            .into_iter()
            .map(|white| RowLists {
                white,
                black: common.clone(),
            })
            .collect()
    }
}

/// Cut `coins` into `n` consecutive runs as even as they go: the remainder adds one coin each to
/// `n` rows starting at row `first_extra` (wrapping), so 740 over 6 from row 0 is 124, 124, 123,
/// 123, 123, 123.
fn deal(coins: &[&String], n: usize, first_extra: usize) -> Vec<Vec<String>> {
    let base = coins.len() / n;
    let extra = coins.len() % n;
    let mut parts = Vec::with_capacity(n);
    let mut at = 0;
    for ix in 0..n {
        let len = base + usize::from((ix + n - first_extra % n) % n < extra);
        parts.push(coins[at..at + len].iter().map(|c| (*c).clone()).collect());
        at += len;
    }
    parts
}

/// Write a target list back into a strategy field's text.
///
/// Every entry of `current` whose match key the target keeps is kept AS WRITTEN — several
/// spellings of one coin included — at the position of its key in `target`. A key the field does
/// not hold takes its spelling from `elsewhere` (the strategy's other list, or the field as the
/// core has it) when one of them writes it, and is added as the key itself only when none does:
/// `BTC_0626` is a delivery contract, and writing `BTC` for it would name another market. The
/// field's own separator is reused.
///
/// Args:
///     current: The field as the strategy holds it now.
///     target: The list it should hold, as match keys, in order.
///     elsewhere: Other texts to take a missing coin's spelling from, in order of preference.
///
/// Returns:
///     The field's new text.
pub(super) fn rewrite_list(current: &str, target: &[String], elsewhere: &[&str]) -> String {
    let sep = coin_list_separator(current);
    let written: Vec<&str> = split_coin_list(current).collect();
    let mut out: Vec<&str> = Vec::with_capacity(target.len());
    for key in target {
        let mut found = false;
        for entry in &written {
            if coin_match_key(entry) == *key && !out.contains(entry) {
                out.push(entry);
                found = true;
            }
        }
        if found || out.iter().any(|e| coin_match_key(e) == *key) {
            continue;
        }
        let spelled: Vec<&str> = elsewhere
            .iter()
            .map(|text| {
                split_coin_list(text)
                    .filter(|entry| coin_match_key(entry) == *key)
                    .collect::<Vec<_>>()
            })
            .find(|entries| !entries.is_empty())
            .unwrap_or_default();
        if spelled.is_empty() {
            out.push(key);
        }
        for entry in spelled {
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
    }
    out.join(sep)
}

/// A field's text as ordered match keys, each once.
pub(super) fn list_keys(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for key in split_coin_list(text).map(coin_match_key) {
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

#[cfg(test)]
mod tests;
