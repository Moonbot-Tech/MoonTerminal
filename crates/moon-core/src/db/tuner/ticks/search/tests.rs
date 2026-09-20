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

/// A Spread deal (entry from the fact) bought at 100 whose tape peaks at `peak` after the
/// fill, then falls back to the fact's exit.
fn prepared(uid: i64, peak: f64) -> PreparedDeal {
    let deal = Deal {
        report_uid: uid,
        core_uid: 1,
        strategy_id: 1,
        kind: "Spread".into(),
        coin: "ACE".into(),
        buy_ms: 1_000 * uid,
        close_ms: 1_000 * uid + 900,
        buy_price: 100.0,
        sell_price: 100.2,
        spent: 1_000.0,
        is_short: false,
        sell_reason: "Sell Price".into(),
        fact_pnl: 2.0,
        deltas: Deltas::default(),
        tick: None,
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
        entry_start: None,
    }
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
        kind: "Spread",
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
    let (tally, spent) = variant_tally(&deals, &base, &defaults, "Spread", &result.values, 0.0);
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
        kind: "Spread",
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
        kind: "Spread",
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
