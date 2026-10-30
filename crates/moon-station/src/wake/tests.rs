//! Feed arrivals during a drain and measured timer-only idle wakeups.

use super::*;
use std::time::{Duration, Instant};

/// Clearing after the consumer wake loses a notification sent while that consumer drains.
#[test]
fn arrivals_during_a_drain_keep_the_next_wake() {
    let (sender, incoming) = feeds();
    sender.notify();
    before_drain(&incoming);
    sender.notify();
    assert!(incoming.try_recv().is_ok());
    assert!(incoming.try_recv().is_err());
}

/// Measure timer-only owner wakeups with the old and current no-bot housekeeping cadences.
#[test]
#[ignore = "wall-clock synthetic station idle wake measurement"]
fn station_idle_poll_measurement() {
    for interval in [Duration::from_millis(100), crate::DIAG_POLL_EVERY] {
        let started = Instant::now();
        let mut wakes = 0;
        while started.elapsed() < Duration::from_secs(2) {
            std::thread::park_timeout(interval);
            wakes += 1;
        }
        println!(
            "idle interval={} ms: {:.3} wakes/s",
            interval.as_millis(),
            wakes as f64 / started.elapsed().as_secs_f64()
        );
    }
}
