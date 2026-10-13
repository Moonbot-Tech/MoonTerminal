//! Shared Settings-window UI helpers (`slider_field`, `section`, `color_row`, and `separator`) and
//! draft binders (`draft_color`, `draft_slider`, and `draft_slider_on`), plus the page primitives
//! (`hint_row`, `num_input`, `confirm`) and thin adapters over MoonUI widgets (`steps`,
//! `usage_bar`, `select_card`).
//!
//! Interface, Lines, and Connections reuse these helpers through re-exports in `settings/mod.rs`.

use std::ops::RangeInclusive;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use moon_ui::{
    MoonBarSegment, MoonButton, MoonButtonVariant, MoonColorPicker, MoonColorPickerEvent,
    MoonColorPickerState, MoonColorPickerTrigger, MoonDisclosure, MoonDisclosureDirection,
    MoonInputState, MoonNumberInput, MoonPalette, MoonSelectableCard, MoonSlider, MoonSliderEvent,
    MoonSliderState, MoonStackedBar, MoonSteps, MoonText, MoonWindowExt, h_flex, rgba_from, v_flex,
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

/// Width of the widest word of `text` in the UI face, body or caption size, in layout pixels.
///
/// Args:
///     text: The label.
///     caption: Whether the label is drawn at the caption size.
///     cx: Application context for the face, size and scale.
///
/// Returns:
///     The widest word's width plus a pixel of rounding slack per side.
pub(super) fn longest_word_w(text: &str, caption: bool, cx: &App) -> f32 {
    let widest = text
        .split_whitespace()
        .map(|word| {
            if caption {
                design::ui_caption_text_width(cx, word, 400.0)
            } else {
                design::ui_body_text_width(cx, word, 400.0)
            }
        })
        .fold(0.0_f32, f32::max);
    widest + 2.0
}

/// A wrapping label that never gets narrower than its longest word.
///
/// A flex row squeezes a `min_w_0` text beside a sibling that will not shrink, down to one glyph
/// per line at worst (the Badges disclosure, the Sounds volume label). Every Settings label goes
/// through here or [`word_floored`], so the floor is set in one place and the layout probe checks
/// every one of them.
///
/// Args:
///     selector: The label's `debug_selector`.
///     text: The label.
///     caption: Whether the label is drawn at the caption size (the caller sets the size).
///     cx: Application context for the measure.
///
/// Returns:
///     The label's box; the caller adds colour and size.
pub(super) fn prose(selector: String, text: String, caption: bool, cx: &App) -> Div {
    word_floored(selector, &text, caption, 0.0, cx).child(text)
}

/// A box holding a labelled control (a checkbox) whose label must not be squeezed below its
/// longest word: the floor is the word plus `lead`, the control's own part before the label.
///
/// Args:
///     selector: The box's `debug_selector`.
///     text: The label drawn inside the control.
///     caption: Whether the label is drawn at the caption size.
///     lead: Unscaled width before the label: the checkbox square and its gap.
///     cx: Application context for the measure.
///
/// Returns:
///     The box; the caller adds the control as its child.
pub(super) fn word_floored(
    selector: String,
    text: &str,
    caption: bool,
    lead: f32,
    cx: &App,
) -> Div {
    let lead = design::ui_value(cx, lead);
    #[cfg(test)]
    super::layout_probe::record_floor(selector.clone(), text, caption, lead);
    let floor = longest_word_w(text, caption, cx) + lead;
    div()
        .debug_selector(move || selector.clone())
        .min_w(px(floor))
}

/// Lead of a `MoonCheckbox` before its label, unscaled: the 16 px square and the 8 px gap.
pub(super) const CHECKBOX_LEAD: f32 = 24.0;

/// A `MoonCheckbox` whose label never breaks inside a word: see [`word_floored`].
///
/// Args:
///     id: The checkbox id; the box's selector is `<id>-box`.
///     label: The checkbox label.
///     checkbox: The checkbox, its label already set to `label`.
///     cx: Application context for the measure.
///
/// Returns:
///     The checkbox in its floored box.
pub(super) fn floored_checkbox(
    id: &str,
    label: &str,
    checkbox: impl IntoElement,
    cx: &App,
) -> AnyElement {
    word_floored(format!("{id}-box"), label, false, CHECKBOX_LEAD, cx)
        .child(checkbox)
        .into_any_element()
}

/// Longest line of explanatory prose inside a full-width card, unscaled and font-scaled: about
/// 90 characters at the body size, so a caption on a wide window still reads as a paragraph.
pub(super) const PROSE_MAX_W: f32 = 500.0;

/// Cap `el` at [`PROSE_MAX_W`]: prose keeps its readable line length however wide its card is.
/// The cap goes on the text element, never on the card around it.
///
/// Args:
///     el: The prose element.
///     cx: Application context for the font scale.
///
/// Returns:
///     The element with its maximum width set.
pub(super) fn cap_prose<E: Styled>(el: E, cx: &App) -> E {
    el.max_w(design::font_w_px(cx, PROSE_MAX_W))
}

/// A checkbox with its explanation as a capped muted caption under its label. MoonUI's own
/// `description` takes no child to cap, so a long one ran across the whole card.
///
/// Args:
///     id: The checkbox id; the caption's selector is `<id>-caption`.
///     label: The checkbox label, for its word floor (see [`floored_checkbox`]).
///     checkbox: The checkbox, its label already set to `label` and no description.
///     caption: The explanation.
///     cx: Application context for theme and scale.
///
/// Returns:
///     The checkbox over its caption.
pub(super) fn checkbox_with_caption(
    id: &str,
    label: &str,
    checkbox: impl IntoElement,
    caption: String,
    cx: &App,
) -> AnyElement {
    let selector = format!("{id}-caption");
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 2.0))
        .child(floored_checkbox(id, label, checkbox, cx))
        .child(cap_prose(
            div()
                .debug_selector(move || selector.clone())
                .pl(design::ui_px(cx, CHECKBOX_LEAD))
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
                .child(caption),
            cx,
        ))
        .into_any_element()
}

/// How many slider-heights of travel a settings track needs to read as a knob.
///
/// Twelve 22 design-px heights is 264, sitting beside the General tab's widest labeled select
/// (260 design units). The multiple is the control's own size, not a monitor-specific pixel guess.
const SLIDER_TRACK_HEIGHTS: f32 = 12.0;

/// The label column of a [`hint_row`], unscaled: the mockup's `minmax(150px, 240px)`.
pub(super) const HINT_LABEL_MIN_W: f32 = 150.0;
/// See [`HINT_LABEL_MIN_W`].
pub(super) const HINT_LABEL_MAX_W: f32 = 240.0;
/// The widest a card's label column grows to fit its longest label (see [`label_col`]), unscaled.
pub(super) const HINT_LABEL_FIT_MAX_W: f32 = 420.0;

/// How far a [`hint_row`]'s label column may grow before the control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LabelCol {
    /// Up to this width, unscaled: [`HINT_LABEL_MAX_W`], or what [`label_col`] measured.
    Fit(f32),
    /// The whole row: for a row with no control, whose caption would otherwise wrap mid-card.
    Full,
    /// Exactly this width, unscaled, never squeezed: a short label that must stay on one line.
    /// A control that does not fit beside it wraps under it instead of narrowing it.
    Line(f32),
}

/// The label column one card's rows share: wide enough for its longest label on one line,
/// between [`HINT_LABEL_MAX_W`] and [`HINT_LABEL_FIT_MAX_W`].
///
/// One width for the whole card keeps its controls in one column; the floor and the wrap under a
/// narrow card stay [`hint_row`]'s.
///
/// Args:
///     labels: The card's row labels.
///     cx: Application context for the UI face, size and scale.
///
/// Returns:
///     The column's upper bound for [`hint_row_in`].
pub(super) fn label_col<S: AsRef<str>>(labels: &[S], cx: &App) -> LabelCol {
    let scale = design::ui_value(cx, 1.0);
    let widest = labels
        .iter()
        .map(|l| design::ui_body_text_width(cx, l.as_ref(), 400.0))
        .fold(0.0_f32, f32::max);
    // A pixel of slack per side: the estimate rounds glyph advances, and a column a hair short
    // breaks the label's last word onto a line of its own.
    let need = (widest + 2.0) / scale;
    LabelCol::Fit(need.clamp(HINT_LABEL_MAX_W, HINT_LABEL_FIT_MAX_W))
}

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

/// The slider of a [`hint_row`]: track, endpoint scale and current value at a fixed width, for
/// the control column beside the row's label.
///
/// The caller supplies the slider range because pinned MoonUI keeps it private, plus one formatter
/// so both endpoints and the current value use the field's unit and rounding contract. A hint row
/// lays its control out at the control's own width, so the field takes the track's full
/// [`slider_track_max_w`] plus the value column; the value follows the track, left-aligned, rather
/// than sitting at the far end of its column. On a body narrower than that the field shrinks to
/// the row (`max_w_full`) and the track gives way first, never below the two endpoint captions.
/// The bound uses the slider's own height and the caption measure, not the live zoom value, so
/// dragging an interface-scale slider would not move the control under the pointer.
///
/// Args:
///     st: Slider state whose current value is displayed.
///     range: Inclusive endpoints displayed below the slider track.
///     format: Field-specific formatter shared by both endpoints and the current value.
///     cx: Application context used for palette and scaled geometry.
///
/// Returns:
///     The field.
pub(super) fn slider_field(
    st: &Entity<MoonSliderState>,
    range: RangeInclusive<f32>,
    format: impl Fn(f32) -> String,
    cx: &App,
) -> AnyElement {
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
    h_flex()
        .w(px(track_max + caption_gap + value_w))
        .max_w_full()
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
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(val),
        )
        .into_any_element()
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

/// The title of a Settings group box in the UI face.
///
/// `MoonGroupBox::title` paints its caption mono; a card title is a label, so Settings hands the
/// box this as its first child instead, in the same size, weight and colour.
///
/// Args:
///     title: The caption.
///     cx: App context, for the palette.
///
/// Returns:
///     The caption element.
pub(super) fn group_title(title: impl Into<SharedString>, cx: &App) -> AnyElement {
    MoonText::new(title)
        .color(MoonPalette::active(cx).text_soft)
        .font_size(10.5)
        .line_height(13.0)
        .weight(600.0)
        .mono(false)
        .uppercase(false)
        .render()
        .into_any_element()
}

/// Build the Settings colour control: the mockup's compact swatch over the shared picker palette.
///
/// Args:
///     st: The picker state the swatch reads and writes.
///
/// Returns:
///     The picker, ready for further builder calls such as `disabled`.
pub(super) fn swatch_picker(st: &Entity<MoonColorPickerState>) -> MoonColorPicker {
    MoonColorPicker::new(st)
        .colors(design::picker_palette())
        .trigger_style(MoonColorPickerTrigger::Swatch)
}

/// Build a colour row: the swatch picker followed by its label, which wraps when long.
pub(super) fn color_row(
    label: &str,
    st: &Entity<MoonColorPickerState>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    // The label shrinks and wraps beside the fixed swatch: a long one takes a second line
    // instead of painting past its card.
    h_flex()
        .w_full()
        .min_w_0()
        .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
        .gap(design::ui_px(cx, 10.0))
        .items_center()
        .child(div().flex_none().child(swatch_picker(st)))
        .child(
            prose("color-row-label".into(), label.to_string(), false, cx)
                .flex_1()
                .min_w(design::ui_px(cx, 120.0))
                .text_color(rgba_from(p.text_soft, 1.0)),
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
    hint_row_in(
        LabelCol::Fit(HINT_LABEL_MAX_W),
        id,
        label,
        hint,
        more,
        control,
        open,
        on_toggle,
        cx,
    )
}

/// A [`hint_row`] whose label column grows as far as `col` allows: one card's [`label_col`], or
/// [`LabelCol::Full`] for a row without a control.
#[allow(clippy::too_many_arguments)]
pub(super) fn hint_row_in(
    col: LabelCol,
    id: &'static str,
    label: String,
    hint: Option<String>,
    more: Option<String>,
    control: AnyElement,
    open: bool,
    on_toggle: Action,
    cx: &App,
) -> AnyElement {
    row_layout(
        id,
        label,
        hint,
        more,
        control,
        open,
        on_toggle,
        Err(col),
        cx,
    )
}

/// A [`hint_row`] laid out in columns, as in a form card: the label column `label_w` wide
/// (unscaled), the control right beside it filling the rest with its hint under it as a muted
/// caption, and a hairline under the row.
///
/// Args:
///     id: Stable element id; the disclosure derives its own from it.
///     label: Setting name.
///     hint: One short line under the control (selector `{id}-hint`), or `None`.
///     more: Longer help shown only while `open`, or `None` for no disclosure at all.
///     control: The editor; it gets the remaining width and may fill it.
///     open: Whether the long help is shown.
///     on_toggle: Flips `open`.
///     label_w: Width of the label column, unscaled.
///     cx: Application context for theme and scale.
///
/// Returns:
///     The row.
#[allow(clippy::too_many_arguments)]
pub(super) fn column_row(
    id: &'static str,
    label: String,
    hint: Option<String>,
    more: Option<String>,
    control: AnyElement,
    open: bool,
    on_toggle: Action,
    label_w: f32,
    cx: &App,
) -> AnyElement {
    row_layout(
        id,
        label,
        hint,
        more,
        control,
        open,
        on_toggle,
        Ok(label_w),
        cx,
    )
}

/// Shared body of [`hint_row_in`] (`label_w` `Err`: a label column from 150 up to its
/// [`LabelCol`], the control at its own width right after it) and [`column_row`] (`Ok`: a fixed
/// label column and the control filling the rest).
#[allow(clippy::too_many_arguments)]
fn row_layout(
    id: &'static str,
    label: String,
    hint: Option<String>,
    more: Option<String>,
    control: AnyElement,
    open: bool,
    on_toggle: Action,
    label_w: Result<f32, LabelCol>,
    cx: &App,
) -> AnyElement {
    let label_w_fixed = label_w.ok();
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
    // Hints and help are captions: smaller and muted under the body-size label. In a column row
    // the hint goes under the control instead: the label column is too narrow for a sentence, and
    // there it would wrap to three or four lines.
    let caption = move |hint: String| {
        prose(format!("{id}-hint"), hint, true, cx)
            .font_family(design::ui_font())
            .text_size(design::t_caption(cx))
            .text_color(muted)
    };
    let field_hint = hint.clone().filter(|_| label_w.is_ok()).map(caption);
    let text = v_flex()
        .map(|el| match label_w {
            Ok(w) => el.flex_none().w(design::ui_px(cx, w)).min_w_0(),
            // The mockup's label column, `minmax(150px, 240px)`, widened to the card's longest
            // label when there is room: the label takes up to its cap and the control follows
            // right after it, so a short label never leaves a hole across the card. The floor
            // holds: a control that does not fit beside it wraps under the label.
            Err(LabelCol::Fit(cap)) => el
                .flex_1()
                .min_w(design::ui_px(cx, HINT_LABEL_MIN_W))
                .max_w(design::ui_px(cx, cap)),
            Err(LabelCol::Full) => el.flex_1().min_w(design::ui_px(cx, HINT_LABEL_MIN_W)),
            Err(LabelCol::Line(w)) => el.flex_none().w(design::ui_px(cx, w)).max_w_full(),
        })
        // A row's label and hint are prose in the UI face, whatever face the page sets.
        .font_family(design::ui_font())
        .gap(design::ui_px(cx, 2.0))
        .child(prose(format!("{id}-label"), label, false, cx))
        .children(hint.filter(|_| label_w_fixed.is_none()).map(caption))
        .children(toggle)
        .children(more.filter(|_| open).map(|more| {
            prose(format!("{id}-more-text"), more, true, cx)
                .text_size(design::t_caption(cx))
                .text_color(muted)
        }));
    h_flex()
        .id(SharedString::from(id))
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 16.0))
        .py(design::ui_px(cx, 6.0))
        .child(text)
        .map(|el| match label_w_fixed {
            Some(_) => el
                .border_b_1()
                .border_color(rgba_from(p.border, 1.0))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(design::ui_px(cx, 2.0))
                        .child(control)
                        .children(field_hint),
                ),
            // Below the label column's floor the control wraps under the label instead of
            // painting past the card.
            None => el.flex_wrap().gap_y(design::ui_px(cx, 4.0)).child(
                div()
                    .debug_selector(move || format!("{id}-control"))
                    .flex_none()
                    .max_w_full()
                    .child(control),
            ),
        })
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
///     id: Stable element id; also the field's `debug_selector`.
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
    NumField {
        id,
        state: state.clone(),
        range,
        step,
        unit,
        enabled: true,
    }
    .into_any_element()
}

/// A Settings number field, laid out at render time so it can measure its own floor width.
///
/// [`num_input`] and `general::num_input_on` build it.
#[derive(IntoElement)]
pub(super) struct NumField {
    pub(super) id: &'static str,
    pub(super) state: Entity<MoonInputState>,
    pub(super) range: RangeInclusive<f64>,
    pub(super) step: f64,
    pub(super) unit: Option<String>,
    /// A switched-off field stays visible, greyed, and keeps its value.
    pub(super) enabled: bool,
}

/// The narrowest a number field may be drawn: both square step buttons, the field's side
/// padding, the wider range end and the unit.
///
/// A content-sized row (`hint_row`) gives the field no width of its own, and a MoonUI build
/// without its own floor then collapses the value box to a few pixels: the digits vanish between
/// `-` and `+`. This floor is measured here so Settings never depends on that.
///
/// Args:
///     range: Allowed values; the longer end's digits are measured.
///     unit: Suffix drawn after the value, or `None`.
///     cx: Application context for the scale and the text system.
///
/// Returns:
///     The minimum rendered width in pixels.
pub(super) fn num_field_min_w(range: &RangeInclusive<f64>, unit: Option<&str>, cx: &App) -> f32 {
    let m = design::CONTROL_TIER.control_metrics();
    let (lo, hi) = (range.start().to_string(), range.end().to_string());
    let widest = if lo.chars().count() > hi.chars().count() {
        lo
    } else {
        hi
    };
    // The value is semibold-ish (500) mono; the unit is mono two points smaller.
    let value_w = design::ui_text_width_zoomed(cx, &widest, m.font_size, 500.0, true);
    let unit_w = unit.map_or(0.0, |unit| {
        design::ui_text_width_zoomed(cx, unit, m.font_size - 2.0, 400.0, true)
            + design::ui_value(cx, 4.0)
    });
    2.0 * design::ui_value(cx, m.height) + 2.0 * design::ui_value(cx, m.pad_x) + value_w + unit_w
}

impl RenderOnce for NumField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let min_w = num_field_min_w(&self.range, self.unit.as_deref(), cx);
        let field = MoonNumberInput::new(self.id)
            .state(&self.state)
            .range(*self.range.start(), *self.range.end())
            .step(self.step)
            .size(design::CONTROL_TIER)
            .disabled(!self.enabled);
        let id = self.id;
        // A typed number is a compared value: mono, like every value Settings shows. A flex row
        // so the control (itself `flex_1`) fills the floor width.
        h_flex()
            .debug_selector(move || id.to_string())
            .min_w(px(min_w))
            .font_family(design::mono())
            .child(match self.unit {
                Some(unit) => field.unit(unit),
                None => field,
            })
    }
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
