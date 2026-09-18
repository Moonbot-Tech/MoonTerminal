use std::time::{Duration, Instant};

use super::*;

#[test]
fn window_asks_go_first_and_nothing_is_queued_twice() {
    let mut pacer = TracePacer::default();
    pacer.push_back([10, 20, 30]);
    pacer.push_front(20);
    pacer.push_front(99);
    pacer.push_back([10]);
    assert_eq!(pacer.queued(), 4);
    let now = Instant::now();
    assert_eq!(pacer.take_due(now), Some(99));
    // The interval gates the next send even with room in the window.
    assert_eq!(pacer.take_due(now), None);
    assert_eq!(pacer.next_due(now), Some(MIN_INTERVAL));
    let later = now + MIN_INTERVAL;
    assert_eq!(pacer.take_due(later), Some(20));
    assert_eq!(pacer.in_flight(), 2);
    // In flight is not re-queued either.
    pacer.push_front(20);
    assert_eq!(pacer.queued(), 2);
}

#[test]
fn the_window_bounds_what_is_unanswered() {
    let mut pacer = TracePacer::default();
    pacer.push_back(1..=20);
    let mut now = Instant::now();
    let mut sent = 0;
    while pacer.take_due(now).is_some() {
        sent += 1;
        now += MIN_INTERVAL;
    }
    assert_eq!(sent, WINDOW);
    assert_eq!(pacer.next_due(now), None, "only an answer can move it now");
    pacer.answered(1, true);
    assert_eq!(pacer.next_due(now), Some(Duration::ZERO));
    assert_eq!(pacer.take_due(now), Some(WINDOW as i64 + 1));
}

#[test]
fn three_failures_in_a_row_abandon_the_queue() {
    let mut pacer = TracePacer::default();
    pacer.push_back(1..=10);
    let mut now = Instant::now();
    let a = pacer.take_due(now).unwrap();
    now += MIN_INTERVAL;
    let b = pacer.take_due(now).unwrap();
    now += MIN_INTERVAL;
    let c = pacer.take_due(now).unwrap();
    pacer.answered(a, false);
    pacer.answered(b, true);
    pacer.answered(c, false);
    assert_eq!(pacer.queued(), 7, "a success in between resets the streak");
    now += MIN_INTERVAL;
    let d = pacer.take_due(now).unwrap();
    now += MIN_INTERVAL;
    let e = pacer.take_due(now).unwrap();
    pacer.answered(d, false);
    pacer.answered(e, false);
    assert_eq!(pacer.queued(), 0);
    // A later ask starts over.
    pacer.push_front(77);
    now += MIN_INTERVAL;
    assert_eq!(pacer.take_due(now), Some(77));
}

#[test]
fn the_queue_is_capped_at_the_back() {
    let mut pacer = TracePacer::default();
    pacer.push_back(1..=(QUEUE_CAP as i64 + 5));
    assert_eq!(pacer.queued(), QUEUE_CAP);
    assert_eq!(pacer.take_due(Instant::now()), Some(1));
}
