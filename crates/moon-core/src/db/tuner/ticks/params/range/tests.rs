//! The automatic search ranges and the typed ones over them.

use std::collections::HashMap;

use super::*;
use crate::db::tuner::LiveStrategy;
use crate::feed::strategy_deps::FieldDeps;

fn live(kind: &str, values: &[(&str, &str)]) -> LiveStrategy {
    LiveStrategy {
        kind: kind.to_string(),
        values: values
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), (*v).to_string()))
            .collect(),
    }
}

fn span(lo: f64, hi: f64, decimals: u32, selected: &[f64]) -> FieldSpan {
    FieldSpan {
        lo,
        hi,
        decimals,
        selected: selected.to_vec(),
    }
}

/// The plan's own example (2026-09-25): MoonShot's corridor p5 0.7, p95 5.0 on this machine, its
/// values to the hundredth, a selected strategy at 1.7, twenty steps — a round step of 0.25 from
/// 0.5 to 5, and the strategy's own 1.7 in the grid exactly, not snapped to 1.75.
#[test]
fn the_corridor_is_cut_into_round_steps_and_keeps_the_strategys_own_value() {
    // Ten at each end, eighty between them, with the hundredths live corridors carry.
    let population: Vec<f64> = [0.7; 10]
        .into_iter()
        .chain((0..80).map(|i| [1.25, 2.0, 3.35, 4.5][i % 4]))
        .chain([5.0; 10])
        .collect();
    let s = field_span(&population, Some(0.0), &[1.7]).expect("a span");
    // The default 0 widens the span down.
    assert_eq!(s.lo, 0.0);
    let s = field_span(&population, None, &[1.7]).expect("a span");
    assert_eq!((s.lo, s.hi, s.decimals), (0.7, 5.0, 2), "{s:?}");
    let resolved = resolve(Some(&s), &TickRange::default(), false, 20);
    let shown = resolved.shown.expect("shown");
    assert_eq!(shown.step, 0.25);
    assert_eq!(shown.from, 0.5);
    assert_eq!(shown.to, 5.0);
    assert!(resolved.points.contains(&1.7));
    assert!(resolved.points.len() <= 20 + 1, "{}", resolved.points.len());
    assert!(
        resolved.points.iter().all(|v| format!("{v}").len() <= 4),
        "{:?}",
        resolved.points
    );
}

/// A field 5 % of the strategies set, the rest at its default of 0: the tails are cut over the
/// values set, so the range still reaches them, and the default stays in.
#[test]
fn a_field_few_strategies_set_is_ranged_over_those_that_set_it() {
    let mut strategies: Vec<LiveStrategy> = (0..57)
        .map(|i| {
            let value = if i % 2 == 0 { "0.001" } else { "0.0015" };
            live("MoonShot", &[("MShotAdd24hDelta", value)])
        })
        .collect();
    strategies.extend((0..1000).map(|_| live("MoonShot", &[])));
    let defaults = HashMap::from([("mshotadd24hdelta".to_string(), 0.0)]);
    let population = Population::of(&strategies, &defaults, &FieldDeps::bundled());
    let values = population.values(&["MoonShot".to_string()], "MShotAdd24hDelta");
    assert_eq!(values.len(), 57, "only the strategies that set it");
    let s = field_span(&values, Some(0.0), &[]).expect("a span");
    assert_eq!(s.lo, 0.0);
    assert_eq!(s.hi, 0.0015);
    assert_eq!(s.decimals, 4);
}

/// A stop per cent left in the dump behind `UseStopLoss = NO` says nothing about stops: +50 on
/// this machine (2026-09-25) would stretch the range past every stop in use.
#[test]
fn a_value_behind_a_switch_that_is_off_is_not_counted() {
    let strategies = vec![
        live("MoonShot", &[("UseStopLoss", "YES"), ("StopLoss", "-3")]),
        live("MoonShot", &[("UseStopLoss", "NO"), ("StopLoss", "50")]),
        // No switch in the dump: the model's fallback, on.
        live("MoonShot", &[("StopLoss", "-1")]),
    ];
    let population = Population::of(&strategies, &HashMap::new(), &FieldDeps::bundled());
    let mut values = population.values(&[], "StopLoss");
    values.sort_by(f64::total_cmp);
    assert_eq!(values, vec![-3.0, -1.0]);
}

/// A kind with a handful of values is ranged over every kind: its own percentiles would be its
/// few values.
#[test]
fn a_kind_with_few_values_takes_every_kind() {
    let mut strategies: Vec<LiveStrategy> = (0..30)
        .map(|i| live("MoonShot", &[("SellPrice", &format!("{}", 1 + i % 3))]))
        .collect();
    strategies.push(live("Drops", &[("SellPrice", "9")]));
    let population = Population::of(&strategies, &HashMap::new(), &FieldDeps::bundled());
    assert_eq!(
        population
            .values(&["MoonShot".to_string()], "SellPrice")
            .len(),
        30
    );
    assert_eq!(
        population.values(&["Drops".to_string()], "SellPrice").len(),
        31
    );
}

/// Negative fields are cut the same way: a stop from −5 to −0.1. Its values carry tenths, so the
/// round step past the raw 0.26 is 0.5 — 0.25 would put hundredths into a field that holds none.
#[test]
fn a_negative_range_is_cut_into_steps_below_zero() {
    let s = span(-5.0, -0.1, 1, &[-1.5]);
    let resolved = resolve(Some(&s), &TickRange::default(), false, 20);
    let shown = resolved.shown.expect("shown");
    assert_eq!(shown.step, 0.5);
    assert!(shown.from <= -5.0, "{shown:?}");
    // The top edge stays where the span ends: snapped out to a step it would be 0, "no stop".
    assert_eq!(shown.to, -0.1);
    assert!(
        resolved.points.iter().all(|v| *v < 0.0),
        "{:?}",
        resolved.points
    );
    assert!(resolved.points.contains(&-1.5) && resolved.points.contains(&-0.1));
}

/// The same on the other side: a corridor all above zero never starts at a corridor of nothing.
#[test]
fn a_positive_range_never_starts_at_zero() {
    let s = span(0.05, 8.0, 2, &[]);
    let resolved = resolve(Some(&s), &TickRange::default(), false, 20);
    let shown = resolved.shown.expect("shown");
    assert_eq!(shown.from, 0.05);
    assert!(
        resolved.points.iter().all(|v| *v > 0.0),
        "{:?}",
        resolved.points
    );
    // The rest stays on the round steps; the kept edge is tried beside them.
    let step = shown.step;
    let off: Vec<f64> = resolved
        .points
        .iter()
        .copied()
        .filter(|v| ((v / step) - (v / step).round()).abs() > 1e-9)
        .collect();
    assert_eq!(off, vec![0.05], "{:?}", resolved.points);
}

/// A typed "to" alone keeps the automatic "from" and its round steps.
#[test]
fn a_typed_top_alone_keeps_the_round_steps() {
    let s = span(0.05, 8.0, 2, &[]);
    let typed = TickRange {
        to: Some(4.0),
        ..TickRange::default()
    };
    let resolved = resolve(Some(&s), &typed, false, 20);
    let step = resolved.shown.expect("shown").step;
    assert!(
        resolved
            .points
            .iter()
            .filter(|v| **v != 0.05)
            .all(|v| ((v / step) - (v / step).round()).abs() < 1e-9),
        "{:?}",
        resolved.points
    );
}

/// A strategy's own value keeps every digit it has, past the grid's precision too.
#[test]
fn a_selected_value_joins_the_grid_exactly() {
    let s = span(0.0, 1.0, 2, &[0.1234567]);
    let resolved = resolve(Some(&s), &TickRange::default(), false, 20);
    assert!(
        resolved.points.contains(&0.1234567),
        "{:?}",
        resolved.points
    );
}

/// Large edges with a fine typed step: the division's noise does not add a point past the cap.
#[test]
fn a_fine_step_over_large_values_is_counted_right() {
    let s = span(1000.0, 1001.0, 5, &[]);
    let typed = TickRange {
        from: Some(1000.0),
        to: Some(1000.00199),
        step: Some(0.00001),
    };
    let resolved = resolve(Some(&s), &typed, false, 20);
    assert_eq!(resolved.error, None);
    assert_eq!(resolved.points.len(), 200);
}

/// A span that does cross zero keeps it: there 0 is one of the values in use.
#[test]
fn a_range_across_zero_keeps_zero() {
    let s = span(-0.5, 2.0, 1, &[]);
    let resolved = resolve(Some(&s), &TickRange::default(), false, 20);
    assert!(resolved.points.contains(&0.0), "{:?}", resolved.points);
}

/// Both typed edges are tried, the top one too when the step does not land on it; the count cap
/// counts it.
#[test]
fn a_typed_top_edge_off_the_step_is_tried_too() {
    let s = span(0.0, 10.0, 1, &[]);
    let typed = TickRange {
        from: Some(1.0),
        to: Some(3.0),
        step: Some(0.7),
    };
    let resolved = resolve(Some(&s), &typed, false, 20);
    assert_eq!(&*resolved.points, &[1.0, 1.7, 2.4, 3.0]);
    let at_cap = TickRange {
        from: Some(0.0),
        to: Some(199.5),
        step: Some(1.0),
    };
    // 200 steps and the edge off them: 201 points, one past the cap.
    assert_eq!(
        resolve(Some(&s), &at_cap, false, 20).error,
        Some(RangeError::TooMany)
    );
}

/// Large values with fine digits keep their neighbours apart: points are compared as they spell.
#[test]
fn close_points_of_large_values_are_not_merged() {
    let s = span(1000.0, 1000.00002, 6, &[]);
    let resolved = resolve(Some(&s), &TickRange::default(), false, 3);
    assert_eq!(resolved.points.len(), 3, "{:?}", resolved.points);
}

/// A step typed alone on a field nothing is known of has nothing to cut: said, not swallowed.
#[test]
fn a_step_alone_with_nothing_known_is_refused() {
    let typed = TickRange {
        step: Some(1.0),
        ..TickRange::default()
    };
    let resolved = resolve(None, &typed, false, 20);
    assert_eq!(resolved.error, Some(RangeError::NoEdges));
    assert!(resolved.points.is_empty());
}

/// A step never finer than the field's precision: whole-number values give whole steps.
#[test]
fn a_whole_number_field_steps_by_whole_numbers() {
    let s = span(0.0, 5.0, 0, &[]);
    let resolved = resolve(Some(&s), &TickRange::default(), false, 20);
    assert_eq!(resolved.shown.expect("shown").step, 1.0);
    assert_eq!(&*resolved.points, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
}

/// A step of 0.1 adds up without the float's tail: `0.30000000000000004` would be written to a
/// strategy as it spells.
#[test]
fn a_tenth_step_spells_clean_values() {
    let s = span(0.0, 1.0, 1, &[]);
    let resolved = resolve(Some(&s), &TickRange::default(), false, 11);
    assert_eq!(resolved.shown.expect("shown").step, 0.1);
    let spelled: Vec<String> = resolved.points.iter().map(|v| spell_number(*v)).collect();
    assert_eq!(
        spelled,
        vec![
            "0", "0.1", "0.2", "0.3", "0.4", "0.5", "0.6", "0.7", "0.8", "0.9", "1"
        ]
    );
}

/// The developer's question of 2026-09-25: a step finer than the field holds must give each value
/// once — a step of 0.3 on an integer field from 1 to 3 is 1, 2, 3, not 1, 1, 2, 2, 2, 3.
#[test]
fn a_step_finer_than_an_integer_field_gives_each_value_once() {
    let s = span(0.0, 10.0, 0, &[]);
    let typed = TickRange {
        from: Some(1.0),
        to: Some(3.0),
        step: Some(0.3),
    };
    let resolved = resolve(Some(&s), &typed, true, 20);
    assert_eq!(resolved.error, None);
    assert_eq!(&*resolved.points, &[1.0, 2.0, 3.0]);
}

/// On a field that is not an integer, the typed step's own precision is kept.
#[test]
fn a_typed_step_finer_than_the_values_holds_on_a_decimal_field() {
    let s = span(10.0, 60.0, 0, &[]);
    let typed = TickRange {
        from: Some(10.0),
        to: Some(11.0),
        step: Some(0.5),
    };
    let resolved = resolve(Some(&s), &typed, false, 20);
    assert_eq!(&*resolved.points, &[10.0, 10.5, 11.0]);
}

/// No grid ever holds a value twice, whatever the step and the selected values.
#[test]
fn no_grid_holds_a_value_twice() {
    let s = span(0.0, 1.0, 2, &[0.25, 0.5, 0.5]);
    for steps in [3, 7, 20, 200] {
        let resolved = resolve(Some(&s), &TickRange::default(), false, steps);
        let points = &resolved.points;
        assert!(
            points.windows(2).all(|w| w[0] < w[1]),
            "{steps}: {points:?}"
        );
    }
}

/// A typed "to" alone keeps the automatic "from" and cuts a new step over the new span.
#[test]
fn a_typed_edge_alone_recuts_the_step() {
    let s = span(0.0, 10.0, 1, &[]);
    let typed = TickRange {
        to: Some(2.0),
        ..TickRange::default()
    };
    let resolved = resolve(Some(&s), &typed, false, 20);
    let shown = resolved.shown.expect("shown");
    assert_eq!((shown.from, shown.to), (0.0, 2.0));
    assert_eq!(
        shown.step, 0.2,
        "one step of the automatic 0.5 would leave five points"
    );
}

/// A typed range the search cannot use is set aside for the automatic one, and says why.
#[test]
fn an_unusable_typed_range_falls_back_to_automatic() {
    let s = span(0.0, 10.0, 1, &[]);
    let auto = resolve(Some(&s), &TickRange::default(), false, 20);
    let inverted = TickRange {
        from: Some(5.0),
        to: Some(1.0),
        step: None,
    };
    let zero = TickRange {
        step: Some(0.0),
        ..TickRange::default()
    };
    let many = TickRange {
        from: Some(0.0),
        to: Some(1000.0),
        step: Some(1.0),
    };
    for (typed, error) in [
        (inverted, RangeError::Inverted),
        (zero, RangeError::BadStep),
        (many, RangeError::TooMany),
    ] {
        let resolved = resolve(Some(&s), &typed, false, 20);
        assert_eq!(resolved.error, Some(error));
        assert_eq!(resolved.points, auto.points);
    }
}

/// Nothing known of a field — no strategy, no default, no selection — gives no span, and no grid
/// the search could vary.
#[test]
fn a_field_nothing_is_known_of_has_no_grid() {
    assert_eq!(field_span(&[], None, &[]), None);
    let resolved = resolve(None, &TickRange::default(), false, 20);
    assert!(resolved.points.is_empty() && resolved.shown.is_none());
}

/// Everyone at one value is a one-point grid, not a range of nothing.
#[test]
fn one_value_everywhere_is_a_one_point_grid() {
    let s = field_span(&[3.0, 3.0], Some(3.0), &[3.0]).expect("a span");
    let resolved = resolve(Some(&s), &TickRange::default(), false, 20);
    assert_eq!(&*resolved.points, &[3.0]);
    assert_eq!(resolved.shown.expect("shown").step, 0.0);
}

/// Every number knob of the axis gets a grid off an ordinary set of live strategies — the guard
/// against a knob added with a key no strategy spells: its row would search nothing.
#[test]
fn every_number_knob_is_ranged_off_strategies_that_set_it() {
    let strategies: Vec<LiveStrategy> = (0..25)
        .map(|i| {
            let values: Vec<(String, String)> = number_fields()
                .map(|f| (f.key.to_string(), format!("{}", 1 + i % 5)))
                .chain(
                    [
                        "UseStopLoss",
                        "UseSecondStop",
                        "UseStopLoss3",
                        "UseTrailing",
                        "UseTakeProfit",
                        "MShotSellAtLastPrice",
                    ]
                    .map(|k| (k.to_string(), "YES".to_string())),
                )
                .collect();
            let borrowed: Vec<(&str, &str)> = values
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            live("MoonShot", &borrowed)
        })
        .collect();
    let population = Population::of(&strategies, &HashMap::new(), &FieldDeps::bundled());
    for field in number_fields() {
        let values = population.values(&["MoonShot".to_string()], field.key);
        let s = field_span(&values, None, &[]);
        let points = resolve(s.as_ref(), &TickRange::default(), false, DEFAULT_STEPS).points;
        assert!(points.len() >= 2, "{}: {points:?}", field.key);
    }
}

/// The grid spells what the strategy format expects: no fraction on a whole number, no trailing
/// noise, no negative zero.
#[test]
fn the_grid_spells_numbers_as_a_strategy_does() {
    let grids = Grids::of([("SellPrice", Arc::from(vec![0.0, 1.0, 1.5, -0.25]))]);
    let field = TICK_PARAMS
        .iter()
        .find(|f| f.key == "SellPrice")
        .expect("a knob");
    assert_eq!(grids.arity(field), 4);
    let spelled: Vec<String> = (0..4).map(|i| grids.spell(field, i)).collect();
    assert_eq!(spelled, vec!["0", "1", "1.5", "-0.25"]);
    assert_eq!(snap(-0.0000001, 2), 0.0);
    assert_eq!(spell_number(snap(-0.0000001, 2)), "0");
}
