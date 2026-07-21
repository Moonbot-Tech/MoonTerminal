use super::super::reader::IniSection;
use super::super::schema_v7::{
    HotkeysPublic, IniBlock, MarketsTable, MoonBotConfig, Shortcuts, ThemeBlock, UiBlock,
};
use super::*;
use crate::config::hotkeys::HotkeysConfig;
use crate::config::orders::OrdersStyleSet;
use crate::config::theme::ChartThemeSet;

/// MoonBotConfig с осмысленными значениями для плана.
fn mb_config() -> MoonBotConfig {
    let mut shortcuts = [0u16; 27];
    shortcuts[0] = 0x8000 | 0x5A; // CancelBuy = Alt+Z
    shortcuts[4] = 0x0075; // ReloadBook = F6 (нет действия в Terminal)
    shortcuts[25] = 0x4000 | 0x2E; // CancelAllBuys = Ctrl+Delete (совпадает с дефолтом)
    MoonBotConfig {
        config_version: 1,
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
                fixed_sell_keys: [0, 0, 0, 0, 0, 0], // пустые — не переносим
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
            current_style: 3, // тёмная
            sections: vec![
                IniSection {
                    name: "ColorsDark".into(),
                    entries: vec![
                        // 0xFF00FF00 → зелёный, alpha FF (непрозрачный) — ок.
                        ("CandleGreen".into(), format!("{}", 0xFF00_FF00u32 as i64)),
                        // Alpha 0x80 — значимая, должна уйти в unsupported.
                        ("CandleRed".into(), format!("{}", 0x80FF_0000u32 as i64)),
                        ("graphBK".into(), "1973790".into()), // 0x001E1E1E
                        ("Unknown".into(), "123".into()),
                        ("BuyOrder".into(), "junk".into()), // не число
                    ],
                },
                IniSection {
                    name: "ColorsLight".into(),
                    entries: vec![("graphBK".into(), "16777215".into())], // белый
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
        ui_theme_light: true, // MoonBot тёмная → будет пункт смены темы
        order_sizes: [50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0],
        order_size_sel: None,
    }
}

fn find<'a>(items: &'a [SettingChange], id: &str) -> Option<&'a SettingChange> {
    items.iter().find(|c| c.id == id)
}

#[test]
fn theme_mode_and_hotkeys_mapped() {
    let (h, t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let plan = build_plan(&mb_config(), &ctx(&h, &t, &o));

    // Тема: у нас светлая, MoonBot тёмная → пункт-изменение.
    let theme = find(&plan.terminal, "ui.theme_mode").unwrap();
    assert_eq!(theme.value, PlannedValue::UiThemeLight(false));
    assert!(!theme.same);

    // Cancel Buy = Alt+Z → keystroke alt-z; текущее пусто. Группа «Хоткеи».
    let cb = find(&plan.hotkeys, "hotkey.cancel_buy").unwrap();
    assert_eq!(cb.value, PlannedValue::Keystroke("alt-z".into()));
    assert_eq!(cb.current, "—");
    assert!(!cb.same);

    // CancelAllBuys Ctrl+Delete совпадает с нашим дефолтом → пункт ЕСТЬ, same.
    assert!(find(&plan.hotkeys, "hotkey.cancel_all_buys").unwrap().same);

    // OKeys F1..F6 совпадают с дефолтами order_size → пункты ЕСТЬ, same.
    assert!(find(&plan.hotkeys, "hotkey.order_size.0").unwrap().same);

    // Пустые SKeys не переносятся (наши дефолты shift-f7.. НЕ стираются) —
    // пункта нет, но пустые посчитаны. Ожидание ВЫЧИСЛЯЕМ из фикстуры и
    // продовой классификации (action_target), а не магическим числом: пустые
    // OKeys/SKeys + переносимые shortcut-слоты с raw == 0.
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
    assert!(expected_empty > 0, "фикстура должна содержать пустые слоты");

    // ReloadBook назначен, но действия нет → unsupported_hotkeys с причиной.
    assert!(plan
        .unsupported_hotkeys
        .iter()
        .any(|u| u.name == "Reload Book" && u.reason.contains("нет такого действия")));
}

#[test]
fn colors_mapped_per_theme_side() {
    let (h, t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let plan = build_plan(&mb_config(), &ctx(&h, &t, &o));

    // CandleGreen (dark) 0x00FF00 отличается от дефолта → пункт-изменение.
    let cg = find(&plan.chart, "theme.candle_up.dark").unwrap();
    assert_eq!(cg.value, PlannedValue::Rgb([0, 255, 0]));
    assert!(!cg.same);

    // graphBK dark = 0x1E1E1E — РАВЕН дефолту темы → пункт ЕСТЬ с same.
    assert!(find(&plan.chart, "theme.bg.dark").unwrap().same);
    // graphBK light = белый — равен светлому дефолту → same.
    assert!(find(&plan.chart, "theme.bg.light").unwrap().same);

    // CandleRed c alpha 0x80 → unsupported, не молча.
    assert!(plan
        .unsupported
        .iter()
        .any(|u| u.name.starts_with("CandleRed") && u.reason.contains("alpha")));
    // Unknown-ключ → unsupported.
    assert!(plan
        .unsupported
        .iter()
        .any(|u| u.name.starts_with("Unknown")));
    // Мусорное значение цвета → unsupported.
    assert!(plan
        .unsupported
        .iter()
        .any(|u| u.name.starts_with("BuyOrder") && u.reason.contains("не разобрано")));
    // Секция Charts без таблицы — целиком в unsupported.
    assert!(plan.unsupported.iter().any(|u| u.name.contains("Charts")));
}

#[test]
fn core_items_and_range_checks() {
    let (h, t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let plan = build_plan(&mb_config(), &ctx(&h, &t, &o));

    let sizes = find(&plan.per_core, "core.order_sizes").unwrap();
    assert_eq!(
        sizes.value,
        PlannedValue::OrderSizes([111.0, 222.0, 333.0, 444.0, 555.0, 666.0])
    );
    assert!(!sizes.same);
    let sel = find(&plan.per_core, "core.order_size_sel").unwrap();
    assert_eq!(sel.value, PlannedValue::OrderSizeSel(3));
    assert_eq!(sel.new, "F4");
    assert!(!sel.same);

    // Fixed-sell — в core_commands (ClientSettings), не в per_core.
    assert!(find(&plan.core_commands, "core.fixed_sell_prices").is_some());
    assert_eq!(
        find(&plan.core_commands, "core.fixed_sell_sel")
            .unwrap()
            .new,
        "S2"
    );

    // bNum вне диапазона → warning, пункта нет.
    let mut mb = mb_config();
    mb.ui.hotkeys.order_size_sel = 9;
    let plan = build_plan(&mb, &ctx(&h, &t, &o));
    assert!(find(&plan.per_core, "core.order_size_sel").is_none());
    assert!(plan.warnings.iter().any(|w| w.contains("bNum")));
}

#[test]
fn tcolor_hex_and_decimal() {
    // Живой экспорт MoonBot: hex-строки 8 символов AARRGGBB.
    assert_eq!(super::parse_tcolor("FF008000"), Some(([0, 128, 0], 0xFF)));
    assert_eq!(
        super::parse_tcolor("FFFFFFFF"),
        Some(([255, 255, 255], 0xFF))
    );
    assert_eq!(super::parse_tcolor("00FF00FF"), Some(([255, 0, 255], 0)));
    // Десятичная запись тоже принимается (ведущих нулей у десятичных нет).
    assert_eq!(super::parse_tcolor("16777215"), Some(([255, 255, 255], 0)));
    assert_eq!(super::parse_tcolor("1973790"), Some(([30, 30, 30], 0)));
    // Мусор — None.
    assert_eq!(super::parse_tcolor("junk"), None);
    assert_eq!(super::parse_tcolor(""), None);
}

#[test]
fn fmt_nums_compact() {
    assert_eq!(super::fmt_nums(&[70.0f64, 80.5, 2500.0]), "70, 80.5, 2500");
    assert_eq!(super::fmt_nums(&[1.0f32, 5.0, 100.0]), "1, 5, 100");
}

#[test]
fn identical_values_marked_same_and_visible() {
    // MoonBot-значения = текущим → пункты ЕСТЬ (полная картина), но с same=true.
    let (h, t, o) = (
        HotkeysConfig::default(),
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
    );
    let mut mb = mb_config();
    mb.theme.current_style = 0; // светлая, как в ctx
    mb.ui.hotkeys.order_sizes = [50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0];
    let plan = build_plan(&mb, &ctx(&h, &t, &o));
    assert!(find(&plan.terminal, "ui.theme_mode").unwrap().same);
    assert!(find(&plan.per_core, "core.order_sizes").unwrap().same);
}
