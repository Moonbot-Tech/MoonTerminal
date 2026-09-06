//! Placement of newly created strategies plus snapshot guards for destructive strategy commands.

use moonproto::StrategySnapshot;

use super::{
    StrategyPlacementGuard, anchor_on_core, plan_insert_positions, regroup_moved,
    strategy_placements_unchanged,
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
    guard.note_queued_sync(moved.clone(), vec![1], 0);

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
        .find("strategy_placements.note_queued_sync(placements,")
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

/// A field the conversion refused must leave the strategy alone, and a strategy that kept every
/// one of its old values must not re-enter the batch.
///
/// Plausible edit this catches: restoring the old `sc.fields.insert(name, fv_from_str(..))` shape
/// with any fallback value — the whole point of `fv_from_str` returning an `Option` is that there
/// is no honest value to insert, and a fallback is what sent a silent zero to the core. And
/// without the `applied == 0` guard a fully refused edit still stamps a new `last_date` and
/// re-syncs every strategy of the core for nothing.
#[test]
fn a_refused_field_leaves_the_strategy_untouched() {
    let arm = command_arm(
        SRC,
        "CoreCmd::EditStrategyFields",
        "CoreCmd::DeleteStrategy",
    );
    assert!(
        arm.contains("let Some(value) = fv_from_str(existing.as_ref(), stype, val) else"),
        "a refused conversion must skip the field, not substitute one"
    );
    assert!(
        arm.contains("if applied == 0 {"),
        "a strategy whose every field was refused must not claim a new revision"
    );
    assert!(
        !arm.contains("fv_from_str(existing.as_ref(), stype, val).unwrap"),
        "no fallback value may be substituted for a refused conversion"
    );
}

/// Only strategies this command actually changed may be claimed as locally edited.
///
/// Plausible edit this catches: marking up front "because we are about to edit them" — a refused
/// edit changes nothing, while `strat_db` spends the next 30 s attributing any genuinely external
/// change to this terminal.
#[test]
fn only_an_applied_edit_claims_local_origin() {
    let arm = command_arm(
        SRC,
        "CoreCmd::EditStrategyFields",
        "CoreCmd::DeleteStrategy",
    );
    let mark = arm
        .find("local_strat_edits.mark(")
        .expect("the arm must still claim local origin for what it edits");
    let rebuild = arm
        .find("rebuild_sync(")
        .expect("the arm rebuilds the full set");
    assert!(
        mark > rebuild,
        "the claim must follow the rebuild that decides what was actually edited"
    );
}

/// `StrategyPlacementGuard::pending_order`: a reorder is owed to the core until the core itself
/// publishes an order. Every other strategy command in that window rebuilds its outgoing list from
/// the CONFIRMED order, so without this the next checkbox or field edit would hand the core back
/// the arrangement the operator had just replaced.
#[test]
fn a_queued_order_is_owed_to_the_core_until_it_publishes_one() {
    let mut guard = StrategyPlacementGuard::new();
    assert_eq!(guard.pending_order(7), None);

    guard.note_queued_sync(vec![(1, String::new())], vec![3, 1, 2], 7);
    // The confirmed order has not moved, so this terminal's sequence is still the newest word.
    assert_eq!(guard.pending_order(7), Some([3, 1, 2].as_slice()));
    assert_eq!(guard.pending_order(7), Some([3, 1, 2].as_slice()));
}

/// The other half of the same rule, and the one that makes it terminate: once the core has
/// published an order — accepting ours or overruling it — the queued sequence is dropped. Kept, it
/// would be re-asserted on every later sync forever, against a core that had already answered.
#[test]
fn the_cores_own_published_order_retires_the_queued_one() {
    let mut guard = StrategyPlacementGuard::new();
    guard.note_queued_sync(vec![(1, String::new())], vec![3, 1, 2], 7);
    assert_eq!(guard.pending_order(9), None);
    // ... and it stays retired, including for a later call that repeats the old version.
    assert_eq!(guard.pending_order(7), None);
}

/// Builds a snapshot carrying only what [`regroup_moved`] reads: its id and its folder path.
fn placed(id: u64, path: &str) -> StrategySnapshot {
    StrategySnapshot::new(
        id,
        1,
        0,
        false,
        moonproto::StrategyKind::from_ordinal(0),
        path,
        Default::default(),
    )
}

/// Names the folder of each row in order, which is what the contiguity rule is about.
fn paths(full: &[StrategySnapshot]) -> Vec<(u64, String)> {
    full.iter()
        .map(|sc| (sc.strategy_id, sc.path.to_string()))
        .collect()
}

/// A drag into a folder that already holds strategies joins that folder's run, so the destination
/// stays ONE group. Left split, the tree — which places a folder where its first strategy appears —
/// can hoist that whole folder somewhere nobody asked for.
#[test]
fn a_move_joins_the_run_its_destination_already_occupies() {
    let mut full = vec![
        placed(1, "X"),
        placed(2, "X"),
        placed(3, "Z"),
        placed(4, "X"),
    ];
    // 4 was relabelled to X by the caller and now has to reach it.
    regroup_moved(&mut full, &[(4, "X".to_string())]);
    assert_eq!(
        paths(&full),
        vec![
            (1, "X".into()),
            (2, "X".into()),
            (4, "X".into()),
            (3, "Z".into())
        ]
    );
}

/// Several rows moved at once queue up behind each other in the order they were given, instead of
/// all taking the same slot — which would reverse them — or anchoring on each other and splitting
/// the run they are trying to join.
#[test]
fn rows_moved_together_land_in_the_order_they_were_given() {
    let mut full = vec![
        placed(1, "X"),
        placed(2, "X"),
        placed(3, "Z"),
        placed(4, "X"),
    ];
    regroup_moved(&mut full, &[(2, "X".to_string()), (4, "X".to_string())]);
    assert_eq!(
        paths(&full),
        vec![
            (1, "X".into()),
            (2, "X".into()),
            (4, "X".into()),
            (3, "Z".into())
        ]
    );
}

/// A folder RENAME reaches this through the same command, and it must move nothing: every row
/// carrying the new name is one of the renamed ones, so there is no existing run to join. Relocating
/// on a rename would silently change the folder's place in the tree.
#[test]
fn a_rename_relocates_nothing() {
    let mut full = vec![placed(1, "B"), placed(2, "C"), placed(3, "B")];
    regroup_moved(&mut full, &[(1, "B".to_string()), (3, "B".to_string())]);
    assert_eq!(
        paths(&full),
        vec![(1, "B".into()), (2, "C".into()), (3, "B".into())]
    );
}

/// A move into a folder that does not exist yet has nothing to join either, so the row stays where
/// it is and the new folder is created around it. The alternative — appending to the end of the
/// whole list — is the placement this module exists to avoid.
#[test]
fn a_move_into_a_new_folder_leaves_the_row_in_place() {
    let mut full = vec![placed(1, "X"), placed(2, "NEW"), placed(3, "X")];
    regroup_moved(&mut full, &[(2, "NEW".to_string())]);
    assert_eq!(
        paths(&full),
        vec![(1, "X".into()), (2, "NEW".into()), (3, "X".into())]
    );
}

/// A row that sits BEFORE its destination run still joins it. The case is worth its own test
/// because a plan expressed in positions gets this one wrong in the opposite direction from the
/// backward move above.
#[test]
fn a_row_ahead_of_its_destination_still_joins_it() {
    let mut full = vec![placed(1, "X"), placed(2, "Z"), placed(3, "Z")];
    regroup_moved(&mut full, &[(1, "Z".to_string())]);
    assert_eq!(
        paths(&full),
        vec![(2, "Z".into()), (3, "Z".into()), (1, "X".into())]
    );
}

/// Two destinations interleaved in one move — what a folder move or rename produces whenever it
/// merges into folders that already exist. Each run comes out whole: a plan carried as absolute
/// positions goes stale as soon as the first relocation crosses another destination's slot, and
/// then both folders end up split.
#[test]
fn two_destinations_in_one_move_each_come_out_contiguous() {
    let mut full = vec![
        placed(101, "D2"),
        placed(1, "D1"),
        placed(11, "D1"),
        placed(21, "D2"),
        placed(22, "D2"),
        placed(12, "D1"),
    ];
    regroup_moved(
        &mut full,
        &[
            (11, "D1".to_string()),
            (21, "D2".to_string()),
            (22, "D2".to_string()),
            (12, "D1".to_string()),
        ],
    );
    let order = paths(&full);
    let at = |id: u64| order.iter().position(|(row, _)| *row == id).expect("row");
    // Every D1 row adjacent to the others, and likewise every D2 row.
    let mut d1 = [at(1), at(11), at(12)];
    let mut d2 = [at(101), at(21), at(22)];
    d1.sort_unstable();
    d2.sort_unstable();
    assert_eq!(d1[2] - d1[0], 2, "D1 must be one contiguous run: {order:?}");
    assert_eq!(d2[2] - d2[0], 2, "D2 must be one contiguous run: {order:?}");
    // ... and the rows joining each run keep the order they were given.
    assert!(at(11) < at(12), "D1 joiners keep their order: {order:?}");
    assert!(at(21) < at(22), "D2 joiners keep their order: {order:?}");
}
