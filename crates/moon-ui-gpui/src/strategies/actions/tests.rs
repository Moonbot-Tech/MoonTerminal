//! Atomic authority tests for delayed Strategies actions.

use super::{
    FieldEditPlan, field_edit_plan_authorized, strategy_action_authorized, strategy_targets_exist,
};

/// Build a one-target field plan with an explicit value for payload-drift tests.
fn field_plan(value: &str) -> FieldEditPlan {
    FieldEditPlan {
        workspace_generation: Some(7),
        targets: vec![(11, 101)],
        edit_keys: vec![(11, 101, "Param".to_string())],
        actions: vec![(
            11,
            vec![(101, vec![("Param".to_string(), value.to_string())])],
        )],
    }
}

/// `actions.rs:strategy_action_authorized` must use `all` rather than filtering hidden targets;
/// changing it to retain visible keys would send B from a stale A+B action after the rail moved.
#[test]
fn stale_multi_core_action_is_refused_instead_of_reduced_to_a_subset() {
    let captured = vec![(11, 101), (22, 202)];
    let surviving_subset: Vec<_> = captured
        .iter()
        .copied()
        .filter(|(core, _)| [22].contains(core))
        .collect();

    assert_eq!(surviving_subset, vec![(22, 202)]);
    assert!(
        !strategy_action_authorized(Some(7), Some(7), Some(&[22]), &captured),
        "the whole captured action must fail before any surviving target is dispatched"
    );
}

/// `actions.rs:strategy_action_authorized` must compare the captured Auto generation; removing
/// that comparison resurrects an A-targeted confirmation after an A -> B -> A rail round trip.
#[test]
fn returning_to_the_same_core_does_not_resurrect_an_old_action() {
    let targets = vec![(11, 101)];

    assert!(!strategy_action_authorized(
        Some(7),
        Some(9),
        Some(&[11]),
        &targets
    ));
    assert!(strategy_action_authorized(None, None, None, &targets));
}

/// `actions.rs:field_edit_plan_authorized` must compare the full current plan; removing that
/// equality sends a stale rendered value after the same target's draft changed before the click.
#[test]
fn field_apply_refuses_a_changed_payload_before_dispatch() {
    let captured = field_plan("old");
    let current = field_plan("new");

    assert!(!field_edit_plan_authorized(
        &captured,
        &current,
        Some(7),
        Some(&[11])
    ));
    assert!(field_edit_plan_authorized(
        &current,
        &current,
        Some(7),
        Some(&[11])
    ));
}

/// `actions.rs:apply_field_edits` must resolve every captured target in the live store; removing
/// that check sends retained field edits to a strategy deleted after the Apply button rendered.
#[test]
fn field_apply_refuses_a_missing_live_strategy() {
    let targets = vec![(11, 101), (11, 202)];
    let existing = [(11, 101)];

    assert!(!strategy_targets_exist(&targets, |key| existing.contains(&key)));
    assert!(strategy_targets_exist(&targets[..1], |key| existing.contains(&key)));
    let source = include_str!("../actions.rs");
    let apply = source
        .split_once("pub(super) fn apply_field_edits")
        .expect("field Apply dispatcher must exist")
        .1;
    assert!(apply.contains("strategy_targets_exist(&plan.targets"));
}

/// `tree/dialogs.rs:request_delete_selection` must store exact targets and generation in `TreeOp`;
/// deriving selection again on confirmation would delete a different surviving selection.
#[test]
fn delete_confirmation_carries_the_complete_producer_identity() {
    let ui = include_str!("../tree/ui.rs");
    let dialogs = include_str!("../tree/dialogs.rs");

    assert!(ui.contains("targets: Vec<Key>"));
    assert!(ui.contains("workspace_generation: Option<u64>"));
    assert!(dialogs.contains("self.delete_selection(&targets, workspace_generation, cx)?"));
}
