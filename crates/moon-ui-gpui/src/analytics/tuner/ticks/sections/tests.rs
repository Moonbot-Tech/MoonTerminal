use super::*;
use moon_core::feed::{SchemaField, SchemaFieldUi};

/// Showing slots for unrelated formula skeletons would advertise knobs the search refuses.
#[test]
fn grid_custom_ema_slots_require_compatible_formulas() {
    let deal = Deal {
        report_uid: 1,
        core_uid: 11,
        core_name: "CORE_A".into(),
        strategy_id: 101,
        kind: "Spread".into(),
        coin: "XALPHA".into(),
        buy_ms: 100_000,
        close_ms: 110_000,
        buy_price: 1.0,
        sell_price: 1.01,
        spent: 100.0,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: 0.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Default::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        drops_detect: None,
        hook_stated_take_pct: None,
        strike_detect: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        quotes: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    };
    let first = HashMap::from([(
        "SellByCustomEMA".into(),
        "EMA(5s,1s)>0.2 AND EMA(15s,1s)<888".into(),
    )]);
    for (source, expected) in [
        (
            "EMA(5s,1s)>0.4 AND EMA(15s,1s)<888",
            vec!["SellByCustomEMA#1"],
        ),
        ("EMA(6s,1s)>0.4 AND EMA(15s,1s)<888", vec![]),
        ("EMA(5s,1s)>888 AND EMA(15s,1s)<0.4", vec![]),
    ] {
        let other = HashMap::from([("SellByCustomEMA".into(), source.into())]);
        let sections = grid_for(
            &CoreStore::default(),
            std::slice::from_ref(&deal),
            [((101, Some(11)), &first), ((202, Some(22)), &other)],
            &FieldDeps::bundled(),
        );
        let offered: Vec<_> = sections
            .iter()
            .flat_map(GridSection::knobs)
            .filter(|field| moon_core::db::tuner::ticks::params::formula_slot(field.key).is_some())
            .map(|field| field.key)
            .collect();
        assert_eq!(offered, expected);
    }
}

/// Virtual fields absent from the core schema must retain editable Sell-order rows.
#[test]
fn custom_ema_slot_rows_are_knob_rows() {
    let knobs = scope_knobs(&["Spread".into()]);
    let out = layout(&[], &knobs, &HashSet::new());
    let sell = find(&out, ParamSection::SellOrder);
    for k in 1..=8 {
        let key = format!("SellByCustomEMA#{k}");
        let row = sell
            .rows
            .iter()
            .find(|row| row.key == key)
            .expect("slot row");
        assert!(matches!(row.role, RowRole::Knob(_)));
    }
}

/// Foreign, fixed and unticked fields must not inflate the section's changed count.
#[test]
fn changed_counts_only_the_sections_live_ticked_knobs() {
    let knobs = scope_knobs(&["DropsDetection".to_string()]);
    let sections = layout(&[], &knobs, &HashSet::new());
    let group = sections
        .iter()
        .find(|s| s.knobs().count() >= 2)
        .expect("two knobs");
    let keys: Vec<_> = group.knobs().map(|k| k.key).collect();
    let live = [keys[0], keys[1]];
    let changes = vec![
        (keys[0].to_string(), "1.3".to_string()),
        ("foreign".to_string(), "2.7".to_string()),
    ];
    assert_eq!(changed_in(group, &live, &changes), (1, 2));
    assert_eq!(changed_in(group, &live[1..], &changes), (0, 1));
}

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
