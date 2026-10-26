//! The gate admits exactly its slot count and hands a freed slot to a waiting listing.

use super::Slots;
use std::sync::mpsc;
use std::time::Duration;

/// A listing beyond the slot count waits until one finishes, then proceeds.
#[test]
fn a_listing_past_the_slot_count_waits_for_a_freed_slot() {
    static GATE: Slots = Slots::new(2);
    let first = GATE.acquire();
    let _second = GATE.acquire();
    let (tx, rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let _third = GATE.acquire();
        tx.send(()).unwrap();
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a third listing must not start while two hold the slots"
    );
    drop(first);
    rx.recv_timeout(Duration::from_secs(5))
        .expect("the freed slot goes to the waiting listing");
    waiter.join().unwrap();
}

/// A listing that panics still gives its slot back.
#[test]
fn an_unwinding_listing_frees_its_slot() {
    static GATE: Slots = Slots::new(1);
    let worker = std::thread::spawn(|| {
        let _slot = GATE.acquire();
        panic!("listing failed");
    });
    assert!(worker.join().is_err());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _slot = GATE.acquire();
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("the slot came back after the panic");
}

/// Waiting listings are served in arrival order, so no core is overtaken indefinitely.
#[test]
fn waiting_listings_are_served_in_arrival_order() {
    static GATE: Slots = Slots::new(1);
    let queued = |count: u64| {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while GATE.state.lock().unwrap().next < count {
            assert!(std::time::Instant::now() < deadline, "waiter never queued");
            std::thread::yield_now();
        }
    };
    let held = GATE.acquire();
    let (tx, rx) = mpsc::channel();
    let mut waiters = Vec::new();
    for name in ["first", "second", "third"] {
        let tx = tx.clone();
        waiters.push(std::thread::spawn(move || {
            let _slot = GATE.acquire();
            tx.send(name).unwrap();
        }));
        queued(waiters.len() as u64 + 1);
    }
    drop(held);
    let order: Vec<_> = (0..3)
        .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    assert_eq!(order, ["first", "second", "third"]);
    for waiter in waiters {
        waiter.join().unwrap();
    }
}

/// The load the resource line prints counts taken slots and queued listings, not free slots.
#[test]
fn the_load_counts_readers_and_the_queue_behind_them() {
    static GATE: Slots = Slots::new(1);
    assert_eq!(GATE.load(), (0, 0));
    let held = GATE.acquire();
    let waiter = std::thread::spawn(|| drop(GATE.acquire()));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while GATE.load() != (1, 1) {
        assert!(std::time::Instant::now() < deadline, "{:?}", GATE.load());
        std::thread::yield_now();
    }
    drop(held);
    waiter.join().unwrap();
    assert_eq!(GATE.load(), (0, 0));
}
