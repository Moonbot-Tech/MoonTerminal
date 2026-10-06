use super::*;

/// Registered valuation thread used to interrupt a long park when corruption is detected.
static WORKER_THREAD: OnceLock<Mutex<Option<std::thread::Thread>>> = OnceLock::new();

/// Wake the active valuation worker so cache recovery starts without waiting for its timer.
pub(in crate::db::valuation) fn wake_for_recovery() {
    let slot = WORKER_THREAD.get_or_init(|| Mutex::new(None));
    let worker = match slot.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    if let Some(worker) = worker {
        worker.unpark();
    }
}

/// Publish the active worker thread for corruption-triggered recovery wakes.
///
/// Args:
///     worker: Newly spawned valuation worker thread.
pub(in crate::db::valuation::worker) fn register_worker(worker: std::thread::Thread) {
    let slot = WORKER_THREAD.get_or_init(|| Mutex::new(None));
    match slot.lock() {
        Ok(mut guard) => *guard = Some(worker),
        Err(poisoned) => *poisoned.into_inner() = Some(worker),
    }
}

/// Background valuation publication and wake handle.
pub struct ValuationHandle {
    /// Monotonic generation incremented after prepared values, coverage state, or current rates
    /// are published.
    pub generation: Arc<AtomicU64>,
    /// Coalescing UI wake edge set beside every data-generation publication.
    pub commit_dirty: Arc<AtomicBool>,
    /// Monotonic counter bumped only when published worker health changes shape.
    ///
    /// Separate from `generation` because a stalled worker commits no data: a surface polling the
    /// data generation alone can never learn that nothing is coming.
    pub status_revision: Arc<AtomicU64>,
    /// Coalescing UI wake edge set beside every `status_revision` bump.
    pub status_dirty: Arc<AtomicBool>,
    /// Latest health snapshot published before the corresponding revision bump.
    pub(in crate::db::valuation::worker) status: Arc<RwLock<ValuationStatus>>,
    /// Whether the application-wide current-rate valuation mode is enabled.
    ///
    /// While this is false the worker issues ZERO provider requests for it. The mode is one
    /// application-wide setting, not a per-window one, so a single flag mirrors it exactly and no
    /// view-lifetime reference count is needed.
    pub(in crate::db::valuation::worker) current_wanted: Arc<AtomicBool>,
    pub(in crate::db::valuation::worker) thread: std::thread::Thread,
}

impl ValuationHandle {
    /// Wake the worker after a committed report outbox change.
    pub fn wake(&self) {
        self.thread.unpark();
    }

    /// Declare whether the application-wide mode needs current-rate valuation.
    ///
    /// Wakes the worker on EITHER edge, because it may be parked until its next scheduled turn —
    /// or, while it has nothing else to do, until a stall deadline far beyond it. Rising, the first
    /// snapshot should not wait that long. Falling, the stage may be sitting in a provider backoff
    /// of up to five minutes, and until it runs again it keeps a failure published for a feature
    /// the user has just switched off. A restored persisted mode must call this at startup for the
    /// same reason.
    ///
    /// Args:
    ///     wanted: True while the application-wide mode is current-rate valuation.
    pub fn set_current_wanted(&self, wanted: bool) {
        if self.current_wanted.swap(wanted, Ordering::Release) != wanted {
            self.wake();
        }
    }

    /// Take the first health snapshot together with the revision it belongs to.
    ///
    /// The two reads are ordered revision-then-snapshot, mirroring the worker's
    /// snapshot-then-revision publish order. Reversed, a publication landing between them would
    /// pair the OLD health with the NEW revision, and every later poll would see matching counters
    /// and keep serving the stale value. That ordering is why callers seed through this method
    /// instead of reading the two fields themselves.
    ///
    /// Returns:
    ///     Revision to poll against, and the health published at or before it.
    pub fn seed_status(&self) -> (u64, ValuationStatus) {
        let revision = self.status_revision.load(Ordering::Relaxed);
        (revision, self.read_status())
    }

    /// Take a fresh snapshot only when the published revision moved.
    ///
    /// Reading the snapshot takes a lock, so callers poll the counter — the same
    /// poll-a-revision-counter contract every panel in this application follows — and pay for the
    /// lock only on a real transition, never per frame.
    ///
    /// Args:
    ///     last: Revision this caller has already applied; updated in place when it moves.
    ///
    /// Returns:
    ///     New health, or `None` while nothing changed.
    pub fn status_if_changed(&self, last: &mut u64) -> Option<ValuationStatus> {
        let revision = self.status_revision.load(Ordering::Relaxed);
        if revision == *last {
            return None;
        }
        *last = revision;
        Some(self.read_status())
    }

    /// Clone the published health.
    ///
    /// Returns:
    ///     Current health, recovered from the lock even if another holder panicked.
    fn read_status(&self) -> ValuationStatus {
        match self.status.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

/// Channel the worker publishes health through.
pub(in crate::db::valuation::worker) struct StatusSink {
    /// Latest published health.
    pub(in crate::db::valuation::worker) status: Arc<RwLock<ValuationStatus>>,
    /// Monotonic counter observed by UI polls.
    pub(in crate::db::valuation::worker) revision: Arc<AtomicU64>,
    /// Coalescing UI wake edge.
    pub(in crate::db::valuation::worker) dirty: Arc<AtomicBool>,
    /// Signature of the last publication, so unchanged turns publish nothing.
    pub(in crate::db::valuation::worker) published: u64,
}

impl StatusSink {
    /// Publish health when the facts the UI renders changed.
    ///
    /// Args:
    ///     status: Current worker health.
    ///     now_ms: Current wall-clock time in Unix milliseconds.
    pub(in crate::db::valuation::worker) fn publish(
        &mut self,
        status: &ValuationStatus,
        now_ms: i64,
    ) {
        let signature = status.signature(now_ms);
        if signature == self.published {
            return;
        }
        self.published = signature;
        match self.status.write() {
            Ok(mut guard) => *guard = status.clone(),
            Err(poisoned) => *poisoned.into_inner() = status.clone(),
        }
        self.revision.fetch_add(1, Ordering::AcqRel);
        self.dirty.store(true, Ordering::Release);
    }
}
