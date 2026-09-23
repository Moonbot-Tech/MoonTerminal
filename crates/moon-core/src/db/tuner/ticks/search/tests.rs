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
        delta_track: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
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
        own: Arc::new(base()),
    }
}

/// A deal of `prepared` whose own strategy takes at `take` per cent.
fn with_take(uid: i64, take: &str) -> PreparedDeal {
    let mut deal = prepared(uid, 101.0);
    deal.own = Arc::new(
        [("SellPrice", take), ("StopLoss", "0")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    );
    deal
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
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS
        .iter()
        .filter(|f| f.group == ParamGroup::Exit)
        .map(|f| f.key.to_string())
        .collect();
    locked.remove("SellPrice");
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        restarts: 3,
        min_n: Some(4),
        seed: Some(7),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
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
        &defaults,
        "PumpsDetection",
        &result.values,
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    );
    assert!((tally.profit - 80.0).abs() < 1e-6);
    assert!((spent - 8_000.0).abs() < 1e-6);
    // And one deal of it as the trade pane draws it: the same parameters, the same replay — the
    // entry at the fact, the take at 1 % on the peak.
    let picture = variant_picture(
        &deals[0],
        &defaults,
        "PumpsDetection",
        &result.values,
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    );
    assert!(picture.corridor.is_empty(), "no entry model, no corridor");
    let outcome = picture.outcome;
    assert_eq!(outcome.fill.map(|f| f.t_ms), Some(deals[0].deal.buy_ms));
    let exit = outcome.exit.expect("an exit");
    assert_eq!(exit.kind, crate::db::tuner::ticks::ExitKind::Take);
    assert!((exit.price - 101.0).abs() < 1e-6, "{exit:?}");
    assert!((outcome.profit_pct.expect("a trade") - 1.0).abs() < 1e-6);
    // And the deal table's plan column: every deal's money, summing to the column's tally.
    let (by_tally, by_spent, money) = variant_tally_by_deal(
        &deals,
        &defaults,
        "PumpsDetection",
        &result.values,
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    );
    assert_eq!(by_tally.n, tally.n);
    assert!((by_tally.profit - tally.profit).abs() < 1e-9);
    assert!((by_spent - spent).abs() < 1e-9);
    assert_eq!(money.len(), 8);
    assert!(money.iter().all(|(_, m)| {
        m.is_some_and(|(money, pct)| (money - 10.0).abs() < 1e-6 && (pct - 1.0).abs() < 1e-6)
    }));
}

#[test]
fn the_holdout_is_scored_but_never_fitted_on() {
    // The first six deals peak at 101, the last two at 100.5: fitted on the first 75 %, the
    // search picks 1 %, which the holdout then fails to reach.
    let deals: Vec<PreparedDeal> = (1..=6)
        .map(|uid| prepared(uid, 101.0))
        .chain((7..=8).map(|uid| prepared(uid, 100.5)))
        .collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    locked.remove("SellPrice");
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        restarts: 1,
        min_n: Some(3),
        seed: Some(1),
        train_frac: 0.75,
        max_passes: DEFAULT_MAX_PASSES,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
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
    let held = HashMap::new();
    let defaults = HashMap::new();
    let all: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: true,
        vary_exit: true,
        locked: &all,
        restarts: 2,
        min_n: None,
        seed: Some(1),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
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

/// A shift replays no path, so the fields that only move the path are not searched under it;
/// the corridor model searches them all.
#[test]
fn a_shift_does_not_search_the_path_only_fields() {
    let held = HashMap::new();
    let defaults = HashMap::new();
    let locked = HashSet::new();
    let keys = |method| {
        let params = SearchParams {
            held: &held,
            defaults: &defaults,
            kind: "MoonShot",
            vary_entry: true,
            vary_exit: false,
            locked: &locked,
            restarts: 1,
            min_n: None,
            seed: Some(1),
            train_frac: 1.0,
            max_passes: DEFAULT_MAX_PASSES,
            model: ModelSettings {
                entry_method: method,
                ..ModelSettings::default()
            },
        };
        varied(&params).iter().map(|f| f.key).collect::<Vec<_>>()
    };
    let shift = keys(super::super::mshot::EntryMethod::Shift);
    let model = keys(super::super::mshot::EntryMethod::Model);
    for path_only in [
        "MShotRaiseWait",
        "MShotReplaceDelay",
        "MShotUsePrice",
        "FastShotAlgo",
    ] {
        assert!(!shift.contains(&path_only), "{path_only} {shift:?}");
        assert!(model.contains(&path_only), "{path_only} {model:?}");
    }
    assert!(shift.contains(&"MShotPrice") && shift.contains(&"MShotAddDistance"));
}

#[test]
fn a_field_the_strategies_disagree_on_runs_at_each_deals_own_value() {
    // Two strategies, one take each: 1 % and 0.2 %. A variant that leaves the take alone must
    // run every deal at its own strategy's take — 10 on the first deal, 2 on the second — and
    // not at a default for a field no one strategy holds for all.
    let deals = vec![with_take(1, "1"), with_take(2, "0.2")];
    let (tally, _, money) = variant_tally_by_deal(
        &deals,
        &HashMap::new(),
        "PumpsDetection",
        &[("StopLoss".to_string(), "0".to_string())],
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    );
    let money: Vec<Option<f64>> = money.iter().map(|(_, m)| m.map(|(v, _)| v)).collect();
    assert_eq!(tally.n, 2, "{money:?}");
    assert!(
        (money[0].unwrap_or(f64::NAN) - 10.0).abs() < 1e-6,
        "{money:?}"
    );
    assert!(
        (money[1].unwrap_or(f64::NAN) - 2.0).abs() < 1e-6,
        "{money:?}"
    );
}

#[test]
fn a_search_holds_each_deals_own_value_and_reports_a_value_one_strategy_lacks() {
    // Two strategies, takes of 1 % and 0.2 %, every tape peaking at 101. Left alone, each deal
    // keeps its own take: 10 and 2. Searched: 1 % wins on both, and it is a change — the second
    // strategy does not hold it — even though the first already does.
    let deals: Vec<PreparedDeal> = (1..=4)
        .map(|uid| with_take(uid, if uid % 2 == 1 { "1" } else { "0.2" }))
        .collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    let model = ModelSettings {
        latency_ms: 0.0,
        ..ModelSettings::default()
    };
    let (tally, _) = variant_tally(&deals, &defaults, "PumpsDetection", &[], model);
    assert!((tally.profit - 24.0).abs() < 1e-6, "{}", tally.profit);
    locked.remove("SellPrice");
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        restarts: 3,
        min_n: Some(2),
        seed: Some(7),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        model,
    };
    let result = suggest(&deals, &params, &SearchHandle::new()).expect("a result");
    assert_eq!(
        result.values,
        vec![("SellPrice".to_string(), "1".to_string())],
        "{result:?}"
    );
    assert!(
        (result.train.profit - 40.0).abs() < 1e-6,
        "{}",
        result.train.profit
    );
    // Held over every base, the same take is no change at all.
    let held: HashMap<String, String> = [("SellPrice".to_string(), "1".to_string())].into();
    let params = SearchParams {
        held: &held,
        ..params
    };
    let result = suggest(&deals, &params, &SearchHandle::new()).expect("a result");
    assert!(result.values.is_empty(), "{result:?}");
}

/// A floor no point can hold is not an answer: the search says it found nothing rather than
/// hand back the richest point that trades fewer deals than asked.
#[test]
fn a_trade_floor_no_point_keeps_finds_nothing() {
    let deals: Vec<PreparedDeal> = (1..=8).map(|uid| prepared(uid, 101.0)).collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    locked.remove("SellPrice");
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        restarts: 3,
        min_n: Some(9),
        seed: Some(7),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    };
    let result = suggest(&deals, &params, &SearchHandle::new());
    assert!(result.is_none(), "{result:?}");
    // Held by every point, the same search answers.
    let params = SearchParams {
        min_n: Some(8),
        ..params
    };
    assert!(suggest(&deals, &params, &SearchHandle::new()).is_some());
}
