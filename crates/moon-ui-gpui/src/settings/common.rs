//! Shared Settings-window UI helpers (`slider_row`, `section`, `color_row`, and `separator`) and
//! draft binders (`draft_color`, `draft_slider`, and `draft_slider_on`), plus the page primitives
//! (`hint_row`, `num_input`, `confirm`) and thin adapters over MoonUI widgets (`steps`,
//! `usage_bar`, `select_card`).
//!
//! Interface, Lines, and Connections reuse these helpers through re-exports in `settings/mod.rs`.

use std::ops::RangeInclusive;

use gpui::*;
use moon_ui::{
    MoonBarSegment, MoonButton, MoonButtonVariant, MoonColorPicker, MoonColorPickerEvent,
    MoonColorPickerState, MoonDisclosure, MoonDisclosureDirection, MoonInputState, MoonNumberInput,
    MoonPalette, MoonSelectableCard, MoonSlider, MoonSliderEvent, MoonSliderState, MoonStackedBar,
    MoonSteps, MoonWindowExt, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::{Backend, design};
use moon_core::config::AppConfig;

/// A number field's input state holding `value`, for a page's `num_input`.
pub(super) fn number_state(
    value: impl ToString,
    window: &mut Window,
    cx: &mut App,
) -> Entity<moon_ui::MoonInputState> {
    let value = value.to_string();
    cx.new(|cx| moon_ui::MoonInputState::new(window, cx).default_value(value))
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
            if let Some(p) = b.preview.as_mut()
                && apply(p, c)
            {
                bcx.notify();
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
/// Set the maximum first: MoonUI recomputes the thumb after each bound setter, and a minimum
/// above its default maximum of 100 would panic before the requested maximum is installed.
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
            .max(max)
            .min(min)
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
            if let Some(p) = b.preview.as_mut()
                && apply(p, f, bcx)
            {
                bcx.notify();
            }
        });
    })
    .detach();
    st
}

/// Click handler of a [`hint_row`] disclosure or a [`ConfirmBtn`].
pub(super) type Action = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// One setting row: label, a one-line hint, an optional longer help behind "подробнее", and the
/// control on the right.
///
/// The long help is a disclosure rather than a tooltip: a tooltip cannot be read while the
/// control is being used and is invisible on touch, while this stays open until it is folded.
///
/// Args:
///     id: Stable element id; the disclosure derives its own from it.
///     label: Setting name.
///     hint: One short line under the label, or `None`.
///     more: Longer help shown only while `open`, or `None` for no disclosure at all.
///     control: The editor, laid out at its own width.
///     open: Whether the long help is shown.
///     on_toggle: Flips `open`; the caller keeps the state (see `SettingsView::more_open`).
///     cx: Application context for theme and scale.
///
/// Returns:
///     The row.
#[allow(clippy::too_many_arguments)]
pub(super) fn hint_row(
    id: &'static str,
    label: String,
    hint: Option<String>,
    more: Option<String>,
    control: AnyElement,
    open: bool,
    on_toggle: Action,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let muted = rgba_from(p.text_muted, 1.0);
    let toggle = more.is_some().then(|| {
        let on_caret = on_toggle.clone();
        h_flex()
            .id(SharedString::from(format!("{id}-more")))
            .gap(design::ui_px(cx, 4.0))
            .items_center()
            .cursor_pointer()
            .text_size(design::t_caption(cx))
            .text_color(rgba_from(p.blue, 1.0))
            .on_click(move |_, window, cx| on_toggle(window, cx))
            .child(
                MoonDisclosure::button(SharedString::from(format!("{id}-caret")), open)
                    .direction(MoonDisclosureDirection::DownUp)
                    .size(design::DISCLOSURE_GLYPH)
                    .box_size(design::DISCLOSURE_BOX)
                    .on_toggle(move |_, window, cx| on_caret(window, cx)),
            )
            .child(
                if open {
                    t!("settings.less")
                } else {
                    t!("settings.more")
                }
                .to_string(),
            )
    });
    let text = v_flex()
        .flex_1()
        .min_w_0()
        .gap(design::ui_px(cx, 2.0))
        .child(
            div()
                .debug_selector(move || format!("{id}-label"))
                .min_w_0()
                .child(label),
        )
        // Hints and help are captions: smaller and muted under the body-size label.
        .children(hint.map(|hint| {
            div()
                .min_w_0()
                .text_size(design::t_caption(cx))
                .text_color(muted)
                .child(hint)
        }))
        .children(toggle)
        .children(more.filter(|_| open).map(|more| {
            div()
                .debug_selector(move || format!("{id}-more-text"))
                .min_w_0()
                .text_size(design::t_caption(cx))
                .text_color(muted)
                .child(more)
        }));
    h_flex()
        .id(SharedString::from(id))
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 16.0))
        .py(design::ui_px(cx, 6.0))
        .child(text)
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// A counted text in the active language's plural form: `base` names keys `base_one`,
/// `base_few` and `base_many`, each with an `n` placeholder.
pub(super) fn count_label(base: &str, n: u64) -> String {
    t!(&super::telegram::plural_key(base, n), n = n).to_string()
}

/// Number field over [`MoonNumberInput`]: typed entry, -/+ and arrow-key steps, a unit suffix,
/// value in the mono face. The caller owns `state` and subscribes to its
/// `MoonInputEvent::Change`, reading the number with [`MoonNumberInput::read_value`].
///
/// Args:
///     id: Stable element id.
///     state: The field's input state.
///     range: Allowed values; the field clamps on blur.
///     step: Increment of one -/+ press or arrow key.
///     unit: Suffix such as "px" or "s", or `None`.
///
/// Returns:
///     The field.
pub(super) fn num_input(
    id: &'static str,
    state: &Entity<MoonInputState>,
    range: RangeInclusive<f64>,
    step: f64,
    unit: Option<String>,
) -> AnyElement {
    let field = MoonNumberInput::new(id)
        .state(state)
        .range(*range.start(), *range.end())
        .step(step)
        .size(design::CONTROL_TIER);
    // A typed number is a compared value: mono, like every value Settings shows.
    div()
        .font_family(design::mono())
        .child(match unit {
            Some(unit) => field.unit(unit),
            None => field,
        })
        .into_any_element()
}

/// Progress through a short sequence (TG login, station install) over [`MoonSteps`].
pub(super) fn steps(id: &'static str, labels: Vec<String>, current: usize) -> AnyElement {
    MoonSteps::new(id)
        .steps(labels)
        .current(current)
        .size(design::CONTROL_TIER)
        .into_any_element()
}

/// One share of a [`usage_bar`].
pub(super) struct UsageShare {
    pub(super) value: f64,
    pub(super) color: u32,
    /// Tooltip naming the share.
    pub(super) label: String,
}

/// Stacked usage bar (Storage: how the total splits by database) over [`MoonStackedBar`].
pub(super) fn usage_bar(id: &'static str, shares: Vec<UsageShare>, total: f64) -> AnyElement {
    MoonStackedBar::new(id)
        .segments(shares.into_iter().map(|share| {
            MoonBarSegment::new(share.value)
                .color(share.color)
                .label(share.label)
        }))
        .total(total)
        .into_any_element()
}

/// Selectable card (theme pick) over [`MoonSelectableCard`].
pub(super) fn select_card(
    id: &'static str,
    selected: bool,
    on_click: Action,
    children: Vec<AnyElement>,
) -> AnyElement {
    MoonSelectableCard::new(id)
        .selected(selected)
        .on_click(move |_, window, cx| on_click(window, cx))
        .children(children)
        .into_any_element()
}

/// How a confirmation button reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfirmKind {
    /// Backs out; the quiet choice.
    Cancel,
    /// Destroys or discards, outlined in red.
    Danger,
    /// Destroys or discards, filled red: the dialog's main action.
    DangerSolid,
    /// The safe main action.
    Primary,
}

/// One confirmation button. The dialog closes before `on` runs.
pub(super) struct ConfirmBtn {
    pub(super) label: String,
    pub(super) kind: ConfirmKind,
    pub(super) on: Action,
}

/// A confirmation dialog: title, body, buttons right-aligned in the given order.
pub(super) struct Confirm {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) buttons: Vec<ConfirmBtn>,
}

/// Open `c` as the window's one confirmation dialog, replacing any already open.
///
/// Args:
///     window: Window the dialog opens over.
///     cx: Application context.
///     c: What the dialog says and offers.
pub(super) fn confirm(window: &mut Window, cx: &mut App, c: Confirm) {
    let Confirm {
        title,
        body,
        buttons,
    } = c;
    let buttons: std::rc::Rc<Vec<ConfirmBtn>> = std::rc::Rc::new(buttons);
    window.open_unique_moon_dialog("settings-confirm", cx, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let mut footer = h_flex().w_full().justify_end().flex_wrap().gap_2();
        for (index, btn) in buttons.iter().enumerate() {
            let on = btn.on.clone();
            let button = MoonButton::new(SharedString::from(format!("settings-confirm-{index}")))
                .label(btn.label.clone())
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    on(window, cx);
                });
            let button = match btn.kind {
                ConfirmKind::Cancel => button.variant(MoonButtonVariant::Neutral),
                ConfirmKind::Danger => button.variant(MoonButtonVariant::OutlineRed),
                ConfirmKind::DangerSolid => button.variant(MoonButtonVariant::Red),
                ConfirmKind::Primary => button.variant(MoonButtonVariant::Blue),
            };
            footer = footer.child(button.render());
        }
        let body = body.clone();
        dialog
            .w(design::ui_px(cx, 440.0))
            .close_button(true)
            .overlay(true)
            .overlay_closable(true)
            .bg(rgba_from(p.shell_high, 1.0))
            .border_color(rgba_from(p.border, 1.0))
            .rounded(design::r_container(cx))
            .text_color(rgba_from(p.text, 1.0))
            .header(
                div()
                    .w_full()
                    .py_2()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.clone()),
            )
            .content(move |content, _window, _cx| content.child(body.clone()))
            .footer(footer.into_any_element())
    });
}

#[cfg(test)]
mod tests;
