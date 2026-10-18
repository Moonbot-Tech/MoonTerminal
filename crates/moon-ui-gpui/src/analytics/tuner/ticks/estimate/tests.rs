use std::time::Duration;

use super::{count_text, duration_text, pick_all_label, side_by_side, side_by_side_on};

/// Dividing the first completion by one would inflate a twelve-thread run's ETA twelvefold.
#[test]
fn first_wave_eta_tracks_pool_throughput() {
    use super::{observed_start_cost, remaining_time};
    let elapsed = Duration::from_secs_f64(12.86);
    let truth = 1071.829 - elapsed.as_secs_f64();
    let eta = remaining_time(1, 1000, elapsed, Some(Duration::from_secs(1100)), 12)
        .unwrap()
        .as_secs_f64();
    assert!((truth / 2.0..=truth * 2.0).contains(&eta));
    // The old arithmetic, one restart at a time, overshoots the truth more than fivefold.
    assert!(
        remaining_time(1, 1000, elapsed, None, 1)
            .unwrap()
            .as_secs_f64()
            > truth * 5.0
    );
    let measured = remaining_time(1, 1000, elapsed, None, 12).unwrap();
    assert!((truth / 2.0..=truth * 2.0).contains(&measured.as_secs_f64()));
    assert!((measured.as_secs_f64() - truth).abs() < truth * 0.02);
    assert_eq!(
        remaining_time(1, 1000, elapsed, Some(Duration::from_secs(2400)), 12),
        Some(Duration::from_secs_f64(2387.14))
    );
    let steady = remaining_time(500, 1000, Duration::from_secs_f64(535.9), None, 12).unwrap();
    assert!((steady.as_secs_f64() - 535.9).abs() < 0.001);
    assert_eq!(observed_start_cost(1, elapsed, 1), Some(elapsed));
    assert_eq!(remaining_time(1, 10, elapsed, None, 1), Some(elapsed * 9));
    assert!(
        remaining_time(1, 1000, elapsed, Some(Duration::from_secs(1)), 12).unwrap()
            > Duration::ZERO
    );
}

/// Publishing a partial first wave would poison the next search button's prediction.
#[test]
fn throughput_is_published_only_after_a_wave_or_completion() {
    use super::{CostKey, publishes};
    assert!(!publishes(1, 1000, 12));
    assert!(publishes(12, 1000, 12));
    assert!(publishes(3, 3, 12));
    assert!(publishes(1, 1000, 0));
    let key = CostKey {
        work: 1.0,
        fallback: 1.0,
        model: Default::default(),
        parallel: 12,
    };
    let wave = Duration::from_secs_f64(12.861948);
    assert_eq!(
        super::observed_start_cost(12, wave, 12),
        Some(wave.div_f64(12.0))
    );
    let observed = Some((key, wave.div_f64(12.0)));
    assert!((key.estimate(1000, observed).as_secs_f64() - 1071.829).abs() < 107.1829);
}

/// An estimate that assumed every CPU runs restarts would promise a 32-CPU box finishes ~28%
/// sooner than its 23-thread search pool can: side by side is bounded by the pool, not the box.
#[test]
fn side_by_side_is_bounded_by_the_search_pool() {
    assert_eq!(side_by_side_on(500, 23), 23);
    assert_eq!(side_by_side_on(5, 23), 5);
    assert_eq!(side_by_side_on(0, 23), 1);
    assert_eq!(side_by_side_on(500, 0), 1);
    assert_eq!(
        side_by_side(100_000),
        moon_core::db::tuner::threshold_search::search_threads()
    );
}

/// A separate startup formula would drift from the measured running ETA after sample changes.
#[test]
fn observed_button_and_zero_progress_eta_share_restart_cost() {
    use super::{CostKey, observed_start_cost, remaining_time};
    use moon_core::db::tuner::ticks::{ModelSettings, search::SearchSize};
    let size = SearchSize {
        points: 50_000.0,
        fresh: 500.0,
        exit_fields: 27,
        ..Default::default()
    };
    let key = CostKey::new(293, 70, 500, &size, ModelSettings::default());
    let elapsed = Duration::from_secs(156);
    let observed = observed_start_cost(100, elapsed, 1).unwrap();
    let button = key.estimate(500, Some((key, observed)));
    assert_eq!(button, Duration::from_secs(780));
    assert_eq!(
        remaining_time(0, 500, Duration::ZERO, Some(button), 1),
        Some(button)
    );
    assert_eq!(
        remaining_time(100, 500, elapsed, None, 1),
        Some(Duration::from_secs(624))
    );
    let larger = CostKey::new(586, 70, 500, &size, ModelSettings::default());
    assert_eq!(
        larger.estimate(500, Some((key, observed))),
        Duration::from_secs(1560)
    );
    let wider = CostKey::new(
        293,
        70,
        500,
        &SearchSize {
            points: 100_000.0,
            fresh: 1000.0,
            ..size
        },
        ModelSettings::default(),
    );
    assert_eq!(
        wider.estimate(500, Some((key, observed))),
        Duration::from_secs(1560)
    );
    let shorter = CostKey::new(
        293,
        70,
        250,
        &SearchSize {
            points: 25_000.0,
            fresh: 250.0,
            ..size
        },
        ModelSettings::default(),
    );
    assert_eq!(
        shorter.estimate(250, Some((key, observed))),
        Duration::from_secs(390)
    );
}

/// The first-run fallback must not repeat the reported four-minute underestimate.
#[test]
fn unobserved_reference_is_conservative_and_zero_progress_cannot_calibrate() {
    use super::{CostKey, observed_start_cost};
    use moon_core::db::tuner::ticks::{ModelSettings, search::SearchSize};
    let size = SearchSize {
        points: 50_000.0,
        fresh: 500.0,
        exit_fields: 27,
        ..Default::default()
    };
    let key = CostKey::new(293, 70, 500, &size, ModelSettings::default());
    let reference = Duration::from_secs(780);
    assert!(key.estimate(500, None).as_secs_f64() / reference.as_secs_f64() >= 1.0);
    assert_eq!(observed_start_cost(0, reference, 1), None);
    assert_eq!(observed_start_cost(500, Duration::ZERO, 1), None);
}

/// A missing sample omits the estimate; an available prediction appears once.
#[test]
fn the_search_button_shows_an_available_prediction_once() {
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

/// Rounding an 80-second estimate up to "2 min" makes the button, the card and the tooltip
/// read as twice the real time. Under ten minutes the leftover seconds stay, and a fraction
/// of a second is dropped rather than rounding the minute up. At ten minutes the figure still
/// rounds up to a whole minute, and an hour still reads as hours and minutes.
#[test]
fn a_duration_keeps_seconds_under_ten_minutes() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(duration_text(Duration::from_millis(200)), "1 s");
    assert_eq!(duration_text(Duration::from_secs(45)), "45 s");
    assert_eq!(duration_text(Duration::from_secs(59)), "59 s");
    assert_eq!(duration_text(Duration::from_secs(60)), "1 min");
    assert_eq!(duration_text(Duration::from_secs(80)), "1 min 20 s");
    assert_eq!(duration_text(Duration::from_millis(80_900)), "1 min 20 s");
    assert_eq!(duration_text(Duration::from_secs(599)), "9 min 59 s");
    assert_eq!(duration_text(Duration::from_secs(600)), "10 min");
    assert_eq!(duration_text(Duration::from_secs(601)), "11 min");
    assert_eq!(duration_text(Duration::from_secs(3600)), "1 h 0 min");
    assert_eq!(
        duration_text(Duration::from_secs(3 * 3600 + 5 * 60 + 40)),
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
