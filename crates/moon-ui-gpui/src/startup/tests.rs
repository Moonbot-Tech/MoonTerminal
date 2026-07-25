//! Report-writer coordination tests.

use std::cell::Cell;
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
