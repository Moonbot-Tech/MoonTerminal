//! Shared Settings-window UI helpers (`slider_row`, `section`, `color_row`, and `separator`) and
//! draft binders (`draft_color`, `draft_slider`, and `draft_slider_on`).
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

/// How many slider-heights of travel a settings track needs to read as a knob.
///
/// Twelve 22 design-px heights is 264, sitting beside the General tab's widest labeled select
/// (260 design units). The multiple is the control's own size, not a monitor-specific pixel guess.
const SLIDER_TRACK_HEIGHTS: f32 = 12.0;

/// Upper bound for the settings slider track: captions-plus-gap, or a run of slider heights.
///
/// The floor is the two endpoint captions so they cannot collide. The ceiling is twelve times the
/// slider's own height so a wide Settings window cannot stretch the track across the pane. `flex_1`
/// still shrinks the track between those two on a narrow window.
///
/// Args:
///     scale_w: Width reserved by the two endpoint captions and their gap, in layout pixels.
///     slider_h: The slider's own height in the same pixel space as `scale_w`.
///
/// Returns:
///     The larger of `scale_w` and twelve slider heights.
fn slider_track_max_w(scale_w: f32, slider_h: f32) -> f32 {
    scale_w.max(slider_h * SLIDER_TRACK_HEIGHTS)
}

/// Build a labeled slider row with a scale below the track and a fixed-width current value.
///
/// The caller supplies the slider range because pinned MoonUI keeps it private, plus one formatter
/// so both endpoints and the current value use the field's unit and rounding contract. The track
/// yields width to the value column at narrow window sizes, including when a colour picker shares
/// its parent row. Its minimum reserves only the two endpoint captions and their gap; its maximum
/// is [`slider_track_max_w`], so a wide window keeps the track, captions and value as one
/// left-aligned group. The bound uses the slider's own height and the caption measure, not the
/// live zoom value, so dragging the interface-scale slider does not move the control under the
/// pointer.
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
    let endpoint_font = design::BODY_TEXT - 2.0;
    let caption_gap = design::ui_value(cx, 10.0);
    let scale_w = design::ui_text_width_zoomed(cx, &min, endpoint_font, 400.0, true)
        + design::ui_text_width_zoomed(cx, &max, endpoint_font, 400.0, true)
        + caption_gap;
    let slider_h = design::ui_value(cx, 22.0);
    let value_w = design::font_w(cx, 76.0);
    let track_max = slider_track_max_w(scale_w, slider_h);
    let row_max = track_max + caption_gap + value_w;
    v_flex()
        .w_full()
        .min_w_0()
        .max_w(px(row_max))
        .child(
            div()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.to_string()),
        )
        .child(
            h_flex()
                .w_full()
                .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
                .gap(px(caption_gap))
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(scale_w))
                        .max_w(px(track_max))
                        .child(MoonSlider::new(st).height(slider_h))
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .font_family(design::mono())
                                .text_size(design::t_caption(cx))
                                .text_color(rgba_from(p.text_muted, 1.0))
                                .child(min)
                                .child(max),
                        ),
                )
                .child(
                    div()
                        .w(px(value_w))
                        .flex_none()
                        .font_family(design::mono())
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

/// Opens a folder in the platform file manager.
pub(super) fn open_folder(path: &std::path::Path) {
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(path).spawn();
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
        .child(MoonColorPicker::new(st).colors(design::picker_palette()))
        .child(
            div()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.to_string()),
        )
}

/// Bind a color picker to the live Settings draft and independent app-global custom history.
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
    let st = crate::controls::color_picker::shared_color_state(init, window, cx);
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
    draft_slider_on(cx, min, max, step, init, DraftSliderApplyOn::Change, apply)
}

/// Choose when a slider writes its draft, keeping expensive previews out of drag ticks.
pub(super) enum DraftSliderApplyOn {
    Change,
    Release,
}

/// Bind a slider to the draft on the selected event, sharing initialization and normalization.
///
/// Release-driven sliders repaint Settings on `Change` so captions read the live slider value
/// without notifying the backend or applying the draft. `apply` notifies the backend only when
/// it returns true, just as it does for change-driven sliders.
pub(super) fn draft_slider_on(
    cx: &mut Context<SettingsView>,
    min: f32,
    max: f32,
    step: f32,
    init: f32,
    apply_on: DraftSliderApplyOn,
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
        let f = match (&apply_on, ev) {
            (DraftSliderApplyOn::Change, MoonSliderEvent::Change(f))
            | (DraftSliderApplyOn::Release, MoonSliderEvent::Release(f)) => f,
            (DraftSliderApplyOn::Release, MoonSliderEvent::Change(_)) => {
                cx.notify();
                return;
            }
            (DraftSliderApplyOn::Change, MoonSliderEvent::Release(_)) => return,
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

#[cfg(test)]
mod tests;
