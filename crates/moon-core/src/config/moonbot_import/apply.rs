//! Применение ВЫБРАННЫХ пунктов [`MoonBotImportPlan`] к `AppConfig` (draft-копии).
//! Только локальные настройки: терминал (тема UI, хоткеи), график/линии (цвета),
//! per-core пресеты размера — для явно выбранных ядер. Группа `core_commands`
//! (fixed-sell) сюда НЕ входит: она уходит ядрам отдельным шагом через
//! существующий ClientSettings-путь (ТЗ §10/§12).
//!
//! The function mutates the supplied config IN MEMORY and writes nothing to disk. The Save button
//! calls `AppConfig::save_with_snapshot()` (spec section 12). A snapshot is required because import
//! changes many settings at once, which is precisely when rollback is needed.

use std::collections::HashSet;

use super::plan::{MoonBotImportPlan, PlannedValue, SettingChange};
use crate::config::{AppConfig, UiThemeMode};

/// Итог применения: сколько пунктов легло и какие id не распознаны (баг-гард —
/// в норме пусто; не паникуем, чтобы не ронять применение из-за одного пункта).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub applied: usize,
    pub unknown_ids: Vec<String>,
}

/// Применить выбранные (`selected` — id пунктов) локальные изменения плана к `cfg`.
/// `target_core_ids` — ядра (ServerConfig.id) для per-core группы.
pub fn apply_local(
    cfg: &mut AppConfig,
    plan: &MoonBotImportPlan,
    selected: &HashSet<String>,
    target_core_ids: &[u64],
) -> ApplyOutcome {
    let mut out = ApplyOutcome::default();
    for item in plan
        .terminal
        .iter()
        .chain(plan.hotkeys.iter())
        .chain(plan.chart.iter())
        .filter(|c| selected.contains(&c.id))
    {
        if apply_item(cfg, item) {
            out.applied += 1;
        } else {
            out.unknown_ids.push(item.id.clone());
        }
    }
    for item in plan.per_core.iter().filter(|c| selected.contains(&c.id)) {
        if apply_per_core(cfg, item, target_core_ids) {
            out.applied += 1;
        } else {
            out.unknown_ids.push(item.id.clone());
        }
    }
    out
}

/// Терминальный/чартовый пункт по id. `false` = id не распознан.
fn apply_item(cfg: &mut AppConfig, item: &SettingChange) -> bool {
    match (&item.value, item.id.as_str()) {
        (PlannedValue::UiThemeLight(light), "ui.theme_mode") => {
            cfg.ui_theme_mode = if *light {
                UiThemeMode::Light
            } else {
                UiThemeMode::Dark
            };
            true
        }
        (PlannedValue::Keystroke(ks), id) => apply_hotkey(cfg, id, ks),
        (PlannedValue::Rgb(rgb), id) => apply_color(cfg, id, *rgb),
        _ => false,
    }
}

fn apply_hotkey(cfg: &mut AppConfig, id: &str, ks: &str) -> bool {
    let h = &mut cfg.hotkeys;
    // Слоты пресетов: hotkey.order_size.{i} / hotkey.sell_preset.{i}.
    if let Some(rest) = id.strip_prefix("hotkey.order_size.") {
        if let Some(slot) = parse_slot::<6>(rest) {
            h.order_size[slot] = ks.to_string();
            return true;
        }
        return false;
    }
    if let Some(rest) = id.strip_prefix("hotkey.sell_preset.") {
        if let Some(slot) = parse_slot::<6>(rest) {
            h.sell_preset[slot] = ks.to_string();
            return true;
        }
        return false;
    }
    let Some(field) = id.strip_prefix("hotkey.") else {
        return false;
    };
    // Зеркало plan::hotkey_field — те же 17 полей-приёмников.
    let target = match field {
        "cancel_buy" => &mut h.cancel_buy,
        "panic_sell" => &mut h.panic_sell,
        "panic_sell_one" => &mut h.panic_sell_one,
        "cancel_all_buys" => &mut h.cancel_all_buys,
        "join_sells" => &mut h.join_sells,
        "switch_charts" => &mut h.switch_charts,
        "new_long" => &mut h.new_long,
        "new_short" => &mut h.new_short,
        "split_order" => &mut h.split_order,
        "split_order_x" => &mut h.split_order_x,
        "shift_buy_up" => &mut h.shift_buy_up,
        "shift_buy_down" => &mut h.shift_buy_down,
        "shift_sell_up" => &mut h.shift_sell_up,
        "shift_sell_down" => &mut h.shift_sell_down,
        "scale_plus" => &mut h.scale_plus,
        "scale_minus" => &mut h.scale_minus,
        "switch_figure" => &mut h.switch_figure,
        _ => return false,
    };
    *target = ks.to_string();
    true
}

fn parse_slot<const N: usize>(s: &str) -> Option<usize> {
    s.parse::<usize>().ok().filter(|i| *i < N)
}

/// Цветовой пункт: `{target}.{side}`, side = light|dark.
fn apply_color(cfg: &mut AppConfig, id: &str, rgb: [u8; 3]) -> bool {
    let (target, side) = match id.rsplit_once('.') {
        Some(pair) => pair,
        None => return false,
    };
    let light = match side {
        "light" => true,
        "dark" => false,
        _ => return false,
    };
    let theme = cfg.theme.get_mut(light);
    let orders = cfg.orders.get_mut(light);
    match target {
        "theme.bg" => theme.bg = rgb,
        "theme.grid" => theme.grid = rgb,
        "theme.cross" => theme.cross = rgb,
        // graphFont: один пункт красит все нейтральные подписи (ТЗ §8).
        "theme.labels" => {
            theme.axis_label = rgb;
            theme.caption_label = rgb;
            theme.readout_label = rgb;
            theme.label_neutral = rgb;
        }
        "theme.candle_up" => theme.candle_up = rgb,
        "theme.candle_down" => theme.candle_down = rgb,
        "theme.candle_neutral" => theme.candle_neutral = rgb,
        "theme.book_bid" => theme.book_bid = rgb,
        "theme.book_ask" => theme.book_ask = rgb,
        "orders.buy.color" => orders.buy.color = rgb,
        "orders.buy.pending_color" => orders.buy.pending_color = Some(rgb),
        "orders.sell.color" => orders.sell.color = rgb,
        "orders.buy_short.color" => orders.buy_short.color = rgb,
        "orders.sell_short.color" => orders.sell_short.color = rgb,
        "orders.trailing.color" => orders.trailing.color = rgb,
        "orders.liq.color" => orders.liq.color = rgb,
        _ => return false,
    }
    true
}

/// Per-core пункт для выбранных ядер. `false` = id не распознан (пустой список
/// ядер — не ошибка плана: пункт применён «в никуда» осознанным выбором).
fn apply_per_core(cfg: &mut AppConfig, item: &SettingChange, target_core_ids: &[u64]) -> bool {
    let apply_to_cores = |cfg: &mut AppConfig, f: &dyn Fn(&mut crate::config::ServerConfig)| {
        for s in cfg
            .servers
            .iter_mut()
            .filter(|s| target_core_ids.contains(&s.id))
        {
            f(s);
        }
    };
    match (&item.value, item.id.as_str()) {
        (PlannedValue::OrderSizes(sizes), "core.order_sizes") => {
            let sizes = *sizes;
            apply_to_cores(cfg, &move |s| s.order_sizes = Some(sizes));
            true
        }
        (PlannedValue::OrderSizeSel(sel), "core.order_size_sel") => {
            let sel = *sel;
            apply_to_cores(cfg, &move |s| s.order_size_sel = Some(sel));
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn applies_theme_hotkeys_and_colors() {
        let mut cfg = AppConfig::default();
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

    #[test]
    fn selection_filter_and_unknown_ids() {
        let mut cfg = AppConfig::default();
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

    #[test]
    fn per_core_targets_only_selected_cores() {
        let mut cfg = AppConfig::default();
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
}
