//! Daily schedule and strategy-readiness regression tests.

use super::*;
use std::sync::mpsc;
use std::sync::Arc;

/// Removing the exact-noon `>=` boundary would leave the new day's slots pending until tomorrow.
#[test]
fn exact_noon_belongs_to_the_new_daily_slot() {
    let noon = 20_000 * DAY_MS + NOON_MS;

    assert_eq!(due_slot_ms(noon - 1), noon - DAY_MS);
    assert_eq!(due_slot_ms(noon), noon);
}

/// Using a fixed 24-hour sleep after work would move every later run by the backup duration.
#[test]
fn next_noon_delay_uses_the_completion_clock() {
    let noon = 20_000 * DAY_MS + NOON_MS;
    let completed = noon + 5 * 60 * 1_000;

    assert_eq!(
        delay_to_next_noon(completed),
        Duration::from_millis((DAY_MS - 5 * 60 * 1_000) as u64)
    );
}

/// Omitting the completion wake would lose noon when an already-running job crosses the boundary.
#[test]
fn a_job_crossing_noon_requests_the_new_daily_slot() {
    let noon = 20_000 * DAY_MS + NOON_MS;

    assert!(daily_slot_advanced(noon - 1, noon));
    assert!(!daily_slot_advanced(noon, noon + 60_000));
}

/// Treating schema initialization or one core commit as readiness would reproduce the observed
/// zero-row/torn multi-core snapshot on a fresh database.
#[test]
fn a_fresh_database_waits_for_every_expected_core() {
    let coordinator = ScheduleCoordinator::new();
    assert!(coordinator.request(HashSet::from([10, 20]), false));
    assert_eq!(coordinator.strategy_claim(), None);

    coordinator.strategy_commit(10);
    assert_eq!(coordinator.strategy_claim(), None);
    coordinator.strategy_commit(99);
    assert_eq!(coordinator.strategy_claim(), None);
    coordinator.strategy_commit(20);
    assert_eq!(coordinator.strategy_claim(), Some(1));
}

/// Waiting for live cores despite inherited rows would withhold a useful catch-up whenever one
/// configured MoonBot is offline during startup.
#[test]
fn inherited_strategy_rows_are_ready_without_live_commits() {
    let coordinator = ScheduleCoordinator::new();

    assert!(coordinator.request(HashSet::from([10, 20]), true));
    assert_eq!(coordinator.strategy_claim(), Some(1));
}

/// Ignoring topology reconciliation would wait forever for a disabled core or publish before an
/// added core contributes its complete set.
#[test]
fn pending_readiness_tracks_runtime_topology() {
    let coordinator = ScheduleCoordinator::new();
    coordinator.request(HashSet::from([10, 20]), false);
    coordinator.strategy_commit(10);

    coordinator.update_expected(HashSet::from([10]));
    let ready_generation = coordinator.strategy_claim().unwrap();
    coordinator.update_expected(HashSet::from([10, 30]));

    assert!(coordinator
        .with_current_topology(ready_generation, || ())
        .is_none());
    assert_eq!(coordinator.strategy_claim(), None);
    coordinator.strategy_commit(30);
    assert!(coordinator.strategy_claim().is_some());
}

/// Releasing the coordinator lock between generation validation and publication would let a new
/// expected core appear while the old canonical slot is being committed.
#[test]
fn topology_updates_wait_for_the_final_publication_claim() {
    let coordinator = Arc::new(ScheduleCoordinator::new());
    coordinator.request(HashSet::from([10]), true);
    let generation = coordinator.strategy_claim().unwrap();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let publisher = coordinator.clone();
    let publish = std::thread::spawn(move || {
        publisher.with_current_topology(generation, || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
    });
    entered_rx.recv().unwrap();
    let (updated_tx, updated_rx) = mpsc::sync_channel(1);
    let updater = coordinator.clone();
    let update = std::thread::spawn(move || {
        updater.update_expected(HashSet::from([10, 20]));
        updated_tx.send(()).unwrap();
    });

    assert!(updated_rx.recv_timeout(Duration::from_millis(50)).is_err());
    release_tx.send(()).unwrap();
    assert!(publish.join().unwrap().is_some());
    updated_rx.recv().unwrap();
    update.join().unwrap();
    assert_ne!(coordinator.strategy_claim(), Some(generation));
}

/// An empty expected set must not publish an empty strategy database merely because its schema
/// exists; settings use an independent job and do not depend on this predicate.
#[test]
fn no_expected_cores_keeps_an_empty_strategy_source_pending() {
    let coordinator = ScheduleCoordinator::new();

    coordinator.request(HashSet::new(), false);

    assert_eq!(coordinator.strategy_claim(), None);
}
