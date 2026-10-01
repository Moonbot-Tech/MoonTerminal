use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::super::test_grids::legacy;
use super::super::{DEFAULT_MAX_PASSES, SearchParams};
use super::*;
use crate::db::tuner::ticks::TICK_PARAMS;

/// A MoonShot search with every field locked but `free`.
fn size_of(free: &[&str], restarts: usize) -> SearchSize {
    size_with(free, restarts, false)
}

/// [`size_of`], screened or not.
fn size_with(free: &[&str], restarts: usize, screen_entry: bool) -> SearchSize {
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
        risk: Default::default(),
        screen_entry,
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
    // A thousand points that replay their entry, at a millisecond each, is a second; read off the
    // fills of a point before, they cost the exit's share.
    let size = SearchSize {
        points: 1000.0,
        fresh: 1000.0,
        ..SearchSize::default()
    };
    assert_eq!(size.time(Duration::from_millis(1)), Duration::from_secs(1));
    let cached = SearchSize { fresh: 0.0, ..size };
    assert!((full_replays(1000.0, 1000.0) - 1000.0 * EXIT_SHARE).abs() < 1e-9);
    assert!(cached.time(Duration::from_millis(1)) < size.time(Duration::from_millis(1)));
    // Who replays anew: every point of an entry search, each exit descent's first under both.
    assert_eq!(entry.fresh, entry.points);
    assert_eq!(exit.fresh, 2.0);
    assert_eq!(both.fresh, both.entry_points);
}

#[test]
fn a_screened_search_scores_every_move_once_and_the_kept_ones_with_their_exit() {
    let free = ["MShotPrice", "MShotPriceMin", "SellPrice"];
    let whole = size_with(&free, 2, false);
    let screened = size_with(&free, 2, true);
    let keep = super::super::screen::KEEP as f64;
    let kept = |key: &str| span(key).min(keep);
    // Two Entry number fields: two ordered pairs, of which the screen keeps both.
    let entry_points =
        2.0 * (3.0 * (kept("MShotPrice") + kept("MShotPriceMin")) + 2.0f64.min(keep));
    assert_eq!(screened.entry_points, entry_points);
    let tried = 2.0 * (3.0 * (span("MShotPrice") + span("MShotPriceMin")) + 2.0);
    assert!(screened.entry_points < whole.entry_points);
    assert_eq!(
        screened.points,
        tried + entry_points * 2.0 * span("SellPrice")
    );
    // One group searched: the screen changes nothing.
    assert_eq!(
        size_with(&["MShotPrice"], 2, true),
        size_of(&["MShotPrice"], 2)
    );
}
