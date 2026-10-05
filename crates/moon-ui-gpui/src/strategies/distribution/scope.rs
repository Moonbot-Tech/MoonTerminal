//! Which strategies the "WL distribution" board covers: the selected folders and cores when the
//! tree holds a folder selection, otherwise the strategy selection.
//!
//! Kept apart from the board's rendering so the rule is testable on plain rows, without a session.

use std::collections::{HashMap, HashSet};

use moon_core::feed::{StrategyRow, strategy_path};
use moon_core::session::CoreId;

use crate::strategies::Key;
use crate::strategies::filter::PreparedFilter;
use crate::strategies::tree::ops;

/// Resolve the strategies the board covers, grouped by core.
///
/// A folder selection outranks the strategy selection, the same precedence a paste follows
/// (`resolve_paste_target`): a folder click leaves the strategy set in place, so reading that set
/// while folders are selected would put strategies on the board that the operator no longer looks
/// at — and a whitelist edit would land on them.
///
/// Args:
///     folders: Selected `(core, folder path)` nodes on visible cores; an empty path is the core
///         node itself.
///     fallback: The strategy selection, asked for only when `folders` is empty — and passed on
///         unchanged then, exactly as the tab read it before folders counted.
///     rows_of: A core's live strategy rows, or `None` when the store does not hold the core.
///     filter: The tree's prepared row filter.
///     core_shown: Whether the tree draws a core at all (the exchange filter).
///
/// Returns:
///     Strategy ids per core.
pub(super) fn board_scope<'a>(
    folders: &[(CoreId, String)],
    fallback: impl FnOnce() -> Vec<Key>,
    rows_of: impl Fn(CoreId) -> Option<&'a [StrategyRow]>,
    filter: &PreparedFilter,
    core_shown: impl Fn(CoreId) -> bool,
) -> HashMap<CoreId, HashSet<u64>> {
    let mut keys: HashMap<CoreId, HashSet<u64>> = HashMap::new();
    if folders.is_empty() {
        for (core, id) in fallback() {
            keys.entry(core).or_default().insert(id);
        }
        return keys;
    }
    // Split each key once, so the row walk below compares segments without allocating. The key
    // is slash-joined while a wire path may use a backslash; segments make the two agree.
    let mut prefixes: HashMap<CoreId, Vec<Vec<String>>> = HashMap::new();
    for (core, path) in folders {
        prefixes
            .entry(*core)
            .or_default()
            .push(strategy_path::split_path(path));
    }
    for (core, prefixes) in prefixes {
        let Some(rows) = rows_of(core).filter(|_| core_shown(core)) else {
            continue;
        };
        // Testing "under ANY selected folder" per row is what dedupes a parent and its child
        // selected together: the row is visited once whichever of them holds it.
        let ids: HashSet<u64> = rows
            .iter()
            .filter(|row| {
                filter.matches(row)
                    && prefixes
                        .iter()
                        .any(|prefix| ops::path_starts_with(&row.folder_path, prefix))
            })
            .map(|row| row.id)
            .collect();
        if !ids.is_empty() {
            keys.insert(core, ids);
        }
    }
    keys
}

#[cfg(test)]
mod tests;
