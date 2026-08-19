//! The fixed caption row above the By-IP tree: one sortable heading per column.
//!
//! It is a separate element from the rows it labels, so both sides read the same
//! [`ByIpWidths`] and the same gap/inset constants — that shared geometry is the only thing
//! keeping a caption over its values once the columns shrink.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex};
use rust_i18n::t;

use super::CoreStatusView;
use super::by_ip_widths::{
    ByIpWidths, CELL_GAP_W, CHEVRON_W, ROW_GAP_W, ROW_INSET_REMS, TREE_SCROLLBAR_W,
};
use super::ordering::GroupSortField;

/// Render the fixed By IP caption row: one heading per column, aligned to the tree rows. Every
/// heading except IP is a sort control — clicking it sorts the server list by that column (an arrow
/// marks the active one). Warnings still pin to the top regardless of the sort.
///
/// Args:
///     p: Active Moon palette.
///     sort: The active `(field, ascending)` sort, to mark the column and drive the toggle.
///     w: Shared column widths for this frame — the same values the rows below use.
///     weak_view: Non-owning panel handle for the sort click.
///     cx: Application context, for the font-scaled caption size and dot-column width.
///
/// Returns:
///     A single header row with a subtle bottom divider.
pub(super) fn server_header(
    p: MoonPalette,
    sort: (GroupSortField, bool),
    w: ByIpWidths,
    weak_view: &WeakEntity<CoreStatusView>,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .gap(px(ROW_GAP_W))
        // The same `rems` inset `MoonListItem` applies to the rows below, so the captions stay over
        // their values at every font setting rather than at exactly one of them.
        .pl(rems(ROW_INSET_REMS))
        .pr(rems(ROW_INSET_REMS))
        .h(px(crate::design::TABLE_HEAD_H))
        // Clip like the rows do: without this the captions spill past the panel edge while the
        // values under them are already clipped.
        .overflow_hidden()
        // Match the MoonDataTable header: filled head background and a bottom separator.
        .bg(rgb(p.table_head))
        .border_b_1()
        .border_color(rgb(p.border))
        .text_size(crate::design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        // Chevron gutter (matches `server_row`'s 12 px expand trigger).
        .child(div().w(px(CHEVRON_W)).flex_none())
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(ROW_GAP_W))
                .overflow_hidden()
                .child(col_sort_header(
                    t!("core_status.col.server").to_string(),
                    GroupSortField::Name,
                    w.name,
                    sort,
                    p,
                    weak_view,
                ))
                // IP is masked, so it is not a sort key — a plain caption.
                .child(
                    div()
                        .w(px(w.ip))
                        .flex_none()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(t!("core_status.hdr.ip").to_string()),
                )
                .child(div().flex_1())
                .child(metric_sort_header(
                    t!("core_status.hdr.cpu").to_string(),
                    GroupSortField::Cpu,
                    w.cpu,
                    w.icon,
                    sort,
                    p,
                    weak_view,
                ))
                .child(metric_sort_header(
                    t!("core_status.hdr.mem").to_string(),
                    GroupSortField::Mem,
                    w.mem,
                    w.icon,
                    sort,
                    p,
                    weak_view,
                ))
                .child(metric_sort_header(
                    t!("core_status.chart_ping").to_string(),
                    GroupSortField::Ping,
                    w.ping,
                    w.icon,
                    sort,
                    p,
                    weak_view,
                ))
                .child(metric_sort_header(
                    t!("core_status.chart_exch").to_string(),
                    GroupSortField::Exch,
                    w.ping,
                    w.icon,
                    sort,
                    p,
                    weak_view,
                ))
                .child(metric_sort_header(
                    t!("core_status.hdr.api_key").to_string(),
                    GroupSortField::ApiKey,
                    w.api,
                    w.icon,
                    sort,
                    p,
                    weak_view,
                ))
                // No icon lead: startup has no `WarnAxis` behind it, so it uses the plain
                // heading like the core ratio rather than a metric heading whose lead never lights.
                .child(col_sort_header(
                    t!("core_status.col.startup").to_string(),
                    GroupSortField::Startup,
                    w.startup,
                    sort,
                    p,
                    weak_view,
                ))
                .child(col_sort_header(
                    t!("core_status.cores").to_string(),
                    GroupSortField::Cores,
                    w.cores,
                    sort,
                    p,
                    weak_view,
                ))
                // Trailing slots, mirroring the rows so the "Ядра" caption lands over the ratio:
                // the connectivity-warning slot, the status dot, and the tree's overlay scrollbar.
                .child(div().w(px(w.icon)).flex_none())
                .child(div().w(px(crate::design::status_dot_w(cx))).flex_none())
                .child(div().w(px(TREE_SCROLLBAR_W)).flex_none()),
        )
}

/// The sort arrow suffix for a heading, matching the MoonDataTable header (`↑` ascending, `↓`
/// descending), or empty when another field is active.
fn sort_arrow(field: GroupSortField, sort: (GroupSortField, bool)) -> &'static str {
    if sort.0 == field {
        if sort.1 { " \u{2191}" } else { " \u{2193}" }
    } else {
        ""
    }
}

/// A clickable metric heading offset by the metric icon's lead so it sits over the value box, with
/// the sort arrow and an active-column highlight.
///
/// Args:
///     label: Localized column heading.
///     field: The sort field this heading selects.
///     value_w: The matching value-box width from the row's `metric_cell`.
///     icon_w: The matching warning-icon lead, so the caption sits over the value.
///     sort: The active sort, for the arrow and highlight.
///     p: Active Moon palette.
///     weak_view: Non-owning panel handle for the sort click.
///
/// Returns:
///     A fixed-width clickable caption cell aligned to its column.
fn metric_sort_header(
    label: String,
    field: GroupSortField,
    value_w: f32,
    icon_w: f32,
    sort: (GroupSortField, bool),
    p: MoonPalette,
    weak_view: &WeakEntity<CoreStatusView>,
) -> impl IntoElement {
    let text = format!("{label}{}", sort_arrow(field, sort));
    let active = sort.0 == field;
    let weak_view = weak_view.clone();
    h_flex()
        .flex_none()
        .gap(px(CELL_GAP_W))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _, app| {
            if let Some(view) = weak_view.upgrade() {
                view.update(app, |this, cx| this.set_group_sort(field, cx));
            }
        })
        .child(div().w(px(icon_w)).flex_none())
        .child(
            div()
                .w(px(value_w))
                // Clip like the value box below it: at the shrink floor a caption would otherwise
                // wrap to a second line inside the fixed-height header.
                .overflow_hidden()
                .whitespace_nowrap()
                .when(active, |el| el.text_color(rgb(p.text_soft)))
                .child(text),
        )
}

/// A clickable fixed-width heading for a non-metric column (name, cores), with the sort arrow and an
/// active-column highlight.
///
/// Args:
///     label: Localized column heading.
///     field: The sort field this heading selects.
///     width: The matching column width.
///     sort: The active sort, for the arrow and highlight.
///     p: Active Moon palette.
///     weak_view: Non-owning panel handle for the sort click.
///
/// Returns:
///     A fixed-width clickable caption.
fn col_sort_header(
    label: String,
    field: GroupSortField,
    width: f32,
    sort: (GroupSortField, bool),
    p: MoonPalette,
    weak_view: &WeakEntity<CoreStatusView>,
) -> impl IntoElement {
    let text = format!("{label}{}", sort_arrow(field, sort));
    let active = sort.0 == field;
    let weak_view = weak_view.clone();
    div()
        .w(px(width))
        .flex_none()
        .overflow_hidden()
        .whitespace_nowrap()
        .cursor_pointer()
        .when(active, |el| el.text_color(rgb(p.text_soft)))
        .on_mouse_down(MouseButton::Left, move |_, _, app| {
            if let Some(view) = weak_view.upgrade() {
                view.update(app, |this, cx| this.set_group_sort(field, cx));
            }
        })
        .child(text)
}
