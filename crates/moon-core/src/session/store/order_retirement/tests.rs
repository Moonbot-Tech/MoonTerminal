//! Destructor ownership and FIFO completion contracts for retired order batches.

use super::*;
use std::sync::{Arc, Mutex};

/// Records the actual thread and order of destruction, independently of queue implementation.
#[derive(Debug)]
struct Probe {
    sequence: usize,
    dropped: Arc<Mutex<Vec<(usize, thread::ThreadId)>>>,
}

impl Drop for Probe {
    /// Observes payload destruction rather than a worker acknowledgement before destruction.
    fn drop(&mut self) {
        self.dropped
            .lock()
            .unwrap()
            .push((self.sequence, thread::current().id()));
    }
}

/// Forgetting batches, destroying them on the caller, or ending before queued work loses parity.
#[test]
fn destroys_every_batch_on_the_worker_before_exit() {
    let caller = thread::current().id();
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let (sender, worker) = spawn();
    let worker_id = worker.thread().id();
    for sequence in 0..QUEUED_BATCHES + 1 {
        sender
            .send(Probe {
                sequence,
                dropped: Arc::clone(&dropped),
            })
            .unwrap();
    }
    drop(sender);
    worker.join().unwrap();
    assert_ne!(caller, worker_id);
    let expected: Vec<_> = (0..QUEUED_BATCHES + 1)
        .map(|sequence| (sequence, worker_id))
        .collect();
    assert_eq!(*dropped.lock().unwrap(), expected);
}
