//! Destructor ownership and FIFO completion contracts for retired order batches.

use super::*;
use std::sync::{Arc, Mutex};

/// Nonempty ownership without any market, core, credential, or live feed fixture.
fn batch() -> OrderRows {
    OrderRows {
        rows: Vec::new(),
        captured_turns: vec![Vec::new()],
    }
}

/// Supplies an isolated full queue to production-drain tests; releasing drops one queued batch.
pub(in crate::session) fn blocked_retirement(
    wake: Option<FeedWakeTx>,
) -> (DeferredRetirement, impl FnMut()) {
    let (sender, receiver) = sync_channel(1);
    let waiting = Arc::new(AtomicBool::new(false));
    sender
        .send(RetiredBatch {
            rows: Some(batch()),
            wake: wake.map(|wake| (wake, Arc::clone(&waiting))),
        })
        .unwrap_or_else(|_| panic!("fixture receiver ended"));
    (
        DeferredRetirement {
            sender,
            rows: None,
            waiting,
        },
        move || drop(receiver.recv().unwrap()),
    )
}

/// A full queue must preserve ownership and must not wake the drain until disposal progresses.
#[test]
fn full_queue_returns_ownership_then_retries_without_a_busy_wake() {
    let (wake, wakes) = crate::feed::feed_wake_pair();
    let (mut retirement, mut release) = blocked_retirement(Some(wake.clone()));
    assert!(!retirement.enqueue(batch(), Some(wake.clone())));
    assert!(!retirement.retry(Some(wake.clone())));
    assert!(wakes.try_recv().is_err());
    release();
    assert!(wakes.try_recv().is_ok());
    wakes.clear();
    assert!(retirement.retry(Some(wake)));
    assert!(retirement.rows.is_none());
    release();
    assert!(
        wakes.try_recv().is_err(),
        "ordinary disposal must not request a redundant drain"
    );
}

/// Measures the former blocking send under a gated full queue against nonblocking ownership
/// deferral. The gate uses synthetic CPU work, not a timing-dependent sleep or assertion.
#[test]
#[ignore = "manual full-queue admission measurement"]
fn bench_saturated_retirement_admission() {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for _ in 0..31 {
        let (sender, receiver) = sync_channel(QUEUED_BATCHES);
        for _ in 0..QUEUED_BATCHES {
            sender
                .send(RetiredBatch {
                    rows: Some(batch()),
                    wake: None,
                })
                .unwrap_or_else(|_| panic!("fixture receiver ended"));
        }
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let start = std::time::Instant::now();
            started_tx.send(()).unwrap();
            sender
                .send(RetiredBatch {
                    rows: Some(batch()),
                    wake: None,
                })
                .unwrap_or_else(|_| panic!("fixture receiver ended"));
            start.elapsed().as_secs_f64() * 1000.0
        });
        started_rx.recv().unwrap();
        for i in 0..100_000 {
            std::hint::black_box(i * 13);
        }
        drop(receiver.recv().unwrap());
        before.push(worker.join().unwrap());
        drop(receiver);
        let (mut retirement, mut release) = blocked_retirement(None);
        let start = std::time::Instant::now();
        assert!(!retirement.enqueue(batch(), None));
        after.push(start.elapsed().as_secs_f64() * 1000.0);
        release();
        assert!(retirement.retry(None));
        release();
    }
    before.sort_by(f64::total_cmp);
    after.sort_by(f64::total_cmp);
    println!(
        "[bench] gated saturated retirement: before {:.6} ms after {:.6} ms",
        before[15], after[15]
    );
}

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
