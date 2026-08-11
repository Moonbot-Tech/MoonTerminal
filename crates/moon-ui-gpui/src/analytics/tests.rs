//! Regression coverage for `AnalyticsWorkspaceScope::pins_core_filter` — a plain data decision,
//! so it is tested directly rather than through a source-text scan.

use super::AnalyticsWorkspaceScope;

/// Mutation: collapsing `pins_core_filter` to an unconditional `true` would pin the core dropdown
/// on Auto's Overview row, silently overruling the user's own core filter and force-scoping every
/// read to the focused group.
#[test]
fn overview_leaves_the_core_filter_unpinned() {
    let scope = AnalyticsWorkspaceScope {
        selected_core: None,
        core_ids: vec![1, 2, 3],
    };
    assert!(!scope.pins_core_filter());
}

/// Mutation: returning `false` for a populated `selected_core` would let Analytics reads escape
/// the concrete core selected on the Auto rail.
#[test]
fn a_concrete_core_still_pins_the_filter() {
    let scope = AnalyticsWorkspaceScope {
        selected_core: Some(7),
        core_ids: vec![7],
    };
    assert!(scope.pins_core_filter());
}
