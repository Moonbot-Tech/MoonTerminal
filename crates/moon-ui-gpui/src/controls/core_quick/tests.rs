//! Regression coverage for the pure core-picker decisions: the All-row toggle, the Select all
//! action's two entry points, and the shared exchange-row id helper.
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

/// `select_all_cores` sets the selection to EXACTLY the selectable set — a stale-only selection
/// (every retained id belongs to a since-deleted core) does not survive as a residual filter.
///
/// This re-homes the concern the deleted `QuickAction::Invert` tests used to pin
/// (`inverting_every_selectable_core_is_refused_even_with_a_stale_survivor` /
/// `..._is_refused_as_unrepresentable`, from before `Invert` itself was removed): a stale-only
/// selection must not read as an effectively-empty, still-filtered state. Those tests answered it
/// by REFUSING the action (the old `quick_action_outcome` guard, `if
/// !selectable.iter().any(|core| next.contains(core))`); that guard is gone along with `Invert`,
/// replaced by a strictly stronger answer: Select all no longer treats a stale id as worth
/// preserving in ANY form, refused or not — it just overwrites it with the clean explicit set.
/// Refusal is now reserved for `selectable.is_empty()` and "already equals", covered by the two
/// tests below.
///
/// Breakage this pins (`core_quick.rs:select_all_outcome`): `next` stops being the selectable set
/// alone and is unioned with `selected` instead (e.g. `let mut next = selected.clone();
/// next.extend(selectable.iter().copied());`), out of a mistaken instinct for consistency with the
/// rest of the module, where a stale id IS deliberately kept. The stale id would then survive
/// Select all indefinitely.
#[test]
fn select_all_replaces_a_stale_only_selection_with_the_clean_full_set() {
    let selectable = vec![1u64, 2, 3];
    let mut selected: HashSet<u64> = [99].into_iter().collect(); // every id here is stale

    let changed = select_all_cores(&mut selected, &selectable);

    assert!(
        changed,
        "a stale-only selection is neither the refused empty-selectable case nor already equal to \
         the selectable set, so Select all must apply and replace it"
    );
    assert_eq!(
        selected,
        HashSet::from([1, 2, 3]),
        "the stale id must not survive as a residual filter after Select all resolves the state"
    );
}

/// `select_all_cores`/`select_all_preview` drop a stale id out of a MIXED selection too, not just a
/// stale-only one — the documented exception to every other function in this module, which keeps a
/// stale id so a vanished core cannot silently broaden a query. Select all broadens to everything by
/// definition, so keeping the id adds nothing and would make the row's count claim more cores than
/// exist.
///
/// Breakage this pins (`core_quick.rs:select_all_outcome`): the same union-with-`selected` mistake
/// as above, phrased as the case someone is most likely to introduce it FOR — "consistency" reads
/// most plausible with a live id already present. `4` from `select_all_preview` here would be the
/// exact over-reporting the docstring on `select_all_outcome` calls out.
#[test]
fn select_all_drops_a_stale_id_out_of_a_mixed_selection() {
    let selectable = vec![1u64, 2, 3];
    let mut selected: HashSet<u64> = [1, 99].into_iter().collect();

    assert_eq!(
        select_all_preview(&selected, &selectable),
        Some(3),
        "the preview must count only the selectable cores, never the stale id riding along"
    );

    assert!(select_all_cores(&mut selected, &selectable));
    assert_eq!(
        selected,
        HashSet::from([1, 2, 3]),
        "the stale id (99) must be dropped, not merged into the explicit full set"
    );
}

/// `select_all_outcome`'s empty-selectable guard: refused, even when the current selection is
/// real, because the result would otherwise be the empty set — which this codebase reads as ALL.
///
/// Breakage this pins (`core_quick.rs:select_all_outcome`): the `if selectable.is_empty() { return
/// None; }` guard is deleted. With a genuinely empty `selected` the bug would be invisible (the
/// empty-vs-empty case already refuses via the equality check below), which is exactly why this
/// test seeds `selected` with a real core: only then does removing the guard change the outcome —
/// from refused to a silent clear of a real selection down to the unfiltered state.
#[test]
fn select_all_is_refused_when_nothing_is_selectable_even_with_a_real_selection() {
    let selectable: Vec<u64> = vec![];
    let mut selected: HashSet<u64> = HashSet::from([5]);

    let changed = select_all_cores(&mut selected, &selectable);

    assert!(
        !changed,
        "with nothing selectable the result would be the empty set, which reads as ALL — refuse \
         rather than silently clear a real selection to the unfiltered state"
    );
    assert_eq!(
        selected,
        HashSet::from([5]),
        "a refused action must leave the selection untouched"
    );
}

/// `select_all_outcome`'s inert guard: refused when the selection already equals the selectable
/// set, so a redundant click reloads nothing.
///
/// Breakage this pins (`core_quick.rs:select_all_outcome`): the `(next != *selected).then_some(next)`
/// equality check is dropped in favour of always returning `Some(next)`. Every click on an already
/// disabled-looking row would then still report a change, so a consumer whose selection already
/// covers every core would reload on a click that changed nothing.
#[test]
fn select_all_is_refused_when_the_selection_already_matches() {
    let selectable = vec![1u64, 2, 3];
    let mut selected: HashSet<u64> = [1, 2, 3].into_iter().collect();

    assert!(!select_all_cores(&mut selected, &selectable));
    assert_eq!(selected, HashSet::from([1, 2, 3]));
}

/// `select_all_preview` reports the RESULT of the action, not a delta against the raw set.
///
/// Breakage this pins (`core_quick.rs:select_all_preview`): the trailing `.map(|next| next.len())`
/// is replaced by a delta computed against the raw (literally empty) `selected` set — e.g. treating
/// a core as "already selected" whenever `selected.is_empty()` and counting only entries that flip
/// relative to that reading. In the implicit-All state Select all then previews `Some(0)`: MoonUI
/// disables a zero-count row, so the one action that makes "all but one" reachable becomes
/// unclickable exactly where it is needed — this is the whole feature.
#[test]
fn selecting_all_from_the_implicit_state_previews_the_full_selectable_count() {
    let selectable = vec![1u64, 2, 3];
    let selected: HashSet<u64> = HashSet::new(); // implicit All

    let preview = select_all_preview(&selected, &selectable);

    assert_eq!(
        preview,
        Some(selectable.len()),
        "the preview must be the resulting selection size, never None (inert) and never Some(0) \
         (the intuitive but wrong 'nothing visibly changed' delta reading)"
    );
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
