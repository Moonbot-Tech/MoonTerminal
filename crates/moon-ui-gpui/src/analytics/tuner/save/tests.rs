//! Atomic identity tests for delayed Analytics Tuner writes.

use std::collections::HashMap;

use super::{resolve_complete_target_cores, save_authority_matches};
use crate::analytics::tuner::shared::{SaveAuthority, SaveTarget};

/// Build one concrete Save target without involving Analytics UI state.
fn target(sid: i64, core: u64) -> SaveTarget {
    SaveTarget {
        sid,
        core: Some(core),
        name: format!("strategy-{sid}"),
    }
}

/// Build the Auto authority for an ordered target set and concrete workspace.
fn authority(
    dialog_seq: u64,
    generation: u64,
    workspace_cores: Vec<u64>,
    targets: &[SaveTarget],
) -> SaveAuthority {
    SaveAuthority {
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
