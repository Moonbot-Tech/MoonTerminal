//! Regression coverage for the pure core-picker decisions: the All-row toggle, the shared
//! exchange-row id helper, and the per-exchange selection state.
//!
//! `core_quick.rs` imports only `std::collections::HashSet`, so `use super::*;` is safe here — it
//! never re-exports `gpui::*`, whose own `#[gpui::test]` would otherwise shadow the built-in
//! `#[test]` attribute and recurse.

use super::*;

/// `toggle_core_selection`'s All-row (`None`) arm must report a change ONLY when it actually
/// discards something, and the following core click must start a fresh, exclusive selection.
///
/// Re-homed from the deleted `analytics::toolbar::toggle_analytics_core_selection` tests: that
/// helper's contract moved here unchanged when Analytics adopted the shared picker.
///
/// Plausible edit this catches: deleting the `None if selected.is_empty() => false` arm as a
/// redundant special case, leaving `None` to always clear and always report `true`. Every one of
/// the six consumers would then treat a no-op click on an already-cleared All row as a change —
/// Report and Analytics would re-run a full `reports.sqlite` query, and the other four would
/// rebuild their row caches, on every such click.
#[test]
fn all_row_toggle_reports_a_change_only_when_something_was_cleared() {
    let mut selected = HashSet::from([1, 2]);

    assert!(
        toggle_core_selection(&mut selected, None),
        "clearing a non-empty explicit selection is a real change"
    );
    assert!(selected.is_empty(), "All must discard every explicit check");

    assert!(
        !toggle_core_selection(&mut selected, None),
        "an already-clear selection has nothing left to discard"
    );

    assert!(toggle_core_selection(&mut selected, Some(2)));
    assert_eq!(
        selected,
        HashSet::from([2]),
        "the first core click after All must be the only explicit check"
    );
}

/// `toggle_core_selection`'s `Some(core)` arm toggles membership and always reports a change.
#[test]
fn a_core_click_toggles_membership_and_always_reports_a_change() {
    let mut selected = HashSet::new();

    assert!(toggle_core_selection(&mut selected, Some(5)));
    assert_eq!(selected, HashSet::from([5]));

    assert!(toggle_core_selection(&mut selected, Some(5)));
    assert!(selected.is_empty());
}

/// `section_core_ids` must forward every rendered exchange member to the batch callback in
/// canonical order.
///
/// Re-homed from the deleted `core_combo::tests::section_batch_includes_every_member_in_order`:
/// the standalone helper it pinned was inlined at its one call site during Phase 2, which left the
/// "every member, in order" contract with nothing pinning it. Extracting the `.collect()` back out
/// to this named helper restores a place for that contract to live.
///
/// Breakage this pins: adding `.take(1)` (or otherwise truncating) the member iterator, so a
/// group-header click toggles only the section's first core while the remaining checkboxes stay
/// unchanged.
#[test]
fn section_core_ids_forwards_every_member_in_order() {
    let members = [(7u64, "First"), (11u64, "Second"), (19u64, "Third")];

    assert_eq!(section_core_ids(&members), vec![7, 11, 19]);
}

/// A section with no members must report `GroupCheck::None`, never the vacuously true `All` — a
/// saved-group section whose members have all been deleted must not render as complete.
#[test]
fn group_check_state_reports_none_for_a_section_with_no_members() {
    let selected: HashSet<u64> = [1, 2].into_iter().collect();

    assert_eq!(group_check_state(&[], &selected), GroupCheck::None);
}
