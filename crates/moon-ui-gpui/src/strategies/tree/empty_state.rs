//! Pure priority ladder for the Strategies tree's empty state. GPUI-free on purpose, so the
//! ordering itself is testable without a window: [`tree_empty_state`] takes only the counts and
//! flags a render frame already has, and [`headline`] maps each variant to its locale key.

use crate::workspace::scope_marker::ScopeMarker;

/// Which sentence the tree pane's empty overlay states, in the priority [`tree_empty_state`]
/// resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TreeEmptyState {
    /// The active workspace preset hides every core the tree would otherwise show.
    HiddenByPreset,
    /// No core is connected at all, unrelated to any preset.
    NoCores,
    /// At least one visible core exists but has not sent its first strategy snapshot yet.
    Loading,
    /// A filter dimension is active and excludes every row.
    NothingMatches,
    /// Cores are visible and loaded, but none carries a strategy.
    NoStrategies,
}

/// Resolve the tree pane's empty state, or `None` while it has rows to show.
///
/// The order is fixed and each condition is checked only once the ones above it are ruled out:
/// a preset hiding every core outranks a bare "no cores" reading (the preset is the actionable
/// fact), an unanswered core outranks "no strategies" (loading is not empty), and a filter reason
/// is named only once nothing else already explains the blank tree.
///
/// Args:
///     visible_cores: Cores the tree's own universe currently lists.
///     nodes: Rows the built tree adapter actually holds.
///     awaiting_snapshot: Whether at least one visible core has not sent a strategy snapshot yet.
///     filter_narrows: Whether the active filter excludes at least one dimension.
///     marker: This frame's scope marker, or `None` when nothing is scope-bound.
///
/// Returns:
///     `None` when `nodes > 0`; otherwise the single most relevant reason the tree is empty.
pub(super) fn tree_empty_state(
    visible_cores: usize,
    nodes: usize,
    awaiting_snapshot: bool,
    filter_narrows: bool,
    marker: Option<&ScopeMarker>,
) -> Option<TreeEmptyState> {
    if nodes > 0 {
        return None;
    }
    if visible_cores == 0 && marker.is_some_and(ScopeMarker::hides_everything) {
        return Some(TreeEmptyState::HiddenByPreset);
    }
    if visible_cores == 0 {
        return Some(TreeEmptyState::NoCores);
    }
    if awaiting_snapshot {
        return Some(TreeEmptyState::Loading);
    }
    if filter_narrows {
        return Some(TreeEmptyState::NothingMatches);
    }
    Some(TreeEmptyState::NoStrategies)
}

/// Localize one empty-state variant's headline sentence.
///
/// Args:
///     state: Variant resolved by [`tree_empty_state`].
///
/// Returns:
///     The headline text for that state, read from its locale key.
pub(super) fn headline(state: TreeEmptyState) -> String {
    match state {
        TreeEmptyState::HiddenByPreset => rust_i18n::t!("workspace.scope.all_hidden").to_string(),
        TreeEmptyState::NoCores => rust_i18n::t!("strat.tree_empty_no_cores").to_string(),
        TreeEmptyState::Loading => rust_i18n::t!("strat.tree_empty_loading").to_string(),
        TreeEmptyState::NothingMatches => rust_i18n::t!("strat.tree_empty_filtered").to_string(),
        TreeEmptyState::NoStrategies => rust_i18n::t!("strat.tree_empty_no_strategies").to_string(),
    }
}

#[cfg(test)]
mod tests;
