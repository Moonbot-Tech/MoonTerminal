//! Process-wide bound on concurrent report readers.
//!
//! Eight slots cover the named simultaneous readers — roughly one Analytics
//! compound read per visible tab, one Report, one valuation worker, one Telegram,
//! one foreground chart, one purge — and turn the unbounded page-cache peak into
//! a hard ceiling of 8 × `READER_CACHE_KIB`. That list is not spare capacity.
//! `crates/moon-ui-gpui/src/panels/chart/report_trades.rs` documents that a chart
//! stack "can hold dozens" of tiles, each refreshing on its own background
//! interval; that interval throttles each tile but does not phase-stagger them,
//! so one report-generation commit can fire many `open_reader_with` calls at
//! once and this budget WILL saturate. That is intended: those readers queue up
//! to [`READER_WAIT`] and, past that, fail retryably. Bounding them is the
//! point, since unbounded they are the descriptor exhaustion this module exists
//! to stop. A chart tile now costs 6 descriptors rather than 9 because it
//! attaches strategies only. Callers keep their own `Connection`; only the
//! count is shared.
//!
//! [`acquire_with_cancellation`] is a genuine OS-level blocking wait on a
//! thread of the app's shared background executor. Cost differs by platform
//! (MoonUI `e4f2d4e9`): macOS uses a GCD global concurrent queue
//! (`moon-gpui-macos/src/dispatcher.rs:50,64`) that grows when threads block;
//! Windows uses the system `ThreadPool` (`moon-gpui-windows/src/dispatcher.rs:62`),
//! which also grows; Linux uses a fixed pool sized `available_parallelism()`
//! (`moon-gpui-linux/src/linux/dispatcher.rs:33-36`), so parked waiters can
//! delay unrelated background work, bounded by [`READER_WAIT`].
//!
//! Every mutex acquisition recovers a poisoned lock: a panic while the slots
//! mutex is held must not disable every report reader for the rest of the
//! process lifetime.

use std::ops::Deref;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use rusqlite::Connection;

/// Maximum simultaneous report readers in this process.
pub(super) const READER_BUDGET: usize = 8;

/// How long a ninth reader waits for a slot before failing retryable.
///
/// Kept at 3 s deliberately. This wait happens first, in `open_reader_with`,
/// and `tune_reader` then applies `busy_timeout` (also 3 s) on the newly opened
/// connection, so the two are sequential: worst case for one `open_reader()` is
/// up to 6 s when the budget is full AND the writer holds a lock. Nothing
/// should rely on a 3 s overall ceiling.
pub(super) const READER_WAIT: Duration = Duration::from_secs(3);

struct ReaderBudget {
    slots: Mutex<usize>,
    freed: Condvar,
}

/// Zero-sized permit: dropping it returns one slot to [`BUDGET`].
pub(super) struct ReaderPermit;

static BUDGET: ReaderBudget = ReaderBudget {
    slots: Mutex::new(READER_BUDGET),
    freed: Condvar::new(),
};

fn lock_slots() -> std::sync::MutexGuard<'static, usize> {
    BUDGET
        .slots
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Result of waiting for a reader slot.
pub(super) enum AcquireOutcome {
    /// A slot was taken; drop the permit to release it.
    Permit(ReaderPermit),
    /// The wait elapsed with no slot.
    Timeout,
    /// The request was cancelled before a slot was taken.
    Cancelled,
}

/// Take one slot, waiting up to `wait` if none are free.
///
/// Held for the sibling test the prover adds; production waits go through
/// [`acquire_with_cancellation`].
///
/// Args:
///     wait: Maximum time to block while every slot is held.
///
/// Returns:
///     A permit when a slot was taken, or `None` on timeout.
#[allow(dead_code)]
pub(super) fn acquire(wait: Duration) -> Option<ReaderPermit> {
    match acquire_with_cancellation(wait, &|| false) {
        AcquireOutcome::Permit(permit) => Some(permit),
        AcquireOutcome::Timeout | AcquireOutcome::Cancelled => None,
    }
}

/// Take one slot, giving up when `cancelled` becomes true rather than waiting
/// out the full timeout or taking a permit a superseded read no longer needs.
///
/// `cancelled` is a probe so this module does not depend on `read_cancel`.
/// The condvar predicate re-checks it on every wake, including spurious ones.
///
/// Args:
///     wait: Maximum time to block while every slot is held.
///     cancelled: Probe that is true when the current request was superseded.
///
/// Returns:
///     A permit, a timeout, or cancellation — never a permit for a cancelled
///     request, and never `Exhausted` for a cancelled one.
pub(super) fn acquire_with_cancellation(
    wait: Duration,
    cancelled: &dyn Fn() -> bool,
) -> AcquireOutcome {
    if cancelled() {
        return AcquireOutcome::Cancelled;
    }
    let mut slots = lock_slots();
    if *slots > 0 {
        *slots -= 1;
        return AcquireOutcome::Permit(ReaderPermit);
    }
    let (mut slots, _timed_out) = BUDGET
        .freed
        .wait_timeout_while(slots, wait, |slots| *slots == 0 && !cancelled())
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cancelled() {
        // A permit-drop notifies exactly one waiter. If that waiter is this
        // cancelled request, the freed slot would otherwise sit unclaimed while
        // other waiters stay parked: pass the notification on.
        if *slots > 0 {
            BUDGET.freed.notify_one();
        }
        return AcquireOutcome::Cancelled;
    }
    if *slots > 0 {
        *slots -= 1;
        return AcquireOutcome::Permit(ReaderPermit);
    }
    AcquireOutcome::Timeout
}

/// Slots still free. Observable for the sibling test the prover adds.
#[allow(dead_code)]
pub(super) fn available() -> usize {
    *lock_slots()
}

impl Drop for ReaderPermit {
    fn drop(&mut self) {
        let mut slots = lock_slots();
        *slots += 1;
        BUDGET.freed.notify_one();
    }
}

/// Report reader whose descriptors are accounted for by a budget permit.
///
/// `conn` is declared before `_permit` so the connection (and its descriptors)
/// drops before the permit that accounted for them. Reordering the fields
/// silently breaks that accounting.
pub struct ReportReader {
    pub(super) conn: Connection,
    pub(super) _permit: ReaderPermit,
}

impl Deref for ReportReader {
    type Target = Connection;

    /// Borrow the inner report connection.
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests;
