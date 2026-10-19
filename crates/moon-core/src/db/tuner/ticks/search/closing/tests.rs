//! The guard every point must keep.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::tests::{prepared, tick};
use super::super::{DEFAULT_MAX_PASSES, PreparedDeal, SearchParams, suggest};
use super::*;
use crate::db::tuner::threshold_search::SearchHandle;
use crate::db::tuner::ticks::{ModelSettings, TICK_PARAMS};

/// Four trades whose variant closes can differ from their factual order without execution cost.
fn ordered_deals(overlap: bool) -> Vec<PreparedDeal> {
    [10_000, 30_000, 20_000, 40_000]
        .into_iter()
        .enumerate()
        .map(|(i, close)| {
            let mut d = prepared(i as i64 + 1, 100.0);
            let price = if i % 2 == 0 { 110.0 } else { 91.0 };
            let close = if overlap {
                close
            } else {
                (i as i64 + 1) * 10_000
            };
            d.deal.buy_ms = if overlap { 1_000 } else { close - 1_000 };
            d.deal.close_ms = if overlap { 50_000 + i as i64 } else { close };
            d.deal.spent = 100.0;
            d.deal.sell_price = price;
            d.deal.fact_pnl = price - 100.0;
            d.ticks = Arc::from(vec![tick(d.deal.buy_ms, 100.0), tick(close, price)]);
            d.own = Arc::new(
                [
                    ("SellPrice", "10"),
                    ("StopLoss", "-9"),
                    ("FastStopLoss", "YES"),
                ]
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            );
            d
        })
        .collect()
}

/// Folding factual order would hide the consecutive variant losses from risk limits and columns.
#[test]
fn variant_drawdown_follows_simulated_closes() {
    check_drawdown(true, 18.0);
}

/// Chronological non-overlapping trades must retain their original drawdown.
#[test]
fn non_overlapping_drawdown_is_unchanged() {
    check_drawdown(false, 9.0);
}

/// Exercise all three folds while keeping the per-deal column in the caller's order.
fn check_drawdown(overlap: bool, expected: f64) {
    let deals = ordered_deals(overlap);
    let defaults = HashMap::new();
    let model = ModelSettings {
        latency_ms: 0.0,
        ..ModelSettings::default()
    };
    let bases = Bases::of(&deals);
    let params = bases.params(
        &HashMap::new(),
        &defaults,
        &Point::new(),
        "PumpsDetection",
        model,
    );
    let fills: Vec<_> = deals
        .iter()
        .map(|d| {
            Some(Fill {
                t_ms: d.deal.buy_ms,
                price: 100.0,
            })
        })
        .collect();
    let replayed = replay(&deals, &bases.of_deal, &params, &fills);
    assert_eq!(replayed.open, 0);
    assert!(
        (replayed.closed.max_dd - expected).abs() < 1e-8,
        "replay max_dd: {}",
        replayed.closed.max_dd
    );
    let score = super::super::score(&deals, &bases.of_deal, &params);
    assert!(
        (score.tally.max_dd - expected).abs() < 1e-8,
        "score max_dd: {}",
        score.tally.max_dd
    );
    let (column, rows) =
        super::super::variant_tally_by_deal(&deals, &defaults, "PumpsDetection", &[], model);
    assert!(
        (column.tally.max_dd - expected).abs() < 1e-8,
        "column max_dd: {}",
        column.tally.max_dd
    );
    assert_eq!(
        rows.iter().map(|(uid, _)| *uid).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
}

fn exit(stop: f64, trailing: f64, take_profit: Option<f64>) -> (EntryParams, ExitParams) {
    (
        EntryParams::Fact,
        ExitParams {
            stop_loss_pct: stop,
            trailing_pct: trailing,
            trailing_take_profit_pct: take_profit,
            ..ExitParams::default()
        },
    )
}

#[test]
fn a_stop_or_a_bare_trailing_guards_a_trade() {
    assert!(protected(&[exit(-2.0, 0.0, None)]));
    assert!(protected(&[exit(0.0, -1.0, None)]));
    // A trailing with a take profit stands nowhere until the take profit is passed.
    assert!(!protected(&[exit(0.0, -1.0, Some(2.0))]));
    assert!(!protected(&[exit(0.0, 0.0, None)]));
    // Every strategy of the point, not most of them.
    assert!(!protected(&[exit(-2.0, 0.0, None), exit(0.0, 0.0, None)]));
}

/// Turning the stop off leaves the falling deals open past the tape — a loss on no record — and
/// a search of the exit refuses such a point rather than scoring it on the deals it did close (the developer,
/// 2026-09-24): the search keeps the stop.
#[test]
fn a_point_that_leaves_a_deal_open_is_refused() {
    let deals: Vec<PreparedDeal> = (1..=6)
        .map(|uid| {
            let mut d = prepared(uid, 101.0);
            let t0 = d.deal.buy_ms;
            d.ticks = Arc::from(vec![
                tick(t0 - 500, 100.0),
                tick(t0, 100.0),
                tick(t0 + 300, 97.0),
            ]);
            d.own = Arc::new(
                [
                    ("SellPrice", "2"),
                    ("StopLoss", "-2"),
                    ("FastStopLoss", "YES"),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            );
            d
        })
        .collect();
    let (held, defaults) = (HashMap::new(), HashMap::new());
    let locked: HashSet<String> = TICK_PARAMS
        .iter()
        .map(|f| f.key.to_string())
        .filter(|k| k != "UseStopLoss")
        .collect();
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        grids: crate::db::tuner::ticks::search::test_grids::legacy(),
        restarts: 2,
        min_n: Some(1),
        seed: Some(3),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: false,
        model: ModelSettings::default(),
    };
    let result = suggest(&deals, &params, &SearchHandle::new()).expect("the stop is a point");
    assert!(
        !result
            .values
            .iter()
            .any(|(k, v)| k == "UseStopLoss" && v == "NO"),
        "{result:?}"
    );
    assert_eq!(result.train.n, 6, "every deal closed, by its stop");
}

/// One deal the strategy as it stands does not close inside its tape — a flat tape that reaches
/// neither the take nor the stop — no longer refuses every point: it leaves the sample, the
/// answer says so, and the one field asked about is searched over the rest (LinKvo, 2026-09-24).
#[test]
fn a_deal_the_strategy_itself_leaves_open_leaves_the_sample() {
    let mut deals: Vec<PreparedDeal> = (1..=6).map(|uid| prepared(uid, 101.0)).collect();
    let mut flat = prepared(7, 101.0);
    let t0 = flat.deal.buy_ms;
    flat.ticks = Arc::from(vec![
        tick(t0 - 500, 100.0),
        tick(t0, 100.0),
        tick(t0 + 900, 99.9),
    ]);
    deals.push(flat);
    let (held, defaults) = (HashMap::new(), HashMap::new());
    let locked: HashSet<String> = TICK_PARAMS
        .iter()
        .map(|f| f.key.to_string())
        .filter(|k| k != "SellPrice")
        .collect();
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        grids: crate::db::tuner::ticks::search::test_grids::legacy(),
        restarts: 2,
        min_n: Some(3),
        seed: Some(5),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    };
    let result = suggest(&deals, &params, &SearchHandle::new()).expect("a point over the rest");
    assert_eq!(result.stats.left_open, 1);
    assert_eq!(result.train.n, 6);
    assert_eq!(
        result.values,
        vec![("SellPrice".to_string(), "1".to_string())]
    );
}

/// A held stop must not make a search silently earn more by excluding ten losing trades.
#[test]
fn held_stop_reports_the_ten_losses_removed_before_the_split() {
    let deals: Vec<_> = (1..=60)
        .map(|uid| {
            let mut d = prepared(uid, 101.0);
            if uid <= 10 {
                let t = d.deal.buy_ms;
                d.ticks = Arc::from(vec![
                    tick(t, 100.0),
                    tick(t + 300, 97.0),
                    tick(t + 900, 97.0),
                ]);
            }
            d.own = Arc::new(
                [
                    ("SellPrice", "1"),
                    ("StopLoss", "-2"),
                    ("FastStopLoss", "YES"),
                ]
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            );
            d
        })
        .collect();
    let held = [("StopLoss".into(), "-20".into())].into_iter().collect();
    let defaults = HashMap::new();
    let locked = TICK_PARAMS
        .iter()
        .filter(|f| f.key != "SellPrice")
        .map(|f| f.key.into())
        .collect();
    let params = held_params(&held, &defaults, &locked);
    let cut = super::super::cut(&deals, &params);
    assert_eq!(cut.left_open.len(), 10);
    assert_eq!(cut.left_open_by_held, 10);
    let result = suggest(&deals, &params, &SearchHandle::new()).expect("fifty closed deals");
    assert_eq!(result.stats.left_open_by_held, 10);
    assert_eq!(result.train.n, 50);
}

/// Reusing a held-completed stop in the as-is pass would hide the second strategy's lost close.
#[test]
fn as_is_replay_completes_a_fresh_empty_point() {
    let mut deals = vec![prepared(1, 100.0), prepared(2, 100.0)];
    for (i, d) in deals.iter_mut().enumerate() {
        let t = d.deal.buy_ms;
        d.ticks = Arc::from(vec![tick(t, 100.0), tick(t + 300, 97.0)]);
        let mut own: HashMap<String, String> = [
            ("SellPrice", "10"),
            ("FastStopLoss", "YES"),
            ("UseStopLoss", if i == 0 { "NO" } else { "YES" }),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        if i == 1 {
            own.insert("StopLoss".into(), "-2".into());
        }
        d.own = Arc::new(own);
    }
    let held = [("UseStopLoss".into(), "YES".into())].into_iter().collect();
    let defaults = HashMap::new();
    let locked = HashSet::new();
    let params = held_params(&held, &defaults, &locked);
    let bases = Bases::of(&deals);
    let grids =
        crate::db::tuner::ticks::params::range::Grids::of([("StopLoss", Arc::from([-20.0]))]);
    let fields: Vec<_> = TICK_PARAMS
        .iter()
        .filter(|f| matches!(f.key, "StopLoss" | "UseStopLoss"))
        .collect();
    let deps = super::super::deps::Dependents::new(
        &grids,
        &fields,
        &[("StopLoss", 0)].into_iter().collect(),
    );
    let held_point = deps.complete(&Point::new(), &bases.owns, &held, &defaults);
    assert_eq!(held_point.get("StopLoss").map(String::as_str), Some("-20"));
    let (_, _, open, by_held) = closable_at_base(&deals, &bases, &params, &deps);
    assert_eq!(open.len(), 2);
    assert_eq!(by_held, 1, "only the second strategy closes as-is");
}

/// Search one exit field while preserving the held values whose sample effect is under test.
fn held_params<'a>(
    held: &'a HashMap<String, String>,
    defaults: &'a HashMap<String, f64>,
    locked: &'a HashSet<String>,
) -> SearchParams<'a> {
    SearchParams {
        held,
        defaults,
        locked,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        grids: crate::db::tuner::ticks::search::test_grids::legacy(),
        restarts: 1,
        min_n: Some(1),
        seed: Some(3),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    }
}
