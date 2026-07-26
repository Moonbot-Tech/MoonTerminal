//! Cooperative control of a running threshold search: cancellation and progress.
//!
//! The search runs on a background thread while its window stays live, so the two need a channel
//! that neither blocks nor allocates per check. Two atomics behind one `Arc` are enough: the
//! caller raises the cancel flag and polls the completed count, the search polls the flag between
//! units of work and publishes its count as it goes.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// Shared cancellation flag and completed-restart counter of one search run.
///
/// Cloning shares the same run: the caller keeps one clone to stop and observe the search, the
/// search holds another. Build a fresh handle per run — the counter never resets.
#[derive(Clone, Default)]
pub struct SearchHandle(Arc<Signals>);

/// The two atomics behind a [`SearchHandle`].
#[derive(Default)]
struct Signals {
    cancelled: AtomicBool,
    completed: AtomicUsize,
}

impl SearchHandle {
    /// Build a handle for one search run.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the search to stop at its next check point.
    ///
    /// One-way: a cancelled handle stays cancelled. Restarts already abandoned cannot be revived,
    /// so a handle that could be un-cancelled would report a count nothing is still producing.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether a stop was requested.
    ///
    /// Relaxed ordering is deliberate: the flag carries no data of its own, and the search's
    /// results travel back through the thread pool's own synchronization.
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }

    /// Whether both handles control the SAME run.
    ///
    /// A caller that keeps one handle per run needs this to tell its own search from the one that
    /// replaced it: two runs are never equal by their counters, which can coincide by chance.
    pub fn same_run(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Restarts finished so far, for live progress and the final completed-restart count.
    pub fn completed(&self) -> usize {
        self.0.completed.load(Ordering::Relaxed)
    }

    /// Record one finished restart. An abandoned restart never calls this.
    pub(super) fn record_restart(&self) {
        self.0.completed.fetch_add(1, Ordering::Relaxed);
    }
}
