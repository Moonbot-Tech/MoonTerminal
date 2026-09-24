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

#[test]
fn every_section_comes_out_in_grid_order() {
    let out = layout(&[], &[]);
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
            "Stops",
            &["UseStopLoss", "StopLoss", "UseTrailing", "TrailingPercent"],
        ),
        section("Filters", &["MinVolume"]),
    ];
    let out = layout(&[kind.as_slice()], &knobs);

    let settings = find(&out, ParamSection::StrategySettings);
    assert_eq!(
        keys(settings)[..3],
        ["MShotPrice", "MShotRepeatWait", "MShotSellAtLastPrice"]
    );
    assert!(matches!(settings.rows[0].role, RowRole::Knob(p) if p.key == "MShotPrice"));
    assert_eq!(settings.rows[1].role, RowRole::Outside);

    let shot = find(&out, ParamSection::SellShot);
    assert_eq!(keys(shot)[..2], ["IgnoreSellShot", "SellShotPriceDown"]);
    // Read by the SellShot walk, never turned.
    assert_eq!(shot.rows[1].role, RowRole::Fixed);
    // The knobs this schema left out follow under their own section.
    assert!(
        shot.rows[2..]
            .iter()
            .all(|r| matches!(r.role, RowRole::Knob(p) if p.section == ParamSection::SellShot))
    );
    assert!(shot.rows.iter().any(|r| r.key == "SellShotDistance"));

    let stops = find(&out, ParamSection::Stops);
    assert_eq!(
        keys(stops)[..4],
        ["UseStopLoss", "StopLoss", "UseTrailing", "TrailingPercent"]
    );
    assert_eq!(stops.rows[0].role, RowRole::Fixed);
    assert_eq!(stops.rows[3].role, RowRole::Outside);

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
    let out = layout(&[], &knobs);
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
    let out = layout(&[kind.as_slice()], &knobs);
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
    let out = layout(&[shot.as_slice(), hook.as_slice()], &knobs);
    assert_eq!(
        keys(find(&out, ParamSection::Stops)),
        ["UseStopLoss", "StopLoss", "StopLossDelay"]
    );
}

#[test]
fn a_knob_another_kind_has_is_outside_for_a_kind_that_does_not_read_it() {
    // A MoonHook's take is `HookSellLevel`; `SellPrice` moves nothing for it.
    let knobs = scope_knobs(&["MoonHook".to_string()]);
    let kind = vec![section("Sell order", &["SellPrice", "SellDelay"])];
    let out = layout(&[kind.as_slice()], &knobs);
    let sell = find(&out, ParamSection::SellOrder);
    assert_eq!(sell.rows[0].role, RowRole::Outside);
    assert!(matches!(sell.rows[1].role, RowRole::Knob(p) if p.key == "SellDelay"));
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
