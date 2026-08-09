//! Regression tests for native-window taskbar suppression lifetime.

use std::sync::{Arc, atomic::AtomicBool};

use super::TaskbarHideTask;

/// `windowing.rs:TaskbarHideTask::drop` must cancel the exact background burst; removing the Drop
/// call makes this assertion fail and lets a released/replaced window keep issuing COM calls.
#[test]
fn dropping_taskbar_authority_cancels_its_worker() {
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let _task = TaskbarHideTask {
            cancelled: cancelled.clone(),
        };
    }
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
}

/// `windowing.rs:TaskbarHideTask::cancel` must be idempotent; replacing an activation burst calls
/// cancel before Drop and a non-idempotent transition could panic while the native window is live.
#[test]
fn taskbar_authority_can_be_cancelled_more_than_once() {
    let task = TaskbarHideTask {
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    task.cancel();
    task.cancel();
    assert!(task.is_cancelled());
}
