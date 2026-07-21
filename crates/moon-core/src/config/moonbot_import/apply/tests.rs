//! Tests for applying selected MoonBot import changes to a runtime config.

use super::super::plan::{PlannedValue, SettingChange};
use super::*;

fn change(id: &str, value: PlannedValue) -> SettingChange {
    SettingChange {
        id: id.into(),
        label: String::new(),
        current: String::new(),
        new: String::new(),
        value,
        same: false,
    }
}

fn plan_with(
    terminal: Vec<SettingChange>,
    chart: Vec<SettingChange>,
    per_core: Vec<SettingChange>,
) -> MoonBotImportPlan {
    MoonBotImportPlan {
        terminal,
        chart,
        per_core,
        ..Default::default()
    }
}

fn all_ids(plan: &MoonBotImportPlan) -> HashSet<String> {
    plan.terminal
        .iter()
        .chain(plan.chart.iter())
        .chain(plan.per_core.iter())
        .map(|c| c.id.clone())
        .collect()
}

/// Protects mapping selected theme, hotkey, and color changes to their config destinations.
#[test]
fn applies_theme_hotkeys_and_colors() {
    let mut cfg = AppConfig::blank(None);
    let plan = plan_with(
        vec![
            change("ui.theme_mode", PlannedValue::UiThemeLight(true)),
            change("hotkey.cancel_buy", PlannedValue::Keystroke("alt-z".into())),
            change(
                "hotkey.order_size.2",
                PlannedValue::Keystroke("ctrl-3".into()),
            ),
        ],
        vec![
            change("theme.candle_up.dark", PlannedValue::Rgb([0, 255, 0])),
            change("theme.labels.light", PlannedValue::Rgb([1, 2, 3])),
            change("orders.buy.color.dark", PlannedValue::Rgb([9, 8, 7])),
        ],
        vec![],
    );
    let out = apply_local(&mut cfg, &plan, &all_ids(&plan), &[]);
    assert_eq!(out.applied, 6);
    assert!(out.unknown_ids.is_empty());
    assert_eq!(cfg.ui_theme_mode, UiThemeMode::Light);
    assert_eq!(cfg.hotkeys.cancel_buy, "alt-z");
    assert_eq!(cfg.hotkeys.order_size[2], "ctrl-3");
    assert_eq!(cfg.theme.get(false).candle_up, [0, 255, 0]);
    // graphFont красит все четыре нейтральные подписи светлой темы.
    let light = cfg.theme.get(true);
    assert_eq!(light.axis_label, [1, 2, 3]);
    assert_eq!(light.caption_label, [1, 2, 3]);
    assert_eq!(light.readout_label, [1, 2, 3]);
    assert_eq!(light.label_neutral, [1, 2, 3]);
    assert_eq!(cfg.orders.get(false).buy.color, [9, 8, 7]);
    // Тёмная тема подписей НЕ тронута (light-пункт).
    assert_ne!(cfg.theme.get(false).axis_label, [1, 2, 3]);
}

/// Protects selection filtering and reporting of selected but unsupported setting ids.
#[test]
fn selection_filter_and_unknown_ids() {
    let mut cfg = AppConfig::blank(None);
    let plan = plan_with(
        vec![
            change("hotkey.cancel_buy", PlannedValue::Keystroke("alt-z".into())),
            change("hotkey.panic_sell", PlannedValue::Keystroke("alt-p".into())),
            change("bogus.id", PlannedValue::Keystroke("x".into())),
        ],
        vec![],
        vec![],
    );
    // Выбран только cancel_buy и bogus.
    let selected: HashSet<String> =
        ["hotkey.cancel_buy".to_string(), "bogus.id".to_string()].into();
    let out = apply_local(&mut cfg, &plan, &selected, &[]);
    assert_eq!(out.applied, 1);
    assert_eq!(out.unknown_ids, vec!["bogus.id".to_string()]);
    assert_eq!(cfg.hotkeys.cancel_buy, "alt-z");
    assert!(cfg.hotkeys.panic_sell.is_empty()); // не выбран — не тронут
}

/// Protects per-core changes from modifying cores outside the selected uid set.
#[test]
fn per_core_targets_only_selected_cores() {
    let mut cfg = AppConfig::blank(None);
    // Три ядра; целимся в id 1 и 3.
    for id in 1..=3u64 {
        cfg.servers.push(crate::config::ServerConfig {
            id,
            uid: id,
            name: format!("s{id}"),
            active: true,
            show_window: true,
            feed: crate::config::FeedFlags::default(),
            key: crate::config::Secret::new(String::new()),
            group: "default".into(),
            market: "BINANCE_FUTURES".into(),
            color: [0xFF, 0xB3, 0x47],
            synthetic: false,
            chart_bundle: String::new(),
            order_sizes: None,
            order_size_sel: None,
            default_alert_strategy: 0,
        });
    }
    let plan = plan_with(
        vec![],
        vec![],
        vec![
            change(
                "core.order_sizes",
                PlannedValue::OrderSizes([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            ),
            change("core.order_size_sel", PlannedValue::OrderSizeSel(3)),
        ],
    );
    let out = apply_local(&mut cfg, &plan, &all_ids(&plan), &[1, 3]);
    assert_eq!(out.applied, 2);
    let sizes = Some([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    assert_eq!(cfg.servers[0].order_sizes, sizes);
    assert_eq!(cfg.servers[0].order_size_sel, Some(3));
    assert_eq!(cfg.servers[1].order_sizes, None); // ядро 2 не выбрано
    assert_eq!(cfg.servers[2].order_sizes, sizes);
}
