//! Shared panel UI helpers: adaptive number formatting, mutually exclusive dropdown items, a data
//! table host, the settings-popup group frame, repaint gating, and the detached-window toolbar
//! action. Panel-specific behavior stays in its owning module.

use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonGroupBox, MoonMenuItem, MoonPalette, MoonTooltipView,
};

use crate::Backend;
use crate::design;
use crate::window::detached::DetachedSpec;

/// Formats a quantity or price with the shared adaptive number formatter.
pub(crate) fn num(v: f64) -> String {
    moon_core::util::fmt::adaptive(v)
}

/// Full-scale value (ms) for a latency line drawn into a 0..100 % plot, shared by the Core Status
/// chart and the warning-badge card so both scale the ping axis identically.
///
/// Rounds the series maximum UP to a tidy 50 ms step, with ~15 % headroom so a line sitting at its
/// own maximum stays visibly below the ceiling instead of hugging (or clipping past) the top edge.
/// Floors at 50 ms, so it is never zero (no divide-by-zero / NaN when mapping into the plot).
pub(crate) fn ms_axis_scale(max_ms: u16) -> f32 {
    (f32::from(max_ms) * 1.15 / 50.0).ceil().max(1.0) * 50.0
}

/// Localized word for one trade-direction filter value, shared by every panel that offers the
/// filter (the Report scope field, the Analytics toolbar) so the three read identically.
pub(crate) fn side_label(side: moon_core::db::SideFilter) -> String {
    match side {
        moon_core::db::SideFilter::All => rust_i18n::t!("report.filter.all").to_string(),
        moon_core::db::SideFilter::Long => rust_i18n::t!("report.side.long").to_string(),
        moon_core::db::SideFilter::Short => rust_i18n::t!("report.side.short").to_string(),
    }
}

/// Selection decoration for a mutually exclusive menu item. `Check` applies the menu item's checked
/// state, while `Highlight` applies its selected state; callers choose the style explicitly.
#[derive(Clone, Copy)]
pub(crate) enum RadioMark {
    Check,
    Highlight,
}

/// Builds mutually exclusive `MoonDropdown` items from `(value, key, label)` options. The item whose
/// copied value equals `current` receives the requested selection mark, and clicking an item invokes
/// `on_select(app, value)`. Returns the menu items in input order.
pub(crate) fn radio_items<T, F>(
    options: impl IntoIterator<Item = (T, SharedString, SharedString)>,
    current: T,
    mark: RadioMark,
    on_select: F,
) -> Vec<MoonMenuItem>
where
    T: Copy + PartialEq + 'static,
    F: Fn(&mut App, T) + Clone + 'static,
{
    options
        .into_iter()
        .map(|(value, key, label)| {
            let on_select = on_select.clone();
            let sel = value == current;
            let item = MoonMenuItem::with_key(key, label);
            let item = match mark {
                RadioMark::Check => item.checked(sel),
                RadioMark::Highlight => item.selected(sel),
            };
            item.on_click(move |_, _, app| on_select(app, value))
        })
        .collect()
}

/// Design-unit padding [`popup_group`] applies inside its frame, on every side.
pub(crate) const POPUP_GROUP_PAD: f32 = 6.0;

/// Design-unit width a [`popup_group`] frame consumes across BOTH sides: its padding plus the
/// 1-wide border `MoonGroupBox` draws.
///
/// Width constants that must fit content inside a group subtract this instead of restating the
/// frame's metrics — three of them used to, and a change to the padding above silently wrong-sized
/// all three.
pub(crate) const POPUP_GROUP_INSET: f32 = 2.0 * (POPUP_GROUP_PAD + 1.0);

/// [`POPUP_GROUP_INSET`] in rendered pixels at the current UI scale.
///
/// Args:
///     cx: Application context supplying the active UI scale.
///
/// Returns:
///     Scaled width the group frame takes from the row inside it.
pub(crate) fn popup_group_inset_px(cx: &App) -> f32 {
    f32::from(design::ui_px(cx, POPUP_GROUP_INSET))
}

/// Builds the shared captioned group box used inside **settings popups**.
///
/// Scope is settings popups only. `panels/order_edit` builds its own `MoonGroupBox` with a heavier
/// padding and an opaque fill, which is right for a dialog body sitting on a panel and wrong here.
///
/// `NoFill` keeps the frame transparent: every caller already sits on a popup surface that paints
/// the background, and a second opaque fill on top of it only muddies that surface.
///
/// Args:
///     id: Stable element identity, unique within the hosting popup.
///     title: Localized caption rendered above the group's contents.
///
/// Returns:
///     A group box ready to receive children.
pub(crate) fn popup_group(
    id: impl Into<SharedString>,
    title: impl Into<SharedString>,
) -> MoonGroupBox {
    MoonGroupBox::new(id)
        .title(title)
        .background_policy(MoonBackgroundPolicy::NoFill)
        .padding(POPUP_GROUP_PAD)
        .gap(4.0)
}

/// Builds the title cell of a settings popup's head row.
///
/// A popup title has to outrank the group captions beneath it, and `MoonGroupBox` draws those at a
/// larger, bolder step than `t_caption` — so the title sits on `t_body`. It also claims the row's
/// free width and truncates, which keeps a long localized title from pushing the head row's
/// trailing controls (apply-to-all, copy/paste, close) off the popup's edge.
///
/// Args:
///     title: Localized popup title.
///     p: Active palette.
///     cx: Application context supplying the text scale.
///
/// Returns:
///     The title cell, ready to be the first child of the head row.
pub(crate) fn popup_title(title: impl Into<SharedString>, p: MoonPalette, cx: &App) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_size(design::t_body(cx))
        .text_color(rgb(p.text))
        .child(title.into())
}

/// Builds the ✕ that dismisses a settings popup.
///
/// Every settings popup needs one, and for the popups that must disable outside-click dismissal to
/// protect a nested dropdown menu it is the ONLY reliable way out — Escape is not, see the Popover
/// entry in `docs-internal/FORK_BUGS.md`.
///
/// Args:
///     id: Stable element identity, unique within the hosting popup.
///     on_click: Handler that closes the popup, normally a `cx.listener`.
///
/// Returns:
///     The rendered close button.
pub(crate) fn popup_close_button(
    id: impl Into<ElementId>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    MoonButton::new(id)
        .label("✕")
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Ghost)
        .on_click(on_click)
        .render()
}

/// Builds an unlabeled compact checkbox with a tooltip. A `div.id.tooltip` wrapper
/// supplies the tooltip because an unlabeled `MoonCheckbox` has none.
pub(crate) fn icon_checkbox(
    id: &str,
    tooltip: String,
    checked: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("{id}-tip")))
        .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(tooltip.clone())).into())
        .child(
            MoonCheckbox::new(SharedString::from(id.to_string()))
                .checked(checked)
                .size(MoonCheckboxSize::Compact)
                .on_change(on_change),
        )
        .into_any_element()
}

/// Hosts a caller-built data table in the shared table-body surface. The container fills available
/// flex space and clips overflow; when `empty` is true, it adds an absolute placeholder row below
/// the table header. `empty_msg` must already be localized by the caller.
pub(crate) fn data_table_host(
    host_id: impl Into<SharedString>,
    empty: bool,
    empty_msg: String,
    p: MoonPalette,
    cx: &App,
    table: impl IntoElement,
) -> impl IntoElement {
    div()
        .id(host_id.into())
        .relative()
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .overflow_hidden()
        .bg(rgb(p.table_body))
        .child(table)
        .when(empty, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(10.0))
                    .top(px(design::table_head_h(cx)))
                    .h(px(design::table_row_h(cx)))
                    .flex()
                    .items_center()
                    .font_family(design::mono())
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(empty_msg),
            )
        })
}

/// Coalesces panel repaint requests by data signature and wall-clock second. A signature change or a
/// new second bucket makes a notification eligible, while a monotonic 250 ms floor limits accepted
/// requests to at most 4 Hz.
#[derive(Default)]
pub(crate) struct RenderGate {
    last_sig: u64,
    last_sec: u64,
    last_notify_at: Option<Instant>,
}

impl RenderGate {
    /// Returns whether `sig` changed or `now_ms` entered a new second bucket and the 250 ms floor has
    /// elapsed. Updates the accepted signature, bucket, and monotonic timestamp only when returning
    /// true.
    pub(crate) fn should_notify(&mut self, sig: u64, now_ms: f64) -> bool {
        let sec = (now_ms as u64) / 1000;
        let changed = sig != self.last_sig || sec != self.last_sec;
        let now = Instant::now();
        let due = self
            .last_notify_at
            .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(250));
        if changed && due {
            self.last_sig = sig;
            self.last_sec = sec;
            self.last_notify_at = Some(now);
            true
        } else {
            false
        }
    }
}

/// Builds the toolbar action that opens a fresh detached window for a panel. After a successful
/// spawn it removes the source panel when its dock handle is available and records a unique
/// `DetachedSpec` in `backend.detached`. `name` must be the stable panel identifier shared by
/// `panel_name`, `remove_panel_by_name`, and `DetachedSpec`.
pub fn detach_button(
    name: &'static str,
    group: String,
    backend: Entity<Backend>,
    dock: Option<WeakEntity<DockArea>>,
) -> AnyElement {
    MoonButton::new(SharedString::from(format!("detach-{name}")))
        .ghost()
        .size(MoonButtonSize::Action)
        .label("⧉")
        .tooltip(rust_i18n::t!("dock.detach_hint").to_string())
        .on_click(move |_, window, app| {
            let spec =
                DetachedSpec::with_saved_geom(&backend, app, group.clone(), name.to_string());
            if let Err(err) =
                crate::window::detached::spawn(app, &backend, &spec, Some(window.window_handle()))
            {
                log::warn!("detach panel failed group={} panel={name}: {err:#}", group);
                return;
            }
            // Remove the source panel only after the detached window opens successfully.
            if let Some(dock) = dock.as_ref().and_then(|d| d.upgrade()) {
                dock.update(app, |area, cx| {
                    area.remove_panel_by_name(name, window, cx);
                });
            }
            // Record the specification after the successful spawn and optional dock removal.
            backend.update(app, |b, _| {
                if !b
                    .detached
                    .iter()
                    .any(|s| s.group == spec.group && s.panel == spec.panel)
                {
                    b.detached.push(spec);
                    b.detached_dirty = true;
                }
            });
        })
        .render()
        .into_any_element()
}
