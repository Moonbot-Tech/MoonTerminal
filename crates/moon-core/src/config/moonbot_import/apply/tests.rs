//! Tests for applying selected MoonBot import changes to a runtime config.

use super::super::plan::{PlannedValue, SettingChange};
use super::*;

/// Build a setter fixture with inert typed preview fields; application ignores the wording.
fn change(id: &str, value: PlannedValue) -> SettingChange {
    SettingChange {
        id: id.into(),
        label: super::super::preview::PreviewCaption::ConfigField(String::new()),
        current: super::super::preview::PreviewValue::Data(String::new()),
        new: super::super::preview::PreviewValue::Data(String::new()),
        value,
        same: false,
    }
}

fn plan_with(
    terminal: Vec<SettingChange>,
    chart: Vec<SettingChange>,
    group_items: Vec<SettingChange>,
) -> MoonBotImportPlan {
    MoonBotImportPlan {
        terminal,
        chart,
        group_items,
        ..Default::default()
    }
}

fn all_ids(plan: &MoonBotImportPlan) -> HashSet<String> {
    plan.terminal
        .iter()
        .chain(plan.chart.iter())
        .chain(plan.group_items.iter())
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
    // graphFont colors all four neutral labels in the light theme.
    let light = cfg.theme.get(true);
    assert_eq!(light.axis_label, [1, 2, 3]);
    assert_eq!(light.caption_label, [1, 2, 3]);
    assert_eq!(light.readout_label, [1, 2, 3]);
    assert_eq!(light.label_neutral, [1, 2, 3]);
    assert_eq!(cfg.orders.get(false).buy.color, [9, 8, 7]);
    // The dark-theme labels remain UNCHANGED by the light item.
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
    // Only cancel_buy and bogus are selected.
    let selected: HashSet<String> =
        ["hotkey.cancel_buy".to_string(), "bogus.id".to_string()].into();
    let out = apply_local(&mut cfg, &plan, &selected, &[]);
    assert_eq!(out.applied, 1);
    assert_eq!(out.unknown_ids, vec!["bogus.id".to_string()]);
    assert_eq!(cfg.hotkeys.cancel_buy, "alt-z");
    // Not selected, so it keeps whatever the config held — the shipped default, not the plan's value.
    assert_eq!(
        cfg.hotkeys.panic_sell,
        crate::config::HotkeysConfig::default().panic_sell
    );
}

/// Three cores in two groups, as the group-item tests below see them: ids 1 and 2 in `desk-a`,
/// id 3 in `desk-b`, none on its own per-core set.
fn three_cores() -> AppConfig {
    let mut cfg = AppConfig::blank(None);
    for id in 1..=3u64 {
        cfg.servers.push(crate::config::ServerConfig {
            id,
            uid: id,
            name: format!("s{id}"),
            active: true,
            show_window: true,
            feed: crate::config::FeedFlags::default(),
            key: crate::config::Secret::new(String::new()),
            group: if id <= 2 { "desk-a" } else { "desk-b" }.into(),
            market: "BINANCE_FUTURES".into(),
            color: [0xFF, 0xB3, 0x47],
            synthetic: false,
            chart_bundle: String::new(),
            default_alert_strategy: 0,
            own_trade_config: false,
            strat_slots: None,
            manual_strategy: None,
            trade: None,
            transport: None,
            workspace_membership: crate::config::WorkspaceMembership::default(),
        });
    }
    cfg
}

/// Regression target: applying only the first selected core's group leaves another selected
/// window group on a different F-key slot than the MoonBot import preview promised.
#[test]
fn selected_cores_update_their_unique_groups() {
    let mut cfg = three_cores();
    let plan = plan_with(
        vec![],
        vec![],
        vec![change(
            "group.order_size_sel",
            PlannedValue::OrderSizeSel(3),
        )],
    );
    let out = apply_local(&mut cfg, &plan, &all_ids(&plan), &[1, 3]);
    assert_eq!(out.applied, 1);
    assert_eq!(cfg.group("desk-a").trade.order_size_sel, 3);
    assert_eq!(cfg.group("desk-b").trade.order_size_sel, 3);
}

/// The manual-trading generation lands in the group's own set, verbatim for the sizes and through
/// the group's TP-mode quantization for the percentages; the slot is re-based from Moonbot's
/// 0-based `sbNum` to the group's 1..=6.
#[test]
fn sizes_and_fixed_sell_land_in_the_group_set() {
    let mut cfg = three_cores();
    let plan = plan_with(
        vec![],
        vec![],
        vec![
            change(
                "group.order_sizes",
                PlannedValue::OrderSizes([50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0]),
            ),
            change(
                "group.fixed_sell_prices",
                PlannedValue::FixedSellPrices([0.5, 1.0, 1.5, 2.0, 3.0, 5.0]),
            ),
            change("group.fixed_sell_sel", PlannedValue::FixedSellSel(2)),
        ],
    );
    let out = apply_local(&mut cfg, &plan, &all_ids(&plan), &[1, 3]);
    assert_eq!(out.applied, 3);
    for group in ["desk-a", "desk-b"] {
        let trade = cfg.group(group).trade;
        assert_eq!(
            trade.order_sizes_usd,
            [50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0]
        );
        assert_eq!(trade.exit.fixed_sell_pcts, [0.5, 1.0, 1.5, 2.0, 3.0, 5.0]);
        assert_eq!(trade.exit.fixed_sell_slot, Some(3));
    }
}

/// A selected core on its own per-core set is skipped: the import never writes a core's set, and
/// its group is reached only through another selected core that reads the group's.
#[test]
fn a_core_on_its_own_settings_is_skipped() {
    let mut cfg = three_cores();
    cfg.servers[2].own_trade_config = true; // id 3, alone in desk-b
    let before = cfg.group("desk-b").trade.clone();
    let plan = plan_with(
        vec![],
        vec![],
        vec![change(
            "group.order_sizes",
            PlannedValue::OrderSizes([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        )],
    );
    let out = apply_local(&mut cfg, &plan, &all_ids(&plan), &[1, 3]);
    assert_eq!(out.applied, 1);
    assert_eq!(
        cfg.group("desk-a").trade.order_sizes_usd,
        [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
    );
    assert_eq!(cfg.group("desk-b").trade, before);
    assert!(cfg.servers[2].trade.is_none());
}
