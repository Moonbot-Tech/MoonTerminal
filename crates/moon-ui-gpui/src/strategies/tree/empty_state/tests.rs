//! Regressions for the Strategies tree empty-state priority ladder.

use moon_core::config::WorkspaceMode;

use crate::workspace::scope_marker::ScopeMarker;

use super::{TreeEmptyState, tree_empty_state};

/// A populated tree does not render an empty-state overlay.
///
/// Plausible breakage: moving a later priority branch ahead of the `nodes > 0` guard would make
/// an already populated tree claim a loading, filtering, or scope-empty state.
#[test]
fn a_populated_tree_has_no_empty_state() {
    let hidden_preset = ScopeMarker::new(Some(WorkspaceMode::AutoTrading), 0, 2);

    assert_eq!(
        tree_empty_state(0, 1, true, true, Some(&hidden_preset)),
        None
    );
}

/// A preset hiding every core takes priority over the generic disconnected-core state.
///
/// Plausible breakage: swapping the hidden-preset and bare no-cores branches would direct users
/// to connect cores that are present but excluded by their workspace preset.
#[test]
fn a_hidden_preset_outranks_a_bare_no_cores_reading() {
    let hidden_preset = ScopeMarker::new(Some(WorkspaceMode::AutoTrading), 0, 2);

    assert_eq!(
        tree_empty_state(0, 0, false, false, Some(&hidden_preset)),
        Some(TreeEmptyState::HiddenByPreset)
    );
}

/// Missing cores remain a generic state unless the marker hides every configured core.
///
/// Plausible breakage: loosening `hides_everything` to `hides_anything` would label a partly
/// narrowed scope as though its preset had hidden the entire tree.
#[test]
fn no_cores_without_a_hiding_marker_stays_the_plain_state() {
    let partially_hidden = ScopeMarker::new(Some(WorkspaceMode::Classic), 1, 2);

    assert_eq!(
        tree_empty_state(0, 0, false, false, None),
        Some(TreeEmptyState::NoCores)
    );
    assert_eq!(
        tree_empty_state(0, 0, false, false, Some(&partially_hidden)),
        Some(TreeEmptyState::NoCores)
    );
}

/// A core awaiting its first snapshot means the tree is still loading.
///
/// Plausible breakage: moving or dropping the loading branch below the filter branch would blame
/// a narrowing filter while a visible core may still send strategies.
#[test]
fn an_unanswered_core_outranks_no_strategies() {
    assert_eq!(
        tree_empty_state(1, 0, true, false, None),
        Some(TreeEmptyState::Loading)
    );
    assert_eq!(
        tree_empty_state(1, 0, true, true, None),
        Some(TreeEmptyState::Loading)
    );
}

/// A narrowing filter is named only after all stronger empty-state reasons are absent.
///
/// Plausible breakage: swapping the filtering and no-strategies branches would make an unfiltered
/// empty tree report a filter and a filtered tree report no strategies.
#[test]
fn a_narrowing_filter_is_blamed_only_after_everything_else_is_ruled_out() {
    assert_eq!(
        tree_empty_state(1, 0, false, true, None),
        Some(TreeEmptyState::NothingMatches)
    );
    assert_eq!(
        tree_empty_state(1, 0, false, false, None),
        Some(TreeEmptyState::NoStrategies)
    );
}
