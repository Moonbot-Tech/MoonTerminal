//! Unit tests for Auto-rail tree-expansion seeding (`initial_expanded_cores` and
//! `seed_selected_core_into`).

// NOT `use super::*`: the grandparent `strategies` module imports `gpui::*`, whose `test` macro
// shadows `#[test]`, and `state.rs` re-exposes that glob via its own `use super::*`. Reach the
// function under test by name instead.
use std::collections::HashSet;

use moon_core::session::CoreId;

use super::{initial_expanded_cores, seed_selected_core_into};

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

/// `strategies/state.rs::seed_selected_core_into` inserts the Auto-selected core into an
/// already-populated expansion set without replacing it.
///
/// Mutation: replace `expanded.extend(seed)` with `*expanded = seed`. Focusing Strategies
/// would then collapse every other core the user had open, leaving only the rail selection.
///
/// Oracle: the product contract is additive — a concrete Auto rail selection must appear in
/// `expanded_cores`, and any other cores the user already expanded must stay. The expected set
/// is that pair, not a value this helper computed.
#[test]
fn seeds_additively_inserts_the_selected_core() {
    let selected: CoreId = 2;
    let mut expanded = HashSet::from([1]);
    seed_selected_core_into(&mut expanded, Some(selected), Some(&[1, 2]));
    assert_eq!(expanded, HashSet::from([1, selected]));
}

/// `strategies/state.rs::seed_selected_core_into` still inserts into an empty stored set.
///
/// Mutation: return without inserting when `expanded` is already empty (treating a stored
/// empty set as "the user collapsed everything"). After collapsing the selected Auto core,
/// close and reopen Strategies; the core stays collapsed and the user re-finds the server
/// by hand.
///
/// Oracle: Auto mode with rail-selected core 4 in a visible workspace of `[4, 5]` must leave
/// `{4}` in `expanded_cores` even when the snapshot stored nothing.
#[test]
fn seeds_an_empty_stored_set_with_the_selected_core() {
    let selected: CoreId = 4;
    let mut expanded = HashSet::new();
    seed_selected_core_into(&mut expanded, Some(selected), Some(&[4, 5]));
    assert_eq!(expanded, HashSet::from([selected]));
}

/// Classic / Auto Overview (`selected_core: None`) must not force-expand any core.
///
/// Companion to `initial_expanded_cores(None, ...)` returning empty: applying that empty seed
/// to a live set is a no-op, so already-expanded cores stay and none are added.
#[test]
fn seeds_nothing_into_an_existing_set_when_unselected() {
    let mut expanded = HashSet::from([1]);
    seed_selected_core_into(&mut expanded, None, Some(&[1, 2]));
    assert_eq!(expanded, HashSet::from([1]));
}

/// A selected core outside the visible workspace scope must not enter `expanded_cores`.
#[test]
fn seeds_nothing_when_selected_core_is_out_of_scope() {
    let mut expanded = HashSet::from([1]);
    seed_selected_core_into(&mut expanded, Some(7), Some(&[1, 2]));
    assert_eq!(expanded, HashSet::from([1]));
}
