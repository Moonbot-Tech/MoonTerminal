//! Applies SELECTED [`MoonBotImportPlan`] items to an `AppConfig` draft copy.
//! Local settings only: terminal (UI theme, hotkeys), chart/lines (colors), and group-local
//! preset selection applied through target cores. The `core_commands` group (fixed-sell) is NOT
//! included here: it is reserved for preview and is not currently sent to cores.
//!
//! The function mutates the supplied config IN MEMORY and writes nothing to disk. The Save button
//! calls `AppConfig::save_with_snapshot()` (spec section 12). A snapshot is required because import
//! changes many settings at once, which is precisely when rollback is needed.

use std::collections::HashSet;

use super::plan::{MoonBotImportPlan, PlannedValue, SettingChange};
use crate::config::{AppConfig, UiThemeMode};

/// Application result: how many items were applied and which ids were unrecognized.
///
/// The unrecognized list is a bug guard and is normally empty. This does not panic, so one
/// item cannot abort the entire application operation.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub applied: usize,
    pub unknown_ids: Vec<String>,
}

/// Applies selected local plan changes to `cfg`, where `selected` contains item ids.
///
/// `target_core_ids` identifies cores whose unique groups receive group-local values.
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
    for item in plan.group_items.iter().filter(|c| selected.contains(&c.id)) {
        if apply_group_item(cfg, item, target_core_ids) {
            out.applied += 1;
        } else {
            out.unknown_ids.push(item.id.clone());
        }
    }
    out
}

/// Applies a terminal/chart item by id. Returns `false` for an unrecognized id.
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
    // Preset slots: hotkey.order_size.{i} / hotkey.sell_preset.{i}.
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
    // Mirrors plan::hotkey_field with the same 17 destination fields.
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

/// Applies a color item: `{target}.{side}`, where side = light|dark.
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
        // graphFont: one item colors all neutral labels (spec section 8).
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

/// Applies a group-local item to the unique groups containing selected cores.
///
/// An empty core list is not a plan error: the item is deliberately applied to no groups.
fn apply_group_item(cfg: &mut AppConfig, item: &SettingChange, target_core_ids: &[u64]) -> bool {
    /// Return selected group names once even when multiple target cores share a group.
    fn target_groups(cfg: &AppConfig, target_core_ids: &[u64]) -> Vec<String> {
        let mut groups = Vec::new();
        for server in cfg
            .servers
            .iter()
            .filter(|server| target_core_ids.contains(&server.id))
        {
            if !groups.contains(&server.group) {
                groups.push(server.group.clone());
            }
        }
        groups
    }

    match (&item.value, item.id.as_str()) {
        (PlannedValue::OrderSizeSel(sel), "group.order_size_sel") => {
            for group in target_groups(cfg, target_core_ids) {
                cfg.group_mut(&group).trade.order_size_sel = *sel;
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
