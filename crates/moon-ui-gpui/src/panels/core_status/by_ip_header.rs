//! The fixed caption row above the By-IP tree: one sortable heading per column, each with a
//! draggable divider at its right edge.
//!
//! It is a separate element from the rows it labels, so both sides read the same
//! [`ByIpWidths`] and the same gap/inset constants — that shared geometry is the only thing
//! keeping a caption over its values once the columns shrink.
//!
//! # Why the resize handle is hand-rolled
//!
//! MoonUI ships no reusable column resizer. Its only one lives inside `MoonDataTable`'s private
//! header (`moon/data_table/header.rs`), driven by a private drag payload and by
//! `MoonDataTableState`'s private header-bounds tracking; `resizable::resize_handle` is
//! `pub(crate)` and is a PANEL splitter, and the exported `moon_h_resizable` family builds resizable
//! panel GROUPS, which would mean restructuring this tree into panels. So the strip below mirrors
//! MoonUI's shape deliberately — same 6 px right-edge overlay, same `ResizeColumn` cursor, same
//! double-click-to-reset gesture — while the ARITHMETIC follows this repo's own splitter idiom in
//! `strategies/split.rs` instead.
//!
//! Two differences from MoonUI are load-bearing, not stylistic:
//!
//! - **`.occlude()` is mandatory here.** MoonUI's caption sorts on `on_click`; these captions sort
//!   on `on_mouse_down`, and a normal child hitbox does not shield its parent, so without it
//!   grabbing a divider would re-sort the column on the way down.
//! - **The drag anchors at the grab, not at the cell origin.** A `flex_1` spacer sits between IP and
//!   CPU, so every column right of it is right-anchored and widening one moves its own left edge —
//!   `pointer_x - origin_x` would compound every frame. [`CoreStatusView::drag_by_ip_col`] carries
//!   the full argument.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonButton, MoonPalette, h_flex};
use rust_i18n::t;

use super::CoreStatusView;
use super::by_ip_widths::{
    ByIpCol, ByIpWidths, CELL_GAP_W, CHEVRON_W, ROW_GAP_W, ROW_INSET_REMS, TREE_SCROLLBAR_W,
};
use super::ip_cell::mask_affordance;
use super::ordering::GroupSortField;
use crate::design::moon_alpha;

/// Width of the divider grab strip, in pixels.
///
/// MIRRORS MoonUI's data-table divider (`moon/data_table/header.rs`, `tokens.ui(6.0)`). Six pixels
/// is already a small target, which is why the strip must span the header's full height — see
/// [`server_header`]'s `h_full` notes.
const HANDLE_W: f32 = 6.0;

/// Pointer x and LOGICAL column width captured at the instant a divider drag began.
///
/// Held on the panel for the life of one drag. "Logical" means the pre-shrink width from
/// [`ByIpWidths::resolved`], never the painted width — [`CoreStatusView::drag_by_ip_col`] explains
/// why anchoring on the painted width would double-apply the shrink factor.
#[derive(Clone, Copy, Debug)]
pub(super) struct ByIpDragAnchor {
    /// Column being resized.
    pub(super) col: ByIpCol,
    /// Pointer x at the grab, in window pixels.
    pub(super) mouse_x: f32,
    /// The column's logical width at the grab.
    pub(super) width: f32,
}

/// GPUI drag payload identifying the panel and the column being resized. Carries no visual.
///
/// `view` is the guard MoonUI uses for the same reason: a docked tab and a detached window each
/// register a listener for this payload type, and without the check a drag in one would resize the
/// columns of the other.
#[derive(Clone, Debug)]
struct ByIpResizeDrag {
    /// Entity id of the panel that owns the handle being dragged.
    view: EntityId,
    /// Column being resized.
    col: ByIpCol,
}

impl Render for ByIpResizeDrag {
    /// Render nothing: the payload exists only for GPUI's drag routing, and the column itself is
    /// the drag feedback.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// Build the divider strip pinned to a caption cell's right edge.
///
/// Args:
///     col: Column this divider resizes.
///     anchor_w: That column's LOGICAL (pre-shrink) width, baked in as the drag's starting point.
///     view: The owning panel, when it is still alive; `None` renders an inert spacer.
///     p: Active Moon palette.
///     window: Host window, for `listener_for`. Only `&Window` is needed — it takes `&self`.
///
/// Returns:
///     An absolutely-positioned grab strip, or an empty element when the panel has gone away.
fn resize_handle(
    col: ByIpCol,
    anchor_w: f32,
    view: Option<&Entity<CoreStatusView>>,
    p: MoonPalette,
    window: &Window,
) -> AnyElement {
    let Some(view) = view else {
        return div().into_any_element();
    };
    let view_id = view.entity_id();
    let reset_view = view.downgrade();
    let begin_view = view.downgrade();

    div()
        // A `&'static str` per column rather than a `format!`: this runs once per caption on every
        // header repaint, and the nine ids are a fixed set known at compile time.
        .id(col.handle_id())
        .absolute()
        .right(px(0.0))
        .top(px(0.0))
        .bottom(px(0.0))
        .w(px(HANDLE_W))
        // MANDATORY, not decoration: the caption behind this strip sorts on `on_mouse_down`, and a
        // normal child hitbox does not shield its parent, so grabbing the divider would re-sort the
        // column before the drag ever started. `occlude` makes the hitboxes behind report un-hovered.
        .occlude()
        .cursor(CursorStyle::ResizeColumn)
        .hover(move |style| style.bg(moon_alpha(p.accent, 0.14)))
        .tooltip(crate::panels::common::text_tooltip(
            t!("core_status.col_resize").to_string(),
        ))
        // Same gesture as a MoonDataTable divider, so the two views answer the same input: a plain
        // double-click frees this column, Shift+double-click frees all of them.
        .on_click(move |event, _, app| {
            if event.click_count() < 2 {
                return;
            }
            let all = event.modifiers().shift;
            if let Some(view) = reset_view.upgrade() {
                view.update(app, |this, cx| this.reset_by_ip_col(col, all, cx));
            }
        })
        .on_drag(
            ByIpResizeDrag { view: view_id, col },
            move |drag, _, window, app| {
                // Runs exactly ONCE, at drag start, past GPUI's drag threshold. Capturing the anchor
                // here is what keeps the arithmetic independent of a cell origin that relayout moves.
                let mouse_x = f32::from(window.mouse_position().x);
                if let Some(view) = begin_view.upgrade() {
                    view.update(app, |this, _| {
                        this.begin_by_ip_resize(ByIpDragAnchor {
                            col,
                            mouse_x,
                            width: anchor_w,
                        });
                    });
                }
                app.new(|_| drag.clone())
            },
        )
        .on_drag_move(window.listener_for(
            view,
            move |this: &mut CoreStatusView,
                  event: &DragMoveEvent<ByIpResizeDrag>,
                  _window,
                  cx: &mut Context<CoreStatusView>| {
                // Copy out of the payload before touching `cx` again: `drag` borrows from it.
                let (owner, dragged) = {
                    let drag = event.drag(cx);
                    (drag.view, drag.col)
                };
                if owner != cx.entity_id() {
                    return;
                }
                this.drag_by_ip_col(dragged, f32::from(event.event.position.x), cx);
            },
        ))
        .into_any_element()
}

/// Render the fixed By IP caption row: one heading per column, aligned to the tree rows. Every
/// heading except IP is a sort control — clicking it sorts the server list by that column (an arrow
/// marks the active one). Warnings still pin to the top regardless of the sort. Each heading also
/// carries a divider at its right edge that resizes the column.
///
/// The IP heading is the exception twice over: it does not sort, and it carries the ONE control that
/// hides or shows every address in the column. That control used to sit on each row, which cost a
/// click per server and, being cleared whenever the panel lost focus, could not hold an address on
/// screen at all.
///
/// Args:
///     p: Active Moon palette.
///     sort: The active `(field, ascending)` sort, to mark the column and drive the toggle.
///     masked: Whether the address column is currently hidden, which picks the control's affordance.
///     w: Shared column widths for this frame — the same values the rows below use, already shrunk.
///     logical: The same widths BEFORE the shrink factor; the divider drags anchor in these.
///     weak_view: Non-owning panel handle for the sort click, the resize handles and the mask toggle.
///     window: Host window, for the drag-move listener.
///     cx: Application context, for the font-scaled caption size and dot-column width.
///
/// Returns:
///     A single header row with a subtle bottom divider.
pub(super) fn server_header(
    p: MoonPalette,
    sort: (GroupSortField, bool),
    masked: bool,
    w: ByIpWidths,
    logical: ByIpWidths,
    weak_view: &WeakEntity<CoreStatusView>,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    // Upgraded once for the whole header: `Window::listener_for` needs a strong handle, and doing it
    // per cell would be nine upgrades a frame.
    let view = weak_view.upgrade();
    let view = view.as_ref();

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
                // EXPLICIT, and the divider strips depend on it: this row is `items_center`, which
                // CENTRES its children rather than stretching them, so without `h_full` every
                // caption cell would size to its ~11 px of text and each `top:0/bottom:0` grab strip
                // would be 6x11 px — technically present, practically unusable.
                .h_full()
                .items_center()
                .gap(px(ROW_GAP_W))
                .overflow_hidden()
                .child(col_sort_header(
                    t!("core_status.col.server").to_string(),
                    GroupSortField::Name,
                    ByIpCol::Name,
                    w.name,
                    ByIpCol::Name.width_of(logical),
                    sort,
                    p,
                    weak_view,
                    view,
                    window,
                ))
                // Not a sort key — a caption plus the column's single mask control, and it
                // still carries its own resize divider like every other caption.
                .child(ip_header(masked, w, logical, p, view, weak_view, window))
                .child(div().flex_1())
                .child(metric_sort_header(
                    t!("core_status.hdr.cpu").to_string(),
                    GroupSortField::Cpu,
                    ByIpCol::Cpu,
                    w.cpu,
                    w.icon,
                    ByIpCol::Cpu.width_of(logical),
                    sort,
                    p,
                    weak_view,
                    view,
                    window,
                ))
                .child(metric_sort_header(
                    t!("core_status.hdr.mem").to_string(),
                    GroupSortField::Mem,
                    ByIpCol::Mem,
                    w.mem,
                    w.icon,
                    ByIpCol::Mem.width_of(logical),
                    sort,
                    p,
                    weak_view,
                    view,
                    window,
                ))
                .child(metric_sort_header(
                    t!("core_status.chart_ping").to_string(),
                    GroupSortField::Ping,
                    ByIpCol::Ping,
                    w.ping,
                    w.icon,
                    ByIpCol::Ping.width_of(logical),
                    sort,
                    p,
                    weak_view,
                    view,
                    window,
                ))
                .child(metric_sort_header(
                    t!("core_status.chart_exch").to_string(),
                    GroupSortField::Exch,
                    ByIpCol::Exch,
                    w.exch,
                    w.icon,
                    ByIpCol::Exch.width_of(logical),
                    sort,
                    p,
                    weak_view,
                    view,
                    window,
                ))
                .child(metric_sort_header(
                    t!("core_status.hdr.api_key").to_string(),
                    GroupSortField::ApiKey,
                    ByIpCol::Api,
                    w.api,
                    w.icon,
                    ByIpCol::Api.width_of(logical),
                    sort,
                    p,
                    weak_view,
                    view,
                    window,
                ))
                // No icon lead, for the same reason startup has none: nothing warns on a build
                // number, so a metric heading's lead would never light here.
                .child(col_sort_header(
                    t!("core_status.col.version").to_string(),
                    GroupSortField::Version,
                    ByIpCol::Version,
                    w.version,
                    ByIpCol::Version.width_of(logical),
                    sort,
                    p,
                    weak_view,
                    view,
                    window,
                ))
                // No icon lead: startup has no `WarnAxis` behind it, so it uses the plain
                // heading like the core ratio rather than a metric heading whose lead never lights.
                .child(col_sort_header(
                    t!("core_status.col.startup").to_string(),
                    GroupSortField::Startup,
                    ByIpCol::Startup,
                    w.startup,
                    ByIpCol::Startup.width_of(logical),
                    sort,
                    p,
                    weak_view,
                    view,
                    window,
                ))
                .child(col_sort_header(
                    t!("core_status.cores").to_string(),
                    GroupSortField::Cores,
                    ByIpCol::Cores,
                    w.cores,
                    ByIpCol::Cores.width_of(logical),
                    sort,
                    p,
                    weak_view,
                    view,
                    window,
                ))
                // Trailing slots, mirroring the rows so the "Ядра" caption lands over the ratio:
                // the connectivity-warning slot, the status dot, and the tree's overlay scrollbar.
                .child(div().w(px(w.icon)).flex_none())
                .child(div().w(px(crate::design::status_dot_w(cx))).flex_none())
                .child(div().w(px(TREE_SCROLLBAR_W)).flex_none()),
        )
}

/// The IP heading: its caption plus the one control that hides or shows the whole address column.
///
/// The icon/tooltip pairing lives in [`mask_affordance`] rather than inline here, because getting it
/// backwards compiles and renders cleanly while telling the user the exact opposite of the truth —
/// see that function for why the old per-row default makes the inverse so easy to write.
///
/// No `stop_propagation` here, unlike the row buttons: the header has no ancestor mouse-down handler
/// to steal the click from.
///
/// Args:
///     masked: Whether the column is currently hidden.
///     w: Shared column widths, supplying the IP column.
///     p: Active Moon palette.
///     weak_view: Non-owning panel handle for the toggle callback.
///
/// Returns:
///     A fixed-width heading cell matching the IP column below it.
fn ip_header(
    masked: bool,
    w: ByIpWidths,
    logical: ByIpWidths,
    p: MoonPalette,
    view: Option<&Entity<CoreStatusView>>,
    weak_view: &WeakEntity<CoreStatusView>,
    window: &Window,
) -> impl IntoElement {
    let weak_view = weak_view.clone();
    let (icon, tooltip) = mask_affordance(masked);
    h_flex()
        .w(px(w.ip))
        .flex_none()
        .relative()
        .items_center()
        .gap_1()
        .overflow_hidden()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(t!("core_status.hdr.ip").to_string()),
        )
        .child(
            div().flex_none().child(
                MoonButton::new("core-status-ip-mask")
                    .xsmall()
                    .ghost()
                    .icon(icon)
                    .tooltip(t!(tooltip).to_string())
                    .on_click(move |_, _window, app| {
                        let Some(view) = weak_view.upgrade() else {
                            return;
                        };
                        view.update(app, |this, cx| this.toggle_ip_mask(cx));
                    }),
            ),
        )
        .child(resize_handle(
            ByIpCol::Ip,
            ByIpCol::Ip.width_of(logical),
            view,
            p,
            window,
        ))
        .text_color(rgb(p.text_muted))
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
/// the sort arrow, an active-column highlight, and a divider on the value box's right edge.
///
/// The divider hangs off the VALUE BOX rather than the whole cell on purpose: the width it resizes
/// is the value width, and the icon lead is chrome that no drag may touch.
///
/// Args:
///     label: Localized column heading.
///     field: The sort field this heading selects.
///     col: The resizable column this heading labels.
///     value_w: The matching value-box width from the row's `metric_cell`.
///     icon_w: The matching warning-icon lead, so the caption sits over the value.
///     anchor_w: `col`'s logical width, which the divider drag anchors in.
///     sort: The active sort, for the arrow and highlight.
///     p: Active Moon palette.
///     weak_view: Non-owning panel handle for the sort click.
///     view: Strong panel handle for the divider, when alive.
///     window: Host window, for the drag-move listener.
///
/// Returns:
///     A fixed-width clickable caption cell aligned to its column.
// One more argument than clippy's default: the resize handle needs both the column identity and its
// pre-shrink width, and every caption in this file already carries the palette and both handles.
#[allow(clippy::too_many_arguments)]
fn metric_sort_header(
    label: String,
    field: GroupSortField,
    col: ByIpCol,
    value_w: f32,
    icon_w: f32,
    anchor_w: f32,
    sort: (GroupSortField, bool),
    p: MoonPalette,
    weak_view: &WeakEntity<CoreStatusView>,
    view: Option<&Entity<CoreStatusView>>,
    window: &Window,
) -> impl IntoElement {
    let text = format!("{label}{}", sort_arrow(field, sort));
    let active = sort.0 == field;
    let weak_view = weak_view.clone();
    h_flex()
        .flex_none()
        .h_full()
        .items_center()
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
                .h_full()
                .flex_none()
                .relative()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        // Clip like the value box below it: at the shrink floor a caption would
                        // otherwise wrap to a second line inside the fixed-height header.
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .when(active, |el| el.text_color(rgb(p.text_soft)))
                        .child(text),
                )
                .child(resize_handle(col, anchor_w, view, p, window)),
        )
}

/// A clickable fixed-width heading for a non-metric column (name, startup, cores), with the sort
/// arrow, an active-column highlight, and a divider on its right edge.
///
/// Args:
///     label: Localized column heading.
///     field: The sort field this heading selects.
///     col: The resizable column this heading labels.
///     width: The matching column width for this frame.
///     anchor_w: The same column's logical width, which the divider drag anchors in.
///     sort: The active sort, for the arrow and highlight.
///     p: Active Moon palette.
///     weak_view: Non-owning panel handle for the sort click.
///     view: Strong panel handle for the divider, when alive.
///     window: Host window, for the drag-move listener.
///
/// Returns:
///     A fixed-width clickable caption.
// See `metric_sort_header`: the divider needs the column identity and its pre-shrink width on top of
// what a caption already carries.
#[allow(clippy::too_many_arguments)]
fn col_sort_header(
    label: String,
    field: GroupSortField,
    col: ByIpCol,
    width: f32,
    anchor_w: f32,
    sort: (GroupSortField, bool),
    p: MoonPalette,
    weak_view: &WeakEntity<CoreStatusView>,
    view: Option<&Entity<CoreStatusView>>,
    window: &Window,
) -> impl IntoElement {
    let text = format!("{label}{}", sort_arrow(field, sort));
    let active = sort.0 == field;
    let weak_view = weak_view.clone();
    div()
        .w(px(width))
        .h_full()
        .flex_none()
        .relative()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .cursor_pointer()
                .when(active, |el| el.text_color(rgb(p.text_soft)))
                .on_mouse_down(MouseButton::Left, move |_, _, app| {
                    if let Some(view) = weak_view.upgrade() {
                        view.update(app, |this, cx| this.set_group_sort(field, cx));
                    }
                })
                .child(text),
        )
        .child(resize_handle(col, anchor_w, view, p, window))
}
