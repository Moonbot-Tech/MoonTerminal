//! The search's field dependencies on hand-made strategies.

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
    Dependents::new(&fields, &start)
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
/// timer. Completing the second's timer switches PriceDown on for the first as well, and its per
/// cent is then completed too — whatever order the strategies come in.
#[test]
fn a_completion_reaches_every_strategy_whatever_the_order() {
    let fields = [field("PriceDownTimer"), field("PriceDownPercent")];
    let start: HashMap<&'static str, usize> = [("PriceDownTimer", 3), ("PriceDownPercent", 4)]
        .into_iter()
        .collect();
    let deps = Dependents::new(&fields, &start);
    let off = own(&[("PriceDownTimer", "0")]);
    let pct = own(&[("PriceDownPercent", "10")]);
    for owns in [[&off, &pct], [&pct, &off]] {
        let out = deps.complete(&point(&[]), &owns, &HashMap::new(), &HashMap::new());
        assert!(out.contains_key("PriceDownTimer"), "{out:?}");
        assert!(out.contains_key("PriceDownPercent"), "{out:?}");
    }
}
