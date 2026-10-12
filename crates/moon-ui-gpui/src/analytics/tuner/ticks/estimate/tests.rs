use std::time::Duration;

use super::{count_text, duration_text, pick_all_label};

/// A missing measured cost must not add a guessed duration; a measured duration appears once.
#[test]
fn the_search_button_shows_only_a_measured_time() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(pick_all_label(None), "Search selected");
    assert_eq!(pick_all_label(Some("3 min")), "Search selected · ≈ 3 min");
}

#[test]
fn a_count_keeps_its_thousands_apart() {
    assert_eq!(count_text(0.0), "0");
    assert_eq!(count_text(999.4), "999");
    assert_eq!(count_text(1000.0), "1\u{202f}000");
    assert_eq!(count_text(4_456_380.0), "4\u{202f}456\u{202f}380");
}

#[test]
fn a_duration_reads_in_the_largest_unit_it_needs() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(duration_text(Duration::from_millis(200)), "1 s");
    assert_eq!(duration_text(Duration::from_secs(59)), "59 s");
    assert_eq!(duration_text(Duration::from_secs(61)), "2 min");
    assert_eq!(
        duration_text(Duration::from_secs(3 * 3600 + 5 * 60)),
        "3 h 5 min"
    );
}

/// Using the all-fields size for a one-field run would display the much larger all-fields time.
#[test]
fn a_one_field_estimate_differs_from_the_all_fields_estimate() {
    use moon_core::db::tuner::ticks::params::range::Grids;
    use moon_core::db::tuner::ticks::search::{SearchParams, search_size};
    use moon_core::db::tuner::ticks::{ModelSettings, TICK_PARAMS};
    use std::collections::{HashMap, HashSet};

    let grids = Grids::of([
        ("SellPrice", std::sync::Arc::from([0.7, 1.1, 1.8])),
        ("StopLoss", std::sync::Arc::from([1.3, 2.1, 3.2])),
    ]);
    let held = HashMap::new();
    let defaults = HashMap::new();
    let all_locked = HashSet::new();
    let one_locked = TICK_PARAMS
        .iter()
        .filter(|f| f.key != "SellPrice")
        .map(|f| f.key.to_string())
        .collect();
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "DropsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &all_locked,
        grids: &grids,
        restarts: 7,
        min_n: None,
        seed: Some(31),
        train_frac: 0.7,
        max_passes: 4,
        model: ModelSettings::default(),
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: true,
    };
    let all = search_size(&params);
    let one = search_size(&SearchParams {
        locked: &one_locked,
        ..params
    });
    assert_eq!(one.exit_fields, 1);
    assert!(all.exit_fields > one.exit_fields);
    assert!(all.time(Duration::from_millis(3)) > one.time(Duration::from_millis(3)));
}
