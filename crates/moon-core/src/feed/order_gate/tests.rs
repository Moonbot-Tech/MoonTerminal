//! The order gate against a consumer that stops reading, and against a lost wake.

use std::time::Duration;

use super::OrderGate;
use crate::feed::{FeedMsg, OrderRow, order_gate_test_feed};

/// An order table with no rows: the gate counts batches, not their contents.
fn table() -> FeedMsg {
    FeedMsg::Orders(Vec::<OrderRow>::new().into())
}

/// A line copy with no rows.
fn lines() -> FeedMsg {
    FeedMsg::OrderLines(Vec::<OrderRow>::new().into())
}

/// #1081: a station's main loop stalled for minutes while every core kept sending a full order
/// table four times a second, and the pile ran the process out of memory. A feed thread that
/// asks the gate before each publication leaves one batch behind, however long nobody reads.
#[test]
fn a_consumer_that_stops_reading_holds_one_order_batch() {
    let (tx, feed, _wakes) = order_gate_test_feed();

    // Ten minutes of a terminal's cadence: a table every fifth turn, line copies in between.
    for turn in 0..12_000 {
        if tx.order_batch_admitted() {
            let batch = if turn % 5 == 0 { table() } else { lines() };
            tx.send(batch).unwrap();
        }
    }

    let mut queued = 0;
    while feed.rx.try_recv().is_ok() {
        queued += 1;
    }
    assert_eq!(
        queued, 1,
        "order batches piled up behind a stalled consumer"
    );
}

/// The refused publication is owed, not dropped: without this wake the feed thread would sleep
/// on its next unrelated deadline while the consumer already has room.
#[test]
fn taking_the_queued_batch_wakes_the_feed_thread_that_was_refused() {
    let (tx, feed, wakes) = order_gate_test_feed();
    tx.send(table()).unwrap();

    assert!(!tx.order_batch_admitted());
    assert!(wakes.try_recv().is_err(), "a refusal is not a wake");

    assert!(matches!(feed.rx.try_recv(), Ok(FeedMsg::Orders(_))));
    assert!(
        wakes.try_recv().is_ok(),
        "the refused feed thread was left asleep"
    );
    assert!(tx.order_batch_admitted());
}

/// A wake for every drained table would turn each of a station's cores four times a second for
/// nothing: the feed thread is woken only when it was refused.
#[test]
fn a_batch_nobody_waits_behind_is_taken_without_a_wake() {
    let (tx, feed, wakes) = order_gate_test_feed();
    tx.send(table()).unwrap();

    assert!(feed.rx.try_recv().is_ok());

    assert!(wakes.try_recv().is_err());
}

/// The gate follows order batches alone. Taking some other message ahead of the table must not
/// let a second table in beside it.
#[test]
fn other_mail_passes_the_gate_and_does_not_open_it() {
    let (tx, feed, wakes) = order_gate_test_feed();

    tx.send(FeedMsg::StrategiesRunning(true)).unwrap();
    assert!(
        tx.order_batch_admitted(),
        "an unrelated message closed the gate"
    );

    tx.send(table()).unwrap();
    assert!(!tx.order_batch_admitted());

    assert!(matches!(
        feed.rx.try_recv(),
        Ok(FeedMsg::StrategiesRunning(true))
    ));
    assert!(
        !tx.order_batch_admitted(),
        "an unrelated message opened the gate"
    );
    assert!(wakes.try_recv().is_err());
}

/// A producer that does not ask (the synthetic core sends a table and a line copy back to back)
/// leaves two batches queued. The refused feed thread is woken by the one that empties the
/// channel, not by the first: woken early it would find the gate still closed.
#[test]
fn the_wake_belongs_to_the_last_of_several_queued_batches() {
    let gate = OrderGate::default();
    gate.sent();
    gate.sent();
    assert!(!gate.admit());

    assert!(!gate.taken());
    assert!(!gate.admit());

    assert!(gate.taken());
    assert!(gate.admit());
}

/// A count taken below zero would wrap, read as "always queued" and stop this core's order
/// publications until the process restarts.
#[test]
fn taking_from_an_empty_gate_leaves_it_open() {
    let gate = OrderGate::default();

    assert!(!gate.taken());

    assert!(gate.admit());
    gate.sent();
    assert!(!gate.admit());
    assert!(gate.taken());
}

/// The refusal and the take race on two threads. If the take could slip between the producer's
/// check and its request for a wake, the feed thread would wait for a wake nobody sends and the
/// core's order table would freeze with the consumer idle.
#[test]
fn a_refused_producer_is_woken_however_the_take_interleaves() {
    const BATCHES: usize = 20_000;
    let (tx, feed, wakes) = order_gate_test_feed();

    let consumer = std::thread::spawn(move || {
        let mut taken = 0;
        while taken < BATCHES {
            match feed.rx.try_recv() {
                Ok(_) => taken += 1,
                Err(_) => std::thread::yield_now(),
            }
        }
    });
    for _ in 0..BATCHES {
        while !tx.order_batch_admitted() {
            wakes
                .recv_timeout(Duration::from_secs(10))
                .expect("the batch was taken and the refused producer was never woken");
        }
        tx.send(table()).unwrap();
    }
    consumer.join().unwrap();
}
