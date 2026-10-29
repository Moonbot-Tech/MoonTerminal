//! Ownership-transfer and nonblocking completion checks for volume readers.

use super::*;

/// Report the thread that releases retained ownership, independent of buffer size.
struct Retired(mpsc::Sender<std::thread::ThreadId>);

impl Drop for Retired {
    /// The drop event is the synchronization edge; the test never waits on a wall-clock sleep.
    fn drop(&mut self) {
        self.0.send(std::thread::current().id()).unwrap();
    }
}

/// Both published state and completed-but-unpolled state must be freed away from the UI caller.
#[test]
fn chart_market_pruning_retires_both_state_owners_off_thread() {
    let caller = std::thread::current().id();
    for pending in [false, true] {
        let (tx, rx) = mpsc::channel();
        let mut prepared = Prepared::new(Retired(tx), Arc::default());
        if pending {
            let (sender, receiver) = mpsc::channel();
            sender
                .send(prepared.ready.take().unwrap())
                .unwrap_or_else(|_| panic!("retirement fixture receiver"));
            prepared.pending = Some(receiver);
        }
        drop(prepared);
        assert_ne!(rx.recv().unwrap(), caller);
    }
}

/// A queued fold must leave the caller immediately and return its state from another thread.
#[test]
fn chart_market_preparation_transfers_ownership_without_waiting() {
    let caller = std::thread::current().id();
    let (release_tx, release_rx) = mpsc::channel();
    let mut prepared = Prepared::new((0, caller), Arc::default());
    prepared.start(move |value| {
        release_rx.recv().unwrap();
        *value = (7, std::thread::current().id());
    });
    assert!(prepared.ready.is_none());
    assert!(
        !prepared.poll(),
        "the reader is deliberately blocked, so polling cannot wait"
    );
    release_tx.send(()).unwrap();
    let value = prepared.pending.take().unwrap().recv().unwrap();
    assert_eq!(value.0, 7);
    assert_ne!(value.1, caller);
}

/// Polling completion adopts the exact state and consumes its notification only once.
#[test]
fn chart_market_preparation_adopts_complete_results() {
    let mut prepared = Prepared::new(1, Arc::default());
    let (tx, rx) = mpsc::channel();
    prepared.ready = None;
    prepared.pending = Some(rx);
    tx.send(9).unwrap();
    assert!(prepared.poll());
    assert_eq!(prepared.ready, Some(9));
    assert!(!prepared.poll());
}
