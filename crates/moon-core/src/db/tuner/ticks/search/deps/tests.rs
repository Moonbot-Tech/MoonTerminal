//! The search's field dependencies on hand-made strategies.

use std::collections::HashSet;

use super::*;
use crate::db::tuner::ticks::TICK_PARAMS;

fn field(key: &str) -> &'static TickParam {
    TICK_PARAMS.iter().find(|f| f.key == key).expect("a knob")
}

fn own(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn point(pairs: &[(&'static str, &str)]) -> Point {
    pairs.iter().map(|(k, v)| (*k, (*v).to_string())).collect()
}

/// The dependents of the trailing's take profit, started at the first step of their grids.
fn deps() -> Dependents {
    let fields = [
        field("UseTrailing"),
        field("TrailingPercent"),
        field("UseTakeProfit"),
        field("TakeProfit"),
    ];
    let start: HashMap<&'static str, usize> = [("TrailingPercent", 0), ("TakeProfit", 2)]
        .into_iter()
        .collect();
    Dependents::new(
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &fields,
        &start,
    )
}

/// A switch turned on with no value behind it gets one: the take profit's per cent at its start.
#[test]
fn a_switch_turned_on_brings_its_values() {
    let bare = own(&[]);
    let p = point(&[("UseTrailing", "YES"), ("UseTakeProfit", "YES")]);
    let out = deps().complete(&p, &[&bare], &HashMap::new(), &HashMap::new());
    assert_eq!(out.get("TakeProfit").map(String::as_str), Some("1"));
    assert_eq!(out.get("TrailingPercent").map(String::as_str), Some("-10"));
}

/// A strategy that holds the value keeps its own: nothing is laid over it.
#[test]
fn a_value_on_record_is_not_replaced() {
    let set = own(&[("TakeProfit", "5.0"), ("TrailingPercent", "-4.0")]);
    let p = point(&[("UseTrailing", "YES"), ("UseTakeProfit", "YES")]);
    let out = deps().complete(&p, &[&set], &HashMap::new(), &HashMap::new());
    assert!(!out.contains_key("TakeProfit") && !out.contains_key("TrailingPercent"));
}

/// A switch left out of the dump reads the model's fallback: `UseTakeProfit` absent is off, so
/// its per cent is not brought in.
#[test]
fn an_absent_switch_reads_the_models_fallback() {
    let bare = own(&[]);
    let p = point(&[("UseTrailing", "YES")]);
    let out = deps().complete(&p, &[&bare], &HashMap::new(), &HashMap::new());
    assert!(!out.contains_key("TakeProfit"), "{out:?}");
    assert!(out.contains_key("TrailingPercent"));
}

/// A field in effect on no strategy moves nothing and is left out of the answer.
#[test]
fn a_field_in_effect_nowhere_is_pruned() {
    let bare = own(&[]);
    let p = point(&[
        ("UseTrailing", "NO"),
        ("TakeProfit", "2"),
        ("TrailingPercent", "-2"),
    ]);
    let out = deps().prune(&p, &[&bare], &HashMap::new(), &HashMap::new());
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(out.contains_key("UseTrailing"));
    // In effect on ONE strategy is enough to stay.
    let on = own(&[("UseTrailing", "YES"), ("UseTakeProfit", "YES")]);
    let p = point(&[("TakeProfit", "2")]);
    let out = deps().prune(&p, &[&bare, &on], &HashMap::new(), &HashMap::new());
    assert!(out.contains_key("TakeProfit"));
}

/// Two strategies, one holding the per cent and one not: the point carries it — one value for
/// both, as Save writes it — at the start step.
#[test]
fn a_completed_value_is_the_points_for_every_strategy() {
    let set = own(&[("TakeProfit", "5.0")]);
    let bare = own(&[]);
    let p = point(&[("UseTrailing", "YES"), ("UseTakeProfit", "YES")]);
    let out = deps().complete(&p, &[&set, &bare], &HashMap::new(), &HashMap::new());
    assert_eq!(out.get("TakeProfit").map(String::as_str), Some("1"));
}

/// A number one strategy lacks can put another's dependent in effect: the first strategy keeps
/// PriceDown off (`PriceDownTimer = 0`) and holds no per cent, the second holds the per cent and no
/// timer — at the core's default, which the dump leaves out. Neither is completed as they stand;
/// a point that switches PriceDown on brings the first one's per cent, whatever order the
/// strategies come in.
#[test]
fn a_completion_reaches_every_strategy_whatever_the_order() {
    let fields = [field("PriceDownTimer"), field("PriceDownPercent")];
    let start: HashMap<&'static str, usize> = [("PriceDownTimer", 3), ("PriceDownPercent", 4)]
        .into_iter()
        .collect();
    let deps = Dependents::new(
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &fields,
        &start,
    );
    let off = own(&[("PriceDownTimer", "0")]);
    let pct = own(&[("PriceDownPercent", "10")]);
    for owns in [[&off, &pct], [&pct, &off]] {
        let out = deps.complete(&point(&[]), &owns, &HashMap::new(), &HashMap::new());
        assert!(out.is_empty(), "{out:?}");
        let on = point(&[("PriceDownTimer", "5")]);
        let out = deps.complete(&on, &owns, &HashMap::new(), &HashMap::new());
        assert!(out.contains_key("PriceDownPercent"), "{out:?}");
    }
}

/// A field a strategy leaves out is at the core's default, not missing: the strategies as they
/// stand get nothing from the others. `MShotAddBTCDelta` 0.03 on one strategy and absent on
/// another — completing the second at the median rewrote its corridor, and the corridor rule then
/// refused the strategy itself (24.09).
#[test]
fn a_field_at_its_default_is_not_completed_from_other_strategies() {
    let fields = [field("MShotAddBTCDelta")];
    let start: HashMap<&'static str, usize> = [("MShotAddBTCDelta", 1)].into_iter().collect();
    let deps = Dependents::new(
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &fields,
        &start,
    );
    let set = own(&[("MShotAddBTCDelta", "0.03")]);
    let bare = own(&[]);
    let out = deps.complete(
        &point(&[]),
        &[&set, &bare],
        &HashMap::new(),
        &HashMap::new(),
    );
    assert!(out.is_empty(), "{out:?}");
}

/// A search of one field locks every other, the per cent of a switch the variant turns on among
/// them — and the answer still carries that per cent: В1 holds `UseTakeProfit = YES` on a
/// trailing strategy that keeps no `TakeProfit`, the search of `SellPrice` alone brings it at the
/// schema default's step (the defaults are keyed lowercase, as `strategy_field_defaults` gives
/// them).
#[test]
fn a_search_of_one_field_completes_what_the_variant_switched_on() {
    use crate::db::tuner::threshold_search::SearchHandle;
    use crate::db::tuner::ticks::search::{
        DEFAULT_MAX_PASSES, SearchParams, suggest, tests::prepared,
    };
    use crate::db::tuner::ticks::settings::ModelSettings;
    use std::sync::Arc;

    let trailing: Arc<HashMap<String, String>> = Arc::new(own(&[
        ("SellPrice", "0.2"),
        ("StopLoss", "-50"),
        ("UseTrailing", "YES"),
        ("TrailingPercent", "-1"),
    ]));
    let deals: Vec<_> = (1..=8)
        .map(|uid| {
            let mut deal = prepared(uid, 101.0);
            deal.own = Arc::clone(&trailing);
            deal
        })
        .collect();
    let held = own(&[("UseTakeProfit", "YES")]);
    let defaults: HashMap<String, f64> = [("takeprofit".to_string(), 1.0)].into_iter().collect();
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
        restarts: 1,
        min_n: Some(4),
        seed: Some(3),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    };
    let result = suggest(&deals, &params, &SearchHandle::new()).expect("a result");
    let take = result
        .values
        .iter()
        .find(|(k, _)| k == "TakeProfit")
        .map(|(_, v)| v.parse::<f64>().expect("a number"));
    assert_eq!(take, Some(1.0), "{:?}", result.values);
    // Nothing else locked moved: the searched field and the completed per cent.
    assert!(
        result
            .values
            .iter()
            .all(|(k, _)| k == "SellPrice" || k == "TakeProfit"),
        "{:?}",
        result.values
    );
}

/// A strategy that already keeps the take profit on with no per cent on record is left as it
/// stands — the search did not switch anything on, the per cent is at the core's default — while
/// one the variant switches on gets the per cent.
#[test]
fn a_field_is_completed_only_for_a_switch_the_variant_turns_on() {
    let fields = [
        field("UseTrailing"),
        field("UseTakeProfit"),
        field("TakeProfit"),
    ];
    let start: HashMap<&'static str, usize> = [("TakeProfit", 2)].into_iter().collect();
    let deps = Dependents::new(
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &fields,
        &start,
    );
    let stored_on = own(&[("UseTrailing", "YES"), ("UseTakeProfit", "YES")]);
    let out = deps.complete(&point(&[]), &[&stored_on], &HashMap::new(), &HashMap::new());
    assert!(!out.contains_key("TakeProfit"), "{out:?}");
    let off = own(&[("UseTrailing", "YES")]);
    let p = point(&[("UseTakeProfit", "YES")]);
    let out = deps.complete(&p, &[&off], &HashMap::new(), &HashMap::new());
    assert_eq!(out.get("TakeProfit").map(String::as_str), Some("1"));
}

/// Every field a number knob's rule reads has a fallback: `effective` fills only the listed
/// conditions, and a condition left absent does not block (`FieldDeps::field_active`) — the
/// strategy as it stands would read as having the field in effect behind a switch that is off,
/// and a switch the variant turns on would bring nothing (`MShotSellAtLastPrice` and its
/// `MShotSellPriceAdjust`).
#[test]
fn every_condition_of_a_number_knob_has_a_fallback() {
    let rules = FieldDeps::bundled();
    let mut missing: Vec<String> = Vec::new();
    for knob in TICK_PARAMS.iter().filter(|f| f.kind == ParamKind::Num) {
        for condition in rules.conditions_of(knob.key) {
            let known = CONDITION_FALLBACKS.iter().any(|(k, _)| *k == condition);
            if !known && !missing.iter().any(|m| m == condition) {
                missing.push(condition.to_string());
            }
        }
    }
    assert!(missing.is_empty(), "no fallback for {missing:?}");
}

/// `MShotSellAtLastPrice` left out is off (the model's own default), so its adjustment is not in
/// effect as the strategy stands: a variant that switches it on brings the adjustment.
#[test]
fn a_sell_at_last_price_switched_on_brings_its_adjustment() {
    let fields = [field("MShotSellPriceAdjust")];
    let start: HashMap<&'static str, usize> = [("MShotSellPriceAdjust", 0)].into_iter().collect();
    let deps = Dependents::new(
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &fields,
        &start,
    );
    let bare = own(&[]);
    let out = deps.complete(&point(&[]), &[&bare], &HashMap::new(), &HashMap::new());
    assert!(out.is_empty(), "{out:?}");
    let held = own(&[("MShotSellAtLastPrice", "YES")]);
    let out = deps.complete(&point(&[]), &[&bare], &held, &HashMap::new());
    assert!(out.contains_key("MShotSellPriceAdjust"), "{out:?}");
}
