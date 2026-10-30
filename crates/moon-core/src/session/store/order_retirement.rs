//! Bounded off-thread destruction of consumed order batches, including their trace allocations.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};

use crate::feed::{FeedWakeTx, OrderRows};

/// Holds several ten-core publication waves while keeping pending ownership bounded.
const QUEUED_BATCHES: usize = 64;

/// One process-wide worker serves all cores without involving the tuner's compute pool.
static SENDER: OnceLock<SyncSender<RetiredBatch>> = OnceLock::new();

/// Notifies the feed drain after destruction, when the bounded queue has made progress.
struct RetiredBatch {
    rows: Option<OrderRows>,
    wake: Option<(FeedWakeTx, Arc<AtomicBool>)>,
}

impl Drop for RetiredBatch {
    /// Drops trace allocations before waking a drain that may have deferred its next batch.
    fn drop(&mut self) {
        drop(self.rows.take());
        if let Some((wake, waiting)) = self.wake.take() {
            if waiting.swap(false, Ordering::AcqRel) {
                wake.notify();
            }
        }
    }
}

/// One overflow slot per session manager. The drain stops consuming feed messages while full,
/// so a slow disposer cannot turn the deferred slot into an unbounded UI-owned backlog.
pub(in crate::session) struct DeferredRetirement {
    sender: SyncSender<RetiredBatch>,
    waiting: Arc<AtomicBool>,
    rows: Option<OrderRows>,
}

impl Default for DeferredRetirement {
    /// Shares the process worker; only the overflow ownership belongs to this manager.
    fn default() -> Self {
        Self {
            sender: sender().clone(),
            waiting: Arc::default(),
            rows: None,
        }
    }
}

impl DeferredRetirement {
    /// Retries the previous overflow batch before admitting another feed message.
    pub(in crate::session) fn retry(&mut self, wake: Option<FeedWakeTx>) -> bool {
        if let Some(rows) = self.rows.take() {
            self.rows = try_retire_to(&self.sender, rows, wake, &self.waiting);
        }
        self.rows.is_none()
    }

    /// Attempts admission and retains the sole overflow batch; false stops the drain immediately.
    pub(in crate::session) fn enqueue(
        &mut self,
        rows: OrderRows,
        wake: Option<FeedWakeTx>,
    ) -> bool {
        assert!(
            self.rows.is_none(),
            "retirement overflow slot already occupied"
        );
        self.rows = try_retire_to(&self.sender, rows, wake, &self.waiting);
        self.rows.is_none()
    }
}

impl Drop for DeferredRetirement {
    /// Even teardown of a saturated manager transfers its last batch off the caller thread.
    /// This one-shot handoff exists only at teardown, never once per saturated feed wake.
    fn drop(&mut self) {
        if let Some(rows) = self.rows.take() {
            let worker = thread::Builder::new()
                .name("order-retirement-tail".into())
                .spawn(move || retire(rows))
                .expect("could not hand off deferred order retirement");
            drop(worker);
        }
    }
}

/// Starts a FIFO disposal worker. Generic ownership lets tests observe destructor execution.
fn spawn<T: Send + 'static>() -> (SyncSender<T>, JoinHandle<()>) {
    let (sender, receiver) = sync_channel(QUEUED_BATCHES);
    let worker = thread::Builder::new()
        .name("order-retirement".into())
        .spawn(move || {
            for batch in receiver {
                drop(batch);
            }
        })
        .expect("could not start order retirement worker");
    (sender, worker)
}

/// Keeps the disposal worker alive for the process; queued rows own no external resources.
fn sender() -> &'static SyncSender<RetiredBatch> {
    SENDER.get_or_init(|| {
        let (sender, worker) = spawn();
        // The process-wide sender never closes during application work. Only heap allocations
        // remain at process exit, so shutdown does not need a storage or network flush here.
        drop(worker);
        sender
    })
}

/// Primes the worker during core construction, before the first timed feed drain.
pub(super) fn warm_up() {
    sender();
}

/// Synchronous admission for standalone core application; the UI drain uses `enqueue`.
pub(super) fn retire(batch: OrderRows) {
    if batch.rows.is_empty() && batch.captured_turns.is_empty() {
        return;
    }
    sender()
        .send(RetiredBatch {
            rows: Some(batch),
            wake: None,
        })
        .unwrap_or_else(|_| panic!("order retirement worker ended unexpectedly"));
}

/// Admits a batch without waiting. A full queue returns ownership to the drain's single slot;
/// an accepted batch wakes that drain after its allocations have been destroyed.
fn try_retire_to(
    sender: &SyncSender<RetiredBatch>,
    batch: OrderRows,
    wake: Option<FeedWakeTx>,
    waiting: &Arc<AtomicBool>,
) -> Option<OrderRows> {
    if batch.rows.is_empty() && batch.captured_turns.is_empty() {
        return None;
    }
    // Arm before admission: if the worker frees the queue concurrently with a Full result,
    // either that completion or a later one must wake the deferred drain. Successful admission
    // disarms it, so routine disposal does not create a second UI wake for every feed message.
    waiting.store(true, Ordering::Release);
    match sender.try_send(RetiredBatch {
        rows: Some(batch),
        wake: wake.map(|wake| (wake, Arc::clone(waiting))),
    }) {
        Ok(()) => {
            waiting.store(false, Ordering::Release);
            None
        }
        Err(TrySendError::Full(mut batch)) => {
            // Returning ownership is not disposal progress and must not create a busy wake loop.
            batch.wake = None;
            batch.rows.take()
        }
        Err(TrySendError::Disconnected(_)) => panic!("order retirement worker ended unexpectedly"),
    }
}

#[cfg(test)]
pub(in crate::session) mod tests;
