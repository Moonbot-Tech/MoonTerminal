use super::*;

/// A large ring outside the requested interval must not force an otherwise avoidable REST fetch.
#[test]
fn capture_budgets_only_the_requested_interval_and_preserves_late_rows() {
    let window = ReplayWindow {
        from_ms: 100,
        to_ms: 900,
        open_ms: 400,
        close_ms: 600,
        over_budget: false,
    };
    let mut capture = ReplayCapture::new(window, 3);
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
    let mut too_many = ReplayCapture::new(window, 1);
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
