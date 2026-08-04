//! Price-scale (Y) dropdowns for the chart-tab strip and AddToChart stacks.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize,
    MoonPalette, MoonTooltipView,
};

/// Price-scale (Y) presets matching egui `dock/controls.rs::SCALES` one-for-one.
///
/// `None` means Auto. The first tuple element is a STABLE, hidden menu-item key. Percentage
/// labels are universal and displayed as written, while the Auto label comes from the locale.
const SCALES: [(&str, Option<f32>); 6] = [
    ("auto", None),
    ("50%", Some(0.50)),
    ("20%", Some(0.20)),
    ("10%", Some(0.10)),
    ("5%", Some(0.05)),
    ("2%", Some(0.02)),
];

/// Returns a preset label for a scale step.
///
/// `None`, or a custom dragged scale that does not exactly match a numeric step, returns the
/// localized Auto label. Numeric presets use their percentage labels from `SCALES`.
fn scale_label(scale: Option<f32>) -> String {
    SCALES
        .iter()
        .find(|(_, value)| *value == scale && value.is_some())
        .map(|(label, _)| (*label).to_string())
        .unwrap_or_else(|| t!("toolbar.scale_auto").to_string())
}

/// Returns the next price-scale step for the Scale +/- shortcuts.
///
/// `SCALES` is the single ordering source: Auto → 50% → 20% → 10% → 5% → 2%, with increasing
/// indices zooming IN. `zoom_in=true` (Scale +) moves toward a smaller percentage; `false`
/// (Scale -) moves outward toward Auto. The ends clamp without wrapping. Exact preset values
/// match directly; a custom dragged value starts from its nearest numeric step.
pub(crate) fn step_scale(current: Option<f32>, zoom_in: bool) -> Option<f32> {
    let idx = SCALES
        .iter()
        .position(|(_, v)| *v == current)
        .unwrap_or_else(|| match current {
            None => 0,
            Some(cur) => SCALES
                .iter()
                .enumerate()
                .filter_map(|(i, (_, v))| v.map(|v| (i, (v - cur).abs())))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
                .unwrap_or(0),
        });
    let next = if zoom_in {
        (idx + 1).min(SCALES.len() - 1)
    } else {
        idx.saturating_sub(1)
    };
    SCALES[next].1
}

/// Builds the shared price-scale dropdown.
///
/// Tab and AddToChart-stack variants differ only in IDs, trigger size (`Micro` or
/// `ToolbarCompact`), and the `on_pick` destination. Appearance, tooltip, magnifier, and the
/// localized short Auto marker are shared.
fn scale_dropdown(
    _cx: &App,
    scale: Option<f32>,
    tip_id: &'static str,
    dropdown_id: &'static str,
    item_key_prefix: &'static str,
    trigger_size: MoonButtonSize,
    p: MoonPalette,
    on_pick: impl Fn(Option<f32>, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let selected_label = scale_label(scale);
    let mut items = Vec::with_capacity(SCALES.len());
    for (key, pct) in SCALES {
        let on_pick = on_pick.clone();
        items.push(
            MoonMenuItem::with_key(format!("{item_key_prefix}-{key}"), scale_label(pct))
                .selected(scale == pct)
                .checked(scale == pct)
                .on_click(move |_, _, cx| on_pick(pct, cx)),
        );
    }

    // Use a magnifier instead of the word "SCALE" and a localized short Auto marker for compactness;
    // expose the full localized "Scale" label through the tooltip.
    let trigger_val = if scale.is_none() {
        t!("toolbar.scale_auto_short").to_string()
    } else {
        selected_label
    };
    div()
        .id(tip_id)
        .tooltip(|_window, cx| {
            cx.new(|_| MoonTooltipView::new(t!("toolbar.scale").to_string()))
                .into()
        })
        .child(
            MoonDropdown::new(dropdown_id)
                // Narrowed 20% from 72: the trigger only ever carries the magnifier plus "A" or a
                // three-character percentage, so the freed width goes to the tab strip beside it.
                .trigger_width_scaled(58.0)
                .trigger_variant(MoonButtonVariant::Neutral)
                .trigger_size(trigger_size)
                .menu_width_scaled(116.0)
                .menu_size(MoonMenuSize::Compact)
                .segment(
                    MoonButtonSegment::new("🔍")
                        .color(p.text_muted)
                        .weight(400.0),
                )
                .segment(
                    MoonButtonSegment::new(trigger_val)
                        .color(p.text)
                        .weight(500.0),
                )
                .items(items),
        )
}

/// Builds the main window's chart-tab-strip scale dropdown beside the layout-settings button.
///
/// Applies the selection ONLY to the active Main, AddToChart, or Custom tab without changing other
/// tabs or windows. Persistence covers Main and AddToChart; the current `persist_scales` path omits
/// Custom tabs.
pub(crate) fn scale_dropdown_for_tabs(
    cx: &App,
    scale: Option<f32>,
    tabs: Entity<crate::chart_tabs::ChartTabs>,
    p: MoonPalette,
) -> AnyElement {
    scale_dropdown(
        cx,
        scale,
        "tabs-scale-tip",
        "tabs-scale-dropdown",
        "scale-tab",
        MoonButtonSize::Micro,
        p,
        move |pct, cx| {
            tabs.update(cx, |t, tcx| t.pick_active_scale(pct, tcx));
        },
    )
    .into_any_element()
}

/// Builds an AddToChart-stack scale dropdown that updates every contained `ChartPanel`.
///
/// This preserves the Delphi model of one graph as one entity while keeping scale control unified
/// for the window/tab.
pub(crate) fn scale_dropdown_for_add_stack(
    cx: &App,
    scale: Option<f32>,
    stack: Entity<crate::chart_tabs::AddChartStack>,
    p: MoonPalette,
) -> AnyElement {
    scale_dropdown(
        cx,
        scale,
        "detached-stack-scale-tip",
        "detached-stack-scale-dropdown",
        "scale-stack",
        MoonButtonSize::ToolbarCompact,
        p,
        move |pct, cx| {
            stack.update(cx, |st, scx| st.set_scale(pct, scx));
        },
    )
    .into_any_element()
}
