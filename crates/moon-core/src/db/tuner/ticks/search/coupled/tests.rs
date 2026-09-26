//! The Delta Modifiers section as the search walks it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::db::tuner::ticks::TICK_PARAMS;
use crate::db::tuner::ticks::params::{StrategyValues, exit_params};
use crate::db::tuner::ticks::search::{DEFAULT_MAX_PASSES, descend};
use crate::db::tuner::ticks::settings::ModelSettings;

fn field(key: &str) -> &'static TickParam {
    TICK_PARAMS
        .iter()
        .find(|f| f.key == key)
        .expect("a grid field")
}

/// One strategy per base, its values the point laid over `own`.
fn bases(owns: Vec<HashMap<String, String>>) -> impl Fn(&Point) -> Vec<(EntryParams, ExitParams)> {
    move |point: &Point| {
        owns.iter()
            .map(|own| {
                let mut values = own.clone();
                for (k, v) in point {
                    values.insert((*k).to_string(), v.clone());
                }
                let defaults = HashMap::new();
                let sv = StrategyValues {
                    values: &values,
                    defaults: &defaults,
                };
                (
                    EntryParams::Fact,
                    exit_params(&sv, ModelSettings::default()),
                )
            })
            .collect()
    }
}

fn own(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// A term moves nothing while no strategy spends the sum, and something as soon as one does —
/// on the sell, or on a stop that exists; a coefficient moves nothing without a term, the stop's
/// also without a stop.
#[test]
fn a_field_is_inert_only_while_its_partner_is_zero_on_every_strategy() {
    let add = field("Add1minDelta");
    let sell = field("SellModifier");
    let stop = field("StopLossModifier");
    let cap = field("MaxModifier");
    let fields = [sell, stop, cap, add];

    let zero = bases(vec![own(&[("StopLoss", "-2")]), own(&[("StopLoss", "-2")])]);
    let coupling = Coupling::of(&fields, &zero);
    let at = Point::new();
    assert!(coupling.inert(add, &at));
    assert!(coupling.inert(sell, &at));
    assert!(coupling.inert(cap, &at));

    // One of two strategies spends the sum on its sell: the term moves that one.
    let one = bases(vec![
        own(&[("SellModifier", "0.5")]),
        own(&[("StopLoss", "-2")]),
    ]);
    let coupling = Coupling::of(&fields, &one);
    assert!(!coupling.inert(add, &at));
    // …but the coefficient still has no term to spend.
    assert!(coupling.inert(sell, &at));

    // A stop coefficient spends the sum only on a stop that exists.
    let no_stop = bases(vec![own(&[
        ("StopLossModifier", "0.2"),
        ("UseStopLoss", "NO"),
        ("Add1minDelta", "1"),
    ])]);
    let coupling = Coupling::of(&fields, &no_stop);
    assert!(coupling.inert(add, &at));
    assert!(coupling.inert(stop, &at));
    assert!(!coupling.inert(sell, &at), "a term is there to spend");

    // A field outside the section is never inert.
    assert!(!coupling.inert(field("SellPrice"), &at));
}

/// A diagonal spans both grids end to end, off zero, from the smallest product to the largest:
/// the coefficient's grid up and down, the term's up.
#[test]
fn a_diagonal_runs_both_grids_from_the_smallest_step_to_the_largest() {
    let paths = Coupling::diagonals(
        crate::db::tuner::ticks::search::test_grids::legacy(),
        field("SellModifier"),
        field("Add1minDelta"),
    );
    assert_eq!(paths.len(), 2, "up and down");
    let pair = |p: &(String, String)| (p.0.clone(), p.1.clone());
    let s = |a: &str, b: &str| (a.to_string(), b.to_string());
    let up = &paths[0];
    assert_eq!(pair(&up[0]), s("0.03", "0.001"));
    assert_eq!(pair(up.last().expect("a step")), s("1.5", "3"));
    let down = &paths[1];
    assert_eq!(pair(&down[0]), s("-0.05", "0.001"));
    assert_eq!(pair(down.last().expect("a step")), s("-0.5", "3"));
    for path in &paths {
        // As long as the longer grid, so each of its steps is visited once.
        assert_eq!(path.len(), 18);
        assert!(path.iter().all(|(c, t)| c != "0" && t != "0"));
    }
}

/// From the corner where everything is zero, no single move helps and the descent walks the
/// pair together; without the coupling it stays where it began.
#[test]
fn the_descent_leaves_the_zero_corner_along_the_diagonal() {
    let sell = field("SellModifier");
    let add = field("Add1minDelta");
    let per_base = bases(vec![own(&[])]);
    // The best sell is lifted by 0.2 to 1 per cent per one per cent of the 1-minute delta.
    let evaluate = |point: &Point| -> Option<Tally> {
        let (_, exit) = &per_base(point)[0];
        let lift = exit.sell_modifier * exit.sell_mods.add_1m;
        let mut tally = Tally::default();
        tally.push(if (0.2..=1.0).contains(&lift) {
            10.0
        } else {
            1.0
        });
        Some(tally)
    };
    let order = [sell, add];
    let start: HashMap<&'static str, usize> = HashMap::new();
    let coupling = Coupling::of(&order, &per_base);
    let walked = descend(
        Point::new(),
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &order,
        &[],
        &coupling,
        &start,
        &evaluate,
        1,
        DEFAULT_MAX_PASSES,
        &SearchHandle::new(),
    )
    .expect("not stopped");
    let (_, exit) = &per_base(&walked.point)[0];
    let lift = exit.sell_modifier * exit.sell_mods.add_1m;
    assert!((0.2..=1.0).contains(&lift), "{:?}", walked.point);
    assert!((walked.score.expect("scored").profit - 10.0).abs() < 1e-9);

    let alone = descend(
        Point::new(),
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &order,
        &[],
        &Coupling::none(),
        &start,
        &evaluate,
        1,
        DEFAULT_MAX_PASSES,
        &SearchHandle::new(),
    )
    .expect("not stopped");
    assert!(alone.point.is_empty(), "{:?}", alone.point);
}

/// A term no strategy spends is not scanned at all: its grid would be a replay per step that
/// cannot move the score.
#[test]
fn an_inert_term_costs_no_replay() {
    let add = field("Add1minDelta");
    let per_base = bases(vec![own(&[])]);
    let replays = AtomicUsize::new(0);
    let evaluate = |_: &Point| -> Option<Tally> {
        replays.fetch_add(1, Ordering::Relaxed);
        let mut tally = Tally::default();
        tally.push(1.0);
        Some(tally)
    };
    let order = [add];
    // No coefficient is searched, so no diagonal either: only the scan could replay.
    let coupling = Coupling::of(&order, &per_base);
    descend(
        Point::new(),
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &order,
        &[],
        &coupling,
        &HashMap::new(),
        &evaluate,
        1,
        DEFAULT_MAX_PASSES,
        &SearchHandle::new(),
    )
    .expect("not stopped");
    assert_eq!(
        replays.load(Ordering::Relaxed),
        1,
        "only the start is scored"
    );
}

/// A pair is stuck only while its coefficient is off everywhere, the sum has no term, and the
/// coefficient has a level to spend on: a coefficient already set spends through a term's own
/// scan, and a stop coefficient with no stop anywhere is never walked — its value would move
/// nothing and still be written. A stop coefficient that is set does not free the sell's pair.
#[test]
fn a_pair_is_stuck_only_while_its_coefficient_is_off_and_can_spend() {
    let sell = field("SellModifier");
    let stop = field("StopLossModifier");
    let add = field("Add1minDelta");
    let fields = [sell, stop, add];
    let at = Point::new();

    // Everything at zero with a stop: both coefficients can spend once a term is there.
    let corner = bases(vec![own(&[("StopLoss", "-2")])]);
    let coupling = Coupling::of(&fields, &corner);
    assert_eq!(coupling.stuck(&at), vec![(sell, add), (stop, add)]);

    // No stop anywhere: the stop coefficient is not walked.
    let no_stop = bases(vec![own(&[("UseStopLoss", "NO")])]);
    let coupling = Coupling::of(&fields, &no_stop);
    assert_eq!(coupling.stuck(&at), vec![(sell, add)]);

    // The stop spends the sum and no term is set: a term's scan pays through the stop only, so
    // the sell's pair is still walked — the stop's is not.
    let stop_set = bases(vec![own(&[
        ("StopLossModifier", "0.2"),
        ("StopLoss", "-2"),
    ])]);
    let coupling = Coupling::of(&fields, &stop_set);
    assert_eq!(coupling.stuck(&at), vec![(sell, add)]);

    // A term is set: no coefficient is stuck, each moves on its own.
    let term_set = bases(vec![own(&[("Add1minDelta", "1"), ("StopLoss", "-2")])]);
    let coupling = Coupling::of(&fields, &term_set);
    assert!(coupling.stuck(&at).is_empty());
}
