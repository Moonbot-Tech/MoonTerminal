//! Synthetic regressions for the strategy list's readiness pass: batching, ordering, cancel,
//! the rerun gap. No user data, no live view.
// Not `super::*`: the parent's `gpui::*` brings gpui's own `test` attribute.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::super::ticks::readiness::Readiness;
use super::super::ticks::state::DealRow;
use super::{
    LAUNCH_DEBOUNCE, RERUN_GAP, ReadyClass, ReadyFilter, ReadyKey, ReadyPass, ReadyProgress,
    ReadyRun, count_strategies, group_by_strategy,
};
use moon_core::db::tuner::ticks::{Deal, Deltas, ModelSettings};

fn deal(uid: i64, strategy_id: i64, core_uid: u64) -> Deal {
    Deal {
        report_uid: uid,
        core_uid,
        core_name: String::new(),
        strategy_id,
        kind: "Spread".into(),
        coin: "XALPHA".into(),
        buy_ms: 1_000,
        close_ms: 2_000,
        buy_price: 1.0,
        sell_price: 1.01,
        spent: 100.0,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: 0.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
        strike_detect: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    }
}

fn key(scope_gen: u64, report_gen: u64) -> ReadyKey {
    ReadyKey {
        scope_gen,
        report_gen,
        tape_rev: 0,
        min_tail_s: 30,
        model: ModelSettings::default(),
    }
}

fn ready(fit: usize, covered: usize) -> Readiness {
    Readiness {
        fit,
        covered,
        fetchable: 0,
    }
}

/// Many strategies counted between two ticks fold in ONE batch: one generation bump, one
/// repaint — never one per strategy.
#[test]
fn partial_counts_fold_once_per_tick() {
    let mut pass = ReadyPass::default();
    let (seq, progress) = pass.begin(key(1, 1), Instant::now());
    let before = pass.generation;
    for i in 0..50 {
        progress.push(format!("{i}@1"), ready(i, i));
    }
    assert!(pass.absorb(seq));
    assert_eq!(pass.generation, before + 1, "one bump for the whole batch");
    assert_eq!(pass.counts.len(), 50);
    assert!(
        !pass.absorb(seq),
        "an empty tick folds nothing and repaints nothing"
    );
    assert_eq!(pass.generation, before + 1);
    progress.push("x@1".into(), ready(1, 1));
    assert!(
        !pass.absorb(seq + 1),
        "a superseded pass's counts are not folded"
    );
}

/// A report move keeps the old counts on screen until the new pass replaces them; a scope move
/// drops them, they describe nothing any more.
#[test]
fn a_report_move_keeps_the_counts_a_scope_move_drops_them() {
    let mut pass = ReadyPass::default();
    let (seq, _) = pass.begin(key(1, 1), Instant::now());
    let counts = HashMap::from([("1@1".to_string(), ready(30, 30))]);
    assert_eq!(pass.land(seq, Ok(Some(counts))), (true, false));
    assert!(matches!(pass.run, ReadyRun::Done));
    assert_eq!(
        pass.class("7@7"),
        ReadyClass::NoTape,
        "a strategy without trades is answered once the pass is done, not left on the dash"
    );
    pass.begin(key(1, 2), Instant::now());
    assert_eq!(pass.counts.len(), 1);
    pass.begin(key(2, 2), Instant::now());
    assert!(pass.counts.is_empty());
}

/// A report or tape change waits out the rerun gap since the last start; a first pass and a
/// scope change only wait the debounce.
#[test]
fn reruns_for_a_report_change_wait_the_gap() {
    let mut pass = ReadyPass::default();
    let now = Instant::now();
    assert_eq!(pass.delay(&key(1, 1), now), LAUNCH_DEBOUNCE);
    pass.begin(key(1, 1), now);
    assert_eq!(pass.delay(&key(1, 2), now), RERUN_GAP);
    assert_eq!(pass.delay(&key(2, 2), now), LAUNCH_DEBOUNCE);
    assert_eq!(
        pass.delay(&key(1, 2), now + RERUN_GAP + Duration::from_secs(1)),
        LAUNCH_DEBOUNCE
    );
}

/// The filter only narrows on "Entry/Exit"; a strategy without a count is pending until the
/// pass fails, then unread.
#[test]
fn the_filter_is_off_elsewhere_and_missing_rows_are_pending() {
    let mut pass = ReadyPass {
        chosen: Some(ReadyFilter::Can),
        ..Default::default()
    };
    assert_eq!(pass.filter(true), ReadyFilter::Can);
    assert_eq!(pass.filter(false), ReadyFilter::All);
    assert_eq!(pass.class("9@9"), ReadyClass::Pending);
    let (seq, _) = pass.begin(key(1, 1), Instant::now());
    pass.land(seq, Err(moon_core::db::ReadFail::NotReady));
    assert_eq!(pass.class("9@9"), ReadyClass::Unread);
    assert!(ReadyFilter::Can.keeps(ReadyClass::Can { fit: 30 }));
    assert!(!ReadyFilter::Can.keeps(ReadyClass::Few { fit: 3 }));
    assert!(!ReadyFilter::Can.keeps(ReadyClass::Pending));
}

/// The rows on screen are counted first, in the list's order; a strategy without an address
/// counts without being replayed; a cancel stops the pass.
#[test]
fn strategies_are_counted_in_list_order_and_a_cancel_stops() {
    let deals = vec![deal(1, 1, 1), deal(2, 2, 1), deal(3, 3, 1), deal(4, 2, 1)];
    let groups = group_by_strategy(deals, &HashMap::new(), &["3@1".into(), "1@1".into()]);
    let order: Vec<&str> = groups.iter().map(|g| g.0.as_str()).collect();
    assert_eq!(order, ["3@1", "1@1", "2@1"]);
    assert_eq!(groups[2].1.len(), 2);
    let progress = ReadyProgress::default();
    let mut replays = 0;
    let out = count_strategies(
        groups.clone(),
        |_| {
            replays += 1;
            Vec::<DealRow>::new()
        },
        &progress,
        Duration::ZERO,
    )
    .expect("not cancelled");
    assert_eq!(replays, 0, "no address, nothing to replay");
    assert_eq!(out.len(), 3);
    assert_eq!(out["2@1"].covered, 0);
    assert_eq!(progress.counts(), (3, 3));
    assert_eq!(progress.drain().len(), 3);
    let cancelled = ReadyProgress::default();
    cancelled.cancel();
    assert!(count_strategies(groups, |_| Vec::new(), &cancelled, Duration::ZERO).is_none());
}
