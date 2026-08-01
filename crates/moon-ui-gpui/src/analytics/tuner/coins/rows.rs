//! The coin table's row MODEL and its cache.
//!
//! Rebuilding it is the expensive part of this panel: it walks every coin the period holds
//! (thousands), folds a match token per row, indexes the scoped aggregates and orders the
//! result. Measured on the real replica in the profile the app ships as `debug`, one pass is
//! ~1.5 ms idle and ~2.6 ms with a search and lists in play — against a 16.6 ms frame.
//!
//! GPUI repaints a view whenever anything asks it to, and a hovered row asks on every row
//! boundary the cursor crosses (`div`'s hover style registers a `MouseMove` listener that
//! calls `notify`). Because the strategy list and coin table share one view, hover traffic
//! must not rebuild this model when none of its inputs changed.
//!
//! So the model is built once per change of the things it actually depends on, and the cache
//! key names every one of them. A miss costs the pass; a hover costs a `Vec` walk.

use std::collections::{HashMap, HashSet};

use gpui::SharedString;

use super::super::list::{MAX_ROWS, partial_sort};
use super::state::{CoinFilter, CoinsState, coin_token};
use moon_core::db::analytics::GroupStat;

/// One row, ready to draw. Owned, so the cache outlives the borrows it was built from —
/// only the `MAX_ROWS` the table can actually show are kept.
pub(in crate::analytics::tuner) struct CoinRow {
    pub(in crate::analytics::tuner) name: SharedString,
    /// The coin's numbers IN THE CURRENT SCOPE; all zeros for a coin the scope never traded.
    pub(in crate::analytics::tuner) stat: GroupStat,
    /// Working membership: what the tick boxes show, NOT what the strategies have saved.
    pub(in crate::analytics::tuner) black: bool,
    pub(in crate::analytics::tuner) white: bool,
    /// Working membership differs from the saved lists — an unwritten edit.
    pub(in crate::analytics::tuner) changed: bool,
    /// Clicked — the strategy list is showing who traded it.
    pub(in crate::analytics::tuner) picked: bool,
}

/// How many coins the search matched, and how many of them each list names.
///
/// A named struct rather than a bare tuple: the four travel together through the builder, the
/// card and the filter bar, and `counts.1` at the far end says nothing about what it counts.
#[derive(Default, Clone, Copy)]
pub(in crate::analytics::tuner) struct CoinCounts {
    pub(in crate::analytics::tuner) all: usize,
    pub(in crate::analytics::tuner) black: usize,
    pub(in crate::analytics::tuner) white: usize,
    pub(in crate::analytics::tuner) changed: usize,
}

/// Everything the row model depends on. Two equal keys MUST mean two identical models — a
/// field missing here is a stale table that no interaction refreshes.
#[derive(PartialEq)]
struct RowsKey {
    /// Identity (not contents) of the two aggregate sets: `LoadState` hands out `Arc`s, and a
    /// new read is a new allocation, so the pointer is the cheap "is this the same data".
    base: usize,
    scoped: usize,
    search: String,
    filter: CoinFilter,
    /// The "trades ≥" threshold. In the key like every other input: without it, moving the
    /// slider changed what the table SHOULD show while the cache kept handing back the model
    /// built before it.
    min_trades: i64,
    /// Which coins are clicked — it decides a row's highlight, so it decides the model.
    picked: Vec<String>,
    sort: Option<(String, bool)>,
    lists_rev: u64,
}

/// The built model, with the key it was built for.
pub(in crate::analytics::tuner) struct RowsCache {
    key: RowsKey,
    pub(in crate::analytics::tuner) rows: Vec<CoinRow>,
    pub(in crate::analytics::tuner) counts: CoinCounts,
    /// Rows before the `MAX_ROWS` cut; the shown/total label reads this complete count.
    pub(in crate::analytics::tuner) total: usize,
}

/// Rebuild the model unless the cache already holds it. Returns the cache to render from.
pub(in crate::analytics::tuner) fn rows_for<'a>(
    state: &'a mut CoinsState,
    base: &[GroupStat],
    scoped: &[GroupStat],
) -> &'a RowsCache {
    let key = RowsKey {
        base: base.as_ptr() as usize,
        scoped: scoped.as_ptr() as usize,
        search: state.search.trim().to_lowercase(),
        filter: state.filter,
        min_trades: state.min_trades,
        picked: {
            let mut v: Vec<String> = state.picked.iter().cloned().collect();
            v.sort();
            v
        },
        sort: state.sort.clone(),
        lists_rev: state.lists_rev,
    };
    if state.rows.as_ref().is_none_or(|c| c.key != key) {
        let (rows, counts, total) = build(state, base, scoped, &key.search);
        state.rows = Some(RowsCache {
            key,
            rows,
            counts,
            total,
        });
    }
    state
        .rows
        .as_ref()
        .expect("just populated above when it was missing")
}

/// The pass itself: merge the two scopes, apply search and the list view, count, order, cut.
fn build(
    state: &CoinsState,
    base: &[GroupStat],
    scoped: &[GroupStat],
    needle: &str,
) -> (Vec<CoinRow>, CoinCounts, usize) {
    let by_key: HashMap<&str, &GroupStat> = scoped.iter().map(|g| (g.key.as_str(), g)).collect();
    // With no lists in play nothing can be a member, so the per-row token — one allocation
    // per coin over thousands of them — is never built.
    let lists_known = !state.work.is_empty() || !state.saved.is_empty();
    let mut rows: Vec<CoinRow> = Vec::new();
    let (mut black_tok, mut white_tok, mut changed_tok) =
        (HashSet::new(), HashSet::new(), HashSet::new());
    let mut counts = CoinCounts::default();
    // Rows come from the base scope; anything the scoped read saw but the base did not is
    // chained on after it, so a coin cannot lose its numbers to a skew between the two reads.
    let seen: HashSet<&str> = base.iter().map(|g| g.key.as_str()).collect();
    let from_base = base.iter().map(|g| {
        // A coin the scope never traded still gets a row — at zero, which is the whole point
        // of listing the full coin set rather than the traded one.
        let stat = by_key.get(g.key.as_str()).copied();
        (g.name.as_str(), stat)
    });
    let extra = scoped
        .iter()
        .filter(|g| !seen.contains(g.key.as_str()))
        .map(|g| (g.name.as_str(), Some(g)));
    for (name, stat) in from_base.chain(extra) {
        // A NULL coin projects as the empty string; it names nothing the user can act on and
        // would tick into a list entry that matches nothing.
        if name.trim().is_empty() {
            continue;
        }
        if !needle.is_empty() && !name.to_lowercase().contains(needle) {
            continue;
        }
        // Membership is the WORKING state — what the tick boxes show and what v1 is scored
        // under; `saved` only decides whether that state counts as changed.
        let (mut black, mut white, mut changed) = (false, false, false);
        if lists_known {
            let token = coin_token(name);
            black = state.work.black.contains(&token);
            white = state.work.white.contains(&token);
            changed = state.is_changed(&token);
            // Counted as DISTINCT tokens: `BTC_RP` and `BTC_1230` are two rows of one list
            // entry, and counting rows would print "2" against a one-coin list.
            if black {
                black_tok.insert(token.clone());
            }
            if white {
                white_tok.insert(token.clone());
            }
            if changed {
                changed_tok.insert(token);
            }
        }
        counts.all += 1;
        // Below the threshold the row is not worth reading; counted first all the same, so
        // the chips keep describing the searched set rather than the trimmed one.
        //
        // "No aggregate" means ZERO trades, not "unknown": at a threshold of 0 — the default —
        // treating it as unknown dropped every coin the scope never traded, which is the whole
        // set this table exists to show.
        if stat.map_or(0, |g| g.n) < state.min_trades {
            continue;
        }
        let keep = match state.filter {
            CoinFilter::All => true,
            CoinFilter::Black => black,
            CoinFilter::White => white,
            CoinFilter::Changed => changed,
        };
        if keep {
            rows.push(CoinRow {
                picked: state.picked.contains(name),
                name: SharedString::from(name.to_string()),
                // Cloned only for rows that survive the filter, and only on a real rebuild.
                stat: stat.cloned().unwrap_or_default(),
                black,
                white,
                changed,
            });
        }
    }
    counts.black = black_tok.len();
    counts.white = white_tok.len();
    counts.changed = changed_tok.len();
    sort_rows(state, &mut rows);
    let total = rows.len();
    rows.truncate(MAX_ROWS);
    (rows, counts, total)
}

/// Order by the active column, bringing the visible head to the front.
///
/// Only `MAX_ROWS` are ever drawn, so the tail is partitioned away in linear time and just
/// the head is fully sorted.
fn sort_rows(state: &CoinsState, rows: &mut [CoinRow]) {
    let Some((key, desc)) = &state.sort else {
        return;
    };
    let desc = *desc;
    // Both comparators close with the coin name, which is unique per row. Without that the
    // order is only partial, and `partial_sort` resolves a tie through `select_nth_unstable_by`
    // — free to reorder equal rows AND to pick arbitrarily among them for the drawn head, so
    // tied coins could swap places or drop out of the visible head between two renders of the
    // same data. Ties are the common case here: `trades` is a small integer.
    if let Some(col) = super::super::COIN_COLS.iter().find(|c| c.key == key) {
        let f = col.sort;
        partial_sort(rows, |a, b| {
            let o = f(&a.stat)
                .partial_cmp(&f(&b.stat))
                .unwrap_or(std::cmp::Ordering::Equal);
            let o = if desc { o.reverse() } else { o };
            o.then_with(|| a.name.cmp(&b.name))
        });
    } else {
        // Compared in ONE direction rather than sorted-then-reversed: reversing also flips
        // rows whose keys are equal, so descending would not mirror ascending.
        partial_sort(rows, |a, b| {
            let (x, y) = (a.name.to_lowercase(), b.name.to_lowercase());
            let o = if desc { y.cmp(&x) } else { x.cmp(&y) };
            o.then_with(|| a.name.cmp(&b.name))
        });
    }
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
