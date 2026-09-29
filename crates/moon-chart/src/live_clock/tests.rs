use super::*;

const NOW: f64 = 1_700_000_000_000.0;

/// Feed a baseline trade, then one fresh trade per second whose time is `offset_ms` off the clock.
fn feed(est: &mut LiveClockOffset, offset_ms: f64, trades: usize) {
    est.observe(Some(NOW - 60_000.0), NOW - 60_000.0);
    for i in 1..=trades {
        let now = NOW + i as f64 * 1_000.0;
        est.observe(Some(now + offset_ms), now);
    }
}

#[test]
fn starts_at_the_local_clock() {
    let est = LiveClockOffset::default();
    assert_eq!(est.edge_ms(0.0), 0.0);
    assert_eq!(est.edge_ms(NOW), NOW);
}

#[test]
fn ticks_ahead_of_the_clock_move_the_edge_forward() {
    let mut est = LiveClockOffset::default();
    feed(&mut est, 2_500.0, 5);
    assert_eq!(est.edge_ms(0.0), 2_500.0);
    assert_eq!(est.edge_ms(NOW), NOW + 2_500.0);
}

#[test]
fn ticks_behind_the_clock_move_the_edge_back() {
    let mut est = LiveClockOffset::default();
    feed(&mut est, -1_200.0, 5);
    assert_eq!(est.edge_ms(0.0), -1_200.0);
}

#[test]
fn a_single_outlier_does_not_move_the_estimate() {
    let mut est = LiveClockOffset::default();
    feed(&mut est, 300.0, 6);
    let now = NOW + 10_000.0;
    // One trade stamped far in the future, but still within the bound.
    est.observe(Some(now + 240_000.0), now);
    assert_eq!(est.edge_ms(0.0), 300.0);
}

#[test]
fn a_stale_market_neither_samples_its_history_nor_drifts() {
    let mut est = LiveClockOffset::default();
    // First sight of a market whose last trade was two minutes ago: baseline only.
    est.observe(Some(NOW - 120_000.0), NOW);
    assert_eq!(est.edge_ms(0.0), 0.0);
    // No new trade for a long while: the same newest time adds nothing.
    est.observe(Some(NOW - 120_000.0), NOW + 300_000.0);
    assert_eq!(est.edge_ms(0.0), 0.0);

    let mut learned = LiveClockOffset::default();
    feed(&mut learned, 800.0, 3);
    learned.observe(Some(NOW + 3_000.0 + 800.0), NOW + 600_000.0);
    assert_eq!(
        learned.edge_ms(0.0),
        800.0,
        "a silent market keeps its estimate"
    );
}

#[test]
fn samples_beyond_the_bound_are_ignored() {
    let mut est = LiveClockOffset::default();
    feed(&mut est, 500.0, 3);
    let now = NOW + 10_000.0;
    est.observe(Some(now + MAX_CLOCK_OFFSET_MS + 1.0), now);
    assert_eq!(est.edge_ms(0.0), 500.0);
    // A replayed trade far behind the clock, newer than anything seen on a long-quiet source.
    let mut quiet = LiveClockOffset::default();
    quiet.observe(Some(NOW - 3_600_000.0), NOW);
    quiet.observe(Some(NOW - MAX_CLOCK_OFFSET_MS - 1.0), NOW);
    assert_eq!(quiet.edge_ms(0.0), 0.0);
}

#[test]
fn a_clock_correction_is_followed_within_a_window() {
    let mut est = LiveClockOffset::default();
    feed(&mut est, 3_000.0, WINDOW);
    for i in 1..=WINDOW {
        let now = NOW + 100_000.0 + i as f64 * 1_000.0;
        est.observe(Some(now + 50.0), now);
    }
    assert_eq!(est.edge_ms(0.0), 50.0);
}
