//! Shared Settings-window UI helpers (`slider_row`, `section`, `color_row`, and `separator`) and
//! draft binders (`draft_color` and `draft_slider`).
//!
//! Interface, Lines, and Connections reuse these helpers through re-exports in `settings/mod.rs`.

use std::{collections::HashSet, ops::RangeInclusive};

use gpui::*;
use moon_ui::{
    MoonAccordion, MoonColorPicker, MoonColorPickerEvent, MoonColorPickerState, MoonPalette,
    MoonSlider, MoonSliderEvent, MoonSliderState, h_flex, rgba_from, v_flex,
};

use super::SettingsView;
use crate::{Backend, design};
use moon_core::config::AppConfig;

/// Build a one-item-per-key collapsible block with MoonUI's `MoonAccordion`.
///
/// Lines and Hotkeys share this header-plus-body helper. Each tab owns expansion in a `HashSet`:
/// `open` is the current state and `set` accesses the collection updated after a toggle click.
pub(super) fn collapse_block(
    cx: &Context<SettingsView>,
    id: SharedString,
    key: &'static str,
    title: SharedString,
    open: bool,
    body: AnyElement,
    set: fn(&mut SettingsView) -> &mut HashSet<&'static str>,
) -> impl IntoElement + use<> {
    let entity = cx.entity();
    MoonAccordion::new(id)
        .item(move |item| item.title(title).open(open).child(body))
        .on_toggle_click(move |open_ixs, _window, cx| {
            let now_open = !open_ixs.is_empty();
            entity.update(cx, |this, c| {
                let s = set(this);
                let changed = if now_open {
                    s.insert(key)
                } else {
                    s.remove(key)
                };
                if changed {
                    c.notify();
                }
            });
        })
}

/// Convert color-picker `Hsla` to an sRGB `[u8; 3]` for `ChartTheme` or `OrdersStyle`.
pub(super) fn hsla_u8(h: Hsla) -> [u8; 3] {
    let c: Rgba = h.into();
    [
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    ]
}

/// Build a labeled slider row with a scale below the track and a fixed-width current value.
///
/// The full-width label avoids clipping at large font sizes. The caller supplies the slider range
/// because pinned MoonUI keeps it private, plus one formatter so both endpoints and the current
/// value use the field's unit and rounding contract.
///
/// Args:
///     label: Localized caption displayed above the slider.
///     st: Slider state whose current value is displayed.
///     range: Inclusive endpoints displayed below the slider track.
///     format: Field-specific formatter shared by both endpoints and the current value.
///     cx: Application context used for palette and scaled geometry.
///
/// Returns:
///     The assembled label, slider, scale, and current-value row.
pub(super) fn slider_row(
    label: &str,
    st: &Entity<MoonSliderState>,
    range: RangeInclusive<f32>,
    format: impl Fn(f32) -> String,
    cx: &App,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let val = st.read(cx).value().end();
    let val = if val == 0.0 { 0.0 } else { val };
    let min = format(*range.start());
    let max = format(*range.end());
    let val = format(val);
    v_flex()
        .w_full()
        .child(
            div()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.to_string()),
        )
        .child(
            h_flex()
                .w_full()
                .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
                .gap(design::ui_px(cx, 10.0))
                .items_center()
                .child(
                    v_flex()
                        .w(design::ui_px(cx, 360.0))
                        .flex_none()
                        .child(MoonSlider::new(st).height(design::ui_value(cx, 22.0)))
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .text_size(design::t_caption(cx))
                                .text_color(rgba_from(p.text_muted, 1.0))
                                .child(min)
                                .child(max),
                        ),
                )
                .child(
                    div()
                        .w(design::font_w_px(cx, 76.0))
                        .flex_none()
                        .text_align(TextAlign::Right)
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(val),
                ),
        )
}

/// Build a section separator ported from egui's `ui.separator()`.
pub(super) fn separator(p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .my(design::ui_px(cx, 8.0))
        .h(px(1.0))
        .bg(rgba_from(p.border, 1.0))
}

/// Build an egui-style section heading with semibold text and top spacing.
pub(super) fn section(title: &str, p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .mt(design::ui_px(cx, 10.0))
        .mb(design::ui_px(cx, 4.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgba_from(p.text, 1.0))
        .child(title.to_string())
}

/// Build an egui-style color row with a swatch picker followed by its label.
pub(super) fn color_row(
    label: &str,
    st: &Entity<MoonColorPickerState>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
        .gap(design::ui_px(cx, 10.0))
        .items_center()
        .child(MoonColorPicker::new(st))
        .child(
            div()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.to_string()),
        )
}

/// Bind a color picker to the live Settings draft.
///
/// Initialize it from `init`. On `Change`, call `apply` against `Backend.preview`; the closure both
/// detects and performs a change and may capture context such as a server index. Notify the backend
/// only when it returns true. Interface, Lines, and Connections delegate through thin wrappers.
pub(super) fn draft_color(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    init: [u8; 3],
    apply: impl Fn(&mut AppConfig, [u8; 3]) -> bool + 'static,
) -> Entity<MoonColorPickerState> {
    let st = cx.new(|cx| {
        MoonColorPickerState::new(window, cx).default_value(design::rgb_bytes_to_hsla(init))
    });
    cx.subscribe(&st, move |this, _emitter, ev: &MoonColorPickerEvent, cx| {
        let MoonColorPickerEvent::Change(h) = ev else {
            return;
        };
        let c = hsla_u8(*h);
        this.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if apply(p, c) {
                    bcx.notify();
                }
            }
        });
    })
    .detach();
    st
}

/// Bind an `f32` slider to the live Settings draft.
///
/// Initialize it from `init`. On `Change`, `apply` detects and performs a change in
/// `Backend.preview`; its backend context supports fields that reinstall the theme. Notify only when
/// the closure returns true.
pub(super) fn draft_slider(
    cx: &mut Context<SettingsView>,
    min: f32,
    max: f32,
    step: f32,
    init: f32,
    apply: impl Fn(&mut AppConfig, f32, &mut Context<Backend>) -> bool + 'static,
) -> Entity<MoonSliderState> {
    let st = cx.new(|_| {
        MoonSliderState::new()
            .min(min)
            .max(max)
            .step(step)
            .default_value(init)
    });
    cx.subscribe(&st, move |this, _emitter, ev: &MoonSliderEvent, cx| {
        let MoonSliderEvent::Change(f) = ev else {
            return;
        };
        // Slider quantization over a negative subrange can produce IEEE -0.0. Normalize it before it
        // reaches the draft or disk.
        let f = f.end();
        let f = if f == 0.0 { 0.0 } else { f };
        this.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if apply(p, f, bcx) {
                    bcx.notify();
                }
            }
        });
    })
    .detach();
    st
}
