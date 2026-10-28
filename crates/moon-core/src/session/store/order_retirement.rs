//! Bounded off-thread destruction of consumed order batches, including their trace allocations.

use std::sync::OnceLock;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::{self, JoinHandle};

use crate::feed::OrderRows;

/// Holds several ten-core publication waves while keeping pending ownership bounded.
const QUEUED_BATCHES: usize = 64;

/// One process-wide worker serves all cores without involving the tuner's compute pool.
static SENDER: OnceLock<SyncSender<OrderRows>> = OnceLock::new();

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
fn sender() -> &'static SyncSender<OrderRows> {
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

/// Transfers obsolete rows to the worker without cloning their strings or trace vectors.
pub(super) fn retire(batch: OrderRows) {
    if batch.rows.is_empty() && batch.captured_turns.is_empty() {
        return;
    }
    sender()
        .send(batch)
        .unwrap_or_else(|_| panic!("order retirement worker ended unexpectedly"));
}

#[cfg(test)]
mod tests;
