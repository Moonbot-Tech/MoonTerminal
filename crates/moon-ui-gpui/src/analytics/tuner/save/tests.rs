//! Atomic identity tests for delayed Analytics Tuner writes.

use std::collections::HashMap;

use super::{
    InlineWriteDecision, inline_write_decision, resolve_complete_target_cores,
    save_authority_matches,
};
use crate::analytics::tuner::shared::{SaveAuthority, SaveTarget};

/// Auto Overview visibility must never authorize a batch target outside the focused group.
#[test]
fn best_authority_refuses_an_outside_core() {
    assert!(super::best_targets_in_scope(
        &[target(101, 11)],
        Some(&[11])
    ));
    assert!(!super::best_targets_in_scope(
        &[target(101, 11), target(202, 22)],
        Some(&[11])
    ));
    assert!(!super::best_targets_in_scope(&[target(101, 11)], Some(&[])));
}

/// A stale or unread current value must refuse consent instead of displaying the captured value.
#[test]
fn fresh_read_must_equal_captured_now() {
    use super::super::ticks::state::NowValue;
    let captured = HashMap::from([("SellPrice".into(), NowValue::Same("1".into()))]);
    assert!(super::captured_values_match(
        &captured,
        &HashMap::from([("SellPrice".into(), "1".into())])
    ));
    assert!(!super::captured_values_match(
        &captured,
        &HashMap::from([("SellPrice".into(), "2".into())])
    ));
    assert!(!super::captured_values_match(&captured, &HashMap::new()));
}

/// Build one concrete Save target without involving Analytics UI state.
fn target(sid: i64, core: u64) -> SaveTarget {
    SaveTarget {
        sid,
        core: Some(core),
        name: format!("strategy-{sid}"),
    }
}

/// Batch confirmation authority is independent of table selection but never of run or checked targets.
#[test]
fn best_authority_accepts_empty_selection_and_rejects_rerun_checkbox_and_scope_drift() {
    use super::{SaveProducer, producer_targets};
    let targets = vec![target(101, 11), target(202, 22)];
    let mut auth = authority(3, 7, vec![11, 22], &targets);
    auth.producer = SaveProducer::BestRun { seq: 9 };
    let current = producer_targets(&auth.producer, vec![], Some((9, targets.clone()))).unwrap();
    assert!(save_authority_matches(
        &auth,
        3,
        Some(7),
        Some(&[11, 22]),
        &current
    ));
    assert!(
        producer_targets(&auth.producer, targets.clone(), Some((10, targets.clone()))).is_none()
    );
    let unchecked = producer_targets(
        &auth.producer,
        targets.clone(),
        Some((9, vec![target(101, 11)])),
    )
    .unwrap();
    assert!(!save_authority_matches(
        &auth,
        3,
        Some(7),
        Some(&[11, 22]),
        &unchecked
    ));
    assert!(!save_authority_matches(
        &auth,
        3,
        Some(8),
        Some(&[11]),
        &current
    ));
    assert!(!save_authority_matches(
        &auth,
        4,
        Some(7),
        Some(&[11, 22]),
        &current
    ));
}

/// Build the Auto authority for an ordered target set and concrete workspace.
fn authority(
    dialog_seq: u64,
    generation: u64,
    workspace_cores: Vec<u64>,
    targets: &[SaveTarget],
) -> SaveAuthority {
    SaveAuthority {
        producer: super::SaveProducer::Selection,
        dialog_seq,
        workspace_generation: Some(generation),
        workspace_cores: Some(workspace_cores),
        targets: targets.iter().map(|t| (t.sid, t.core)).collect(),
    }
}

/// `save.rs:save_authority_matches` must compare the complete target list and workspace scope;
/// replacing equality with surviving-target filtering would turn an A+B Save into a B-only write.
#[test]
fn a_multi_core_save_is_refused_whole_when_one_target_becomes_hidden() {
    let captured = vec![target(101, 11), target(202, 22)];
    let auth = authority(3, 7, vec![11, 22], &captured);
    let surviving = vec![target(202, 22)];

    assert!(!save_authority_matches(
        &auth,
        3,
        Some(7),
        Some(&[22]),
        &surviving
    ));
    assert!(save_authority_matches(
        &auth,
        3,
        Some(7),
        Some(&[22, 11]),
        &captured
    ));
}

/// `save.rs:save_authority_matches` must retain the generation check even when scope and targets
/// look identical again; removing it resurrects an old confirmation after A -> B -> A.
#[test]
fn returning_to_the_same_scope_does_not_resurrect_an_old_save() {
    let targets = vec![target(101, 11)];
    let auth = authority(3, 7, vec![11], &targets);

    assert!(!save_authority_matches(
        &auth,
        3,
        Some(9),
        Some(&[11]),
        &targets
    ));
}

/// `save.rs:resolve_complete_target_cores` must collect with `Option`; changing a missing concrete
/// target to `continue` would produce and dispatch the surviving B plan.
#[test]
fn live_target_drift_returns_no_plan_instead_of_a_partial_plan() {
    let targets = vec![target(101, 11), target(202, 22)];
    let live = HashMap::from([(101, Vec::new()), (202, vec![22])]);

    assert_eq!(resolve_complete_target_cores(&targets, &live), None);
}

/// `save.rs:resolve_complete_target_cores` must preserve `None`-core fan-out in Classic; forcing
/// concrete authority there would stop the existing legacy all-live-copies Save behavior.
#[test]
fn classic_aggregate_target_remains_intentionally_unscoped() {
    let targets = vec![SaveTarget {
        sid: 101,
        core: None,
        name: "legacy".to_string(),
    }];
    let live = HashMap::from([(101, vec![11, 22])]);

    assert_eq!(
        resolve_complete_target_cores(&targets, &live),
        Some(vec![vec![11, 22]])
    );
}

/// `save.rs:resolve_complete_target_cores` must reject an empty legacy target before grouping;
/// returning an empty vector would still let another target in the same Classic batch dispatch.
#[test]
fn missing_classic_aggregate_target_also_refuses_the_complete_batch() {
    let targets = vec![
        SaveTarget {
            sid: 101,
            core: None,
            name: "missing".to_string(),
        },
        target(202, 22),
    ];
    let live = HashMap::from([(101, Vec::new()), (202, vec![22])]);

    assert_eq!(resolve_complete_target_cores(&targets, &live), None);
}

/// Removing either inline guard would allow stale authority or an edited V1 to be sent.
#[test]
fn ticks_inline_write_refuses_stale_authority_and_changed_v1_without_clearing_the_draft() {
    let targets = vec![target(303, 33)];
    let auth = authority(4, 8, vec![33], &targets);
    let confirm = vec![("StopLoss".to_string(), "1.7".to_string())];
    let mut draft = crate::analytics::tuner::ticks::state::TicksState::default();
    draft.set_variant("StopLoss", "1.7".into());
    let current = draft.variant_changes();
    let stale = save_authority_matches(&auth, 4, Some(10), Some(&[33]), &targets);
    assert_eq!(
        inline_write_decision(&confirm, &current, stale),
        InlineWriteDecision::Drop
    );
    assert_eq!(draft.variant_changes(), confirm);
    let valid = save_authority_matches(&auth, 4, Some(8), Some(&[33]), &targets);
    assert_eq!(
        inline_write_decision(&confirm, &current, valid),
        InlineWriteDecision::Send
    );
    draft.set_variant("StopLoss", "2.6".into());
    assert_eq!(
        inline_write_decision(&confirm, &draft.variant_changes(), valid),
        InlineWriteDecision::Drop
    );
    assert_eq!(
        draft.variant_changes(),
        vec![("StopLoss".to_string(), "2.6".to_string())]
    );
}
