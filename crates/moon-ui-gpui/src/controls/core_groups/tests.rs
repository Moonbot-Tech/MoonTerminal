//! Unit coverage for the core picker's saved-group pure decisions: applying a group click and
//! resolving what a new group should save. GPUI-free, so these run without a window.

use std::collections::HashSet;

use super::{
    GroupClick, apply_core_group, group_facts, group_is_applied, saved_group_cores, saves_core,
};

/// Named breakage (`apply_core_group`): dropping the `applicable.is_empty()` guard lets a group
/// with no selectable member clear the selection. Since an empty selection means ALL cores, this
/// is the headline bug of the whole subsystem: clicking a group scoped to a different panel would
/// silently BROADEN a narrow query (e.g. one core in Report) to every core.
#[test]
fn apply_core_group_is_inert_when_no_member_is_selectable() {
    let mut selected: HashSet<u64> = [10, 20].into_iter().collect();
    let group = vec![99, 98]; // neither is in this consumer's selectable scope
    let selectable = vec![10, 20, 30];

    let changed = apply_core_group(&mut selected, &group, &selectable, GroupClick::Replace);

    assert!(!changed, "an inapplicable group must report no change");
    assert_eq!(
        selected,
        [10, 20].into_iter().collect::<HashSet<u64>>(),
        "the selection must be left untouched -- clearing it silently means ALL cores"
    );
}

/// Named breakage (`apply_core_group`): dropping the `next == *selected` check turns a repeat
/// Replace click on the same group into a toggle, unlike `next_core_filter`'s All row -- a
/// group row names a destination, and clicking it twice must not land somewhere else.
#[test]
fn replace_onto_an_already_equal_selection_is_a_no_op() {
    let mut selected: HashSet<u64> = [1, 2].into_iter().collect();
    let group = vec![1, 2];
    let selectable = vec![1, 2, 3];

    let changed = apply_core_group(&mut selected, &group, &selectable, GroupClick::Replace);

    assert!(
        !changed,
        "Replace onto an identical selection must report no change"
    );
    assert_eq!(selected, [1, 2].into_iter().collect::<HashSet<u64>>());
}

/// Named breakage (`apply_core_group`): a future author reintroducing the "empty means All, so
/// leave it empty" rule for Union too would make the first additive click on a group from the
/// implicit-All state a no-op, instead of narrowing to the group the way a first exchange click
/// already does (`core_combo::toggle_exchange_cores`).
#[test]
fn union_from_the_implicit_all_selection_narrows_to_the_group() {
    let mut selected: HashSet<u64> = HashSet::new();
    let group = vec![1, 2];
    let selectable = vec![1, 2, 3];

    let changed = apply_core_group(&mut selected, &group, &selectable, GroupClick::Union);

    assert!(
        changed,
        "Union from the implicit-All state must change the selection"
    );
    assert_eq!(
        selected,
        [1, 2].into_iter().collect::<HashSet<u64>>(),
        "Union from empty (All) must narrow to the group's applicable members, not stay empty"
    );
}

/// Named breakage (`saved_group_cores`): dropping the `configured.contains(core)` clause would
/// let Report -- whose selectable list deliberately includes long-deleted cores -- resurrect a
/// dead core into a brand new group. The implicit-All selection must also materialize to the
/// live set rather than being stored as "empty means all", which would silently change meaning
/// as cores are added later.
#[test]
fn saved_group_cores_filters_against_configured_and_materializes_implicit_all() {
    let selected: HashSet<u64> = HashSet::new(); // implicit All
    let selectable = vec![1, 2, 3]; // Report's list includes a deleted core (3)
    let configured: HashSet<u64> = [1, 2].into_iter().collect(); // 3 no longer exists

    let saved = saved_group_cores(&selected, &selectable, &configured);

    assert_eq!(
        saved,
        vec![1, 2],
        "a deleted core (3) must never be resurrected into a new group, and the implicit-All \
         selection must materialize to the currently live set"
    );
}

/// Named breakage (`group_facts`): appending the missing-member warning unconditionally would
/// show a spurious "· N missing" on every group row, even one with no dead member.
#[test]
fn group_facts_appends_the_dead_warning_only_when_present() {
    let clean = group_facts("3 cores".to_string(), 0);
    assert_eq!(
        clean, "3 cores",
        "no dead members means no trailing warning"
    );

    // The whole expected string, not a length or a prefix: a length comparison is true for ANY
    // appended text, so it would survive a wrong count, a wrong locale key or a separator change
    // -- exactly the mutations this test exists to name.
    let with_dead = group_facts("3 cores".to_string(), 2);
    let expected = format!(
        "3 cores \u{B7} {}",
        rust_i18n::t!("common.core_pick.group_dead_n", n = 2)
    );
    assert_eq!(
        with_dead, expected,
        "a dead count above zero appends exactly the localized missing-member warning"
    );
}

/// Named breakage (`saves_core`): dropping the `configured` clause would let the Save row build a
/// group out of cores that no longer exist -- in Report, whose selectable list deliberately
/// includes cores deleted long ago, that resurrects them into a brand-new group.
///
/// Asserted on `saves_core` DIRECTLY. Comparing it against `saved_group_cores` would prove
/// nothing: the latter is literally `selectable.iter().filter(|c| saves_core(..))`, so agreement
/// between them is an identity that holds whatever either function's body says. That the UI's
/// enabled gate and its click payload both route through this one predicate is a WIRING fact, and
/// it is pinned where wiring facts belong -- `theme_contract::core_pick`, reading the call sites.
#[test]
fn saves_core_requires_both_selection_and_a_configured_core() {
    let selected: HashSet<u64> = [1, 3].into_iter().collect();
    let configured: HashSet<u64> = [1, 2].into_iter().collect();

    assert!(
        saves_core(&selected, &configured, 1),
        "a selected, configured core is saved"
    );
    assert!(
        !saves_core(&selected, &configured, 3),
        "a selected core that is no longer configured must NOT be saved"
    );
    assert!(
        !saves_core(&selected, &configured, 2),
        "a configured core outside the selection must not be saved"
    );

    // The implicit-All selection materializes to every configured core, and to nothing else.
    let all: HashSet<u64> = HashSet::new();
    assert!(
        saves_core(&all, &configured, 2),
        "an empty selection means ALL cores, so a configured core is saved"
    );
    assert!(
        !saves_core(&all, &configured, 3),
        "even under implicit-All, an unconfigured core must not be saved"
    );
}

/// Named breakage (`group_is_applied`): the tick must mean "the selection IS this group", not
/// "the selection touches this group". Relaxing the equality to an overlap test — the plausible
/// edit, since `contains`/`any` reads as the obvious way to answer it — would tick every group
/// sharing one core with the current filter, so several rows would claim a scope only one of them
/// has.
#[test]
fn group_is_applied_only_on_an_exact_match_of_the_applicable_members() {
    let selectable: HashSet<u64> = [1, 2, 3].into_iter().collect();
    let group = vec![1, 2];

    let exact: HashSet<u64> = [1, 2].into_iter().collect();
    assert!(
        group_is_applied(&group, &selectable, &exact),
        "a selection equal to the group's applicable members is that group"
    );

    let superset: HashSet<u64> = [1, 2, 3].into_iter().collect();
    assert!(
        !group_is_applied(&group, &selectable, &superset),
        "a superset -- what Ctrl+click builds from a second group -- is NOT this group"
    );

    let overlap: HashSet<u64> = [1].into_iter().collect();
    assert!(
        !group_is_applied(&group, &selectable, &overlap),
        "sharing one core with the group is not being the group"
    );

    // Out-of-scope members are excluded before comparing, so a group half of which this consumer
    // cannot select still ticks when the selectable half is exactly what is selected.
    let partly_out = vec![1, 2, 99];
    assert!(
        group_is_applied(&partly_out, &selectable, &exact),
        "members this consumer cannot select must not block the match"
    );
}

/// Named breakage (`group_is_applied`): a future author "handles" the implicit-All selection by
/// materializing it into the selectable set first — the plausible edit, because `saved_group_cores`
/// two functions away legitimately does exactly that. Then a group covering every selectable core
/// would tick while the user has chosen no group at all, and the picker would claim a filter it is
/// not applying.
///
/// (An earlier version of this test named "drop the `selected.is_empty()` short-circuit" instead.
/// That mutation stayed GREEN — the equality already rejects an empty selection — which showed the
/// short-circuit was dead code, so it is gone from the production side rather than pinned here.)
#[test]
fn group_is_applied_never_ticks_the_implicit_all_selection() {
    let selectable: HashSet<u64> = [1, 2].into_iter().collect();
    let none_selected: HashSet<u64> = HashSet::new();

    assert!(
        !group_is_applied(&[1, 2], &selectable, &none_selected),
        "empty means ALL cores, which is not the same as this group"
    );
    assert!(
        !group_is_applied(&[99], &selectable, &none_selected),
        "a group with no applicable member must not tick against an empty selection either"
    );
}
