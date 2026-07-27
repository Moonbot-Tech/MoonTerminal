//! Placement of newly created strategies inside the core's list.

use super::{anchor_on_core, plan_insert_positions};

/// An anchor is honoured only on the core it names.
///
/// Plausible edit this catches: the core is dropped from the anchor "because the caller already
/// knows which core it sends to" — and a cross-core paste then anchors to whatever unrelated
/// strategy happens to hold that small id on the destination.
#[test]
fn an_anchor_from_another_core_is_dropped() {
    assert_eq!(anchor_on_core(Some((7, 42)), 7), Some(42), "its own core");
    assert_eq!(anchor_on_core(Some((7, 42)), 9), None, "a different core");
    assert_eq!(anchor_on_core(None, 7), None);
}

/// The source file, read at COMPILE time so the guard below cannot drift from it.
const SRC: &str = include_str!("../commands.rs");

/// A copy lands directly after the strategy it was copied from.
#[test]
fn a_copy_is_inserted_directly_after_its_source() {
    assert_eq!(plan_insert_positions(&[10, 20, 30], &[Some(20)]), vec![2]);
    assert_eq!(plan_insert_positions(&[10, 20, 30], &[Some(10)]), vec![1]);
    assert_eq!(plan_insert_positions(&[10, 20, 30], &[Some(30)]), vec![3]);
}

/// No anchor, or one this core does not have, uses the append fallback.
#[test]
fn an_absent_or_unknown_anchor_appends() {
    assert_eq!(plan_insert_positions(&[10, 20], &[None]), vec![2]);
    assert_eq!(plan_insert_positions(&[10, 20], &[Some(99)]), vec![2]);
    assert_eq!(plan_insert_positions(&[], &[Some(5)]), vec![0]);
}

/// Several specs sharing one anchor keep the order they were given.
///
/// Placing each at `anchor + 1` from a fixed index would reverse them.
#[test]
fn a_batch_after_one_anchor_keeps_its_own_order() {
    assert_eq!(
        plan_insert_positions(&[10, 20], &[Some(10), Some(10), Some(10)]),
        vec![1, 2, 3],
        "10, first, second, third, 20"
    );
}

/// Mixed anchors stay correct even though earlier insertions shift later ones.
///
/// Planning from one static snapshot of `ids` puts the second spec at index 2 — which by then
/// is IN FRONT of 30, not after it.
#[test]
fn mixed_anchors_account_for_the_shift_each_insertion_causes() {
    assert_eq!(
        plan_insert_positions(&[10, 20, 30], &[Some(10), Some(30)]),
        vec![1, 4],
        "after inserting at 1 the list is 10,new,20,30 — so 30 now ends at index 3"
    );
}

/// The handler must actually USE the planner.
///
/// Plausible edit this catches: changing the `CreateStrategies` arm to `full.push(...)` leaves
/// every planner assertion above green while placing every copy at the bottom of the core.
#[test]
fn the_create_handler_places_rather_than_appends() {
    // Just the CreateStrategies arm: the Restore arm below it legitimately appends, since a
    // restored strategy has no source to sit beside.
    let start = SRC
        .find("CoreCmd::CreateStrategies")
        .expect("the create arm must exist");
    let end = SRC[start..]
        .find("CoreCmd::RestoreStrategy")
        .map(|i| start + i)
        .expect("the restore arm follows it");
    let arm = &SRC[start..end];
    assert!(
        arm.contains("plan_insert_positions("),
        "the CreateStrategies arm must plan positions"
    );
    assert!(
        !arm.contains("full.push("),
        "a created strategy must be placed with `full.insert`, never appended"
    );
}
