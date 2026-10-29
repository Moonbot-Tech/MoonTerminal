//! Bounded owner work publication with a causal wake after successful admission.

use super::Work;
use std::sync::{Arc, Mutex, mpsc};

/// Host-provided wake; called outside the channel and callback locks after work is queued.
pub type WorkWaker = Arc<dyn Fn() + Send + Sync>;

/// Shared callback retained by the service and each work producer.
pub(super) type SharedWaker = Arc<Mutex<Option<WorkWaker>>>;

/// Producer half of the bounded application bridge.
#[derive(Clone)]
pub(super) struct WorkSender {
    pub(super) tx: mpsc::SyncSender<Work>,
    pub(super) waker: SharedWaker,
}

impl WorkSender {
    /// Return whether work was admitted, waking only after it becomes visible to the owner.
    pub(super) fn try_send(&self, work: Work) -> bool {
        if self.tx.try_send(work).is_err() {
            return false;
        }
        wake(&self.waker);
        true
    }
}

/// Clone under the lock, then invoke outside it so callbacks may safely replace themselves.
pub(super) fn wake(shared: &SharedWaker) {
    let callback = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(callback) = callback {
        callback();
    }
}

#[cfg(test)]
mod tests;
