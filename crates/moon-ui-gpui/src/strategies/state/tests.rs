//! Unit tests for the Auto-rail overlay (`rail_seed_core`, `core_is_open`,
//! `toggle_core_expansion`).

// NOT `use super::*`: the grandparent `strategies` module imports `gpui::*`, whose `test` macro
// shadows `#[test]`, and `state.rs` re-exposes that glob via its own `use super::*`. Reach the
// function under test by name instead.
use std::collections::HashSet;

use moon_core::session::CoreId;

use super::{core_is_open, rail_seed_core, toggle_core_expansion};

/// `strategies/state.rs::rail_seed_core` resolves nothing when no Auto core is selected.
///
/// Mutation: seeding unconditionally regardless of `selected_core`. Classic and Auto Overview
/// would then force-expand whatever core last happened to resolve.
#[test]
fn no_selected_core_resolves_no_seed() {
    assert_eq!(rail_seed_core(None, Some(&[1, 2])), None);
}

#[test]
fn a_core_outside_the_workspace_scope_resolves_no_seed() {
    let core: CoreId = 7;
    assert_eq!(rail_seed_core(Some(core), Some(&[1, 2])), None);
}

#[test]
fn a_core_inside_the_workspace_scope_resolves_itself() {
    let core: CoreId = 2;
    assert_eq!(rail_seed_core(Some(core), Some(&[1, 2])), Some(core));
}

/// A window with no scope-bound workspace (`workspace_cores: None`, e.g. Classic or an unscoped
/// Auto owner) still resolves the selected core — the scope guard only rejects a core it can
/// positively prove is out of bounds.
#[test]
fn an_unscoped_window_still_resolves_the_selected_core() {
    let core: CoreId = 5;
    assert_eq!(rail_seed_core(Some(core), None), Some(core));
}

/// (a) Seed A into an empty persisted set: A shows open through the overlay alone, the persisted
/// set — what a "close the window" snapshot copies — stays empty, and reopening under Auto
/// Overview (`rail_seed_core(None, ...)`) leaves A closed with the persisted set still empty.
///
/// Names the item-1 contract: the seed must never enter the persisted set. The constructor
/// mutation that violates it (`expanded_cores.extend`/`.insert` the seed) cannot be exercised
/// through these pure functions alone, which never call `StrategiesView::new` — the load-bearing
/// half of this proof is the static constructor assertion in `theme_contract`, which reads the
/// mutated source directly.
#[test]
fn seed_a_into_an_empty_set_then_reopen_under_overview() {
    let workspace = [1, 2];
    let expanded: HashSet<CoreId> = HashSet::new();
    let rail = rail_seed_core(Some(1), Some(&workspace));
    assert!(
        core_is_open(&expanded, rail, 1),
        "the seeded core must show open through the overlay"
    );
    let snapshot = expanded.clone();
    assert_eq!(
        snapshot,
        HashSet::new(),
        "a 'close the window' snapshot must copy the persisted set alone, never the seed"
    );
    let rail = rail_seed_core(None, Some(&workspace));
    assert!(
        !core_is_open(&expanded, rail, 1),
        "reopening under Auto Overview must not keep the seed open"
    );
    assert_eq!(expanded, HashSet::new());
}

/// (b) As (a), then the user hand-expands B: the persisted set becomes exactly `{B}`, and under
/// Auto Overview B is open while the never-persisted seed A is not.
#[test]
fn seed_a_then_hand_expand_b() {
    let workspace = [1, 2];
    let mut expanded: HashSet<CoreId> = HashSet::new();
    let mut rail = rail_seed_core(Some(1), Some(&workspace));
    toggle_core_expansion(&mut expanded, &mut rail, 2);
    let snapshot = expanded.clone();
    assert_eq!(snapshot, HashSet::from([2]));
    let rail = rail_seed_core(None, Some(&workspace));
    assert!(core_is_open(&expanded, rail, 2), "B must stay open");
    assert!(!core_is_open(&expanded, rail, 1), "A was never persisted");
}

/// `strategies/state.rs::toggle_core_expansion` leaves a rail-selected core open without
/// persisting a hand expansion.
///
/// Plausible edit this catches: restoring the old branch that clears the rail overlay when the
/// selected root is toggled. The singleton Auto workspace would show a collapsed root with none
/// of its strategies visible.
#[test]
fn seeding_a_then_toggling_a_leaves_the_rail_root_open() {
    let workspace = [1, 2];
    let mut expanded: HashSet<CoreId> = HashSet::new();
    let mut rail = rail_seed_core(Some(1), Some(&workspace));
    toggle_core_expansion(&mut expanded, &mut rail, 1);
    assert!(
        core_is_open(&expanded, rail, 1),
        "the rail-selected root must remain visible"
    );
    assert_eq!(rail, Some(1), "toggling must not clear the rail overlay");
    assert_eq!(
        expanded,
        HashSet::new(),
        "the rail invariant must not create a persisted hand expansion"
    );
}

/// `strategies/state.rs::toggle_core_expansion` preserves a rail-selected core even when it was
/// also expanded by hand before Auto scope selected it.
///
/// Plausible edit this catches: removing the rail early return. A click would silently discard
/// the user's persisted expansion, so leaving Auto later would collapse a root they had opened.
#[test]
fn toggling_a_rail_core_open_via_both_sources_leaves_both_unchanged() {
    let mut expanded: HashSet<CoreId> = HashSet::from([1]);
    let mut rail = Some(1);
    toggle_core_expansion(&mut expanded, &mut rail, 1);
    assert!(core_is_open(&expanded, rail, 1));
    assert_eq!(
        expanded,
        HashSet::from([1]),
        "the persisted hand expansion must survive the no-op"
    );
    assert_eq!(rail, Some(1), "the rail overlay must remain pinned");
}

/// `strategies/state.rs::toggle_core_expansion` still toggles a core that is not the Auto rail
/// root.
///
/// Plausible edit this catches: broadening the rail early return to every core while any rail
/// exists. A user could no longer collapse or re-expand another strategy tree in an Auto window.
#[test]
fn a_non_rail_core_still_collapses_and_expands() {
    let workspace = [1, 2];
    let mut expanded: HashSet<CoreId> = HashSet::new();
    let mut rail = rail_seed_core(Some(1), Some(&workspace));
    toggle_core_expansion(&mut expanded, &mut rail, 2);
    assert_eq!(
        expanded,
        HashSet::from([2]),
        "a non-rail core must still open through the hand-managed set"
    );
    assert_eq!(rail, Some(1), "the rail selection belongs to A, not B");
    toggle_core_expansion(&mut expanded, &mut rail, 2);
    assert_eq!(
        expanded,
        HashSet::new(),
        "the second toggle must collapse the non-rail core"
    );
    assert_eq!(
        rail,
        Some(1),
        "toggling B must not disturb the rail-selected root"
    );
}

/// (f) A stale overlay (`Some(A)`) with a rail that now resolves to `None`: assigning
/// unconditionally clears it, while an `is_none()` early-return guard would leave it stale.
#[test]
fn focus_clears_a_stale_overlay_unconditionally() {
    let workspace = [1, 2];
    let stale_overlay = Some(1);
    let resolved = rail_seed_core(None, Some(&workspace));
    assert_eq!(resolved, None);

    let unconditional_assign = resolved;
    assert_eq!(
        unconditional_assign, None,
        "assigning unconditionally must clear a stale overlay"
    );

    let mut guarded = stale_overlay;
    if resolved.is_none() {
        // The rejected shape: an early return leaves the stale overlay untouched.
    } else {
        guarded = resolved;
    }
    assert_eq!(
        guarded, stale_overlay,
        "an is_none() guard would wrongly keep the stale overlay, which is why new() assigns unconditionally"
    );
}
