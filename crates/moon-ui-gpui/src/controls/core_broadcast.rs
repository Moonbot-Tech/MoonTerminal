//! Cross-window core filter: one selection published by the Profit Monitor and adopted by every
//! main-window panel that owns a core selector.
//!
//! The transport is the ordinary Backend + revision-entity pair (`Backend::core_filter` and
//! `crate::CoreFilterRevision`); this module owns only the two pure decisions — what one click on a
//! row of cores makes of the selection, and what one panel makes of the result. Keeping both here
//! rather than in the monitor is what lets them be tested without a window, and keeps them beside
//! [`super::core_combo`], whose empty-means-all convention and multi-select rule they share.
//!
//! Empty means ALL cores in every consumer, so "clear the filter" and "select nothing" are the same
//! value — a panel with an empty retained set queries its whole group.
//!
//! Deliberately NOT wired: the Analytics window, whose core selection is persisted and belongs to
//! that tool's own session; the global Assets window and the Analytics-owned standalone Report,
//! both application-wide surfaces with a scope of their own; and the Screener, the Log and the
//! header pill, which are single-select source pickers rather than filters. The panels that DO
//! adopt are pinned by `tests/theme_contract/analytics.rs`.

use std::collections::HashSet;

use moon_core::session::CoreId;

#[cfg(test)]
mod tests;

/// Resolve what one click on a row's cores makes of the current broadcast selection.
///
/// Plain click REPLACES: the clicked row becomes the whole filter, unless it already is it — then
/// the filter clears back to all cores, so the same gesture that narrowed the view widens it again.
/// `additive` (Ctrl / ⌘) hands the row to [`super::toggle_exchange_cores`] instead, which is the
/// multi-select rule every core selector in the terminal already uses.
///
/// Args:
///     current: Currently broadcast selection; empty means every core.
///     clicked: Cores merged into the clicked row — one in Core mode, an exchange's worth in
///         Exchange mode.
///     additive: Whether the secondary modifier asked for multi-selection.
///
/// Returns:
///     The replacement selection. An empty `clicked` row cannot change anything and returns
///     `current` unchanged, so a row that outlived its cores is inert rather than clearing the
///     filter.
pub(crate) fn next_core_filter(
    current: &HashSet<CoreId>,
    clicked: &[CoreId],
    additive: bool,
) -> HashSet<CoreId> {
    if additive {
        // Every clicked core is its own availability here: the row is the universe being toggled.
        let mut next = current.clone();
        let clicked: HashSet<CoreId> = clicked.iter().copied().collect();
        super::toggle_exchange_cores(&mut next, &clicked, clicked.iter().copied());
        return next;
    }
    if clicked.is_empty() {
        return current.clone();
    }
    let clicked: HashSet<CoreId> = clicked.iter().copied().collect();
    if *current == clicked {
        return HashSet::new();
    }
    clicked
}

/// Apply a broadcast to one panel's retained core filter.
///
/// The monitor spans the whole terminal while a panel sees one group or scope, so three answers are
/// possible and only one of them is "take it verbatim":
///
/// - An EMPTY broadcast is the release, and it reaches every panel — including one that ignored the
///   narrowing broadcast before it and is holding a hand-set filter of its own. Releasing WIDENS
///   what that panel shows, and telling the two cases apart would cost every panel a memory of the
///   last broadcast it accepted: more state than an occasional extra widening is worth.
/// - A broadcast naming none of this panel's cores is not about this panel. It is ignored, so a
///   group window that cannot show the clicked core keeps showing its own cores instead of an empty
///   table it has no way to explain.
/// - Otherwise the panel takes the INTERSECTION. Passing foreign ids through would be worse than
///   useless: Assets prunes its retained set against its own scope on the next rebuild, and a set
///   pruned to empty reads as "all cores" — the exact inversion of what was asked.
///
/// Args:
///     retained: The panel's own filter, replaced in place when the broadcast applies to it.
///     broadcast: Cores currently published by the Profit Monitor.
///     available: Every core in the adopting panel's own scope.
///
/// Returns:
///     Whether `retained` changed and the panel must rebuild.
pub(crate) fn apply_core_broadcast(
    retained: &mut HashSet<CoreId>,
    broadcast: &HashSet<CoreId>,
    available: impl IntoIterator<Item = CoreId>,
) -> bool {
    let next = if broadcast.is_empty() {
        HashSet::new()
    } else {
        let kept: HashSet<CoreId> = available
            .into_iter()
            .filter(|core| broadcast.contains(core))
            .collect();
        if kept.is_empty() {
            return false;
        }
        kept
    };
    if *retained == next {
        return false;
    }
    *retained = next;
    true
}
