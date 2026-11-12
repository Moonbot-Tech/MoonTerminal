//! What a save wakes, on hand-built strategy cards: no database, no view.

use std::collections::HashMap;

use moon_core::db::tuner::{Bound, StratFilters, Variant, WAKE_EXTRAS};

use super::{plan_slots, warnings, with_woken, woken_by};

/// A found card with these stored bounds, every Ignore flag off.
fn card(bounds: &[(&'static str, Option<f64>, Option<f64>)]) -> StratFilters {
    StratFilters {
        found: true,
        use_bvsv: false,
        bounds: bounds
            .iter()
            .map(|(col, lo, hi)| (*col, (*lo, *hi)))
            .collect(),
        ..StratFilters::default()
    }
}

fn bound(field: &str, from: Option<f64>, to: Option<f64>) -> Bound {
    Bound {
        field: field.to_string(),
        from,
        to,
    }
}

/// The columns a save of `bounds` wakes, in grid order.
fn woken_cols(strat: &StratFilters, bounds: &[Bound]) -> Vec<&'static str> {
    woken_by(strat, bounds, &HashMap::new(), false)
        .iter()
        .map(|w| w.spec.col)
        .collect()
}

/// Fitting one volume bound clears `IgnoreVolume`, and the section's other stored bounds start
/// filtering with it. The what-if must count them — the deals behind the fit were traded with
/// the section asleep — while a side the variant sets itself keeps the variant's number.
#[test]
fn a_fitted_bound_wakes_the_rest_of_its_section() {
    let mut strat = card(&[
        ("hvol", Some(10.0), Some(10_000.0)),
        ("dvol", Some(100.0), Some(100_000.0)),
        ("d3h", Some(0.0), Some(20.0)),
    ]);
    strat.ignore_volume = true;
    let v1 = vec![bound("hvol", Some(50.0), None)];
    assert_eq!(
        woken_cols(&strat, &v1),
        ["hvol", "dvol"],
        "Delta was never asleep"
    );
    let variants = vec![
        Variant::default(),
        Variant {
            bounds: v1,
            ..Variant::default()
        },
    ];
    let laid = with_woken(variants, &strat, &HashMap::new(), false);
    assert!(laid[0].bounds.is_empty(), "the fact takes no bound");
    let side = |field: &str| {
        laid[1]
            .bounds
            .iter()
            .find(|b| b.field == field)
            .map(|b| (b.from, b.to))
    };
    assert_eq!(side("hvol"), Some((Some(50.0), Some(10_000.0))));
    assert_eq!(side("dvol"), Some((Some(100.0), Some(100_000.0))));
    assert_eq!(side("d3h"), None);
}

/// Clearing the master flag wakes every section whose own flag is already off, and none whose
/// own flag stays on.
#[test]
fn the_master_flag_wakes_only_sections_not_ignored_on_their_own() {
    let mut strat = card(&[
        ("dvol", Some(100.0), Some(100_000.0)),
        ("d3h", Some(0.0), Some(20.0)),
        ("lev", Some(1.0), None),
    ]);
    strat.ignore_filters = true;
    strat.ignore_base = true;
    let v1 = vec![bound("hvol", Some(50.0), None)];
    assert_eq!(woken_cols(&strat, &v1), ["dvol", "d3h"]);
    // Nothing written, nothing cleared: a variant with no bound wakes nothing.
    assert!(woken_cols(&strat, &[]).is_empty());
    // An unknown card wakes nothing either.
    strat.found = false;
    assert!(woken_cols(&strat, &v1).is_empty());
}

/// A slot the save leaves alone wakes as it is. A slot the save hands to another field keeps
/// the side that field does not set — the save writes the type and only the sides it has — so
/// the leftover wakes as a bound of the NEW field, and the what-if and the dialog both say so.
#[test]
fn a_slot_given_to_another_field_wakes_its_leftover_side_on_that_field() {
    let _locale = crate::test_locale::force("en");
    let mut strat = card(&[]);
    strat.ignore_delta = true;
    strat.slots = vec![(2, "d1h", Some(0.0), Some(20.0))];
    assert_eq!(
        woken_cols(&strat, &[bound("d3h", None, Some(15.0))]),
        ["d1h"]
    );
    // d15m takes the first slot nobody foreign holds — Delta2, d1h's — and sets only a cap.
    let v1 = vec![bound("d15m", None, Some(5.0))];
    assert_eq!(woken_cols(&strat, &v1), ["d15m"]);
    let laid = with_woken(
        vec![
            Variant::default(),
            Variant {
                bounds: v1,
                ..Variant::default()
            },
        ],
        &strat,
        &HashMap::new(),
        false,
    );
    let d15m = laid[1].bounds.iter().find(|b| b.field == "d15m").unwrap();
    assert_eq!((d15m.from, d15m.to), (Some(0.0), Some(5.0)));
    assert!(laid[1].bounds.iter().all(|b| b.field != "d1h"));
    // The dialog names the side the save leaves behind, not the one it writes.
    let changes: Vec<(String, String)> = [
        ("Delta2_Type", "15m"),
        ("Delta2_Max", "5"),
        ("IgnoreDelta", "NO"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    let lines = warnings(&strat, &changes, false);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("Delta2_Min = 0"), "{lines:?}");
    assert!(!lines[0].contains("Delta2_Max"), "{lines:?}");
}

/// An empty column is no what-if: staged toggles alone lay nothing under it, while the same
/// toggles under a typed bound do.
#[test]
fn an_empty_variant_takes_no_woken_bound() {
    let mut strat = card(&[("dvol", Some(100.0), Some(100_000.0))]);
    strat.ignore_volume = true;
    let staged: HashMap<&'static str, bool> = [("IgnoreVolume", false)].into();
    let variants = vec![
        Variant::default(),
        Variant::default(),
        Variant {
            bounds: vec![bound("d3h", None, Some(15.0))],
            ..Variant::default()
        },
    ];
    let laid = with_woken(variants, &strat, &staged, false);
    assert!(laid[1].bounds.is_empty());
    assert!(laid[2].bounds.iter().any(|b| b.field == "dvol"));
}

/// A slot of a type the report has no column for wakes with the deltas; the dialog names it by
/// type unless this save takes the slot, and prints stored numbers unrounded.
#[test]
fn the_dialog_names_a_foreign_slot_and_keeps_small_numbers() {
    let _locale = crate::test_locale::force("en");
    let step = WAKE_EXTRAS
        .iter()
        .find(|extra| extra.key == "PriceStepMax")
        .expect("the Price group's step");
    let mut strat = card(&[]);
    strat.ignore_filters = true;
    strat.foreign_slots = vec![(3, "2h".to_string())];
    strat.extras = vec![(step, 0.00004)];
    let changes: Vec<(String, String)> = [("MinVolume", "200"), ("IgnoreFilters", "NO")]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let lines = warnings(&strat, &changes, false);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("PriceStepMax = 0.00004"), "{lines:?}");
    assert!(lines[0].contains("Delta3_Type = 2h"), "{lines:?}");
    let mut taking = changes.clone();
    taking.push(("Delta3_Type".to_string(), "15m".to_string()));
    assert!(!warnings(&strat, &taking, false)[0].contains("Delta3_Type"));
}

/// The slot rule the save has always had: a field keeps its own slot, a free slot comes before
/// a foreign one, a foreign one is overwritten last and reported, and a third field is dropped.
#[test]
fn slots_keep_their_field_then_fill_free_then_foreign() {
    let mut strat = card(&[]);
    strat.slots = vec![(3, "d5m", None, None)];
    strat.foreign_slots = vec![(2, "2h".to_string())];
    let plan = plan_slots(
        &strat,
        &[
            ("d15m", Some(1.0), None),
            ("d5m", None, Some(2.0)),
            ("d1m", Some(3.0), None),
        ],
    );
    assert_eq!(
        plan.assigned,
        vec![(3, "d5m", None, Some(2.0)), (2, "d15m", Some(1.0), None)]
    );
    assert_eq!(plan.replaced, vec![(2, "2h".to_string(), "d15m")]);
    assert_eq!(plan.dropped, vec!["d1m"]);
    // With both slots free the fields take them in order.
    let free = plan_slots(
        &card(&[]),
        &[("d15m", Some(1.0), None), ("d5m", None, None)],
    );
    assert_eq!(
        free.assigned,
        vec![(2, "d15m", Some(1.0), None), (3, "d5m", None, None)]
    );
}

/// The dialog names the stored bounds a cleared flag wakes and this save does not rewrite, by
/// the strategy's own parameter names, and the limits outside the grid — but not one whose own
/// group flag keeps it asleep, and not the anchor's numbers on a bulk save.
#[test]
fn the_dialog_lists_what_wakes_and_the_save_does_not_write() {
    let _locale = crate::test_locale::force("en");
    let penalty = WAKE_EXTRAS
        .iter()
        .find(|extra| extra.key == "PenaltyTime")
        .expect("the Time group's pause");
    let mut strat = card(&[
        ("hvol", Some(10.0), Some(10_000.0)),
        ("dvol", Some(100.0), Some(50_000.0)),
    ]);
    strat.ignore_filters = true;
    strat.ignore_volume = true;
    strat.factory.insert("hvol");
    strat.extras = vec![(penalty, 300.0)];
    let changes: Vec<(String, String)> = [
        ("MinHourlyVolume", "50"),
        ("IgnoreFilters", "NO"),
        ("IgnoreVolume", "NO"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    let lines = warnings(&strat, &changes, false);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("MaxHourlyVolume = 10000"), "{lines:?}");
    assert!(
        !lines[0].contains("MinHourlyVolume"),
        "the save writes that side itself: {lines:?}"
    );
    assert!(
        lines[0].contains("MinVolume = 100, MaxVolume = 50000"),
        "{lines:?}"
    );
    assert!(lines[1].contains("PenaltyTime = 300"), "{lines:?}");
    // The Time group asleep under its own flag does not wake with the master one.
    strat.other_ignores.insert("IgnoreTime");
    assert_eq!(warnings(&strat, &changes, false).len(), 1);
    // A bulk save says one thing and prints none of the anchor's numbers.
    let bulk = warnings(&strat, &changes, true);
    assert_eq!(bulk.len(), 1);
    assert!(!bulk[0].contains("10000"), "{bulk:?}");
    // No flag cleared, nothing to say — in either dialog.
    let quiet = vec![("MinHourlyVolume".to_string(), "50".to_string())];
    assert!(warnings(&strat, &quiet, false).is_empty());
    assert!(warnings(&strat, &quiet, true).is_empty());
}
