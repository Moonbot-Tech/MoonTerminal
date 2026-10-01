use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::tests::prepared;
use super::super::*;
use super::off_grid;

/// A deal of `prepared` whose tape peaks at +1 % and whose own strategy takes at `take`.
fn taking_at(uid: i64, take: &str) -> PreparedDeal {
    let mut deal = prepared(uid, 101.0);
    deal.own = Arc::new(
        [("SellPrice", take), ("StopLoss", "-50")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    );
    deal
}

fn sell_price_only() -> HashSet<String> {
    TICK_PARAMS
        .iter()
        .filter(|f| f.group == ParamGroup::Exit && f.key != "SellPrice")
        .map(|f| f.key.to_string())
        .collect()
}

/// A typed range that leaves the strategy's own value out is the range the answer comes from:
/// the strategy's take of 1 % beats every step of 0.2..0.6, and the search must still answer the
/// best step, never fall back on the value the range excludes.
#[test]
fn a_range_without_the_strategys_value_answers_from_the_range() {
    let deals: Vec<PreparedDeal> = (1..=8).map(|uid| taking_at(uid, "1")).collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let locked = sell_price_only();
    let grids = Grids::of([("SellPrice", Arc::from([0.2, 0.4, 0.6]))]);
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        grids: &grids,
        restarts: 3,
        min_n: Some(4),
        seed: Some(7),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    };
    let result = suggest(&deals, &params, &SearchHandle::new()).expect("a result");
    assert_eq!(
        result.values,
        vec![("SellPrice".to_string(), "0.6".to_string())],
        "{result:?}"
    );
}

/// Only a searched number field whose own value some strategy holds off its grid is pinned, at
/// its start step; a value on the grid, or a field with no value anywhere, is left to the base.
#[test]
fn only_a_value_off_the_grid_is_pinned_at_its_start() {
    let field = TICK_PARAMS
        .iter()
        .find(|f| f.key == "SellPrice")
        .expect("SellPrice");
    let grids = Grids::of([("SellPrice", Arc::from([0.2, 0.4, 0.6]))]);
    let start = HashMap::from([("SellPrice", 2usize)]);
    let own = |take: &str| HashMap::from([("SellPrice".to_string(), take.to_string())]);
    let defaults = HashMap::new();
    let (on, off, none) = (own("0.4"), own("1"), HashMap::new());
    assert!(off_grid(&[field], &grids, &start, &[&on], &defaults).is_empty());
    assert!(off_grid(&[field], &grids, &start, &[&none], &defaults).is_empty());
    // A strategy that leaves the field out runs at the schema's default: off the grid, it pins.
    let off_default = HashMap::from([("sellprice".to_string(), 1.0)]);
    let pinned = off_grid(&[field], &grids, &start, &[&none], &off_default);
    assert_eq!(pinned.get("SellPrice").map(String::as_str), Some("0.6"));
    let pinned = off_grid(&[field], &grids, &start, &[&on, &off], &defaults);
    assert_eq!(pinned.get("SellPrice").map(String::as_str), Some("0.6"));
}
