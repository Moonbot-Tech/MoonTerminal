use std::sync::Arc;
use std::time::{Duration, Instant};

use moon_chart::auto_figures::tf::BarSource;
use moon_chart::auto_figures::{Found, FoundShape};
use moon_core::config::auto_figures::{
    AutoFigurePreset, AutoFigureRow, AutoFiguresCfg, DetectorKind, DetectorParams, SwingParams,
    TfId,
};
use moon_core::market::ChartCandle;

use super::{
    AUTO_TF_DWELL, DwellTf, PaneSlots, RowSlot, accept, drawable, drop_gone_panes, needs_publish,
    needs_run, pane_shows, plan_rows, realign, retry_due_ms, slot_map, split_no_cache,
};
use crate::chartdx::auto_figures_sync::job::{AutoRowOutcome, RowKey};

const HOUR: i64 = 3_600_000;

fn key(last_closed_ms: i64) -> RowKey {
    RowKey {
        detector: DetectorParams::Swing(SwingParams::default()),
        tf: TfId::H1,
        last_closed_ms,
        chart_ready: true,
        chart_deep_history: false,
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
                origin: None,
                shape: FoundShape::Horizontal { price },
                touches: Vec::new(),
                strength: 2.0,
                first_ms: 1.0,
                last_ms: 2.0,
                broken: None,
                touch_volume: 0.0,
                tolerance_frac: 0.0,
                tag: None,
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

/// Breakage (#1): a retry instant that passed while the row's own run is in flight turned into a
/// 0 ms wake; each sync found the row still pending and armed it again — a spin for the whole job.
#[test]
fn a_pending_row_with_a_past_retry_arms_no_wake() {
    let now = Instant::now();
    let mut slot = RowSlot {
        retry: Some((key(0), now)),
        ..RowSlot::default()
    };
    let later = now + Duration::from_secs(1);
    // Not pending: the passed retry is due now.
    assert!(retry_due_ms(&slot, later, 1_000.0).is_some());
    slot.pending = Some(key(HOUR));
    assert_eq!(retry_due_ms(&slot, later, 1_000.0), None);
}

/// Breakage (#7): slots kept by position across a swap — each row then published its neighbour's
/// answer in its own style until its run returned.
#[test]
fn swapped_rows_carry_their_slots_and_a_foreign_answer_is_not_drawn() {
    let a = AutoFigureRow::new(DetectorParams::Swing(SwingParams::default()));
    let b = AutoFigureRow::new(DetectorKind::Trend.default_params());
    let map = slot_map(&[a.clone(), b.clone()], &[b.clone(), a.clone()]);
    assert_eq!(map, vec![Some(1), Some(0)]);

    let key_a = key(0);
    let key_b = RowKey {
        detector: DetectorKind::Trend.default_params(),
        ..key(0)
    };
    let slot = |k: RowKey| RowSlot {
        done: Some(Arc::new(outcome(k, &[100.0]))),
        current: Some(k),
        ..RowSlot::default()
    };
    let mut slots = vec![slot(key_a), slot(key_b)];
    realign(&mut slots, &map);
    assert_eq!(slots[0].done.as_ref().unwrap().key, key_b);
    assert_eq!(slots[1].done.as_ref().unwrap().key, key_a);

    // Without the realignment the slot at position 0 holds A's answer while the row there is now
    // B: the publication check refuses it.
    let mut stale = slot(key_a);
    stale.current = Some(key_b);
    assert!(drawable(&stale).is_none());
    // A newer bar of the same row is not foreign: the last answer stays on screen meanwhile.
    stale.current = Some(key(HOUR));
    assert!(drawable(&stale).is_some());
}

/// A wall clock ten minutes into a four-hour bucket (2023-11-14 20:10 UTC), so an hour later the
/// M5, M15 and H1 rows have a new closed bar while the H4 row does not.
const EYE_NOW_MS: f64 = (1_699_992_000_000_i64 + 10 * 60_000) as f64;

/// A pane on a market, with nothing evaluated yet.
fn eye_pane() -> PaneSlots {
    PaneSlots::new((7, "BTCUSDT".to_string()))
}

/// How many rows of `cfg` put anything on the chart — what a full evaluation runs.
fn drawn_rows(cfg: &AutoFiguresCfg) -> usize {
    cfg.rows.iter().filter(|r| r.is_drawn()).count()
}

/// Answer every job of `jobs` the way a finished run would, so the slots hold a cache.
fn answer(pane: &mut PaneSlots, jobs: &[(usize, RowKey)], now: Instant) {
    for (ix, key) in jobs {
        assert!(accept(&mut pane.rows[*ix], outcome(*key, &[100.0]), now));
    }
}

/// Hiding a pane with the eye evaluates nothing — no job, no wake — even once its bars moved on,
/// and leaves every cached answer where it was.
///
/// Breakage: the eye routed as "publish an empty set" while the keys go on being evaluated, which
/// starts a detector run per bar close for lines nobody can see.
#[test]
fn hiding_with_the_eye_runs_no_detector_and_keeps_the_cache() {
    let now = Instant::now();
    let cfg = AutoFiguresCfg::shipped();
    let mut pane = eye_pane();
    let first = plan_rows(&cfg, &mut pane, Some(HOUR), true, false, 0, now, EYE_NOW_MS);
    assert_eq!(
        first.jobs.len(),
        drawn_rows(&cfg),
        "a shown pane runs every row once"
    );
    answer(&mut pane, &first.jobs, now);

    pane.eye = Some(false);
    let later = EYE_NOW_MS + (5 * HOUR) as f64;
    let hidden = plan_rows(&cfg, &mut pane, Some(HOUR), true, false, 0, now, later);
    assert!(hidden.jobs.is_empty(), "a hidden pane starts no detector");
    assert!(
        hidden.wake.is_none(),
        "and arms no wake for a bar nobody sees"
    );
    assert!(
        pane.rows.iter().all(|s| s.done.is_some()),
        "the answers stay cached for the next show"
    );
}

/// Showing the pane again on the same bar takes the cached answers — no run, and a publication so
/// they are drawn at once — and after the bars moved it runs ONLY the rows whose key moved.
///
/// Breakage: the show dropping the slots (every row recomputed), or waiting for a run to publish
/// (nothing drawn until the next bar close).
///
/// The shipped set plus the 4h levels: every shipped row closes a bar within the hour, and "only
/// the stale rows" needs a row whose bar did not.
#[test]
fn showing_again_takes_the_cache_and_runs_only_stale_rows() {
    let now = Instant::now();
    let mut cfg = AutoFiguresCfg::shipped();
    cfg.rows.push(AutoFigurePreset::Levels4h.row());
    let mut pane = eye_pane();
    let first = plan_rows(&cfg, &mut pane, Some(HOUR), true, false, 0, now, EYE_NOW_MS);
    answer(&mut pane, &first.jobs, now);
    pane.published = true;

    // Hidden: the sync takes the lines off, which is what clears `published`.
    pane.eye = Some(false);
    assert!(
        plan_rows(&cfg, &mut pane, Some(HOUR), true, false, 0, now, EYE_NOW_MS)
            .jobs
            .is_empty()
    );
    pane.published = false;

    // Shown on the same bar: nothing to run, and the cache is published as it is.
    pane.eye = Some(true);
    let same = plan_rows(&cfg, &mut pane, Some(HOUR), true, false, 0, now, EYE_NOW_MS);
    assert!(same.jobs.is_empty(), "the same key takes the cached answer");
    assert!(
        needs_publish(&pane, false),
        "and the cached lines are put back without waiting for a run"
    );

    // Hidden over an hour, shown again: only the rows whose bar closed meanwhile run.
    pane.eye = Some(false);
    let later = EYE_NOW_MS + HOUR as f64;
    assert!(
        plan_rows(&cfg, &mut pane, Some(HOUR), true, false, 0, now, later)
            .jobs
            .is_empty()
    );
    pane.eye = Some(true);
    let moved = plan_rows(&cfg, &mut pane, Some(HOUR), true, false, 0, now, later);
    assert!(
        !moved.jobs.is_empty() && moved.jobs.len() < drawn_rows(&cfg),
        "some rows moved and some did not: {} of {}",
        moved.jobs.len(),
        drawn_rows(&cfg)
    );
    for (ix, slot) in pane.rows.iter().enumerate() {
        let (Some(done), Some(current)) = (slot.done.as_ref(), slot.current.as_ref()) else {
            continue;
        };
        let runs = moved.jobs.iter().any(|(j, _)| *j == ix);
        assert_eq!(
            runs,
            done.key != *current,
            "row {ix} runs exactly when its cached answer is for another key"
        );
    }
}

/// On a tab kind whose popup switch is OFF the eye still shows the kind's rows on this pane — they
/// run and are drawn — and on a kind whose switch is ON the eye hides them; with no row drawn at
/// all it has nothing to show.
///
/// Breakage: the eye only able to hide (the shipped-off kinds could never be shown per coin), or
/// the popup's switch read before the eye.
#[test]
fn the_eye_outranks_the_tab_switch_but_not_the_rows() {
    let now = Instant::now();
    let off = AutoFiguresCfg::shipped_off();
    let mut pane = eye_pane();
    assert!(
        plan_rows(&off, &mut pane, Some(HOUR), true, false, 0, now, EYE_NOW_MS)
            .jobs
            .is_empty(),
        "the switch is off and the eye untouched: nothing runs"
    );
    pane.eye = Some(true);
    let shown = plan_rows(&off, &mut pane, Some(HOUR), true, false, 0, now, EYE_NOW_MS);
    assert_eq!(
        shown.jobs.len(),
        drawn_rows(&off),
        "the eye shows the kind's rows on this pane"
    );

    assert!(!pane_shows(&AutoFiguresCfg::shipped(), Some(false)));
    let mut none_drawn = AutoFiguresCfg::shipped();
    none_drawn.rows.iter_mut().for_each(|r| r.visible = false);
    assert!(
        !pane_shows(&none_drawn, Some(true)),
        "the eye cannot draw rows the popup hid"
    );
}

/// The eye's choice ends with the market: a pane that moves to another coin leaves its slots —
/// eye included — behind, and the new coin follows the tab's switch again. A pane merely hidden on
/// the same coin keeps its slots, and a frozen trade window keeps nothing.
///
/// Breakage: the override keyed by pane index instead of `(core, market)`, which would carry "hide"
/// onto the next coin opened in that pane.
#[test]
fn the_eye_ends_with_the_market() {
    let cfg = AutoFiguresCfg::shipped();
    let btc = (7, "BTCUSDT".to_string());
    let eth = (7, "ETHUSDT".to_string());
    let mut panes = vec![eye_pane()];
    panes[0].eye = Some(false);

    // Same coin, still on screen: kept, eye and all.
    assert!(drop_gone_panes(&mut panes, &[(0, btc.clone())], false).is_empty());
    assert_eq!(panes[0].eye, Some(false));

    // The pane switched to another coin: the old slots go, and nothing carries over.
    assert_eq!(
        drop_gone_panes(&mut panes, &[(0, eth.clone())], false),
        vec![btc]
    );
    assert!(panes.is_empty());
    let eye = panes.iter().find(|p| p.target == eth).and_then(|p| p.eye);
    assert!(
        pane_shows(&cfg, eye),
        "the new coin follows the tab's switch"
    );

    // A trade window keeps nothing even for a target still on screen.
    let mut frozen = vec![eye_pane()];
    assert_eq!(
        drop_gone_panes(&mut frozen, &[(0, (7, "BTCUSDT".to_string()))], true).len(),
        1
    );
}
