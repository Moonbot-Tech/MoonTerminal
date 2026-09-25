use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::super::test_grids::legacy;
use super::super::{DEFAULT_MAX_PASSES, SearchParams};
use super::*;
use crate::db::tuner::ticks::TICK_PARAMS;

/// A MoonShot search with every field locked but `free`.
fn size_of(free: &[&str], restarts: usize) -> SearchSize {
    let held = HashMap::new();
    let defaults = HashMap::new();
    let locked: HashSet<String> = TICK_PARAMS
        .iter()
        .map(|f| f.key.to_string())
        .filter(|k| !free.contains(&k.as_str()))
        .collect();
    search_size(&SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "MoonShot",
        vary_entry: true,
        vary_exit: true,
        locked: &locked,
        grids: legacy(),
        restarts,
        min_n: None,
        seed: Some(1),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        model: ModelSettings::default(),
    })
}

fn span(key: &str) -> f64 {
    let field = TICK_PARAMS.iter().find(|f| f.key == key).expect("a knob");
    (legacy().arity(field) - 1) as f64
}

#[test]
fn one_group_counts_its_passes_and_both_count_an_exit_search_per_entry_point() {
    // The exit alone: three passes over the take's grid, per restart.
    let exit = size_of(&["SellPrice"], 2);
    assert!(!exit.nested());
    assert_eq!(exit.points, 2.0 * 3.0 * span("SellPrice"));
    // The entry alone: its passes, plus the pairs — none with one number field.
    let entry = size_of(&["MShotPrice"], 2);
    assert_eq!(entry.points, 2.0 * 3.0 * span("MShotPrice"));
    // Two Entry number fields: two ordered pairs on top.
    let two = size_of(&["MShotPrice", "MShotPriceMin"], 1);
    assert_eq!(
        two.points,
        3.0 * (span("MShotPrice") + span("MShotPriceMin")) + 2.0
    );
    // Both groups: every entry point runs two passes of the exit.
    let both = size_of(&["MShotPrice", "SellPrice"], 2);
    assert!(both.nested());
    assert_eq!((both.entry_fields, both.exit_fields), (1, 1));
    assert_eq!((entry.entry_fields, entry.exit_fields), (1, 0));
    assert_eq!(both.entry_points, 2.0 * 3.0 * span("MShotPrice"));
    assert_eq!(both.points, both.entry_points * 2.0 * span("SellPrice"));
    // A thousand points at a millisecond each is a second.
    let size = SearchSize {
        points: 1000.0,
        ..SearchSize::default()
    };
    assert_eq!(size.time(Duration::from_millis(1)), Duration::from_secs(1));
}
