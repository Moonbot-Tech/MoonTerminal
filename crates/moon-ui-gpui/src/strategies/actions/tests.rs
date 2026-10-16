//! Atomic authority tests for delayed Strategies actions.

use super::{
    FieldEditPlan, StartStopPlan, buy_refresh_enabled, field_edit_plan_authorized,
    retain_live_field_edits, start_stop_core_scope, start_stop_payload, strategy_action_authorized,
    strategy_targets_exist,
};
use crate::strategies::filter::{QuickFilter, StrategyFilter};
use crate::strategies::tree::ops::NavNode;
use moon_core::feed::{FeedMsg, StrategyRow};
use moon_core::session::CoreStore;
use moon_core::session::store::CoreData;
use std::collections::HashMap;

/// Two cores with synthetic rows; CORE_A has both checked and unchecked strategies.
fn bulk_cores() -> (Vec<(u64, String)>, HashMap<u64, CoreData>) {
    let make_row = |id, checked| StrategyRow {
        id,
        name: format!("UNIT_{id:02}"),
        kind: "Test".into(),
        kind_ordinal: 0,
        folder_path: String::new(),
        checked,
        is_short: false,
        fields: Vec::new(),
    };
    let mut data = HashMap::new();
    let mut core_a = CoreData::default();
    core_a.strategies = vec![make_row(1, true), make_row(2, true), make_row(3, false)];
    data.insert(7, core_a);
    let mut core_b = CoreData::default();
    core_b.strategies = vec![make_row(4, true)];
    data.insert(8, core_b);
    (vec![(7, "CORE_A".into()), (8, "CORE_B".into())], data)
}

/// Ignoring the visible core set would count and dispatch CORE_B under a filter showing CORE_A.
/// A core row alone is sufficient even with its subtree folded; engine actions cover that core's
/// whole checked set, and hidden-core staging must survive unchanged.
#[test]
fn filtered_bulk_payload_counts_and_dispatches_only_visible_cores() {
    let (cores, data) = bulk_cores();
    let filter = StrategyFilter {
        quick: Some(QuickFilter::Stopped),
        ..Default::default()
    };
    let visible = start_stop_core_scope(&filter, &[NavNode::Core(7)]);
    let mut staged = HashMap::from([((7, 2), false), ((7, 3), true), ((8, 4), false)]);
    let (targets, actions) =
        start_stop_payload(&cores, |core| data.get(&core), &staged, visible.as_ref());
    let plan = StartStopPlan {
        workspace_generation: None,
        targets,
        actions,
    };
    assert_eq!(plan.targets, vec![(7, 1), (7, 2), (7, 3)]);
    assert_eq!(plan.actions, vec![(7, vec![(2, false), (3, true)])]);
    assert_eq!((plan.target_count(), plan.core_count()), (3, 1));
    plan.clear_applied_staging(&mut staged);
    assert_eq!(staged, HashMap::from([((8, 4), false)]));
}

/// Treating absent rows as visible would start hidden-only checked cores. Missing core snapshots
/// must also fail closed even if an old navigation row still names that core.
#[test]
fn hidden_only_or_missing_bulk_targets_disable_the_action() {
    let (cores, data) = bulk_cores();
    let filter = StrategyFilter {
        search: "missing".into(),
        ..Default::default()
    };
    let visible = start_stop_core_scope(&filter, &[]);
    let (targets, actions) = start_stop_payload(
        &cores,
        |core| data.get(&core),
        &HashMap::new(),
        visible.as_ref(),
    );
    let plan = StartStopPlan {
        workspace_generation: None,
        targets,
        actions,
    };
    assert_eq!((plan.target_count(), plan.core_count()), (0, 0));
    assert!(plan.actions.is_empty());
    let visible = start_stop_core_scope(&filter, &[NavNode::Core(7)]);
    let (targets, actions) =
        start_stop_payload(&cores, |_| None, &HashMap::new(), visible.as_ref());
    assert!(targets.is_empty() && actions.is_empty());
}

/// Applying expansion visibility without a filter would change the existing collapsed-tree
/// action: no filter must continue counting and acting on all workspace checked strategies.
#[test]
fn unfiltered_bulk_payload_includes_all_checked_cores() {
    let (cores, data) = bulk_cores();
    let visible = start_stop_core_scope(&StrategyFilter::default(), &[]);
    assert!(visible.is_none());
    let (targets, actions) = start_stop_payload(
        &cores,
        |core| data.get(&core),
        &HashMap::new(),
        visible.as_ref(),
    );
    assert_eq!(targets, vec![(7, 1), (7, 2), (8, 4)]);
    assert_eq!(actions, vec![(7, vec![]), (8, vec![])]);
}

/// Missing one narrowing dimension would let that filter start cores with no row. All quick
/// chips and dropdowns share the same actual tree-core scope; blank search preserves no-filter behavior.
#[test]
fn every_filter_dimension_activates_tree_core_scoping() {
    use moon_core::session::core_order::ExchangeSection;
    let mut filters = vec![
        StrategyFilter {
            search: "UNIT".into(),
            ..Default::default()
        },
        StrategyFilter {
            kind: Some(1),
            ..Default::default()
        },
        StrategyFilter {
            dir: Some(true),
            ..Default::default()
        },
        StrategyFilter {
            exchange: Some(ExchangeSection::Unidentified),
            ..Default::default()
        },
        StrategyFilter {
            active_only: true,
            ..Default::default()
        },
    ];
    filters.extend(QuickFilter::ALL.into_iter().map(|quick| StrategyFilter {
        quick: Some(quick),
        ..Default::default()
    }));
    for filter in filters {
        let scope =
            start_stop_core_scope(&filter, &[NavNode::Core(7), NavNode::Strategy(7, 1)]).unwrap();
        assert_eq!(scope.into_iter().collect::<Vec<_>>(), vec![7]);
    }
    assert!(
        start_stop_core_scope(
            &StrategyFilter {
                search: " , ! ".into(),
                ..Default::default()
            },
            &[]
        )
        .is_none()
    );
}

/// Removing a supported wire name disables real MoonShot edits; broadening the names/kinds offers
/// an action the core ignores. The oracle is MoonProto's standing-BUY refresh protocol contract.
#[test]
fn buy_refresh_requires_supported_moonshot_fields_and_no_open_edit() {
    for name in ["OrderSize", "Short", "EmulatorMode", "AutoCancelBuy"] {
        assert!(buy_refresh_enabled([("MoonShot", name)], false), "{name}");
        assert!(
            !buy_refresh_enabled([("MoonShot", name)], true),
            "overlapping {name}"
        );
        assert!(
            !buy_refresh_enabled([("MoonHook", name)], false),
            "other kind {name}"
        );
    }
    for name in [
        "SellPrice",
        "AutoCancelLowerBuy",
        "SignalType",
        "order_size",
        "ordersize",
    ] {
        assert!(!buy_refresh_enabled([("MoonShot", name)], false), "{name}");
    }
    assert!(!buy_refresh_enabled([], false));
    assert!(buy_refresh_enabled(
        [("MoonHook", "Short"), ("MoonShot", "OrderSize")],
        false
    ));
    assert!(!buy_refresh_enabled(
        [("MoonHook", "OrderSize"), ("MoonShot", "SellPrice")],
        false
    ));
}

/// Dropping the flag at either UI boundary silently turns the explicit action into plain Save.
///
/// The buttons live on the tab strip above both tabs (`distribution/view.rs`,
/// `field_edit_actions`), not in the parameters header any more.
#[test]
fn refresh_button_dispatches_one_flagged_edit_and_rechecks_eligibility() {
    let buttons = include_str!("../distribution/view.rs");
    let actions = include_str!("../actions.rs");
    assert!(buttons.contains("this.apply_field_edits(apply_plan.as_ref(), true, cx)"));
    assert!(buttons.contains("this.apply_field_edits(apply_plan.as_ref(), false, cx)"));
    assert!(buttons.contains(".disabled(!can_refresh)"));
    let apply = actions
        .split_once("pub(super) fn apply_field_edits(")
        .unwrap()
        .1;
    let check = apply
        .find("!self.can_refresh_buys(&sendable, b.session.store())")
        .unwrap();
    let send = apply.find(".edit_strategies_with_order_refresh(").unwrap();
    assert!(check < send);
    assert_eq!(
        apply
            .matches(".edit_strategies_with_order_refresh(")
            .count(),
        1
    );
    assert!(!apply.contains(".edit_strategies("));
    let compact: String = apply.split_whitespace().collect();
    assert!(
        compact
            .replace(",)", ")")
            .contains(".edit_strategies_with_order_refresh(*core,edits.clone(),apply_to_orders)")
    );
}

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

/// Build a synthetic snapshot row; folder paths distinguish bulk removal from single deletion.
fn draft_row(id: u64, folder: &str) -> StrategyRow {
    StrategyRow {
        id,
        name: format!("DROPS_{id:02}"),
        kind: "Test".into(),
        kind_ordinal: 0,
        folder_path: folder.into(),
        checked: false,
        is_short: false,
        fields: Vec::new(),
    }
}

/// Publish exactly the same store message used for core-originated, folder and drag deletions.
fn draft_snapshot(store: &mut CoreStore, core: u64, rows: Vec<StrategyRow>) {
    store.ensure(core);
    store
        .core_mut(core)
        .unwrap()
        .apply(FeedMsg::Strategies(rows));
}

/// Keeping an orphan would leave the action bar pending and poison a later surviving edit.
/// Matching id alone would instead discard the other core's still-live draft.
#[test]
fn field_drafts_drop_deleted_identity_and_allow_a_later_apply() {
    const CORE_A: u64 = 7;
    const CORE_B: u64 = 8;
    let mut store = CoreStore::default();
    draft_snapshot(&mut store, CORE_A, vec![draft_row(1, ""), draft_row(2, "")]);
    draft_snapshot(&mut store, CORE_B, vec![draft_row(1, "")]);
    let mut edits = HashMap::from([
        ((CORE_A, 1, "OrderSize".into()), "20".into()),
        ((CORE_A, 1, "Short".into()), "Yes".into()),
        ((CORE_B, 1, "OrderSize".into()), "30".into()),
    ]);
    assert!(!retain_live_field_edits(&mut edits, &store));
    draft_snapshot(&mut store, CORE_A, vec![draft_row(2, "")]);
    assert!(retain_live_field_edits(&mut edits, &store));
    assert_eq!(
        edits,
        HashMap::from([((CORE_B, 1, "OrderSize".into()), "30".into())])
    );
    let empty = FieldEditPlan::capture(&edits, Some(&[CORE_A]), None);
    assert!(empty.edit_keys().is_empty());
    assert!(empty.targets.is_empty() && empty.actions.is_empty());

    edits.insert((CORE_A, 2, "OrderSize".into()), "40".into());
    let next = FieldEditPlan::capture(&edits, Some(&[CORE_A]), None);
    assert_eq!(next.targets, vec![(CORE_A, 2)]);
    assert_eq!(
        next.actions,
        vec![(CORE_A, vec![(2, vec![("OrderSize".into(), "40".into())])])]
    );
    assert!(strategy_targets_exist(&next.targets, |(core, id)| {
        crate::strategies::logic::row(&store, core, id).is_some()
    }));
}

/// Cleaning only a selection/delete handler misses folder children, drag removals and hidden
/// cores. Sequential store replacements must retire every absent draft without touching survivors.
#[test]
fn field_drafts_reconcile_folder_and_drag_removals_from_store() {
    const CORE_A: u64 = 7;
    const CORE_B: u64 = 8;
    let mut store = CoreStore::default();
    draft_snapshot(
        &mut store,
        CORE_A,
        vec![
            draft_row(1, "DROPS"),
            draft_row(2, "DROPS/CHILD"),
            draft_row(3, ""),
        ],
    );
    draft_snapshot(&mut store, CORE_B, vec![draft_row(1, "")]);
    let mut edits = HashMap::from([
        ((CORE_A, 1, "OrderSize".into()), "10".into()),
        ((CORE_A, 2, "OrderSize".into()), "20".into()),
        ((CORE_A, 3, "OrderSize".into()), "30".into()),
        ((CORE_B, 1, "OrderSize".into()), "40".into()),
    ]);
    draft_snapshot(&mut store, CORE_A, vec![draft_row(3, "")]);
    assert!(retain_live_field_edits(&mut edits, &store));
    let visible = FieldEditPlan::capture(&edits, Some(&[CORE_B]), None);
    assert_eq!(visible.targets, vec![(CORE_B, 1)]);
    assert_eq!(edits.len(), 2);
    draft_snapshot(&mut store, CORE_A, Vec::new());
    assert!(retain_live_field_edits(&mut edits, &store));
    assert_eq!(
        edits,
        HashMap::from([((CORE_B, 1, "OrderSize".into()), "40".into())])
    );
    // Reappearance of a deleted id must not resurrect the abandoned values.
    draft_snapshot(&mut store, CORE_A, vec![draft_row(1, "")]);
    assert!(!retain_live_field_edits(&mut edits, &store));
    assert!(
        FieldEditPlan::capture(&edits, Some(&[CORE_A]), None)
            .actions
            .is_empty()
    );
}

/// Reducing a racing captured batch would send a subset on the first click. Removing the guard
/// sends a vanished id; failing to reconcile afterwards makes every following click fail too.
#[test]
fn field_drafts_rebuild_after_a_racing_apply_refuses_the_whole_plan() {
    const CORE_A: u64 = 7;
    let mut store = CoreStore::default();
    draft_snapshot(&mut store, CORE_A, vec![draft_row(1, ""), draft_row(2, "")]);
    let mut edits = HashMap::from([
        ((CORE_A, 1, "OrderSize".into()), "10".into()),
        ((CORE_A, 2, "OrderSize".into()), "20".into()),
    ]);
    let captured = FieldEditPlan::capture(&edits, Some(&[CORE_A]), Some(9));
    draft_snapshot(&mut store, CORE_A, vec![draft_row(2, "")]);
    let before_refresh = FieldEditPlan::capture(&edits, Some(&[CORE_A]), Some(9));
    assert!(field_edit_plan_authorized(
        &captured,
        &before_refresh,
        Some(9),
        Some(&[CORE_A])
    ));
    assert!(!strategy_targets_exist(&captured.targets, |(core, id)| {
        crate::strategies::logic::row(&store, core, id).is_some()
    }));
    assert!(retain_live_field_edits(&mut edits, &store));
    let next = FieldEditPlan::capture(&edits, Some(&[CORE_A]), Some(9));
    assert_eq!(next.targets, vec![(CORE_A, 2)]);
    assert_eq!(
        next.actions,
        vec![(CORE_A, vec![(2, vec![("OrderSize".into(), "20".into())])])]
    );
    assert!(!field_edit_plan_authorized(
        &captured,
        &next,
        Some(9),
        Some(&[CORE_A])
    ));
    assert!(field_edit_plan_authorized(
        &next,
        &next,
        Some(9),
        Some(&[CORE_A])
    ));
    assert!(strategy_targets_exist(&next.targets, |(core, id)| {
        crate::strategies::logic::row(&store, core, id).is_some()
    }));
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

/// N on the Start/Stop buttons: a ticked row counts, a staged untick counts (the press sends it),
/// an unticked untouched row does not, and nothing ticked or staged means zero — the disabled state.
#[test]
fn start_stop_targets_count_ticked_rows_and_staged_changes_only() {
    use super::core_start_stop_targets;
    use moon_core::feed::StrategyRow;
    use std::collections::HashMap;
    let row = |id: u64, checked: bool| StrategyRow {
        id,
        name: format!("UNIT_{id:02}"),
        kind: "Test".to_string(),
        kind_ordinal: 0,
        folder_path: String::new(),
        checked,
        is_short: false,
        fields: Vec::new(),
    };
    let rows = [row(1, true), row(2, true), row(3, false), row(4, false)];
    let mut staged = HashMap::new();
    staged.insert((7, 2), false);
    staged.insert((7, 4), true);
    let (targets, checks) = core_start_stop_targets(7, &rows, &staged);
    assert_eq!(targets, vec![(7, 1), (7, 2), (7, 4)]);
    assert_eq!(checks, vec![(2, false), (4, true)]);

    let idle = [row(1, false), row(2, false)];
    let (targets, checks) = core_start_stop_targets(7, &idle, &HashMap::new());
    assert!(targets.is_empty() && checks.is_empty());
}
