use std::collections::HashMap;

use super::super::test_grids::legacy;
use super::super::{DEFAULT_MAX_PASSES, grid_index};
use super::*;
use crate::db::tuner::ticks::TICK_PARAMS;

fn field(key: &str) -> &'static TickParam {
    TICK_PARAMS.iter().find(|f| f.key == key).expect("a knob")
}

/// A score over one entry field and one exit field, by their grid steps: the start (10, 5) scores
/// 5; the exit alone a step up, (10, 6), scores 6; the entry a step up under that exit, (11, 6),
/// scores 4 — worse — and only with its own exit, (11, 7), does it score 10. Anything else scores 1.
fn objective<'a>(
    entry: &'static TickParam,
    exit: &'static TickParam,
    start: &'a HashMap<&'static str, usize>,
) -> impl Fn(&Point) -> Option<Tally> + Sync + 'a {
    move |point: &Point| {
        let at = (
            grid_index(legacy(), entry, point, start).expect("entry"),
            grid_index(legacy(), exit, point, start).expect("exit"),
        );
        let mut tally = Tally::default();
        tally.push(match at {
            (10, 5) => 5.0,
            (10, 6) => 6.0,
            (11, 6) => 4.0,
            (11, 7) => 10.0,
            _ => 1.0,
        });
        Some(tally)
    }
}

/// The entry that only pays with its own exit: a descent of both groups settles on the exit's
/// own step (6) — the entry move is judged under the exit tuned for the old entry and reads as
/// worse — where the nested one, searching the exit under every entry point, reaches 10.
#[test]
fn the_nested_descent_reaches_an_entry_that_pays_only_with_its_own_exit() {
    let entry = field("MShotPrice");
    let exit = field("SellPrice");
    let start: HashMap<&'static str, usize> = [(entry.key, 10), (exit.key, 5)].into();
    let evaluate = objective(entry, exit, &start);
    let handle = SearchHandle::new();
    let none = coupled::Coupling::none();
    let searched = std::sync::atomic::AtomicUsize::new(0);
    let nested = Nested {
        grids: legacy(),
        start: &start,
        entry: &[entry],
        exit: &[exit],
        pairs: &[],
        entry_coupling: &none,
        exit_coupling: &none,
        min_n: 1,
        max_passes: DEFAULT_MAX_PASSES,
        handle: &handle,
        refused: &|_: &Point| false,
        searched: &searched,
    };
    let walked = descend_nested(Point::new(), &nested, &evaluate).expect("not stopped");
    let at = (
        grid_index(legacy(), entry, &walked.point, &start),
        grid_index(legacy(), exit, &walked.point, &start),
    );
    assert_eq!(at, (Some(11), Some(7)), "{:?}", walked.point);
    assert!((walked.score.expect("scored").profit - 10.0).abs() < 1e-9);
    // Every entry point the outer descent scored ran an exit search, and said so — once each: a
    // point scored again in the restart is the one already found.
    let searched = searched.load(std::sync::atomic::Ordering::Relaxed);
    assert!(searched > 1, "{searched}");
    assert_eq!(handle.points(), searched);
    assert!(
        searched < legacy().arity(entry) * 3,
        "each entry value once per pass at most: {searched}"
    );
    // The flat descent over both groups stops on the exit's own step: the defect this fixes.
    let flat = descend(
        Point::new(),
        legacy(),
        &[entry, exit],
        &[],
        &none,
        &start,
        &evaluate,
        1,
        DEFAULT_MAX_PASSES,
        &SearchHandle::new(),
    )
    .expect("not stopped");
    assert!((flat.score.expect("scored").profit - 6.0).abs() < 1e-9);
}

/// A stop inside the inner search stops the whole descent: nothing is answered.
#[test]
fn a_stopped_nested_descent_answers_nothing() {
    let entry = field("MShotPrice");
    let exit = field("SellPrice");
    let start: HashMap<&'static str, usize> = [(entry.key, 10), (exit.key, 5)].into();
    let evaluate = objective(entry, exit, &start);
    let handle = SearchHandle::new();
    handle.cancel();
    let none = coupled::Coupling::none();
    let nested = Nested {
        grids: legacy(),
        start: &start,
        entry: &[entry],
        exit: &[exit],
        pairs: &[],
        entry_coupling: &none,
        exit_coupling: &none,
        min_n: 1,
        max_passes: DEFAULT_MAX_PASSES,
        handle: &handle,
        refused: &|_: &Point| false,
        searched: &std::sync::atomic::AtomicUsize::new(0),
    };
    assert!(descend_nested(Point::new(), &nested, &evaluate).is_none());
}
