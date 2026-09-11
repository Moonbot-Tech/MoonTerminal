//! Applies SELECTED [`MoonBotImportPlan`] items to an `AppConfig` draft copy.
//! Local settings only: terminal (UI theme, hotkeys), chart/lines (colors), and group-local
//! preset selection applied through target cores. The `core_commands` group (fixed-sell) is NOT
//! included here: it is reserved for preview and is not currently sent to cores.
//!
//! The function mutates the supplied config IN MEMORY and writes nothing to disk. The Save button
//! calls `AppConfig::save()` (spec section 12). Daily recovery snapshots remain independent of the
//! number of edits applied by one import.

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
            // The plan carries a theme as ONE BOOLEAN, so "dark" cannot say WHICH dark theme it
            // means. Writing `Dark` unconditionally would convert Graphite to Dark — and it would
            // do it on a row the preview marks `same` (`plan.rs:145` compares the same boolean),
            // so the user is told nothing changes and then loses their theme. Move the mode only
            // when the colour SET actually differs; importing "dark" onto a mode that already
            // draws the dark set is genuinely a no-op.
            if *light != cfg.ui_theme_mode.is_light() {
                cfg.ui_theme_mode = if *light {
                    UiThemeMode::Light
                } else {
                    UiThemeMode::Dark
                };
            }
            true
        }
        (PlannedValue::SplitParts(parts), "hotkey.split_parts") => {
            cfg.hotkeys.split_parts = *parts;
            true
        }
        (PlannedValue::Keystroke(ks), id) => apply_hotkey(cfg, id, ks),
        (PlannedValue::Rgb(rgb), id) => apply_color(cfg, id, *rgb),
        _ => false,
    }
}

/// Writes one imported keystroke to the slot its plan id names.
///
/// The id is read back through `plan::slot_for_id`, the same table that wrote it, so the plan and
/// the application cannot disagree about which field an id means. They used to be two hand-kept
/// maps in two files — eighteen names each, with a comment in this one saying it "mirrors" the
/// other — and each ended in a silent fallback, so a name typed differently on one side imported
/// nothing and reported nothing.
///
/// `false` for an id this build does not know, which the caller counts as an unapplied item.
fn apply_hotkey(cfg: &mut AppConfig, id: &str, ks: &str) -> bool {
    let Some(slot) = super::plan::slot_for_id(id) else {
        return false;
    };
    cfg.hotkeys.set_key(slot, ks.to_string());
    // Writing the value it already held is not a failure to import: the caller counts items it
    // applied, and `set_key` answers whether anything CHANGED, which is a different question.
    true
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
