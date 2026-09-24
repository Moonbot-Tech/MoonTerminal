//! The guard every point must keep.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::tests::{prepared, tick};
use super::super::{DEFAULT_MAX_PASSES, PreparedDeal, SearchParams, suggest};
use super::*;
use crate::db::tuner::threshold_search::SearchHandle;
use crate::db::tuner::ticks::{ModelSettings, TICK_PARAMS};

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
/// such a point is refused rather than scored on the deals it did close (the developer,
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
        restarts: 2,
        min_n: Some(1),
        seed: Some(3),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
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
