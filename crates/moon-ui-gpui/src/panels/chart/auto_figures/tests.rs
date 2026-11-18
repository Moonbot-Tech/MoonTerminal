use std::sync::Arc;
use std::time::{Duration, Instant};

use moon_chart::auto_figures::tf::BarSource;
use moon_chart::auto_figures::{Found, FoundShape};
use moon_core::config::auto_figures::{DetectorParams, SwingParams, TfId};
use moon_core::market::ChartCandle;

use super::{AUTO_TF_DWELL, DwellTf, RowSlot, accept, needs_run, split_no_cache};
use crate::chartdx::auto_figures_sync::job::{AutoRowOutcome, RowKey};

const HOUR: i64 = 3_600_000;

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
        lagging: false,
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
        recent: Vec::new(),
    }
}

#[test]
fn a_row_runs_once_per_key_and_not_while_it_backs_off() {
    let now = Instant::now();
    let slot = RowSlot {
        done: Some(Arc::new(outcome(key(10 * HOUR), &[]))),
        ..RowSlot::default()
    };
    // The same closed bar: nothing to do, however often the observer asks.
    assert!(!needs_run(&slot, &key(10 * HOUR), now));
    // The next bar closed: one run.
    assert!(needs_run(&slot, &key(11 * HOUR), now));
    // Other parameters on the same bar: one run too.
    let edited = RowKey {
        detector: DetectorParams::Swing(SwingParams {
            lookback_bars: 50,
            ..SwingParams::default()
        }),
        ..key(10 * HOUR)
    };
    assert!(needs_run(&slot, &edited, now));
    // Already running for that key: not a second time.
    let running = RowSlot {
        pending: Some(key(11 * HOUR)),
        ..RowSlot::default()
    };
    assert!(!needs_run(&running, &key(11 * HOUR), now));
    // A held-off key waits out its back-off, then runs; another key is not held by it.
    let backing_off = RowSlot {
        retry: Some((key(11 * HOUR), now + Duration::from_secs(5))),
        ..RowSlot::default()
    };
    assert!(!needs_run(&backing_off, &key(11 * HOUR), now));
    assert!(needs_run(
        &backing_off,
        &key(11 * HOUR),
        now + Duration::from_secs(6)
    ));
    assert!(needs_run(&backing_off, &key(12 * HOUR), now));
}

/// A read that did not happen backs off 10 s, 30 s, 60 s, 60 s — bounded, never a loop — and a
/// definitive answer resets the ladder.
///
/// Breakage: a fixed short retry (a dead cache worker polled every few seconds forever), an
/// unbounded growth, or a failure count that never resets.
#[test]
fn failed_reads_back_off_along_a_bounded_ladder() {
    let now = Instant::now();
    let mut slot = RowSlot::default();
    let mut waits = Vec::new();
    for _ in 0..4 {
        slot.pending = Some(key(HOUR));
        let failed = AutoRowOutcome {
            read_failed: true,
            ..outcome(key(HOUR), &[])
        };
        assert!(
            !accept(&mut slot, failed, now),
            "a failed read draws nothing"
        );
        let (_, at) = slot.retry.expect("a failed read is retried");
        waits.push(at.duration_since(now).as_secs());
    }
    assert_eq!(waits, vec![10, 30, 60, 60]);
    slot.pending = Some(key(HOUR));
    assert!(accept(&mut slot, outcome(key(HOUR), &[]), now));
    assert!(slot.retry.is_none());
    assert_eq!(slot.failures, 0);
}

/// A market with no cache address answers its cache rows at once, empty and final: no job row, and
/// once accepted no retry is armed — the row reruns only when its key moves.
///
/// Breakage: treating "no cache" as a failed read (a retry every 10 s on every market without a
/// cache, each one a detector job and a counter tick).
#[test]
fn a_market_without_a_cache_is_answered_without_a_retry() {
    let now = Instant::now();
    // An H1 row on a 5-minute chart only a day deep: its bars would come from the cache.
    let chart: Vec<ChartCandle> = (0..288)
        .map(|i| ChartCandle {
            t_open_ms: (i * 5 * 60_000) as f64,
            open: 1.0,
            high: 2.0,
            low: 0.5,
            close: 1.5,
            volume: 0.0,
            quote_volume: 0.0,
        })
        .collect();
    let rows = vec![(0, key(HOUR))];
    let (run, resolved) = split_no_cache(rows.clone(), &chart, 5 * 60_000, false);
    assert!(run.is_empty(), "nothing is left for a job to do");
    assert_eq!(resolved.len(), 1);
    let (_, answer) = resolved.into_iter().next().expect("one answer");
    assert!(!answer.read_failed && !answer.lagging && answer.found.is_empty());
    let mut slot = RowSlot {
        pending: Some(key(HOUR)),
        ..RowSlot::default()
    };
    accept(&mut slot, answer, now);
    assert!(slot.retry.is_none(), "no retry is scheduled");
    assert!(!needs_run(
        &slot,
        &key(HOUR),
        now + Duration::from_secs(3_600)
    ));
    // With a cache, the same row goes to the job untouched.
    let (run, resolved) = split_no_cache(rows, &chart, 5 * 60_000, true);
    assert_eq!(run.len(), 1);
    assert!(resolved.is_empty());
}

/// A lagging answer is drawn but not final: the same key reruns after the 15 s recheck, and a
/// complete answer then ends the rechecks.
///
/// Breakage: caching a bucket built from a half-written cache as final until the next bar of the
/// row's timeframe closes (an hour, for an H1 row).
#[test]
fn a_lagging_answer_is_rechecked_and_then_settles() {
    let now = Instant::now();
    let mut slot = RowSlot {
        pending: Some(key(HOUR)),
        ..RowSlot::default()
    };
    let lagging = AutoRowOutcome {
        lagging: true,
        ..outcome(key(HOUR), &[100.0])
    };
    assert!(
        accept(&mut slot, lagging, now),
        "the partial answer is drawn"
    );
    assert!(!needs_run(&slot, &key(HOUR), now + Duration::from_secs(14)));
    assert!(needs_run(&slot, &key(HOUR), now + Duration::from_secs(15)));
    slot.pending = Some(key(HOUR));
    assert!(accept(&mut slot, outcome(key(HOUR), &[100.0]), now));
    assert!(slot.retry.is_none());
    assert!(!needs_run(&slot, &key(HOUR), now + Duration::from_secs(60)));
}

/// Breakage: switching on the first threshold crossing (no dwell) — a wheel sweep would start a
/// detector job per crossing — or never adopting a span that stayed put.
#[test]
fn an_auto_timeframe_switches_only_after_the_span_held_past_the_dwell() {
    let t0 = Instant::now();
    let mut dwell = DwellTf::default();
    // 2 h on screen: the first answer is taken at once.
    assert_eq!(dwell.update(2.0 * HOUR as f64, t0), (TfId::M1, None));
    // Zoomed out to 8 h (past 4 h × 1.25): asked for M5, held.
    let (tf, recheck) = dwell.update(8.0 * HOUR as f64, t0);
    assert_eq!(tf, TfId::M1);
    assert_eq!(recheck, Some(t0 + AUTO_TF_DWELL));
    // Back to 2 h before the dwell ran out: the switch is forgotten.
    let half = t0 + AUTO_TF_DWELL / 2;
    assert_eq!(dwell.update(2.0 * HOUR as f64, half), (TfId::M1, None));
    // Out again; the wait restarts from this moment, not from the first crossing.
    let again = t0 + AUTO_TF_DWELL;
    assert_eq!(dwell.update(8.0 * HOUR as f64, again).0, TfId::M1);
    assert_eq!(
        dwell.update(8.0 * HOUR as f64, again + AUTO_TF_DWELL / 2).0,
        TfId::M1
    );
    // Held past the dwell: adopted.
    assert_eq!(
        dwell.update(8.0 * HOUR as f64, again + AUTO_TF_DWELL),
        (TfId::M5, None)
    );
}
