//! Startup cleanup must hold the autoload barrier until its worker has finished.

use super::{FIRST_DELAY, Phase, clear_for_autoload, lock};
use std::time::Instant;

/// Restore scheduler state even when a barrier assertion unwinds.
struct RestorePhase(Phase);

impl Drop for RestorePhase {
    /// Return the process scheduler to the state the fixture borrowed.
    fn drop(&mut self) {
        *lock() = self.0;
    }
}

/// Removing the initial wait or opening the barrier while running would race retention with fetch.
#[test]
fn first_tick_waits_and_autoload_remains_blocked_until_done() {
    let _restore = RestorePhase(*lock());
    *lock() = Phase::Waiting(Instant::now() + FIRST_DELAY);
    assert!(matches!(*lock(), Phase::Waiting(due) if due <= Instant::now() + FIRST_DELAY));
    assert!(!clear_for_autoload());
    *lock() = Phase::Running;
    assert!(!clear_for_autoload());
    *lock() = Phase::Done;
    assert!(clear_for_autoload());
}
