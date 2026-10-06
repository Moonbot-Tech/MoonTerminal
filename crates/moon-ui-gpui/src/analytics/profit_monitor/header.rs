//! Profit Monitor header.

use super::*;

/// Return the localized grouping title.
///
/// Args:
///     group: Grouping axis.
///
/// Returns:
///     User-facing selector label.
pub(super) fn group_title(group: GroupMode) -> String {
    match group {
        GroupMode::Core => t!("profit_monitor.group.core"),
        GroupMode::Exchange => t!("profit_monitor.group.exchange"),
    }
    .to_string()
}

/// Render the period preset as one standard MoonUI dropdown.
///
/// The presets arrive already grouped by [`MonitorPeriod::GROUPS`] — day, calendar, rolling, then
/// the unbounded one — with a separator standing BETWEEN the groups and never after the last, the
/// same grouped layout the Report panel's period picker uses.
///
/// Args:
///     selected: Currently active preset.
///     view: Monitor entity receiving selection changes.
///
/// Returns:
///     A compact dropdown carrying every period choice, grouped by family.
pub(super) fn period_dropdown(
    selected: MonitorPeriod,
    view: Entity<ProfitMonitorView>,
    cx: &App,
) -> MoonDropdown {
    let mut items: Vec<MoonMenuItem> = Vec::new();
    for group in MonitorPeriod::GROUPS {
        if !items.is_empty() {
            items.push(MoonMenuItem::separator());
        }
        let view = view.clone();
        let options: Vec<(MonitorPeriod, SharedString, SharedString)> = group
            .iter()
            .map(|period| {
                (
                    *period,
                    SharedString::from(period.id()),
                    SharedString::from(period.title()),
                )
            })
            .collect();
        // Per group rather than once over a flat list: `radio_items` marks the current preset in
        // whichever group holds it, so the selection is unaffected by the split.
        items.extend(crate::panels::radio_items(
            options,
            selected,
            crate::panels::RadioMark::Highlight,
            move |app, period| {
                view.update(app, |this, cx| this.set_period(period, cx));
            },
        ));
    }
    MoonDropdown::new("profit-monitor-period")
        .label(selected.title())
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::density(cx))
        .fit_trigger_width(100.0, 150.0)
        .fit_menu_width(130.0, 190.0)
        .items(items)
}

/// Render the ⚙ button that opens the monitor's display settings.
///
/// Args:
///     open: Whether the settings popup is currently showing.
///
/// Returns:
///     The settings popover's trigger, from the shared gear helper.
pub(super) fn settings_trigger(open: bool) -> impl IntoElement {
    crate::panels::popup_gear_trigger(
        "profit-monitor-settings",
        t!("profit_monitor.settings.title").to_string(),
        open,
    )
}

/// Render the custom title bar shared with other tool visuals.
///
/// Args:
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Title cluster and native-looking controls.
pub(super) fn window_header(palette: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, HEADER_HEIGHT, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(palette.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(palette.border, 1.0))
        .child(
            MoonWindowFrame::tool("profit-monitor-title", 0.0)
                .title_cluster(t!("profit_monitor.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |element| {
            element.child(
                MoonWindowFrame::tool("profit-monitor-controls", 0.0)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(HEADER_HEIGHT)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Render automatic-refresh status without requiring a manual button.
///
/// Args:
///     active: Whether a database replacement is in flight.
///     failed: Whether the latest automatic replacement failed while old rows remain visible.
///     show_label: Whether the colored dot has room for its text label.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Compact live-status cluster.
pub(super) fn auto_status(
    active: bool,
    failed: bool,
    show_label: bool,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    let label = if active {
        t!("profit_monitor.refreshing").to_string()
    } else if failed {
        t!("profit_monitor.refresh_failed").to_string()
    } else {
        t!("profit_monitor.auto").to_string()
    };
    h_flex()
        .id("profit-monitor-auto-status")
        .flex_none()
        .gap(design::ui_px(cx, 6.0))
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(moon(palette.text_muted))
        .tooltip(crate::panels::common::text_tooltip(label.clone()))
        .child(
            div()
                .w(design::ui_px(cx, 6.0))
                .h(design::ui_px(cx, 6.0))
                .rounded_full()
                .bg(moon(if active {
                    palette.orange
                } else if failed {
                    palette.red
                } else {
                    palette.green
                })),
        )
        .when(show_label, |status| status.child(label))
        .into_any_element()
}
