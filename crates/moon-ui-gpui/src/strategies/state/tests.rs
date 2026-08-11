//! Unit tests for `initial_expanded_cores` — the Auto rail's tree-expansion seed.

// NOT `use super::*`: the grandparent `strategies` module imports `gpui::*`, whose `test` macro
// shadows `#[test]`, and `state.rs` re-exposes that glob via its own `use super::*`. Reach the
// function under test by name instead.
use std::collections::HashSet;

use moon_core::session::CoreId;

use super::initial_expanded_cores;

/// `strategies/state.rs::initial_expanded_cores` seeds the tree with the Auto rail's selected
/// core, not an unconditionally empty set.
///
/// Mutation: replacing the `HashSet::from([core])` result with `HashSet::new()`. The window would
/// then always open fully collapsed even with a concrete Auto core selected on the rail, so the
/// user has to re-find that server by hand every time.
///
/// `initial_expanded_cores` is a pure function of `(Option<CoreId>, Option<&[CoreId]>) ->
/// HashSet<CoreId>` with no GPUI dependency, so it is exercised directly rather than through a
/// headlessly-constructed `StrategiesView`.
#[test]
fn no_selected_core_seeds_nothing() {
    assert_eq!(initial_expanded_cores(None, Some(&[1, 2])), HashSet::new());
}

#[test]
fn a_core_outside_the_workspace_scope_seeds_nothing() {
    let core: CoreId = 7;
    assert_eq!(
        initial_expanded_cores(Some(core), Some(&[1, 2])),
        HashSet::new()
    );
}

#[test]
fn a_core_inside_the_workspace_scope_seeds_itself() {
    let core: CoreId = 2;
    assert_eq!(
        initial_expanded_cores(Some(core), Some(&[1, 2])),
        HashSet::from([core])
    );
}

/// A window with no scope-bound workspace (`workspace_cores: None`, e.g. Classic or an
/// unscoped Auto owner) still seeds the selected core — the scope guard only rejects a core it
/// can positively prove is out of bounds.
#[test]
fn an_unscoped_window_still_seeds_the_selected_core() {
    let core: CoreId = 5;
    assert_eq!(
        initial_expanded_cores(Some(core), None),
        HashSet::from([core])
    );
}
