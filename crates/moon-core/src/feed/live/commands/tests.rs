//! Placement of newly created strategies plus snapshot guards for destructive strategy commands.

use super::{
    StrategyPlacementGuard, anchor_on_core, plan_insert_positions, strategy_placements_unchanged,
};

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

/// `feed/live/commands.rs:strategy_placements_unchanged`: comparing vectors without sorting would
/// reject a harmless snapshot reorder and leave the user's empty folder behind.
#[test]
fn conditional_folder_delete_ignores_snapshot_order() {
    let expected = vec![(1, "alpha".to_string()), (2, "beta".to_string())];
    let reordered = vec![(2, "beta".to_string()), (1, "alpha".to_string())];

    assert!(strategy_placements_unchanged(reordered, expected));
}

/// `feed/live/commands.rs:strategy_placements_unchanged`: ignoring ids, paths, or length would let
/// a queued create, delete, or move pass and could delete a strategy the user did not purge.
#[test]
fn conditional_folder_delete_detects_every_placement_change() {
    let expected = vec![(1, "alpha".to_string()), (2, "beta".to_string())];

    assert!(!strategy_placements_unchanged(
        vec![(1, "alpha".to_string())],
        expected.clone()
    ));
    assert!(!strategy_placements_unchanged(
        vec![(1, "alpha".to_string()), (2, "beta/child".to_string())],
        expected.clone()
    ));
    assert!(!strategy_placements_unchanged(
        vec![
            (1, "alpha".to_string()),
            (2, "beta".to_string()),
            (3, "alpha".to_string())
        ],
        expected
    ));
}

/// `feed/live/commands.rs:StrategyPlacementGuard::allows_snapshot`: ignoring the queued full-list
/// shadow would let a stale live snapshot authorize deletion after a same-terminal move/create.
#[test]
fn conditional_deletes_require_live_and_queued_placements_to_agree() {
    let original = vec![(1, "alpha".to_string())];
    let moved = vec![(1, "beta".to_string())];
    let mut guard = StrategyPlacementGuard::new();
    guard.note_queued_sync(moved.clone());

    assert!(!guard.allows_snapshot(Some(original.clone()), original));
    assert!(guard.allows_snapshot(Some(moved.clone()), moved.clone()));
    assert!(!guard.allows_snapshot(
        Some(vec![(1, "beta".to_string()), (2, "beta".to_string())]),
        moved
    ));
    assert!(!guard.allows_snapshot(None, Vec::new()));
}

/// Extract one command arm from comment-free source so guard assertions cannot match a neighboring
/// handler or a disabled line.
fn command_arm<'a>(code: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
    let start = code.find(start_marker).expect("the guarded arm must exist");
    let end = code[start..]
        .find(end_marker)
        .map(|offset| start + offset)
        .expect("the following command arm must exist");
    &code[start..end]
}

/// Assert that one arm checks the combined live/queued shadow before its sole delete call.
fn assert_shadow_guarded_delete(arm: &str, delete_call: &str) {
    let guard = arm
        .find("if strategy_placements.allows(client, expected_placements)")
        .expect("the arm must use the live-plus-queued placement guard");
    let delete = arm
        .find(delete_call)
        .expect("the guarded branch must issue its destructive command");

    assert!(
        guard < delete,
        "the live-plus-queued placement guard must precede deletion"
    );
    assert_eq!(
        arm.matches(delete_call).count(),
        1,
        "the arm must contain no second unconditional delete"
    );
}

/// `feed/live/commands.rs:rebuild_sync`: omitting the centralized shadow update after queue
/// acceptance would make a queued move/create invisible to both conditional handlers.
#[test]
fn every_full_list_sync_updates_the_placement_shadow() {
    let body = command_arm(SRC, "fn rebuild_sync(", "pub(super) fn drain_commands(");
    let queued = body
        .find("client.strategies().sync_local_strategies(full)")
        .expect("the rebuild path must queue its full list");
    let shadow = body
        .find("strategy_placements.note_queued_sync(placements)")
        .expect("accepted full-list syncs must update the synchronous shadow");

    assert!(
        queued < shadow,
        "queue acceptance must precede shadow adoption"
    );
}

/// `feed/live/commands.rs:DeleteStrategyIfUnchanged`: bypassing its exact snapshot comparison would
/// delete a moved target and make Analytics clean the folder captured before that move.
#[test]
fn conditional_strategy_handler_requires_an_unchanged_snapshot() {
    let code: String = SRC
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let arm = command_arm(
        &code,
        "CoreCmd::DeleteStrategyIfUnchanged",
        "CoreCmd::DeleteFolder",
    );

    assert_shadow_guarded_delete(arm, "client.strategies().delete(id, \"\")");
}

/// `feed/live/commands.rs:DeleteEmptyFolder`: bypassing its exact snapshot comparison would send an
/// unconditional folder-wide delete after stale UI evidence and could remove a new strategy.
#[test]
fn conditional_folder_handler_requires_a_snapshot_and_unchanged_placements() {
    let code: String = SRC
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let arm = command_arm(
        &code,
        "CoreCmd::DeleteEmptyFolder",
        "CoreCmd::CreateStrategies",
    );

    assert_shadow_guarded_delete(arm, "client.strategies().delete(0, path.as_str())");
}
