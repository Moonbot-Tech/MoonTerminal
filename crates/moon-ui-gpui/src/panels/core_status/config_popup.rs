//! The Core Status alert-axis configuration popover: the gear beside the mode control, with one
//! checkbox per warning axis. Split out of `mod.rs` because it is a self-contained control.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonPopover,
    MoonPopoverPlacement, v_flex,
};
use rust_i18n::t;

use super::CoreStatusView;
use crate::Backend;
use crate::design;
use moon_core::config::layout::WarnAxesCfg;

/// Popover width (UI px) of the alert-axis toggle list.
const WARN_CFG_W: f32 = 168.0;

impl CoreStatusView {
    /// The alert-axis toggle popover: a gear beside the mode control with one checkbox per warning
    /// axis. Unchecking an axis stops the backend recording it AND hides its history from the charts
    /// and the Alerts list (the read paths filter it out), so "off" means neither written nor shown.
    ///
    /// Args:
    ///     cx: View context, for the palette, the current toggles, and the open-state callback.
    ///
    /// Returns:
    ///     The gear trigger wrapped in its popover.
    pub(super) fn warn_gear(&self, cx: &Context<Self>) -> impl IntoElement {
        let axes = self.backend.read(cx).warn_axes();
        let view = cx.entity();
        let gear = MoonButton::new("core-status-warn-gear")
            .label("⚙")
            .size(MoonButtonSize::Micro)
            .variant(MoonButtonVariant::Ghost)
            .tooltip(t!("core_status.warn_cfg.title").to_string())
            .render();
        // Order mirrors the badge card: CPU, memory, ping, then connectivity.
        let content = v_flex()
            .w_full()
            .gap(design::ui_px(cx, 4.0))
            .px(design::ui_px(cx, 6.0))
            .py(design::ui_px(cx, 6.0))
            .child(warn_axis_check(
                "cs-warn-cpu",
                t!("core_status.warn_cfg.cpu").to_string(),
                axes.cpu,
                &self.backend,
                |axes, on| axes.cpu = on,
            ))
            .child(warn_axis_check(
                "cs-warn-mem",
                t!("core_status.warn_cfg.mem").to_string(),
                axes.mem,
                &self.backend,
                |axes, on| axes.mem = on,
            ))
            .child(warn_axis_check(
                "cs-warn-ping",
                t!("core_status.warn_cfg.ping").to_string(),
                axes.ping,
                &self.backend,
                |axes, on| axes.ping = on,
            ))
            .child(warn_axis_check(
                "cs-warn-conn",
                t!("core_status.warn_cfg.conn").to_string(),
                axes.conn,
                &self.backend,
                |axes, on| axes.conn = on,
            ))
            .into_any_element();
        MoonPopover::new("core-status-warn-popover")
            // The gear sits at the row's right edge, so anchor the panel's right edge to it.
            .placement(MoonPopoverPlacement::BottomEnd)
            .content_width_ui(WARN_CFG_W)
            .close_on_content_click(false)
            .open(self.warn_cfg_open)
            .on_open_change(move |open, _window, app| {
                view.update(app, |this, cx| {
                    this.warn_cfg_open = open;
                    cx.notify();
                });
            })
            .trigger(gear)
            .content(content)
    }
}

/// One alert-axis checkbox: reflects the stored flag and writes the flipped set back to the backend.
///
/// `set` mutates one field of the toggle set; the whole set is then persisted and the engine
/// invalidated. `mark_backend_dirty` fires a notify so the panel repaints and every chart rebuilds
/// its badges against the new filter.
///
/// Args:
///     id: Stable checkbox element identity.
///     label: Localized axis label.
///     checked: Current stored state of this axis.
///     backend: Shared backend the toggle writes through.
///     set: Field mutator applied to a copy of the current toggle set.
///
/// Returns:
///     A compact labeled checkbox element.
fn warn_axis_check(
    id: &'static str,
    label: String,
    checked: bool,
    backend: &Entity<Backend>,
    set: fn(&mut WarnAxesCfg, bool),
) -> impl IntoElement {
    let backend = backend.clone();
    MoonCheckbox::new(SharedString::from(id))
        .label(label)
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let on = *ch;
            backend.update(app, |b, cx| {
                let mut axes = b.warn_axes();
                set(&mut axes, on);
                b.set_warn_axes(axes);
                b.mark_backend_dirty(cx);
            });
        })
}
