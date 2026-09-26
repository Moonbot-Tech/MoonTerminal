use super::*;
use moon_core::feed::{SchemaField, SchemaFieldUi};

fn section(title: &str, names: &[&str]) -> SchemaSection {
    SchemaSection {
        title: title.to_string(),
        fields: names
            .iter()
            .map(|name| SchemaField {
                name: name.to_string(),
                type_name: "Double".to_string(),
                ui: SchemaFieldUi::Edit,
                picklist: Vec::new(),
                default: None,
            })
            .collect(),
    }
}

fn keys(grid: &GridSection) -> Vec<&str> {
    grid.rows.iter().map(|r| r.key.as_str()).collect()
}

fn find(out: &[GridSection], s: ParamSection) -> &GridSection {
    out.iter().find(|g| g.section == s).expect("section")
}

/// Every field of `kinds` in use, lowercase: the layout as it stood before rows were chosen.
fn all_used(kinds: &[&[SchemaSection]]) -> HashSet<String> {
    kinds
        .iter()
        .flat_map(|k| k.iter())
        .flat_map(|s| s.fields.iter())
        .map(|f| f.name.to_ascii_lowercase())
        .collect()
}

fn used(names: &[&str]) -> HashSet<String> {
    names.iter().map(|n| n.to_ascii_lowercase()).collect()
}

#[test]
fn every_section_comes_out_in_grid_order() {
    let out = layout(&[], &[], &HashSet::new());
    let order: Vec<ParamSection> = out.iter().map(|g| g.section).collect();
    assert_eq!(order, ParamSection::GRID_ORDER);
    assert!(out.iter().all(|g| g.rows.is_empty()));
}

#[test]
fn the_schema_places_every_field_and_marks_what_the_model_turns() {
    let knobs = scope_knobs(&["MoonShot".to_string()]);
    // The core's own spellings: a backslash and a doubled space do not split a section off.
    let kind = vec![
        section(
            "Strategy  settings",
            &["MShotPrice", "MShotRepeatWait", "MShotSellAtLastPrice"],
        ),
        section(
            "Sell order\\SellShot",
            &["IgnoreSellShot", "SellShotPriceDown"],
        ),
        section(
            "Sell order\\SellSpread",
            &["IgnoreSellSpread", "SellSpreadDistance"],
        ),
        section(
            "Stops",
            &[
                "UseStopLoss",
                "StopLoss",
                "UseTrailing",
                "TrailingPercent",
                "TrailingSpread",
            ],
        ),
        section("Filters", &["MinVolume"]),
    ];
    let out = layout(&[kind.as_slice()], &knobs, &all_used(&[kind.as_slice()]));

    let settings = find(&out, ParamSection::StrategySettings);
    assert_eq!(
        keys(settings)[..3],
        ["MShotPrice", "MShotRepeatWait", "MShotSellAtLastPrice"]
    );
    assert!(matches!(settings.rows[0].role, RowRole::Knob(p) if p.key == "MShotPrice"));
    assert_eq!(settings.rows[1].role, RowRole::Outside);

    // SellShot and SellSpread are not modelled: every field is drawn, none of them live, and no
    // knob follows under them.
    for (s, names) in [
        (
            ParamSection::SellShot,
            ["IgnoreSellShot", "SellShotPriceDown"],
        ),
        (
            ParamSection::SellSpread,
            ["IgnoreSellSpread", "SellSpreadDistance"],
        ),
    ] {
        let grid = find(&out, s);
        assert_eq!(keys(grid), names);
        assert!(grid.rows.iter().all(|r| r.role == RowRole::Unmodelled));
        assert_eq!(grid.knobs().count(), 0);
    }

    let stops = find(&out, ParamSection::Stops);
    assert_eq!(
        keys(stops)[..5],
        [
            "UseStopLoss",
            "StopLoss",
            "UseTrailing",
            "TrailingPercent",
            "TrailingSpread"
        ]
    );
    // The stop's switch and the trailing stop are knobs since 2026-09-24; the trailing's spread is
    // the sale's, not the model's.
    assert!(matches!(stops.rows[0].role, RowRole::Knob(p) if p.key == "UseStopLoss"));
    assert!(matches!(stops.rows[3].role, RowRole::Knob(p) if p.key == "TrailingPercent"));
    assert_eq!(stops.rows[4].role, RowRole::Outside);

    // A section outside the grid is not drawn.
    assert!(
        out.iter()
            .flat_map(|g| g.rows.iter())
            .all(|r| r.key != "MinVolume")
    );
}

#[test]
fn a_knob_the_schema_does_not_place_goes_under_its_own_section_once() {
    let knobs = scope_knobs(&["MoonShot".to_string()]);
    let out = layout(&[], &knobs, &HashSet::new());
    for knob in &knobs {
        let at: Vec<ParamSection> = out
            .iter()
            .filter(|g| g.rows.iter().any(|r| r.key == knob.key))
            .map(|g| g.section)
            .collect();
        assert_eq!(at, [knob.section], "{}", knob.key);
    }
    // With the schema, the field keeps the schema's place and is not drawn a second time.
    let kind = vec![section("Sell order", &["SellPrice"])];
    let out = layout(&[kind.as_slice()], &knobs, &HashSet::new());
    let placed: usize = out
        .iter()
        .map(|g| g.rows.iter().filter(|r| r.key == "SellPrice").count())
        .sum();
    assert_eq!(placed, 1);
}

#[test]
fn two_kinds_share_a_section_without_repeating_a_field() {
    let knobs = scope_knobs(&["MoonShot".to_string(), "MoonHook".to_string()]);
    let shot = vec![section("Stops", &["UseStopLoss", "StopLoss"])];
    let hook = vec![section("Stops", &["StopLoss", "StopLossDelay"])];
    let kinds = [shot.as_slice(), hook.as_slice()];
    let out = layout(&kinds, &knobs, &all_used(&kinds));
    let stops = keys(find(&out, ParamSection::Stops));
    // The two kinds' fields first, each once; the section's knobs no schema here places follow.
    assert_eq!(stops[..3], ["UseStopLoss", "StopLoss", "StopLossDelay"]);
    let mut seen = stops.clone();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), stops.len(), "{stops:?}");
}

#[test]
fn a_knob_another_kind_has_is_outside_for_a_kind_that_does_not_read_it() {
    // A MoonHook's take is `HookSellLevel`; `SellPrice` moves nothing for it.
    let knobs = scope_knobs(&["MoonHook".to_string()]);
    let kind = vec![section("Sell order", &["SellPrice", "SellDelay"])];
    let out = layout(&[kind.as_slice()], &knobs, &used(&["SellPrice"]));
    let sell = find(&out, ParamSection::SellOrder);
    assert_eq!(sell.rows[0].role, RowRole::Outside);
    assert!(matches!(sell.rows[1].role, RowRole::Knob(p) if p.key == "SellDelay"));
}

/// Only the knobs and the fields a strategy of the scope switches on are drawn: a MoonShot scope
/// that keeps SellSpread off (`IgnoreSellSpread` at its default YES) and never touched
/// `TrailingSpread` shows neither — the SellSpread section is left with no row — while every knob
/// stands, used or not, and a field outside the model in use keeps its place in the schema's
/// order.
#[test]
fn only_knobs_and_the_fields_in_use_are_drawn() {
    let knobs = scope_knobs(&["MoonShot".to_string()]);
    let kind = vec![
        section("Strategy settings", &["MShotPrice", "MShotRepeatWait"]),
        section(
            "Sell order\\SellSpread",
            &["IgnoreSellSpread", "SellSpreadDistance"],
        ),
        section(
            "Stops",
            &[
                "UseStopLoss",
                "StopLoss",
                "DontSellBelowLiq",
                "TrailingSpread",
            ],
        ),
    ];
    let out = layout(
        &[kind.as_slice()],
        &knobs,
        &used(&["MShotRepeatWait", "DontSellBelowLiq"]),
    );
    let settings = find(&out, ParamSection::StrategySettings);
    assert_eq!(keys(settings)[..2], ["MShotPrice", "MShotRepeatWait"]);
    assert_eq!(settings.rows[1].role, RowRole::Outside);
    assert!(find(&out, ParamSection::SellSpread).rows.is_empty());
    let stops = keys(find(&out, ParamSection::Stops));
    assert_eq!(stops[..3], ["UseStopLoss", "StopLoss", "DontSellBelowLiq"]);
    assert!(!stops.contains(&"TrailingSpread"), "{stops:?}");
    // Every knob of the scope is drawn once, whether a strategy moved it or not.
    for knob in &knobs {
        let n: usize = out
            .iter()
            .map(|g| g.rows.iter().filter(|r| r.key == knob.key).count())
            .sum();
        assert_eq!(n, 1, "{}", knob.key);
    }
}

/// SellSpread switched on in one strategy of the scope brings its section back, inactive, with
/// the fields that strategy uses.
#[test]
fn a_section_the_model_lacks_is_drawn_while_a_strategy_uses_it() {
    let knobs = scope_knobs(&["MoonShot".to_string()]);
    let kind = vec![section(
        "Sell order\\SellSpread",
        &["IgnoreSellSpread", "SellSpreadDistance", "SellSpreadDelay"],
    )];
    let out = layout(
        &[kind.as_slice()],
        &knobs,
        &used(&["IgnoreSellSpread", "SellSpreadDistance"]),
    );
    let spread = find(&out, ParamSection::SellSpread);
    assert_eq!(keys(spread), ["IgnoreSellSpread", "SellSpreadDistance"]);
    assert!(spread.rows.iter().all(|r| r.role == RowRole::Unmodelled));
}

#[test]
fn a_strategy_id_past_i64_max_is_the_reports_negative_one() {
    // The report stores the core's u64 id as a signed column: HookTestO1 on GateF is
    // -7944420346259379305 there (2026-09-24).
    let report: i64 = -7_944_420_346_259_379_305;
    assert!(same_strategy(report as u64, report));
    assert!(same_strategy(42, 42));
    assert!(!same_strategy(42, 43));
}

/// A section holding knobs of both groups marks the fewer: MoonShot's Strategy settings carry the
/// entry corridor and two exit fields; a section of one group, or split evenly, marks none.
#[test]
fn the_odd_group_of_a_mixed_section_is_the_fewer() {
    let knobs = scope_knobs(&["MoonShot".to_string()]);
    let out = layout(&[], &knobs, &HashSet::new());
    let strategy = find(&out, ParamSection::StrategySettings);
    assert!(keys(strategy).contains(&"MShotSellPriceAdjust"));
    assert!(keys(strategy).contains(&"MShotPrice"));
    assert_eq!(strategy.minority_group(), Some(ParamGroup::Exit));
    assert_eq!(find(&out, ParamSection::Stops).minority_group(), None);
    // An even split has no odd one out.
    let even = GridSection {
        section: ParamSection::StrategySettings,
        rows: ["MShotPrice", "MShotSellPriceAdjust"]
            .iter()
            .map(|key| {
                let param = moon_core::db::tuner::ticks::TICK_PARAMS
                    .iter()
                    .find(|p| p.key == *key)
                    .expect("a knob");
                GridRow {
                    key: key.to_string(),
                    role: RowRole::Knob(param),
                }
            })
            .collect(),
    };
    assert_eq!(even.minority_group(), None);
}
