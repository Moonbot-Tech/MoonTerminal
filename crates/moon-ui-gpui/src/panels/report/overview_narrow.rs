//! Auto Overview's Report-only core narrowing.
//!
//! Under Auto "Full summary" the Report's core trigger is the shared core combo over the Overview
//! scope's own cores. What the user picks there narrows ONLY this Report's read — never the
//! workspace scope, the rail or an action path — and lives apart from the Classic retained
//! selection so neither mode leaks into the other. These are the pure decisions behind it.

use std::collections::HashSet;

use moon_core::config::CoreGroup;
use moon_core::session::CoreId;

/// Resolve the narrowing against the Overview scope it applies to.
///
/// A narrowing that no longer intersects the scope (a persisted set whose cores left the group)
/// falls back to the whole scope rather than to an empty result, and one covering the whole scope
/// is no narrowing at all.
///
/// Args:
///     scope: The Overview scope's ids, in canonical order.
///     narrow: The Report's retained narrowing; empty means the whole scope.
///
/// Returns:
///     The narrowed ids in scope order, or `None` when the whole scope applies.
pub(super) fn narrowed_ids(scope: &[CoreId], narrow: &HashSet<CoreId>) -> Option<Vec<CoreId>> {
    if narrow.is_empty() {
        return None;
    }
    let kept: Vec<CoreId> = scope
        .iter()
        .copied()
        .filter(|core| narrow.contains(core))
        .collect();
    (!kept.is_empty() && kept.len() < scope.len()).then_some(kept)
}

/// The selection the combo renders: the effective narrowing, or empty for the whole scope.
///
/// Args:
///     scope: The Overview scope's ids.
///     narrow: The Report's retained narrowing.
///
/// Returns:
///     The narrowed ids as a set, empty when [`narrowed_ids`] resolves to the whole scope.
pub(super) fn shown_selection(scope: &[CoreId], narrow: &HashSet<CoreId>) -> HashSet<CoreId> {
    narrowed_ids(scope, narrow)
        .map(|ids| ids.into_iter().collect())
        .unwrap_or_default()
}

/// Choose the trigger's text for the current narrowing.
///
/// Args:
///     scope: The Overview scope's ids.
///     narrow: The Report's retained narrowing.
///     groups: Saved core groups, matched against the scope the same way the menu ticks them.
///     overview: The localized "Full summary" word shown for the whole scope.
///     cores_n: Localized "N cores" summary for a narrowing that is no saved group.
///
/// Returns:
///     `overview` for the whole scope, the saved group's name when the narrowing is exactly that
///     group within the scope, else `cores_n` of the narrowed count.
pub(super) fn trigger_label(
    scope: &[CoreId],
    narrow: &HashSet<CoreId>,
    groups: &[CoreGroup],
    overview: &str,
    cores_n: &dyn Fn(usize) -> String,
) -> String {
    let Some(ids) = narrowed_ids(scope, narrow) else {
        return overview.to_string();
    };
    let universe: HashSet<CoreId> = scope.iter().copied().collect();
    let selected: HashSet<CoreId> = ids.iter().copied().collect();
    groups
        .iter()
        .find(|group| crate::controls::group_is_applied(&group.cores, &universe, &selected))
        .map_or_else(|| cores_n(ids.len()), |group| group.name.clone())
}

#[cfg(test)]
mod tests;
