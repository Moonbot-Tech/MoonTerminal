use std::sync::Arc;
use std::time::{Duration, Instant};

use moon_chart::auto_figures::tf::BarSource;
use moon_chart::auto_figures::{Found, FoundShape};
use moon_core::config::auto_figures::{
    DetectorParams, SwingParams, Take, TakeOrder, TakeSide, TfId,
};

use super::{RowSlot, kept_lines, needs_run};
use crate::chartdx::auto_figures_sync::job::{AutoRowOutcome, RowKey};

fn key(last_closed_ms: i64) -> RowKey {
    RowKey {
        detector: DetectorParams::Swing(SwingParams::default()),
        tf: TfId::H1,
        last_closed_ms,
        chart_ready: true,
    }
}

fn outcome(key: RowKey, levels: &[f64]) -> AutoRowOutcome {
    AutoRowOutcome {
        key,
        source: BarSource::Cache5m,
        bars: 200,
        needed: 200,
        last_close: Some(100.0),
        read_failed: false,
        found: levels
            .iter()
            .map(|&price| Found {
                shape: FoundShape::Horizontal { price },
                touches: Vec::new(),
                strength: 2.0,
                first_ms: 1.0,
                last_ms: 2.0,
                broken: None,
            })
            .collect(),
    }
}

#[test]
fn a_row_runs_once_per_key_and_not_while_it_backs_off() {
    let now = Instant::now();
    let hour = 3_600_000;
    let slot = RowSlot {
        done: Some(Arc::new(outcome(key(10 * hour), &[]))),
        ..RowSlot::default()
    };
    // The same closed bar: nothing to do, however often the observer asks.
    assert!(!needs_run(&slot, &key(10 * hour), now));
    // The next bar closed: one run.
    assert!(needs_run(&slot, &key(11 * hour), now));
    // Other parameters on the same bar: one run too.
    let edited = RowKey {
        detector: DetectorParams::Swing(SwingParams {
            lookback_bars: 50,
            ..SwingParams::default()
        }),
        ..key(10 * hour)
    };
    assert!(needs_run(&slot, &edited, now));
    // Already running for that key: not a second time.
    let running = RowSlot {
        pending: Some(key(11 * hour)),
        ..RowSlot::default()
    };
    assert!(!needs_run(&running, &key(11 * hour), now));
    // A failed read waits out its back-off, then runs.
    let backing_off = RowSlot {
        retry_at: Some(now + Duration::from_secs(5)),
        ..RowSlot::default()
    };
    assert!(!needs_run(&backing_off, &key(11 * hour), now));
    assert!(needs_run(
        &backing_off,
        &key(11 * hour),
        now + Duration::from_secs(6)
    ));
}

#[test]
fn kept_lines_carry_their_side_of_the_reference_price() {
    let found = outcome(key(0), &[90.0, 95.0, 105.0, 112.0]);
    let take = Take {
        count: 1,
        side: TakeSide::Both,
        order: TakeOrder::Nearest,
    };
    let kept = kept_lines(&found, &take, 100.0, 0.0);
    // The nearest above (105, index 2) and the nearest below (95, index 1), each with its side.
    assert_eq!(kept, vec![(2, true), (1, false)]);
    // No reference price, no side and no distance: nothing is kept.
    assert!(kept_lines(&found, &take, f64::NAN, 0.0).is_empty());
}
