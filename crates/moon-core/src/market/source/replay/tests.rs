use super::*;

/// Periodic selection must keep the newest two even after an older in-window resend.
#[test]
fn live_capture_small_budget_survives_late_in_window_resend() {
    let mut capture = LiveCapture::new(99_997, 100_000, 2);
    for stamp in (99_997..=100_000).chain([99_997]) {
        capture.push(Tick {
            time_ms: stamp as f64,
            price: 10.0,
            qty: 1.0,
            side: Side::Buy,
        });
        assert!(capture.ticks.len() <= 4);
    }
    let result = capture.finish(Some(100_000));
    assert_eq!(
        result
            .ticks
            .iter()
            .map(|t| t.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![99_999, 100_000]
    );
    assert!(result.truncated);
}

/// An old appended resend must not hide recent prints earlier in the retained sequence.
#[test]
fn live_capture_preserves_recent_prints_before_late_old_resends() {
    let mut capture = LiveCapture::new(99_997, 100_000, 4000);
    for stamp in (0..100_000).chain([10, 20, 30]) {
        capture.push(Tick {
            time_ms: stamp as f64,
            price: 10.0,
            qty: 1.0,
            side: Side::Buy,
        });
    }
    let delta = capture.finish(Some(99_999));
    assert_eq!(
        delta
            .ticks
            .iter()
            .map(|tick| tick.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![99_997, 99_998, 99_999]
    );
    assert!(!delta.truncated);
}

/// Capping before clipping, or retaining the oldest rows, would starve live chart polling.
#[test]
fn live_chart_cap_keeps_newest_inside_requested_span() {
    let ticks: Vec<Tick> = [500, 100, 400, 200, 300, 600]
        .into_iter()
        .map(|time| Tick {
            time_ms: time as f64,
            price: 10.0,
            qty: 1.0,
            side: Side::Buy,
        })
        .collect();
    let capture = |max| {
        let mut capture = LiveCapture::new(250, 500, max);
        for tick in &ticks {
            capture.push(*tick);
        }
        capture.finish(Some(600))
    };
    let kept = capture(2);
    assert_eq!(
        kept.ticks
            .iter()
            .map(|tick| tick.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![400, 500]
    );
    assert!(kept.truncated);
    let clipped = capture(10);
    assert_eq!(
        clipped
            .ticks
            .iter()
            .map(|tick| tick.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![300, 400, 500]
    );
    assert!(!clipped.truncated);
    assert!(capture(0).ticks.is_empty());
}

/// A cap reached exactly is complete; only a dropped print makes a response truncated.
#[test]
fn live_chart_reports_4000_vs_4001_and_retains_empty_delta_freshness() {
    let tick = |stamp| Tick {
        time_ms: stamp as f64,
        price: 10.0,
        qty: 1.0,
        side: Side::Buy,
    };
    let mut capture = LiveCapture::new(0, 5000, 4000);
    for stamp in 1..=4000 {
        capture.push(tick(stamp));
    }
    let exact = capture.finish(Some(4000));
    assert_eq!(exact.ticks.len(), 4000);
    assert!(!exact.truncated);
    let mut capture = LiveCapture::new(0, 5000, 4000);
    for stamp in (1..=4001).rev() {
        capture.push(tick(stamp));
    }
    let overflow = capture.finish(Some(4001));
    assert_eq!(overflow.ticks.first().unwrap().time_ms, 2.0);
    assert_eq!(overflow.ticks.last().unwrap().time_ms, 4001.0);
    assert_eq!(overflow.ticks.len(), 4000);
    assert!(overflow.truncated);
    let mut capture = LiveCapture::new(4500, 5000, 4000);
    capture.push(tick(4001));
    let empty = capture.finish(Some(4001));
    assert!(empty.ticks.is_empty());
    assert!(!empty.truncated);
    assert_eq!(empty.last_tick_ms, Some(4001));
}

/// A full requested core window must retain its newest prints instead of becoming unavailable.
#[test]
fn live_capture_over_core_budget_keeps_newest_even_with_late_resends() {
    let mut capture = LiveCapture::new(0, 300_000, 4000);
    for stamp in (0..=250_001).rev() {
        capture.push(Tick {
            time_ms: stamp as f64,
            price: 10.0,
            qty: 1.0,
            side: Side::Buy,
        });
    }
    let result = capture.finish(Some(250_001));
    assert!(result.truncated);
    assert_eq!(result.ticks.len(), 4000);
    assert_eq!(result.ticks.first().unwrap().time_ms, 246_002.0);
    assert_eq!(result.ticks.last().unwrap().time_ms, 250_001.0);
    assert_eq!(result.last_tick_ms, Some(250_001));
}

/// A large ring outside the requested interval must not force an otherwise avoidable REST fetch.
#[test]
fn capture_budgets_only_the_requested_interval_and_preserves_late_rows() {
    let window = ReplayWindow {
        from_ms: 100,
        to_ms: 900,
        open_ms: 400,
        close_ms: 600,
        margin_ms: 0,
        long_position_ms: 5 * 60_000,
        over_budget: false,
    };
    let mut capture = ReplayCapture::new(window, 3, CoreSpanRule::BracketPosition);
    for stamp in [0, 1, 2, 3, 4, 5, 1_000, 600, 400, 500] {
        capture.push(Tick {
            time_ms: stamp as f64,
            price: 10.0,
            qty: 1.0,
            side: Side::Buy,
        });
    }
    let result = capture
        .finish()
        .expect("unrelated history must not spend the interval budget");
    assert_eq!(
        result
            .ticks
            .iter()
            .map(|t| t.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![400, 500, 600]
    );
    assert_eq!(result.covered, (100, 900));
    let mut too_many = ReplayCapture::new(window, 1, CoreSpanRule::BracketPosition);
    for stamp in [400, 600] {
        too_many.push(Tick {
            time_ms: stamp as f64,
            price: 10.0,
            qty: 1.0,
            side: Side::Buy,
        });
    }
    assert!(
        too_many.finish().is_none(),
        "a truncated requested interval must not claim coverage"
    );
}

/// A partial donor cannot suppress REST unless both entry and exit remain in its retained span.
#[test]
fn core_replay_requires_both_trade_edges_and_clips_context() {
    let window = ReplayWindow {
        from_ms: 100,
        to_ms: 900,
        open_ms: 400,
        close_ms: 600,
        margin_ms: 0,
        long_position_ms: 5 * 60_000,
        over_budget: false,
    };
    assert_eq!(usable_span(0, 1_000, window), Some((100, 900)));
    assert_eq!(usable_span(300, 700, window), Some((300, 700)));
    assert_eq!(
        usable_span(401, 900, window),
        None,
        "missing entry must use REST"
    );
    assert_eq!(
        usable_span(100, 599, window),
        None,
        "missing exit must use REST"
    );
    assert_eq!(usable_span(i64::MAX, i64::MIN, window), None);
}

/// A capture keeps whatever the ring holds inside the span, whichever edges the ring reaches.
#[test]
fn a_capture_takes_any_overlap_and_clips_to_the_ring() {
    let window = ReplayWindow {
        from_ms: 100,
        to_ms: 900,
        open_ms: 100,
        close_ms: 900,
        margin_ms: 0,
        long_position_ms: 5 * 60_000,
        over_budget: false,
    };
    // The ring reaches neither edge: what it holds is still exhaustive.
    assert_eq!(overlap_span(300, 700, window), Some((300, 700)));
    // The ring lags behind the span's end at close time: filed up to its last print.
    assert_eq!(overlap_span(0, 850, window), Some((100, 850)));
    // The ring moved on past the whole span, or never reached it.
    assert_eq!(overlap_span(901, 2_000, window), None);
    assert_eq!(overlap_span(0, 99, window), None);
    assert_eq!(overlap_span(i64::MAX, i64::MIN, window), None);
}
