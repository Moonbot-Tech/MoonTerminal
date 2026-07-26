//! Order-size (F1-F6) and fixed-sell (S1-S6) preset strips with native click, double-click,
//! Ctrl+wheel, tooltip, and inline-edit behavior.

use gpui::*;
use moon_core::feed::ClientSettingsEdit;
use moon_ui::{MoonAccent, MoonInput, MoonInputState, MoonSegmentItem, MoonSegmentedControl};
use rust_i18n::t;

use super::fmt::{fmt_adaptive, fmt_sell_pct, scroll_dy, wheel_step};
use crate::Backend;

/// Base floor for a preset cell's fitted width.
const MIN_CELL_W: f32 = 34.0;
/// Base ceiling for a preset cell's fitted width.
///
/// An anomalous value cannot stretch the toolbar across the window; MoonUI truncates only the
/// visible label while the inline editor still exposes the complete value.
const MAX_CELL_W: f32 = 104.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Action selected by an order-size preset click.
enum SizeClickAction {
    Select(usize),
    Edit(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Action selected by a fixed-sell preset click.
enum SellClickAction {
    SelectFixed(usize),
    EngageMain,
    Edit(usize),
}

/// Resolve an order-size click without coupling click-count semantics to backend mutation.
///
/// Args:
///     index: Zero-based preset index.
///     click_count: Native click count reported by GPUI.
///
/// Returns:
///     Selection for a single click or inline editing for a double-or-later click.
fn size_click_action(index: usize, click_count: usize) -> SizeClickAction {
    if click_count >= 2 {
        SizeClickAction::Edit(index)
    } else {
        SizeClickAction::Select(index)
    }
}

/// Resolve a fixed-sell click without coupling click-count semantics to backend mutation.
///
/// Args:
///     index: Zero-based preset index.
///     selected_slot: Currently engaged one-based fixed-sell slot.
///     click_count: Native click count reported by GPUI.
///
/// Returns:
///     Inline editing for a double-or-later click, main take profit when the active slot is clicked,
///     or the one-based fixed-sell slot to engage.
fn sell_click_action(
    index: usize,
    selected_slot: Option<usize>,
    click_count: usize,
) -> SellClickAction {
    if click_count >= 2 {
        SellClickAction::Edit(index)
    } else if selected_slot == Some(index + 1) {
        SellClickAction::EngageMain
    } else {
        SellClickAction::SelectFixed(index + 1)
    }
}

/// Return the direction of one preset step, or `None` when the gesture must not edit a value.
///
/// Ctrl prevents an ordinary toolbar scroll from rewriting trading parameters. A non-zero
/// vertical delta excludes horizontal trackpad gestures.
///
/// Args:
///     modifiers: Keyboard modifiers captured with the wheel event.
///     delta: Two-dimensional wheel or trackpad delta.
///
/// Returns:
///     `Some(true)` for an upward step, `Some(false)` for a downward step, or `None`.
fn wheel_step_dir(modifiers: Modifiers, delta: ScrollDelta) -> Option<bool> {
    if !modifiers.control {
        return None;
    }
    let dy = scroll_dy(delta);
    if dy == 0.0 {
        return None;
    }
    Some(dy > 0.0)
}

/// Six preset items fitted by MoonUI before the toolbar computes its row budget.
///
/// Rendering and budgeting consume the same resolved items, so Terminal does not mirror segment
/// padding, gaps, selected text weight, truncation, or pixel rounding.
pub(super) struct FittedCells {
    items: [MoonSegmentItem; 6],
}

impl FittedCells {
    /// Fit all preset labels through MoonUI at the current theme and font size.
    ///
    /// Args:
    ///     cx: Application context used by MoonUI for theme-aware measurement.
    ///     labels: The six complete preset values.
    ///
    /// Returns:
    ///     Six fitted items with stable, pre-render resolved widths.
    pub(super) fn fit(cx: &App, labels: [String; 6]) -> Self {
        Self {
            items: labels
                .map(|label| MoonSegmentItem::new("", label).fit_width(cx, MIN_CELL_W, MAX_CELL_W)),
        }
    }

    /// Return the exact total width consumed by all six rendered cells.
    ///
    /// Returns:
    ///     Combined fitted width in rendered pixels.
    pub(super) fn total_width(&self) -> f32 {
        self.items.iter().map(MoonSegmentItem::resolved_width).sum()
    }
}

/// Format six complete group-owned preset values.
///
/// Args:
///     values: Complete values for the group.
///     fmt: Value formatter shared by all six cells.
///
/// Returns:
///     Six display labels.
fn labels(values: [f64; 6], fmt: impl Fn(f64) -> String) -> [String; 6] {
    values.map(fmt)
}

/// Return the group's complete order-size labels.
///
/// Args:
///     values: Complete group-local USD-equivalent order-size values.
///
/// Returns:
///     Six formatted order-size labels.
pub(super) fn size_labels(values: [f64; 6]) -> [String; 6] {
    labels(values, fmt_adaptive)
}

/// Return the group's complete fixed-sell percentage labels.
///
/// Args:
///     pcts: Complete group-local fixed-sell percentages.
///
/// Returns:
///     Six formatted percentage labels.
pub(super) fn sell_labels(pcts: [f64; 6]) -> [String; 6] {
    labels(pcts, |pct| format!("{}%", fmt_sell_pct(pct)))
}

/// Build the order-size preset strip.
///
/// Single click selects and persists a preset, double click requests inline editing, and
/// Ctrl+wheel changes the value by its order of magnitude.
///
/// Args:
///     cells: MoonUI-fitted preset cells used by both rendering and toolbar budgeting.
///     sel: Selected zero-based preset index.
///     edit_ix: Zero-based cell to replace with the inline editor.
///     input: Shared editor state.
///     backend: Application state receiving edits.
///     group: Window group that owns the local USD presets.
///     unit: USDT-equivalent unit shown in tooltips.
///
/// Returns:
///     The configured segmented control.
pub(super) fn size_strip(
    cells: &FittedCells,
    sel: usize,
    edit_ix: Option<usize>,
    input: &Entity<MoonInputState>,
    backend: Entity<Backend>,
    group: String,
    unit: &str,
) -> impl IntoElement {
    let unit = unit.to_string();
    let items = (0..6).map(|index| {
        cells.items[index]
            .clone()
            .selected(sel == index)
            .tooltip(t!("toolbar.size_hint", n = index + 1, unit = unit.as_str()).to_string())
    });

    let click_backend = backend.clone();
    let click_group = group.clone();
    let mut segment = MoonSegmentedControl::new("toolbar-size-presets")
        .accent(MoonAccent::Amber)
        .items(items)
        .on_click(move |index, event, _, app| {
            click_backend.update(app, |backend, cx| {
                match size_click_action(index, event.click_count()) {
                    SizeClickAction::Select(index) => {
                        backend.set_order_size_sel(&click_group, index);
                    }
                    SizeClickAction::Edit(index) => {
                        backend.order_size_edit_req = Some((click_group.clone(), index));
                    }
                }
                backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
                cx.notify();
            });
        })
        .on_scroll(move |index, event, _, app| {
            let Some(up) = wheel_step_dir(event.modifiers, event.delta) else {
                return;
            };
            backend.update(app, |backend, cx| {
                let current = backend.order_size_value(&group, index);
                let next = wheel_step(current, up, 1.0);
                if next != current {
                    backend.set_order_size_value(&group, index, next);
                    backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
                    cx.notify();
                }
            });
        });

    if let Some(index) = edit_ix.filter(|index| *index < 6) {
        segment = segment.replace_item(
            index,
            MoonInput::new("toolbar-size-edit").state(input).small(),
        );
    }
    segment.render()
}

/// Build the fixed-sell preset strip beside the main take-profit button.
///
/// Single click engages a slot (or restores main TP when clicking the active slot), double click
/// requests inline editing, and Ctrl+wheel changes the group-owned percentage. Manual-strategy
/// mode disables all cells because the toolbar passes `None`.
///
/// Args:
///     cells: MoonUI-fitted preset cells used by both rendering and toolbar budgeting.
///     sel_slot: Selected one-based fixed-sell slot.
///     edit_ix: Zero-based cell to replace with the inline editor.
///     input: Shared editor state.
///     backend: Application state receiving edits.
///     group: Interactive group; absence disables the strip.
///
/// Returns:
///     The configured segmented control.
pub(super) fn sell_strip(
    cells: &FittedCells,
    sel_slot: Option<usize>,
    edit_ix: Option<usize>,
    input: &Entity<MoonInputState>,
    backend: Entity<Backend>,
    group: Option<String>,
) -> impl IntoElement {
    let interactive = group.is_some();
    let items = (0..6).map(|index| {
        let mut item = cells.items[index]
            .clone()
            .selected(sel_slot == Some(index + 1))
            .disabled(!interactive);
        if interactive {
            item = item.tooltip(t!("toolbar.sell_hint", n = index + 1).to_string());
        }
        item
    });

    let click_backend = backend.clone();
    let click_group = group.clone();
    let mut segment = MoonSegmentedControl::new("toolbar-sell-presets")
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |index, event, _, app| {
            let Some(group) = click_group.as_deref() else {
                return;
            };
            click_backend.update(app, |backend, cx| {
                match sell_click_action(index, sel_slot, event.click_count()) {
                    SellClickAction::Edit(index) => {
                        backend.sell_edit_req = Some((group.to_string(), index));
                    }
                    SellClickAction::EngageMain => {
                        backend.edit_group_exit(group, ClientSettingsEdit::EngageMainTakeProfit);
                        backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
                    }
                    SellClickAction::SelectFixed(slot) => {
                        backend
                            .edit_group_exit(group, ClientSettingsEdit::SelectFixedSellSlot(slot));
                        backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
                    }
                }
                cx.notify();
            });
        })
        .on_scroll(move |index, event, _, app| {
            let Some(up) = wheel_step_dir(event.modifiers, event.delta) else {
                return;
            };
            let Some(group) = group.as_deref() else {
                return;
            };
            backend.update(app, |backend, cx| {
                let current = backend.fixed_sell_pct(group, index);
                let next = wheel_step(current, up, 0.5);
                if next != current {
                    backend.edit_group_exit(
                        group,
                        ClientSettingsEdit::SetFixedSellPct {
                            slot: index + 1,
                            pct: next,
                        },
                    );
                    backend.order_size_rev = backend.order_size_rev.wrapping_add(1);
                    cx.notify();
                }
            });
        });

    if let Some(index) = edit_ix.filter(|index| interactive && *index < 6) {
        segment = segment.replace_item(
            index,
            MoonInput::new("toolbar-sell-edit").state(input).small(),
        );
    }
    segment.render()
}

#[cfg(test)]
mod tests;
