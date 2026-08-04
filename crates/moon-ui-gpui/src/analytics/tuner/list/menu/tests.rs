//! Gate decisions for the strategy-row purge. Explicit imports: the parent re-exports `gpui::*`,
//! whose own `test` would shadow the built-in attribute.

use super::{PurgeGate, purge_gate};

/// Every core replicates and every strategy is live — the "nothing is in the way" baseline.
fn open() -> (impl Fn(u64) -> bool, impl Fn(u64, u64) -> bool) {
    (|_| true, |_, _| true)
}

/// Tightening the gate beyond its documented live-state checks would hide a valid purge action.
#[test]
fn a_live_strategy_is_allowed() {
    let (replicates, live) = open();

    assert_eq!(
        purge_gate("7@3", Some(2), replicates, live),
        PurgeGate::Allowed {
            core_uid: 3,
            sid: 7
        }
    );
}

/// Relaxing this would send `DeleteStrategy { id: 0 }`, which the feed turns into the FOLDER
/// delete form with an empty path — a different command with a different blast radius.
#[test]
fn manual_orders_are_refused() {
    let (replicates, live) = open();

    assert_eq!(
        purge_gate("0@3", Some(2), replicates, live),
        PurgeGate::Manual
    );
}

/// A core with report replication off never echoes the soft-delete, so the first step could never
/// confirm and the sequence would sit waiting until it timed out.
#[test]
fn a_core_that_does_not_replicate_its_report_is_refused() {
    assert_eq!(
        purge_gate("7@3", Some(2), |_| false, |_, _| true),
        PurgeGate::NoReportFeed
    );
}

/// `CoreStore` keeps the pre-outage strategy snapshot, so a gate that only asked "does the store
/// know it" would stay enabled on a disconnected core.
#[test]
fn a_core_that_is_not_live_is_refused() {
    assert_eq!(
        purge_gate("7@3", Some(2), |_| true, |_, _| false),
        PurgeGate::Offline
    );
}

/// Treating the row's deleted marker as live would offer an action for a strategy already gone.
#[test]
fn an_already_deleted_strategy_is_refused() {
    let (replicates, live) = open();

    assert_eq!(
        purge_gate("7@3", Some(0), replicates, live),
        PurgeGate::AlreadyDeleted
    );
}

/// No strategy database attached says nothing about the core — liveness alone decides.
#[test]
fn an_unknown_liveness_marker_does_not_refuse_by_itself() {
    let (replicates, live) = open();

    assert!(matches!(
        purge_gate("7@3", None, replicates, live),
        PurgeGate::Allowed { .. }
    ));
}

/// A soft-delete is addressed per core; a legacy key carrying none cannot name a target.
#[test]
fn a_key_without_a_core_is_refused() {
    let (replicates, live) = open();

    assert_eq!(
        purge_gate("7", Some(2), replicates, live),
        PurgeGate::Offline
    );
}

/// Each refusal must carry its own wording, or the greyed item would explain the wrong thing.
#[test]
fn every_refusal_names_its_own_reason() {
    let reasons = [
        PurgeGate::Manual.reason_key(),
        PurgeGate::AlreadyDeleted.reason_key(),
        PurgeGate::NoReportFeed.reason_key(),
        PurgeGate::Offline.reason_key(),
    ];

    assert!(reasons.iter().all(|reason| reason.is_some()));
    let mut distinct = reasons.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), reasons.len(), "reasons must not be shared");
    assert!(
        PurgeGate::Allowed {
            core_uid: 1,
            sid: 1
        }
        .reason_key()
        .is_none()
    );
}
