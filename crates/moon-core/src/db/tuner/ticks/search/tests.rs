//! The search on a synthetic sample where the right answer is known.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::*;
use crate::db::tuner::ticks::Deltas;
use crate::feed::types::Side;

fn tick(t_ms: i64, price: f64) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// A PumpsDetection deal (entry from the fact, take by `SellPrice`) bought at 100 whose tape peaks at `peak` after the
/// fill, then falls back to the fact's exit.
fn prepared(uid: i64, peak: f64) -> PreparedDeal {
    let deal = Deal {
        report_uid: uid,
        core_uid: 1,
        core_name: String::new(),
        strategy_id: 1,
        kind: "PumpsDetection".into(),
        coin: "ACE".into(),
        buy_ms: 1_000 * uid,
        close_ms: 1_000 * uid + 900,
        buy_price: 100.0,
        sell_price: 100.2,
        spent: 1_000.0,
        is_short: false,
        sell_reason: "Sell Price".into(),
        fact_pnl: 2.0,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
        step_lag_ms: 0.0,
        stop_anchor: None,
        own_entry: None,
    };
    let t0 = deal.buy_ms;
    let ticks: Vec<Tick> = vec![
        tick(t0 - 500, 100.0),
        tick(t0, 100.0),
        tick(t0 + 300, peak),
        tick(t0 + 600, 100.2),
        tick(t0 + 900, 100.2),
    ];
    PreparedDeal {
        deal,
        ticks: Arc::from(ticks),
        entry_line: None,
        trail_ms: 0,
    }
}

/// The stamp of a tape's last print past the deal's close.
fn tape_end_ms(d: &PreparedDeal) -> i64 {
    d.ticks
        .last()
        .map(|t| (t.time_ms as i64) - d.deal.close_ms)
        .unwrap_or(0)
}

fn base() -> HashMap<String, String> {
    [("SellPrice", "0.2"), ("StopLoss", "0")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn the_search_raises_the_take_to_what_every_tape_reaches() {
    // Every deal peaks at 101.0: a take of 1 % fills on all of them; 1.2 % on none.
    let deals: Vec<PreparedDeal> = (1..=8).map(|uid| prepared(uid, 101.0)).collect();
    let base = base();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS
        .iter()
        .filter(|f| f.group == ParamGroup::Exit)
        .map(|f| f.key.to_string())
        .collect();
    locked.remove("SellPrice");
    let params = SearchParams {
        base: &base,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        restarts: 3,
        min_n: Some(4),
        seed: Some(7),
        train_frac: 1.0,
        latency_ms: 0.0,
    };
    let handle = SearchHandle::new();
    let result = suggest(&deals, &params, &handle).expect("a result");
    assert_eq!(
        result.values,
        vec![("SellPrice".to_string(), "1".to_string())],
        "{result:?}"
    );
    assert_eq!(result.train.n, 8);
    assert!(
        (result.train.profit - 80.0).abs() < 1e-6,
        "{}",
        result.train.profit
    );
    assert!(result.holdout.is_none());
    assert_eq!(handle.completed(), 3);
    // The same values through the variant column.
    let (tally, spent) = variant_tally(
        &deals,
        &base,
        &defaults,
        "PumpsDetection",
        &result.values,
        0.0,
    );
    assert!((tally.profit - 80.0).abs() < 1e-6);
    assert!((spent - 8_000.0).abs() < 1e-6);
}

#[test]
fn the_holdout_is_scored_but_never_fitted_on() {
    // The first six deals peak at 101, the last two at 100.5: fitted on the first 75 %, the
    // search picks 1 %, which the holdout then fails to reach.
    let deals: Vec<PreparedDeal> = (1..=6)
        .map(|uid| prepared(uid, 101.0))
        .chain((7..=8).map(|uid| prepared(uid, 100.5)))
        .collect();
    let base = base();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    locked.remove("SellPrice");
    let params = SearchParams {
        base: &base,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        restarts: 1,
        min_n: Some(3),
        seed: Some(1),
        train_frac: 0.75,
        latency_ms: 0.0,
    };
    let handle = SearchHandle::new();
    let result = suggest(&deals, &params, &handle).expect("a result");
    assert_eq!(result.values[0].1, "1");
    assert_eq!(result.train.n, 6);
    let holdout = result.holdout.expect("a holdout");
    assert_eq!(holdout.n, 0, "neither held-back deal reaches 1 %");
}

#[test]
fn a_cancelled_run_answers_nothing_and_nothing_varied_answers_nothing() {
    let deals: Vec<PreparedDeal> = (1..=3).map(|uid| prepared(uid, 101.0)).collect();
    let base = base();
    let defaults = HashMap::new();
    let all: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    let params = SearchParams {
        base: &base,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: true,
        vary_exit: true,
        locked: &all,
        restarts: 2,
        min_n: None,
        seed: Some(1),
        train_frac: 1.0,
        latency_ms: 0.0,
    };
    let handle = SearchHandle::new();
    assert!(
        suggest(&deals, &params, &handle).is_none(),
        "everything locked"
    );
    let none: HashSet<String> = HashSet::new();
    let params = SearchParams {
        locked: &none,
        ..params
    };
    let handle = SearchHandle::new();
    handle.cancel();
    assert!(suggest(&deals, &params, &handle).is_none());
    assert!(handle.abandoned());
}

/// The sample is judged on ONE exit horizon — the shortest HELD trail among its deals, the
/// coverage's word rather than the last print's: a tape that prints past the horizon is cut
/// there, a tape that prints less is left alone, a print exactly on the horizon stays, and a
/// quiet tail (held 8 s, last print at the close) does not shorten the horizon below what
/// is held.
#[test]
fn the_common_horizon_is_the_shortest_held_trail_and_clips_only_the_longer_tapes() {
    let mut long = prepared(1, 101.0);
    let close = long.deal.close_ms;
    // Prints 5 s and 10 s past the close, on top of the fixture's last print AT the close.
    let mut ticks: Vec<Tick> = long.ticks.to_vec();
    ticks.push(tick(close + 5_000, 100.1));
    ticks.push(tick(close + 10_000, 100.0));
    long.ticks = Arc::from(ticks);
    long.trail_ms = 10_000;
    let mut short = prepared(2, 101.0);
    let close2 = short.deal.close_ms;
    let mut ticks: Vec<Tick> = short.ticks.to_vec();
    ticks.push(tick(close2 + 5_000, 100.3));
    short.ticks = Arc::from(ticks);
    short.trail_ms = 5_000;
    // Held 8 s past the close, but the market printed nothing there.
    let mut quiet = prepared(3, 101.0);
    quiet.trail_ms = 8_000;
    assert_eq!(
        tape_end_ms(&quiet),
        0,
        "the fixture's tape ends at the close"
    );

    let mut deals = vec![long.clone(), short.clone(), quiet.clone()];
    assert_eq!(common_horizon_ms(&deals), Some(5_000));
    assert_eq!(common_horizon_ms(&[]), None);
    clip_to_horizon(&mut deals, 5_000);
    assert_eq!(
        tape_end_ms(&deals[0]),
        5_000,
        "the long tape is cut at the horizon, the print on it stays"
    );
    assert_eq!(deals[0].ticks.len(), long.ticks.len() - 1);
    assert_eq!(
        deals[1].ticks.len(),
        short.ticks.len(),
        "the short one is untouched"
    );
    assert_eq!(deals[2].ticks.len(), quiet.ticks.len());
    // A quiet deal alone with the long one: the horizon is what it HOLDS, 8 s, not the
    // zero its last print would say — the long tape keeps its 5-s print and loses the 10-s one.
    let mut with_quiet = vec![long.clone(), quiet];
    let horizon = common_horizon_ms(&with_quiet).expect("two deals");
    assert_eq!(horizon, 8_000);
    clip_to_horizon(&mut with_quiet, horizon);
    assert_eq!(tape_end_ms(&with_quiet[0]), 5_000);
    // A negative trail (a hand-built deal) reads as zero, never as a horizon before the close.
    let mut odd = long;
    odd.trail_ms = -1;
    assert_eq!(common_horizon_ms(&[odd]), Some(0));
}
