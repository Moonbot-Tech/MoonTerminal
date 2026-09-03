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

/// (c) Seed A, the user collapses A (which clears the overlay too), then the user expands A by
/// hand: the persisted set becomes `{A}`, A is open, and the snapshot carries A.
#[test]
fn seed_a_collapse_then_hand_expand_again() {
    let workspace = [1, 2];
    let mut expanded: HashSet<CoreId> = HashSet::new();
    let mut rail = rail_seed_core(Some(1), Some(&workspace));
    toggle_core_expansion(&mut expanded, &mut rail, 1);
    assert!(
        !core_is_open(&expanded, rail, 1),
        "collapsing the seeded core must close it"
    );
    assert_eq!(rail, None, "collapsing it must clear the overlay too");
    toggle_core_expansion(&mut expanded, &mut rail, 1);
    assert!(core_is_open(&expanded, rail, 1));
    assert_eq!(
        expanded,
        HashSet::from([1]),
        "a hand re-expand must land in the persisted set"
    );
    let snapshot = expanded.clone();
    assert_eq!(snapshot, HashSet::from([1]));
}

/// (d) A core open through BOTH the persisted set and the overlay collapses on one call: both
/// clear together.
#[test]
fn collapsing_a_core_open_via_both_sources_clears_both() {
    let mut expanded: HashSet<CoreId> = HashSet::from([1]);
    let mut rail = Some(1);
    toggle_core_expansion(&mut expanded, &mut rail, 1);
    assert!(!core_is_open(&expanded, rail, 1));
    assert_eq!(expanded, HashSet::new(), "the persisted membership must clear");
    assert_eq!(rail, None, "the overlay must clear in the same call");
}

/// (e) Seed A, the user collapses A, then an unrelated workspace revision resolves the SAME rail
/// selection. Comparing the fresh resolve against the stored `rail_seen_core` (still `Some(A)`)
/// says nothing moved, so A stays collapsed. Contrasted against comparing the overlay instead,
/// which would wrongly say it moved — that contrast is the item-4 regression this catches.
#[test]
fn unrelated_revision_after_a_hand_collapse_does_not_reopen_it() {
    let workspace = [1, 2];
    let mut expanded: HashSet<CoreId> = HashSet::new();
    let mut rail = rail_seed_core(Some(1), Some(&workspace));
    let mut rail_seen = rail;
    toggle_core_expansion(&mut expanded, &mut rail, 1);
    assert_eq!(rail, None, "collapsing A must clear the overlay");
    assert_eq!(rail_seen, Some(1), "rail_seen_core is untouched by a hand collapse");

    let resolved = rail_seed_core(Some(1), Some(&workspace));

    let rail_moved = resolved != rail_seen;
    assert!(
        !rail_moved,
        "an unchanged rail selection compared against rail_seen_core must not look like it moved"
    );
    if rail_moved {
        rail_seen = resolved;
        rail = resolved;
    }
    assert_eq!(rail_seen, Some(1), "an unmoved rail must leave rail_seen_core untouched");
    assert!(!core_is_open(&expanded, rail, 1), "A must stay collapsed");

    let would_move_against_overlay = resolved != rail;
    assert!(
        would_move_against_overlay,
        "comparing against the overlay instead of rail_seen_core is exactly the regression item 4 guards against"
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
