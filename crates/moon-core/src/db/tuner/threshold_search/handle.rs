//! Cooperative control of a running threshold search: cancellation, progress, and whether the
//! run got through everything it planned.
//!
//! The search runs on a background thread while its window stays live, so the two need a channel
//! that neither blocks nor allocates per check. A few atomics behind one `Arc` are enough: the
//! caller raises the cancel flag and polls progress, the search polls the flag between units of
//! work and publishes what it got through.
//!
//! "Did this run finish?" is answered by [`SearchHandle::abandoned`] rather than by comparing the
//! completed count against a requested one. A run may be composed of MANY searches — candidate
//! sets, folds, a final refit — so a caller counting restarts against the number it asked for is
//! comparing against the wrong denominator the moment more than one search runs behind the same
//! handle. Whoever drops a unit of work says so; nobody has to infer it.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// Shared control and progress signals of one search run.
///
/// Cloning shares the same run: the caller keeps one clone to stop and observe the search, the
/// search holds another. Build a fresh handle per run — nothing here resets.
#[derive(Clone, Default)]
pub struct SearchHandle(Arc<Signals>);

/// The atomics behind a [`SearchHandle`].
#[derive(Default)]
struct Signals {
    cancelled: AtomicBool,
    completed: AtomicUsize,
    /// Set once any unit of work is dropped unfinished. See [`SearchHandle::abandoned`].
    abandoned: AtomicBool,
    /// Packed `(step, done, total)` of a multi-stage run. See [`SearchHandle::stage`].
    stage: AtomicU64,
}

/// Bits reserved for `done` and `total` in the packed stage word; `step` takes what is left.
///
/// A single 64-bit word rather than three counters so a reader cannot catch them mid-update and
/// render a `done` from one step against a `total` from the next.
const STAGE_FIELD_BITS: u32 = 24;

/// Largest value the packed `done`/`total` fields can hold.
const STAGE_FIELD_MAX: usize = (1 << STAGE_FIELD_BITS) - 1;

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

    /// Restarts finished so far, used as plain-search progress or composition's internal work
    /// counter.
    pub fn completed(&self) -> usize {
        self.0.completed.load(Ordering::Relaxed)
    }

    /// Record one finished restart. An abandoned restart never calls this.
    pub(super) fn record_restart(&self) {
        self.0.completed.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether any unit of work was dropped unfinished, i.e. the answer is less than was asked
    /// for.
    ///
    /// This is the honest "was it cut short", and it is deliberately NOT
    /// `is_cancelled()`: a stop that arrives after the last unit has finished abandons nothing,
    /// and reporting it as a stop would tell the user their search was truncated when it was
    /// complete. It is equally deliberately not `completed() < requested`, which stops being
    /// meaningful as soon as several searches run behind one handle.
    pub fn abandoned(&self) -> bool {
        self.0.abandoned.load(Ordering::Relaxed)
    }

    /// Mark that a unit of work was dropped unfinished. One-way, like cancellation.
    pub(super) fn note_abandoned(&self) {
        self.0.abandoned.store(true, Ordering::Relaxed);
    }

    /// Publish the stage of a multi-stage run: `step` is the outer iteration, `done` of `total`
    /// the progress within it.
    ///
    /// Values above what the packed word holds are clamped rather than wrapped: this drives a
    /// caption, and a clamped count merely stops rising while a wrapped one counts backwards.
    pub(super) fn set_stage(&self, step: usize, done: usize, total: usize) {
        let packed = ((step as u64) << (2 * STAGE_FIELD_BITS))
            | ((done.min(STAGE_FIELD_MAX) as u64) << STAGE_FIELD_BITS)
            | total.min(STAGE_FIELD_MAX) as u64;
        // Never zero once a stage has been published, so `stage()` can use zero for "no stage".
        self.0.stage.store(packed | 1 << 63, Ordering::Relaxed);
    }

    /// Withdraw the published stage: the run continues, but no longer has a step to report.
    ///
    /// The alternative — leaving the last stage standing — is worse than showing nothing. A
    /// caption frozen on "step 7, field 12 of 24" for the whole final refit does not read as
    /// stale; it reads as a search stuck on field 12.
    pub(super) fn clear_stage(&self) {
        self.0.stage.store(0, Ordering::Relaxed);
    }

    /// The stage of a multi-stage run as `(step, done, total)`, or `None` before the first one is
    /// published or after it is withdrawn — also the normal state of the plain single-stage
    /// search.
    pub fn stage(&self) -> Option<(usize, usize, usize)> {
        let packed = self.0.stage.load(Ordering::Relaxed);
        if packed == 0 {
            return None;
        }
        let field = |shift: u32| (packed >> shift) as usize & STAGE_FIELD_MAX;
        let step = (packed & !(1 << 63)) >> (2 * STAGE_FIELD_BITS);
        Some((step as usize, field(STAGE_FIELD_BITS), field(0)))
    }
}
