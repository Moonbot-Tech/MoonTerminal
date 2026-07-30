//! Report-writer coordination tests.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use super::consume_report_commit;

/// `startup.rs:consume_report_commit` must invoke the dedicated report-revision notification for
/// a set edge; removing that call leaves an open Analytics window stale until an unrelated UI wake.
#[test]
fn committed_report_edge_notifies_once() {
    let dirty = AtomicBool::new(true);
    let notifications = Cell::new(0);

    consume_report_commit(Some(&dirty), || {
        notifications.set(notifications.get() + 1);
    });
    consume_report_commit(Some(&dirty), || {
        notifications.set(notifications.get() + 1);
    });

    assert_eq!(notifications.get(), 1);
    assert!(!dirty.load(Ordering::Acquire));
}

/// Read the startup source governed by the ordering contract.
///
/// Returns:
///     UTF-8 source text from the sibling `startup.rs`.
fn startup_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("startup.rs");
    std::fs::read_to_string(path).unwrap()
}

/// Return the byte position of a required source anchor.
///
/// Args:
///     source: Complete startup source.
///     anchor: Exact architectural call-site anchor.
///
/// Returns:
///     First matching byte position.
fn position(source: &str, anchor: &str) -> usize {
    source
        .find(anchor)
        .unwrap_or_else(|| panic!("missing startup contract anchor: {anchor}"))
}

/// `startup.rs:run` must read the reports uid floor before recovery and recover before the writer.
///
/// Moving `report_recovery::prepare()` above `observed_uid_floor` loses deleted-core uid history
/// when the damaged replica is replaced. Moving it below `app.run` or bypassing the private permit
/// starts a writer before the damaged main/WAL/SHM set has been safely preserved.
#[test]
fn report_recovery_stays_between_uid_floor_and_writer_start() {
    let source = startup_source();
    let uid_floor = position(&source, "let uid_floor = observed_uid_floor");
    let recovery = position(
        &source,
        "let report_write_permit = moon_core::db::report_recovery::prepare()",
    );
    let app_run = position(&source, "app.run(move |cx|");
    let writer = position(
        &source,
        "report_write_permit.and_then(moon_core::db::spawn_writer)",
    );

    assert!(uid_floor < recovery);
    assert!(recovery < app_run);
    assert!(app_run < writer);
}
