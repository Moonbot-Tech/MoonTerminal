//! Shared panel UI helpers: adaptive number formatting, mutually exclusive dropdown items, a data
//! table host, repaint gating, and the detached-window toolbar action. Panel-specific behavior stays
//! in its owning module.

use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{DockArea, MoonButton, MoonButtonSize, MoonMenuItem, MoonPalette};

use crate::Backend;
use crate::design;
use crate::detached::DetachedSpec;

/// Formats a quantity or price with the shared adaptive number formatter.
pub(crate) fn num(v: f64) -> String {
    moon_core::util::fmt::adaptive(v)
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
                crate::detached::spawn(app, &backend, &spec, Some(window.window_handle()))
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
