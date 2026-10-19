//! Atomic identity tests for delayed Analytics Tuner writes.

use std::collections::HashMap;

use super::{
    InlineWriteDecision, inline_write_decision, resolve_complete_target_cores,
    save_authority_matches,
};
use crate::analytics::tuner::shared::{SaveAuthority, SaveDialog, SaveTarget};

/// Restoring shared-only rendering hides edits on a mixed save while confirmation sends them.
#[test]
fn seam_mixed_save_preview_lists_every_targets_exact_rows() {
    let targets = vec![target(101, 11), target(202, 22)];
    let dlg = SaveDialog {
        authority: authority(3, 7, vec![11, 22], &targets),
        targets,
        changes: Vec::new(),
        olds: Vec::new(),
        per_target: Some(vec![
            vec![
                ("DropsPriceDelta".into(), "3".into()),
                ("MinVolume".into(), "20".into()),
            ],
            vec![("MinVolume".into(), "20".into())],
        ]),
        per_target_olds: vec![vec![Some("2".into()), None], vec![Some("10".into())]],
        notes: Vec::new(),
        warns: Vec::new(),
        copy: false,
    };
    let groups = super::save_preview_groups(&dlg).expect("aligned payload");
    let rows: Vec<_> = groups
        .iter()
        .flat_map(|group| {
            group
                .changes
                .iter()
                .enumerate()
                .map(move |(i, (key, value))| {
                    (
                        group.name,
                        key.as_str(),
                        group.olds.get(i).and_then(Option::as_deref),
                        value.as_str(),
                    )
                })
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (Some("strategy-101"), "DropsPriceDelta", Some("2"), "3"),
            (Some("strategy-101"), "MinVolume", None, "20"),
            (Some("strategy-202"), "MinVolume", Some("10"), "20"),
        ]
    );
    let mut malformed = dlg;
    malformed
        .per_target
        .as_mut()
        .expect("individual edits")
        .pop();
    assert!(super::save_preview_groups(&malformed).is_none());
}

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
