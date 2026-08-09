//! Regression coverage for crash-recoverable Classic window ownership persistence.

use std::collections::HashMap;

use super::{WindowStateSnapshot, recover_pending, save_all_to_paths};
use crate::window::detached::DetachedSpec;

/// Build one isolated directory for a persistence regression.
fn temp_dir(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "moonterminal-window-state-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("temporary persistence directory must be created");
    path
}

/// `window_state_persist.rs:recover_pending` must replay both authorities from the journal;
/// removing recovery loses a panel when a process stops between the two public file replacements.
#[test]
fn pending_snapshot_recovers_both_window_authorities() {
    let dir = temp_dir("recover");
    let pending = dir.join("pending.json");
    let docks = dir.join("docks.json");
    let detached = dir.join("detached.json");
    let snapshot = WindowStateSnapshot {
        docks: HashMap::new(),
        detached: vec![DetachedSpec::new("alpha".into(), "orders".into())],
    };
    std::fs::write(
        &pending,
        serde_json::to_vec_pretty(&snapshot).expect("snapshot must serialize"),
    )
    .expect("pending snapshot must be staged");

    let recovered = recover_pending(&pending, &docks, &detached)
        .expect("valid pending snapshot must be recovered");
    assert_eq!(recovered.detached.len(), 1);
    assert!(!pending.exists());
    let persisted: Vec<DetachedSpec> = serde_json::from_slice(
        &std::fs::read(&detached).expect("detached authority must be replayed"),
    )
    .expect("replayed detached authority must remain valid JSON");
    assert_eq!(persisted[0].group, "alpha");
    assert_eq!(persisted[0].panel, "orders");
    std::fs::remove_dir_all(dir).expect("temporary persistence directory must be removed");
}

/// `window_state_persist.rs:save_all_to_paths` must retain its journal after either public write
/// fails; clearing it would make the successful half authoritative after a crash.
#[test]
fn failed_second_file_keeps_joint_snapshot_for_retry() {
    let dir = temp_dir("failure");
    let pending = dir.join("pending.json");
    let docks = dir.join("docks.json");
    let detached = dir.join("detached-blocker");
    std::fs::create_dir(&detached).expect("detached destination blocker must be created");
    let specs = vec![DetachedSpec::new("alpha".into(), "orders".into())];

    assert!(!save_all_to_paths(
        &HashMap::new(),
        &specs,
        &pending,
        &docks,
        &detached,
    ));
    assert!(pending.exists());
    assert!(docks.exists());
    std::fs::remove_dir_all(dir).expect("temporary persistence directory must be removed");
}
