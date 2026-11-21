use super::super::reader::IniSection;
use super::super::schema_v7::{
    HotkeysPublic, IniBlock, MarketsTable, MoonBotConfig, Shortcuts, ThemeBlock, UiBlock,
};
use super::*;
use crate::config::hotkeys::HotkeysConfig;
use crate::config::orders::OrdersStyleSet;
use crate::config::theme::ChartThemeSet;

/// Builds a MoonBotConfig with meaningful values for plan tests.
fn mb_config() -> MoonBotConfig {
    let mut shortcuts = [0u16; 27];
    shortcuts[0] = 0x8000 | 0x5A; // CancelBuy = Alt+Z
    shortcuts[4] = 0x0075; // ReloadBook = F6 (no corresponding Terminal action)
    shortcuts[25] = 0x4000 | 0x2E; // CancelAllBuys = Ctrl+Delete (matches the default)
    MoonBotConfig {
        config_version: 1,
        x_t_mode: false,
        ui: UiBlock {
            hide_demo_button: false,
            confirm_close: false,
            new_markets_on_top: false,
            coins_sort_order: 0,
            hotkeys: HotkeysPublic {
                filled: true,
                ver: 2,
                order_sizes: [111.0, 222.0, 333.0, 444.0, 555.0, 666.0],
                order_size_sel: 3,
                order_size_keys: [0x70, 0x71, 0x72, 0x73, 0x74, 0x75], // F1..F6
                split_parts: 2,
                fixed_sell_sel: 1,
                fixed_sell_keys: [0, 0, 0, 0, 0, 0], // Empty shortcuts are not imported.
                fixed_sell_prices: [1.0, 5.0, 10.0, 25.0, 50.0, 100.0],
                shortcuts: Shortcuts(shortcuts),
            },
            strat_editor_chapters: String::new(),
            markets_table: MarketsTable {
                sort_col: 0,
                col_visible: [false; 41],
                col_pos: [0; 41],
            },
            main_buttons_index: 0,
            strat_expanded: [false; 11],
        },
        theme: ThemeBlock {
            current_style: 3, // Dark theme.
            sections: vec![
                IniSection {
                    name: "ColorsDark".into(),
                    entries: vec![
                        // 0xFF00FF00 → green, alpha FF (opaque), which is valid.
                        ("CandleGreen".into(), format!("{}", 0xFF00_FF00u32 as i64)),
                        // Alpha 0x80 is meaningful and must go to unsupported.
                        ("CandleRed".into(), format!("{}", 0x80FF_0000u32 as i64)),
                        ("graphBK".into(), "1973790".into()), // 0x001E1E1E
                        ("Unknown".into(), "123".into()),
                        ("BuyOrder".into(), "junk".into()), // Not a number.
                    ],
                },
                IniSection {
                    name: "ColorsLight".into(),
                    entries: vec![("graphBK".into(), "16777215".into())], // White.
                },
            ],
        },
        ini: IniBlock {
            sections: vec![IniSection {
                name: "Charts".into(),
                entries: vec![("Some".into(), "1".into())],
            }],
        },
    }
}

fn ctx<'a>(
    hotkeys: &'a HotkeysConfig,
    theme: &'a ChartThemeSet,
    orders: &'a OrdersStyleSet,
) -> PlanContext<'a> {
    PlanContext {
        hotkeys,
        theme,
        orders,
        ui_theme_light: true, // MoonBot is dark, so the plan will contain a theme change.
    }
}

fn find<'a>(items: &'a [SettingChange], id: &str) -> Option<&'a SettingChange> {
    items.iter().find(|c| c.id == id)
}

/// Pins the mapping and typed refusal, catching a missing-action hotkey being offered.
#[test]
fn theme_mode_and_hotkeys_mapped() {
    let (h, t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let plan = build_plan(&mb_config(), &ctx(&h, &t, &o));

    // The local theme is light and MoonBot is dark, producing a change item.
    let theme = find(&plan.terminal, "ui.theme_mode").unwrap();
    assert_eq!(theme.value, PlannedValue::UiThemeLight(false));
    assert!(!theme.same);

    // Cancel Buy = Alt+Z → keystroke alt-z, which is ALSO the shipped default now that the
    // Terminal's keyboard defaults are Moonbot's own: the item exists for completeness with
    // `same` set. "Hotkeys" group.
    let cb = find(&plan.hotkeys, "hotkey.cancel_buy").unwrap();
    assert_eq!(cb.value, PlannedValue::Keystroke("alt-z".into()));
    assert_eq!(cb.current, PreviewValue::Data("alt-z".into()));
    assert!(cb.same);

    // The fixture's CancelAllBuys is Ctrl+Delete, which is NOT Moonbot's own Alt+A default that
    // this build ships, so that item is a real change.
    assert!(!find(&plan.hotkeys, "hotkey.cancel_all_buys").unwrap().same);

    // OKeys F1..F6 match the order_size defaults, so the items EXIST with same set.
    assert!(find(&plan.hotkeys, "hotkey.order_size.0").unwrap().same);

    // Empty SKeys are not imported, so the local shift-f7.. defaults are NOT cleared. No item
    // is created, but empty values are counted. COMPUTE the expectation from the fixture and
    // production classification (action_target), not a magic number: empty OKeys/SKeys plus
    // importable shortcut slots with raw == 0.
    assert!(find(&plan.hotkeys, "hotkey.sell_preset.0").is_none());
    let mb = mb_config();
    let h = &mb.ui.hotkeys;
    let expected_empty = h.order_size_keys.iter().filter(|k| **k == 0).count()
        + h.fixed_sell_keys.iter().filter(|k| **k == 0).count()
        + SHORTCUT_ACTIONS
            .iter()
            .filter(|a| action_target(**a).is_some() && h.shortcuts.get(**a) == 0)
            .count();
    assert_eq!(plan.hotkeys_empty, expected_empty);
    assert!(expected_empty > 0, "the fixture must contain empty slots");

    // ReloadBook is assigned but has no action, so it enters unsupported_hotkeys with a reason.
    assert!(plan.unsupported_hotkeys.iter().any(|u| u.name
        == PreviewCaption::Action(ShortcutAction::ReloadBook)
        && u.reason == ImportReason::NoAction));
}

/// Pins color mapping and typed evidence, catching alpha loss and malformed colors being accepted.
#[test]
fn colors_mapped_per_theme_side() {
    let (h, t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let plan = build_plan(&mb_config(), &ctx(&h, &t, &o));

    // CandleGreen (dark) 0x00FF00 differs from the default, producing a change item.
    let cg = find(&plan.chart, "theme.candle_up.dark").unwrap();
    assert_eq!(cg.value, PlannedValue::Rgb([0, 255, 0]));
    assert!(!cg.same);

    // graphBK dark = 0x1E1E1E MATCHES the theme default, so the item EXISTS with same set.
    assert!(find(&plan.chart, "theme.bg.dark").unwrap().same);
    // graphBK light = white, matching the light-theme default, so same is set.
    assert!(find(&plan.chart, "theme.bg.light").unwrap().same);

    // CandleRed with alpha 0x80 goes to unsupported rather than being silently discarded.
    assert!(plan.unsupported.iter().any(|u| u.name
        == PreviewCaption::ColorField {
            key: "CandleRed".into(),
            light: false
        }
        && u.reason == ImportReason::ColorAlpha { alpha: 0x80 }));
    // An unknown key goes to unsupported.
    assert!(plan.unsupported.iter().any(|u| u.name
        == PreviewCaption::ColorField {
            key: "Unknown".into(),
            light: false
        }));
    // An invalid color value goes to unsupported.
    assert!(plan.unsupported.iter().any(|u| u.name
        == PreviewCaption::ColorField {
            key: "BuyOrder".into(),
            light: false
        }
        && u.reason
            == ImportReason::InvalidColor {
                value: "junk".into()
            }));
    // The Charts section has no mapping table and goes entirely to unsupported.
    assert!(plan.unsupported.iter().any(
        |u| matches!(&u.name, PreviewCaption::IniEntry { section, .. } if section == "Charts")
    ));
}

/// Pins group import decisions and warnings, catching dropped range validation or whole-set refusal.
#[test]
fn core_items_and_range_checks() {
    let (h, t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let plan = build_plan(&mb_config(), &ctx(&h, &t, &o));

    // The whole manual-trading generation is group-local now: numbers as they are, no currency
    // guess, nothing addressed to a core.
    let sizes = find(&plan.group_items, "group.order_sizes").unwrap();
    assert_eq!(
        sizes.value,
        PlannedValue::OrderSizes([111.0, 222.0, 333.0, 444.0, 555.0, 666.0])
    );
    assert_eq!(
        sizes.new,
        PreviewValue::Data("111, 222, 333, 444, 555, 666".into())
    );
    assert!(
        !plan
            .warnings
            .iter()
            .any(|warning| matches!(warning, ImportWarning::InvalidOrderSizes { .. }))
    );
    let sel = find(&plan.group_items, "group.order_size_sel").unwrap();
    assert_eq!(sel.value, PlannedValue::OrderSizeSel(3));
    assert_eq!(sel.new, PreviewValue::Data("B4".into()));
    assert!(!sel.same);
    let pcts = find(&plan.group_items, "group.fixed_sell_prices").unwrap();
    assert_eq!(
        pcts.value,
        PlannedValue::FixedSellPrices([1.0, 5.0, 10.0, 25.0, 50.0, 100.0])
    );
    let slot = find(&plan.group_items, "group.fixed_sell_sel").unwrap();
    assert_eq!(slot.value, PlannedValue::FixedSellSel(1));
    assert_eq!(slot.new, PreviewValue::Data("S2".into()));

    // An out-of-range bNum produces a warning and no item.
    let mut mb = mb_config();
    mb.ui.hotkeys.order_size_sel = 9;
    let plan = build_plan(&mb, &ctx(&h, &t, &o));
    assert!(find(&plan.group_items, "group.order_size_sel").is_none());
    assert!(
        plan.warnings
            .iter()
            .any(|w| matches!(w, ImportWarning::OrderSizeSelection { value: 9 }))
    );

    // The same for sbNum.
    let mut mb = mb_config();
    mb.ui.hotkeys.fixed_sell_sel = 6;
    let plan = build_plan(&mb, &ctx(&h, &t, &o));
    assert!(find(&plan.group_items, "group.fixed_sell_sel").is_none());
    assert!(
        plan.warnings
            .iter()
            .any(|w| matches!(w, ImportWarning::FixedSellSelection { value: 6 }))
    );

    // A set with a value that cannot be a size or a percentage is withheld whole, with a warning.
    let mut mb = mb_config();
    mb.ui.hotkeys.order_sizes[2] = -1.0;
    mb.ui.hotkeys.fixed_sell_prices[4] = f32::NAN;
    let plan = build_plan(&mb, &ctx(&h, &t, &o));
    assert!(find(&plan.group_items, "group.order_sizes").is_none());
    assert!(find(&plan.group_items, "group.fixed_sell_prices").is_none());
    assert!(
        plan.warnings
            .iter()
            .any(|w| matches!(w, ImportWarning::InvalidOrderSizes { .. }))
    );
    assert!(
        plan.warnings
            .iter()
            .any(|w| matches!(w, ImportWarning::InvalidFixedSellPrices { .. }))
    );

    // A Hotkeys block Moonbot never filled offers nothing at all, whatever its zeros would read as.
    let mut mb = mb_config();
    mb.ui.hotkeys.filled = false;
    let plan = build_plan(&mb, &ctx(&h, &t, &o));
    assert!(plan.group_items.is_empty(), "{:?}", plan.group_items);
    assert!(
        plan.warnings
            .iter()
            .any(|w| matches!(w, ImportWarning::HotkeysUnfilled))
    );

    // A fractional percentage previews as typed, not as its widened bits.
    let mut mb = mb_config();
    mb.ui.hotkeys.fixed_sell_prices[0] = 33.3;
    let plan = build_plan(&mb, &ctx(&h, &t, &o));
    assert_eq!(
        find(&plan.group_items, "group.fixed_sell_prices")
            .unwrap()
            .new,
        PreviewValue::Data("33.3, 5, 10, 25, 50, 100".into())
    );

    // A zero size is what the group's load-time repair replaces with the default, so it is not
    // offered either; a zero percentage is a legal "sell at entry" and is.
    let mut mb = mb_config();
    mb.ui.hotkeys.order_sizes[5] = 0.0;
    mb.ui.hotkeys.fixed_sell_prices[0] = 0.0;
    let plan = build_plan(&mb, &ctx(&h, &t, &o));
    assert!(find(&plan.group_items, "group.order_sizes").is_none());
    assert!(find(&plan.group_items, "group.fixed_sell_prices").is_some());
}

#[test]
fn tcolor_hex_and_decimal() {
    // Live MoonBot export: eight-character AARRGGBB hex strings.
    assert_eq!(super::parse_tcolor("FF008000"), Some(([0, 128, 0], 0xFF)));
    assert_eq!(
        super::parse_tcolor("FFFFFFFF"),
        Some(([255, 255, 255], 0xFF))
    );
    assert_eq!(super::parse_tcolor("00FF00FF"), Some(([255, 0, 255], 0)));
    // Decimal notation is also accepted (decimal values have no leading zeros).
    assert_eq!(super::parse_tcolor("16777215"), Some(([255, 255, 255], 0)));
    assert_eq!(super::parse_tcolor("1973790"), Some(([30, 30, 30], 0)));
    // Invalid input returns None.
    assert_eq!(super::parse_tcolor("junk"), None);
    assert_eq!(super::parse_tcolor(""), None);
}

#[test]
fn fmt_nums_compact() {
    assert_eq!(super::fmt_nums(&[70.0f64, 80.5, 2500.0]), "70, 80.5, 2500");
    assert_eq!(super::fmt_nums(&[1.0f32, 5.0, 100.0]), "1, 5, 100");
}

/// Pins how `Hotkeys.SplitParts` reaches the Split Order X count.
///
/// Plausible breakage: reporting an unset (zero) value as out of range floods every import with a
/// false "not imported" row, dropping an above-maximum value loses a real user setting, and
/// comparing `same` against the clamped reading hides a hand-edited invalid file from repair.
#[test]
fn split_parts_import_skips_unset_clamps_high_and_diffs_the_raw_value() {
    let (t, o) = (ChartThemeSet::default(), OrdersStyleSet::default());
    let h = HotkeysConfig::default();
    let plan = build_plan(&mb_config(), &ctx(&h, &t, &o));
    let item = find(&plan.hotkeys, "hotkey.split_parts").expect("SplitParts must be importable");
    assert_eq!(item.new, PreviewValue::Data("2".into()));
    assert!(item.same, "the fixture matches the shipped default");

    // An unfilled Hotkeys block, and a never-configured zero, are silent rather than "out of range".
    let mut unset = mb_config();
    unset.ui.hotkeys.split_parts = 0;
    let plan = build_plan(&unset, &ctx(&h, &t, &o));
    assert!(find(&plan.hotkeys, "hotkey.split_parts").is_none());
    assert!(
        !plan
            .unsupported
            .iter()
            .any(|u| u.name == PreviewCaption::ConfigField("SplitParts".into()))
    );
    let mut unfilled = mb_config();
    unfilled.ui.hotkeys.filled = false;
    let plan = build_plan(&unfilled, &ctx(&h, &t, &o));
    assert!(find(&plan.hotkeys, "hotkey.split_parts").is_none());

    // Above the maximum the value still arrives, capped, and the cap is stated.
    let mut high = mb_config();
    high.ui.hotkeys.split_parts = 25;
    let plan = build_plan(&high, &ctx(&h, &t, &o));
    assert_eq!(
        find(&plan.hotkeys, "hotkey.split_parts").unwrap().new,
        PreviewValue::Data("20".into())
    );
    assert!(plan.warnings.iter().any(|w| matches!(
        w,
        ImportWarning::SplitPartsClamped {
            parts: 25,
            max: 20,
            value: 20
        }
    )));

    // A raw out-of-range local value differs from the import even when its clamped reading matches.
    let broken = HotkeysConfig {
        split_parts: 0,
        ..HotkeysConfig::default()
    };
    let plan = build_plan(&mb_config(), &ctx(&broken, &t, &o));
    assert!(!find(&plan.hotkeys, "hotkey.split_parts").unwrap().same);
}

#[test]
fn identical_values_marked_same_and_visible() {
    // MoonBot values match current values, so items EXIST for completeness with same=true.
    let (h, t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let mut mb = mb_config();
    mb.theme.current_style = 0; // Light, matching ctx.
    let plan = build_plan(&mb, &ctx(&h, &t, &o));
    assert!(find(&plan.terminal, "ui.theme_mode").unwrap().same);
}

/// Every id the plan writes is an id the application reads back to the SAME slot.
///
/// This is the whole reason the two tables became one. They were eighteen hand-typed names in
/// `plan` and eighteen more in `apply`, each ending in a silent fallback — so a name spelled
/// differently on one side meant the preview offered a hotkey, the user ticked it, and nothing was
/// imported and nothing said so.
///
/// Plausible breakage: giving either direction its own spelling again, or letting a family's index
/// round-trip to a different one.
#[test]
fn every_plan_id_round_trips_to_its_own_slot() {
    for slot in KeySlot::all() {
        let id = hotkey_id(slot);
        assert_eq!(
            slot_for_id(&id),
            Some(slot),
            "{slot:?} writes {id} and reads back as something else"
        );
    }
}

/// An id this build does not know is refused rather than silently landing somewhere.
///
/// Plausible breakage: a family prefix that accepts any index would write past its array, and an
/// unknown name that fell through to a default would import into the wrong field.
#[test]
fn an_unknown_plan_id_names_no_slot() {
    for id in [
        "hotkey.no_such_action",
        "hotkey.order_size.6",
        "hotkey.order_size.x",
        "hotkey.manual_strategy.10",
        "theme.bg.dark",
        "hotkey.",
        "",
    ] {
        assert_eq!(slot_for_id(id), None, "{id} must name no slot");
    }
}

/// Removing the Trading mapping, placing it after S1-S6, or gating it on Hotkeys.Filled
/// would lose the exported TP representation or quantize percentages in the old mode.
#[test]
fn xtmode_adds_a_typed_mode_row_before_fixed_sell_prices() {
    let (h, t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let mut mb = mb_config();
    let before = build_plan(&mb, &ctx(&h, &t, &o));
    assert!(find(&before.group_items, "group.take_profit_mode").is_none());
    mb.x_t_mode = true;
    let mut extended = build_plan(&mb, &ctx(&h, &t, &o));
    let mode = extended.group_items.remove(0);
    assert_eq!(mode.id, "group.take_profit_mode");
    assert_eq!(
        mode.value,
        PlannedValue::TakeProfitMode(TakeProfitMode::Extended)
    );
    assert_eq!(
        mode.new,
        PreviewValue::ExtendedTakeProfit { groups: Vec::new() }
    );
    let pcts = extended
        .group_items
        .iter_mut()
        .find(|item| item.id == "group.fixed_sell_prices")
        .unwrap();
    assert_eq!(
        pcts.value,
        PlannedValue::FixedSellPrices([10.0, 50.0, 100.0, 250.0, 500.0, 1000.0])
    );
    assert_eq!(
        pcts.new,
        PreviewValue::Data("10, 50, 100, 250, 500, 1000".into())
    );
    *pcts = find(&before.group_items, "group.fixed_sell_prices")
        .unwrap()
        .clone();
    assert_eq!(extended, before);

    mb.ui.hotkeys.filled = false;
    let unfilled = build_plan(&mb, &ctx(&h, &t, &o));
    assert_eq!(unfilled.group_items, vec![mode]);
}

/// Dropping either level mapping/setter loses imported colours; treating auto as equal hides
/// a requested override. Both theme sides must remain independent of their wall colours.
#[test]
fn book_level_theme_import_plans_and_applies_explicit_colours() {
    let (h, mut t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    t.dark.book_bid = [0, 0, 0];
    let mut mb = mb_config();
    mb.theme.sections = vec![
        IniSection {
            name: "ColorsDark".into(),
            entries: vec![
                ("BookLevelGreen".into(), "00000000".into()),
                ("BookLevelRed".into(), "00FF3300".into()),
            ],
        },
        IniSection {
            name: "ColorsLight".into(),
            entries: vec![
                ("BookLevelGreen".into(), "000066FF".into()),
                ("BookLevelRed".into(), "00330066".into()),
            ],
        },
    ];
    let plan = build_plan(&mb, &ctx(&h, &t, &o));
    let selected: std::collections::HashSet<String> =
        plan.chart.iter().map(|v| v.id.clone()).collect();
    assert_eq!(selected.len(), 4);
    assert!(plan.chart.iter().all(|v| !v.same));
    let mut cfg = crate::config::AppConfig::blank(None);
    cfg.theme = t;
    let before_dark = (cfg.theme.dark.book_bid, cfg.theme.dark.book_ask);
    let before_light = (cfg.theme.light.book_bid, cfg.theme.light.book_ask);
    let result = super::super::apply::apply_local(&mut cfg, &plan, &selected, &[]);
    assert_eq!(result.applied, 4);
    assert!(result.unknown_ids.is_empty());
    assert_eq!(cfg.theme.dark.book_level_bid, Some([0, 0, 0]));
    assert_eq!(cfg.theme.dark.book_level_ask, Some([255, 51, 0]));
    assert_eq!(cfg.theme.light.book_level_bid, Some([0, 102, 255]));
    assert_eq!(cfg.theme.light.book_level_ask, Some([51, 0, 102]));
    assert_eq!(
        (cfg.theme.dark.book_bid, cfg.theme.dark.book_ask),
        before_dark
    );
    assert_eq!(
        (cfg.theme.light.book_bid, cfg.theme.light.book_ask),
        before_light
    );
    let repeated = build_plan(&mb, &ctx(&h, &cfg.theme, &o));
    assert!(repeated.chart.iter().all(|v| v.same));
}

/// Every colour key the mapping table names, on both theme sides, plus the refusal paths.
fn every_color_key(alpha: &str) -> Vec<(String, String)> {
    [
        "graphBK",
        "graphNet",
        "graphCursor",
        "graphFont",
        "CandleGreen",
        "CandleRed",
        "CandleNeutral",
        "OrderBookGreen",
        "OrderBookRed",
        "BuyOrder",
        "BuyPendingOrder",
        "SellOrder",
        "BuyShort",
        "SellShort",
        "Trailing",
        "LiqPrice",
        "BookLevelGreen",
        "BookLevelRed",
        "BuyOrderDone",
        "MarkPrice",
        "LiqOrdersLong",
        "LiqOrdersShort",
        "Unknown",
    ]
    .iter()
    .enumerate()
    .map(|(i, key)| {
        let shade = (i * 11) as u8;
        (
            key.to_string(),
            format!("{alpha}{shade:02X}33{:02X}", 255 - shade),
        )
    })
    .chain([
        ("Junk".to_string(), "junk".to_string()),
        ("Half".to_string(), "80112233".to_string()),
    ])
    .collect()
}

/// Pins the whole colour mapping output, item by item and in order, so a reshaped mapping that
/// drops, reorders or relabels a key, or changes a `same` verdict, fails here.
#[test]
fn every_color_key_maps_in_order() {
    let (h, mut t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    // An explicit level override that matches the imported light value exercises the stored
    // comparison; the dark side stays automatic.
    t.light.book_level_bid = Some([0xB0, 0x33, 0x4F]);
    let mut mb = mb_config();
    mb.theme.sections = vec![
        IniSection {
            name: "ColorsDark".into(),
            entries: every_color_key("00"),
        },
        IniSection {
            name: "ColorsLight".into(),
            entries: every_color_key("ff"),
        },
    ];
    mb.ini.sections = vec![
        IniSection {
            name: "Charts".into(),
            entries: vec![("Some".into(), "1".into())],
        },
        IniSection {
            name: "ArbColors".into(),
            entries: vec![("Other".into(), "2".into())],
        },
    ];
    let mut plan = MoonBotImportPlan::default();
    map_colors(&mb, &ctx(&h, &t, &o), &mut plan);
    let chart: Vec<String> = plan.chart.iter().map(|c| format!("{c:?}")).collect();
    let unsupported: Vec<String> = plan.unsupported.iter().map(|u| format!("{u:?}")).collect();
    assert_eq!(chart, EXPECTED_CHART);
    assert_eq!(unsupported, EXPECTED_UNSUPPORTED);
    assert!(plan.terminal.is_empty() && plan.hotkeys.is_empty() && plan.group_items.is_empty());
}

const EXPECTED_CHART: &[&str] = &[
    "SettingChange { id: \"theme.bg.light\", label: ChartBackground, current: Data(\"#FFFFFF\"), new: Data(\"#0033FF\"), value: Rgb([0, 51, 255]), same: false }",
    "SettingChange { id: \"theme.grid.light\", label: Grid, current: Data(\"#ECEFF2\"), new: Data(\"#0B33F4\"), value: Rgb([11, 51, 244]), same: false }",
    "SettingChange { id: \"theme.cross.light\", label: Crosshair, current: Data(\"#808080\"), new: Data(\"#1633E9\"), value: Rgb([22, 51, 233]), same: false }",
    "SettingChange { id: \"theme.labels.light\", label: NeutralLabels, current: Data(\"#000000\"), new: Data(\"#2133DE\"), value: Rgb([33, 51, 222]), same: false }",
    "SettingChange { id: \"theme.candle_up.light\", label: CandleUp, current: Data(\"#1A9E5C\"), new: Data(\"#2C33D3\"), value: Rgb([44, 51, 211]), same: false }",
    "SettingChange { id: \"theme.candle_down.light\", label: CandleDown, current: Data(\"#E63B3B\"), new: Data(\"#3733C8\"), value: Rgb([55, 51, 200]), same: false }",
    "SettingChange { id: \"theme.candle_neutral.light\", label: CandleNeutral, current: Data(\"#969696\"), new: Data(\"#4233BD\"), value: Rgb([66, 51, 189]), same: false }",
    "SettingChange { id: \"theme.book_bid.light\", label: BookBid, current: Data(\"#76C59D\"), new: Data(\"#4D33B2\"), value: Rgb([77, 51, 178]), same: false }",
    "SettingChange { id: \"theme.book_ask.light\", label: BookAsk, current: Data(\"#F08989\"), new: Data(\"#5833A7\"), value: Rgb([88, 51, 167]), same: false }",
    "SettingChange { id: \"orders.buy.color.light\", label: BuyLine, current: Data(\"#000000\"), new: Data(\"#63339C\"), value: Rgb([99, 51, 156]), same: false }",
    "SettingChange { id: \"orders.buy.pending_color.light\", label: BuyPendingLine, current: Data(\"#B00000\"), new: Data(\"#6E3391\"), value: Rgb([110, 51, 145]), same: false }",
    "SettingChange { id: \"orders.sell.color.light\", label: SellLine, current: Data(\"#0000FF\"), new: Data(\"#793386\"), value: Rgb([121, 51, 134]), same: false }",
    "SettingChange { id: \"orders.buy_short.color.light\", label: BuyShortLine, current: Data(\"#8B008B\"), new: Data(\"#84337B\"), value: Rgb([132, 51, 123]), same: false }",
    "SettingChange { id: \"orders.sell_short.color.light\", label: SellShortLine, current: Data(\"#800000\"), new: Data(\"#8F3370\"), value: Rgb([143, 51, 112]), same: false }",
    "SettingChange { id: \"orders.trailing.color.light\", label: TrailingLine, current: Data(\"#00008B\"), new: Data(\"#9A3365\"), value: Rgb([154, 51, 101]), same: false }",
    "SettingChange { id: \"orders.liq.color.light\", label: LiquidationLine, current: Data(\"#FF0000\"), new: Data(\"#A5335A\"), value: Rgb([165, 51, 90]), same: false }",
    "SettingChange { id: \"theme.book_level_bid.light\", label: ConfigField(\"BookLevelGreen\"), current: Data(\"#B0334F\"), new: Data(\"#B0334F\"), value: Rgb([176, 51, 79]), same: true }",
    "SettingChange { id: \"theme.book_level_ask.light\", label: ConfigField(\"BookLevelRed\"), current: Data(\"#FFABAB\"), new: Data(\"#BB3344\"), value: Rgb([187, 51, 68]), same: false }",
    "SettingChange { id: \"theme.bg.dark\", label: ChartBackground, current: Data(\"#1E1E1E\"), new: Data(\"#0033FF\"), value: Rgb([0, 51, 255]), same: false }",
    "SettingChange { id: \"theme.grid.dark\", label: Grid, current: Data(\"#282828\"), new: Data(\"#0B33F4\"), value: Rgb([11, 51, 244]), same: false }",
    "SettingChange { id: \"theme.cross.dark\", label: Crosshair, current: Data(\"#808080\"), new: Data(\"#1633E9\"), value: Rgb([22, 51, 233]), same: false }",
    "SettingChange { id: \"theme.labels.dark\", label: NeutralLabels, current: Data(\"#D3D3D3\"), new: Data(\"#2133DE\"), value: Rgb([33, 51, 222]), same: false }",
    "SettingChange { id: \"theme.candle_up.dark\", label: CandleUp, current: Data(\"#1A9E5C\"), new: Data(\"#2C33D3\"), value: Rgb([44, 51, 211]), same: false }",
    "SettingChange { id: \"theme.candle_down.dark\", label: CandleDown, current: Data(\"#E63B3B\"), new: Data(\"#3733C8\"), value: Rgb([55, 51, 200]), same: false }",
    "SettingChange { id: \"theme.candle_neutral.dark\", label: CandleNeutral, current: Data(\"#808080\"), new: Data(\"#4233BD\"), value: Rgb([66, 51, 189]), same: false }",
    "SettingChange { id: \"theme.book_bid.dark\", label: BookBid, current: Data(\"#1C6440\"), new: Data(\"#4D33B2\"), value: Rgb([77, 51, 178]), same: false }",
    "SettingChange { id: \"theme.book_ask.dark\", label: BookAsk, current: Data(\"#8C2E2E\"), new: Data(\"#5833A7\"), value: Rgb([88, 51, 167]), same: false }",
    "SettingChange { id: \"orders.buy.color.dark\", label: BuyLine, current: Data(\"#D2691E\"), new: Data(\"#63339C\"), value: Rgb([99, 51, 156]), same: false }",
    "SettingChange { id: \"orders.buy.pending_color.dark\", label: BuyPendingLine, current: Data(\"#FF6A00\"), new: Data(\"#6E3391\"), value: Rgb([110, 51, 145]), same: false }",
    "SettingChange { id: \"orders.sell.color.dark\", label: SellLine, current: Data(\"#7FC9FF\"), new: Data(\"#793386\"), value: Rgb([121, 51, 134]), same: false }",
    "SettingChange { id: \"orders.buy_short.color.dark\", label: BuyShortLine, current: Data(\"#B0C4DE\"), new: Data(\"#84337B\"), value: Rgb([132, 51, 123]), same: false }",
    "SettingChange { id: \"orders.sell_short.color.dark\", label: SellShortLine, current: Data(\"#D3D3D3\"), new: Data(\"#8F3370\"), value: Rgb([143, 51, 112]), same: false }",
    "SettingChange { id: \"orders.trailing.color.dark\", label: TrailingLine, current: Data(\"#6495ED\"), new: Data(\"#9A3365\"), value: Rgb([154, 51, 101]), same: false }",
    "SettingChange { id: \"orders.liq.color.dark\", label: LiquidationLine, current: Data(\"#FF4500\"), new: Data(\"#A5335A\"), value: Rgb([165, 51, 90]), same: false }",
    "SettingChange { id: \"theme.book_level_bid.dark\", label: ConfigField(\"BookLevelGreen\"), current: Data(\"#237D50\"), new: Data(\"#B0334F\"), value: Rgb([176, 51, 79]), same: false }",
    "SettingChange { id: \"theme.book_level_ask.dark\", label: ConfigField(\"BookLevelRed\"), current: Data(\"#AF3A3A\"), new: Data(\"#BB3344\"), value: Rgb([187, 51, 68]), same: false }",
];

const EXPECTED_UNSUPPORTED: &[&str] = &[
    "Unsupported { name: ColorField { key: \"BuyOrderDone\", light: true }, reason: ClosedOrderOpacity }",
    "Unsupported { name: ColorField { key: \"MarkPrice\", light: true }, reason: NoStyle }",
    "Unsupported { name: ColorField { key: \"LiqOrdersLong\", light: true }, reason: NoStyle }",
    "Unsupported { name: ColorField { key: \"LiqOrdersShort\", light: true }, reason: NoStyle }",
    "Unsupported { name: ColorField { key: \"Unknown\", light: true }, reason: UnmappedColor }",
    "Unsupported { name: ColorField { key: \"Junk\", light: true }, reason: InvalidColor { value: \"junk\" } }",
    "Unsupported { name: ColorField { key: \"Half\", light: true }, reason: ColorAlpha { alpha: 4 } }",
    "Unsupported { name: ColorField { key: \"BuyOrderDone\", light: false }, reason: ClosedOrderOpacity }",
    "Unsupported { name: ColorField { key: \"MarkPrice\", light: false }, reason: NoStyle }",
    "Unsupported { name: ColorField { key: \"LiqOrdersLong\", light: false }, reason: NoStyle }",
    "Unsupported { name: ColorField { key: \"LiqOrdersShort\", light: false }, reason: NoStyle }",
    "Unsupported { name: ColorField { key: \"Unknown\", light: false }, reason: UnmappedColor }",
    "Unsupported { name: ColorField { key: \"Junk\", light: false }, reason: InvalidColor { value: \"junk\" } }",
    "Unsupported { name: ColorField { key: \"Half\", light: false }, reason: ColorAlpha { alpha: 4 } }",
    "Unsupported { name: IniEntry { section: \"Charts\", key: \"Some\", value: \"1\" }, reason: UnmappedSection }",
    "Unsupported { name: IniEntry { section: \"ArbColors\", key: \"Other\", value: \"2\" }, reason: UnmappedSection }",
];
