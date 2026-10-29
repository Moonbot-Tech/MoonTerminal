//! Bounded-concurrency preparation and ownership transfer for retained volume folds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

/// Source generations: reconnects and catalog changes can reset sequences without an archive merge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct HistoryKey {
    pub generation: u64,
    pub client_epoch: u64,
    pub catalog_rev: u64,
    pub archive_rev: u64,
}

/// One owned preparation, including destruction of a superseded retained buffer.
type Job = Box<dyn FnOnce() + Send>;

/// Queue work on two shared readers rather than creating a thread for every chart request.
pub(super) fn dispatch(job: impl FnOnce() + Send + 'static) {
    static QUEUE: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    QUEUE
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            let rx = Arc::new(Mutex::new(rx));
            for index in 0..2 {
                let rx = rx.clone();
                std::thread::Builder::new()
                    .name(format!("market-volume-{index}"))
                    .spawn(move || {
                        loop {
                            let job = rx.lock().expect("volume queue poisoned").recv();
                            match job {
                                Ok(job) => job(),
                                Err(_) => break,
                            }
                        }
                    })
                    .expect("start retained volume reader");
            }
            tx
        })
        .send(Box::new(job))
        .expect("retained volume readers stopped");
}

/// A fold has exactly one owner and at most one in-flight preparation per cache entry.
pub(super) struct Prepared<T: Send + 'static> {
    pub(super) ready: Option<T>,
    pending: Option<mpsc::Receiver<T>>,
    completed: Arc<AtomicU64>,
}

impl<T: Send + 'static> Prepared<T> {
    /// Start with an empty fold; no retained history is read on the caller's thread.
    pub(super) fn new(value: T, completed: Arc<AtomicU64>) -> Self {
        Self {
            ready: Some(value),
            pending: None,
            completed,
        }
    }

    /// Adopt a completed fold without waiting; return whether a result arrived.
    pub(super) fn poll(&mut self) -> bool {
        let Some(receiver) = self.pending.as_ref() else {
            return false;
        };
        match receiver.try_recv() {
            Ok(value) => {
                self.ready = Some(value);
                self.pending = None;
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => panic!("retained volume preparation failed"),
        }
    }

    /// Transfer the fold to the reader pool; a pending entry cannot enqueue another job.
    pub(super) fn start(&mut self, update: impl FnOnce(&mut T) + Send + 'static) {
        let Some(mut value) = self.ready.take() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        let completed = self.completed.clone();
        dispatch(move || {
            update(&mut value);
            // A pruned entry no longer needs the result; dropping it remains on this reader.
            if tx.send(value).is_ok() {
                completed.fetch_add(1, Ordering::Release);
            }
        });
    }
}

impl<T: Send + 'static> Drop for Prepared<T> {
    /// Pruning a chart cache must not free its retained tape on the UI thread.
    fn drop(&mut self) {
        if let Some(value) = self.ready.take() {
            dispatch(move || drop(value));
        }
        if let Some(receiver) = self.pending.take() {
            // A completed but unpolled result belongs to the channel, not `ready` yet.
            dispatch(move || drop(receiver));
        }
    }
}

#[cfg(test)]
mod tests;
