//! Modal feedback distinguishes replaceable reads from blocking writes and searches.

use std::time::Duration;

use super::blocking_overlay_due;
use crate::analytics::diag::BUSY_OVERLAY_DELAY;

/// A cold page or several cancelled reads cannot prevent navigation after the feedback delay.
#[test]
fn replaceable_base_reads_never_occlude_navigation() {
    for reads in 1..=3 {
        assert!(!blocking_overlay_due(
            reads,
            reads,
            Some(Duration::from_secs(10)),
        ));
    }
}

/// Writes and explicit searches retain the existing delay and need a started batch.
#[test]
fn blocking_operations_preserve_the_delayed_modal_feedback() {
    assert!(!blocking_overlay_due(1, 0, None));
    assert!(!blocking_overlay_due(
        1,
        0,
        Some(BUSY_OVERLAY_DELAY - Duration::from_nanos(1)),
    ));
    assert!(blocking_overlay_due(1, 0, Some(BUSY_OVERLAY_DELAY)));
    assert!(!blocking_overlay_due(0, 0, Some(BUSY_OVERLAY_DELAY)));
}

/// Draining either cancelled read leaves an overlapping write modal until that write finishes.
#[test]
fn replaceable_read_completions_cannot_unblock_an_ongoing_write() {
    let elapsed = Some(Duration::from_secs(10));
    for (busy_ops, latest_overlay_ops) in [(3, 2), (2, 1), (1, 0)] {
        assert!(blocking_overlay_due(busy_ops, latest_overlay_ops, elapsed));
    }
    assert!(!blocking_overlay_due(0, 0, elapsed));
    // The inverse completion order also leaves the remaining reads nonmodal.
    assert!(!blocking_overlay_due(2, 2, elapsed));
}
