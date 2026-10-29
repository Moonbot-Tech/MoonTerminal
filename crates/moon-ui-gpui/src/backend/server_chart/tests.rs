//! Ring behavior for the shared CPU/memory chart history: per-second dedup and the one-hour cap.
//!
//! Explicit imports (no `use super::*`) per the crate's test convention.

use std::net::{IpAddr, Ipv4Addr};

use super::{CAP, ServerChartHistory, ServerPingHistory};

/// A throwaway loopback address to key one ring under.
fn ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

/// `record` must ignore a second sample for the same second: history is at most one point per
/// second, so multiple panels feeding the same second never double-count it.
#[test]
fn same_second_is_deduplicated() {
    let mut hist = ServerChartHistory::default();
    hist.record(ip(), 100, (10, 20));
    hist.record(ip(), 100, (90, 90)); // same second — must be dropped
    let ring = hist.ring(ip()).expect("ring exists");
    assert_eq!(ring.len(), 1);
    assert_eq!(ring.back(), Some(&(10, 20)));
}

/// A stale or out-of-order second (<= the last recorded one) must not append: the ring stays
/// strictly forward in time.
#[test]
fn out_of_order_second_is_ignored() {
    let mut hist = ServerChartHistory::default();
    hist.record(ip(), 100, (10, 20));
    hist.record(ip(), 99, (30, 40)); // older — must be dropped
    assert_eq!(hist.ring(ip()).map(|r| r.len()), Some(1));
}

/// Consecutive seconds accumulate one point each, newest at the back.
#[test]
fn forward_seconds_accumulate() {
    let mut hist = ServerChartHistory::default();
    for sec in 0..5 {
        hist.record(ip(), sec, (sec as u8, (sec * 2) as u8));
    }
    let ring = hist.ring(ip()).expect("ring exists");
    assert_eq!(ring.len(), 5);
    assert_eq!(ring.front(), Some(&(0, 0)));
    assert_eq!(ring.back(), Some(&(4, 8)));
}

/// The ring is capped to one hour: older points fall off the front so memory stays bounded.
#[test]
fn ring_is_capped_to_one_hour() {
    let mut hist = ServerChartHistory::default();
    for sec in 0..(CAP as i64 + 100) {
        hist.record(ip(), sec, (1, 1));
    }
    assert_eq!(hist.ring(ip()).map(|r| r.len()), Some(CAP));
}

/// An unseen key has no ring at all (distinct from an empty one).
#[test]
fn unseen_key_has_no_ring() {
    let hist = ServerChartHistory::default();
    assert!(hist.ring(ip()).is_none());
}

/// Removing a core releases its samples and last-second stamp; unrelated server history survives.
#[test]
fn removed_subjects_leave_every_history_ring() {
    let other = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));
    let mut hist = ServerChartHistory::default();
    let mut ping = ServerPingHistory::default();
    hist.record(other, 100, (10, 20));
    ping.record(other, 100, 30);

    hist.retain(|key| key == ip());
    ping.retain(|key| key == ip());

    assert!(hist.ring(other).is_none());
    assert!(ping.ring(other).is_none());
}

/// Measure a synthetic day of 100,000 historical server keys shrinking to the ten still configured.
#[test]
#[ignore = "manual chart-history key retention scale measurement"]
fn bench_chart_history_key_retention_day() {
    let mut samples = Vec::new();
    for _ in 0..5 {
        let mut hist = ServerChartHistory::default();
        for key in 0..100_000_u32 {
            hist.record(IpAddr::V4(Ipv4Addr::from(key)), 100, (10, 20));
        }
        let before = (0..100_000_u32)
            .filter(|key| hist.ring(IpAddr::V4(Ipv4Addr::from(*key))).is_some())
            .count();
        let start = std::time::Instant::now();
        hist.retain(|key| matches!(key, IpAddr::V4(ip) if u32::from(ip) < 10));
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        let after = (0..100_000_u32)
            .filter(|key| hist.ring(IpAddr::V4(Ipv4Addr::from(*key))).is_some())
            .count();
        assert_eq!((before, after), (100_000, 10));
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[bench] chart-history keys retained=100000 -> 10 prune median={:.6} ms",
        samples[2]
    );
}
