//! The design's fixed size system and font-scale inversion.

use super::*;

// ---- The design's fixed size system -----------------------------------------------------------

/// The tier the design's ordinary controls render at: buttons, inputs, checkboxes and the chrome
/// bands derive their metrics from it. Dense strips pin `MoonSize::Xs` themselves.
pub const CONTROL_TIER: MoonSize = MoonSize::Sm;

/// The design's text adjustment on MoonUI's font channel: `tokens.font(v)` renders `v + 3`, which
/// is what the reviewed design was measured against. Installed into the theme once.
pub const DESIGN_FONT_DELTA: f32 = 3.0;

/// The body text size in design pixels: [`CONTROL_TIER`]'s control font. [`t_caption`],
/// [`t_body`], [`t_body_lg`] and [`t_title`] are this plus their step.
pub const BODY_TEXT: f32 = CONTROL_TIER.control_metrics().font_size;

/// Unscaled step [`t_body_lg`] adds on [`BODY_TEXT`] so a mark reads above its neighbours.
pub const BODY_LG_STEP: f32 = 1.0;

/// The `MoonInput` size the terminal's fixed-small inputs carry: `Small` (22/10/13), the size the
/// reviewed design measured every such field at.
pub const INPUT_SIZE: MoonInputSize = MoonInputSize::Small;

/// The `MoonInput` size of a cell in a dense grid whose rows carry caption text: the
/// [`MoonSize::Xs`] tier's metrics — the tier caption text belongs to — with the text at
/// [`t_caption`] and no vertical padding, so the box is one caption line tall and a row holding
/// it stands as high as its text-only neighbours.
///
/// `height` is the tier's LINE height, not 0: the input draws its box from the size `height`
/// selects, never from `Custom`'s own `h` (that lands on the multi-line height,
/// `docs-internal/FORK_BUGS.md`), and the box comes out one scaled line tall — 19 px against a
/// 19.5 px caption line at the design scale (`design/tests.rs`).
pub fn dense_input_size(cx: &App) -> MoonInputSize {
    let m = MoonSize::Xs.control_metrics();
    MoonInputSize::Custom {
        height: m.line_height,
        radius: m.radius,
        font_size: font_base_for(cx, f32::from(t_caption(cx))),
        line_height: m.line_height,
        pad_x: m.pad_x,
        pad_y: 0.0,
        gap: m.gap,
    }
}

/// A square icon-only button as tall as a [`dense_input_size`] box — the reset beside the tuner
/// grid's range cells, which must not make its row taller than the cells it sits with. Pass
/// [`dense_glyph_btn_w`] to its `width` for the square.
pub fn dense_glyph_btn_size() -> MoonButtonSize {
    let m = MoonSize::Xs.control_metrics();
    MoonButtonSize::Custom {
        height: m.line_height,
        radius: m.radius,
        font_size: m.font_size,
        line_height: m.line_height,
        gap: m.gap,
    }
}

/// Rendered width of the square [`dense_glyph_btn_size`] button: its own drawn height, for a
/// RENDERED width (`MoonButton::width`), as [`glyph_btn_w`] is.
pub fn dense_glyph_btn_w(cx: &App) -> f32 {
    ui_value(cx, MoonSize::Xs.control_metrics().line_height)
}

pub fn ui_value(cx: &App, value: f32) -> f32 {
    MoonTheme::active_tokens(cx).ui(value)
}

/// MoonUI-INTERNAL legacy font channel: `value * ui_scale + font_delta`.
///
/// This is NOT the terminal's text channel any more — that is [`t_body`]. The only
/// legitimate remaining use is mirroring a formula MoonUI itself computes; its one such caller is
/// `settings/connections/table.rs` / `columns.rs`, which reproduces MoonUI's own `Scaled`
/// dropdown-trigger width.
///
/// Args:
///     cx: Application context used to read the active tokens.
///     value: Design-reference size MoonUI would pass through `tokens.font()`.
///
/// Returns:
///     The MoonUI font-channel size, in logical pixels.
pub fn font_value(cx: &App, value: f32) -> f32 {
    MoonTheme::active_tokens(cx).font(value)
}

/// Line-box height on the terminal's text channel: [`ui_value`] of `value` plus the step from
/// [`base_text`] to [`BODY_TEXT`] (3 at the default `mono_font_size` of 11).
///
/// Moves with [`t_body`] so a line box matches the text in it.
///
/// Args:
///     cx: Application context used to read the active tokens.
///     value: Unscaled design-reference line height.
///
/// Returns:
///     The rendered line height in logical pixels.
pub fn line_value(cx: &App, value: f32) -> f32 {
    ui_value(cx, value + BODY_TEXT - base_text(cx))
}

/// The BASE size a MoonUI component must be handed so its text RENDERS at `target` logical pixels.
///
/// The inverse of [`font_value`]. Every MoonUI size is a design-reference number that the theme
/// scales at render time, which is right for anything laid out in the application's own type — and
/// wrong for the one case that is not: the chart's captions are sized by the CHART's font, and a
/// control placed in the room one of them reserved has to match that number exactly, or the box and
/// the words in it grow apart.
///
/// Solved rather than derived, because the theme exposes the forward function and not its terms.
/// `font` is affine (`value * scale + delta`, floored at one pixel), so two probes taken clear of
/// that floor give the slope and the intercept, and the inverse is exact.
///
/// Args:
///     cx: Application context used to read the active tokens.
///     target: The size the text has to come out at, in logical pixels.
///
/// Returns:
///     The base value to pass, never below one pixel.
pub fn font_base_for(cx: &App, target: f32) -> f32 {
    invert_font_scale(
        font_value(cx, PROBE_LOW),
        font_value(cx, PROBE_HIGH),
        target,
    )
}

/// Solve `scale * base + delta = target` from two samples of the forward function.
///
/// Split out from [`font_base_for`] because it is the only part that can be WRONG: the rest is two
/// calls into the theme. A degenerate pair — a theme that scales everything to one number — answers
/// the target itself rather than dividing by zero.
///
/// Args:
///     low: What the forward function returns for [`PROBE_LOW`].
///     high: The same for [`PROBE_HIGH`].
///     target: The rendered size wanted.
///
/// Returns:
///     The base to hand the component, never below one pixel.
pub(super) fn invert_font_scale(low: f32, high: f32, target: f32) -> f32 {
    let slope = (high - low) / (PROBE_HIGH - PROBE_LOW);
    if !slope.is_finite() || slope <= 0.01 {
        return target.max(1.0);
    }
    let intercept = low - slope * PROBE_LOW;
    ((target - intercept) / slope).max(1.0)
}

pub fn fit_h_value(cx: &App, base_height: f32, base_line_height: f32, base_pad_y: f32) -> f32 {
    MoonTheme::active_tokens(cx).fit_height(base_height, base_line_height, base_pad_y)
}

pub fn ui_px(cx: &App, value: f32) -> Pixels {
    px(ui_value(cx, value))
}

/// [`line_value`] as `Pixels`.
pub fn line_px(cx: &App, value: f32) -> Pixels {
    px(line_value(cx, value))
}

/// Return the MoonUI theme's base terminal text size, `mono_font_size`, whose default is 11.
///
/// The three raw-GPUI text tiers below derive from this value, so a `.toml` base-size change moves
/// them together.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The unscaled base font size.
pub fn base_text(cx: &App) -> f32 {
    MoonTheme::active_tokens(cx).typography.mono_font_size
}

/// Return the caption text size for raw GPUI elements such as `div().text_size(...)`.
///
/// The raw-GPUI tiers derive from [`BODY_TEXT`]. MoonUI components such as
/// `MoonText`, `MoonButtonSegment`, and `MoonDataCell` already scale their own default or supplied
/// base size. Do not pass a `t_*` result or [`font_value`] into them, because that applies scaling
/// twice.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     12 design px, for badges, small labels, and counters.
pub fn t_caption(cx: &App) -> Pixels {
    t_body_step_px(cx, -2.0)
}

/// Return the theme-base body size for raw GPUI text, tables, and monospaced values.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     14 design px, [`BODY_TEXT`] itself.
pub fn t_body(cx: &App) -> Pixels {
    t_body_step_px(cx, 0.0)
}

/// Return the one-step-up body size for a row that must read above its neighbours in place.
///
/// Sits between [`t_body`] and [`t_title`] for the case where a row is emphasized INSIDE a list of
/// fixed-height rows: `t_title` is three steps up, and its line box outgrows a row height that
/// does not track the font, so the text clips. One step clears the neighbours while still
/// fitting.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     15 design px.
pub fn t_body_lg(cx: &App) -> Pixels {
    t_body_step_px(cx, BODY_LG_STEP)
}

/// Return the title size for raw GPUI headings and large accents.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     17 design px.
pub fn t_title(cx: &App) -> Pixels {
    t_body_step_px(cx, 3.0)
}

/// Already-scaled [`MoonTextMetrics`] for `MoonText::rendered_metrics`, which bypasses MoonUI's
/// font scaling and therefore uses the terminal's `tokens.ui` text channel directly.
///
/// `step` is 0 for body/control text, -2 for a caption, +1 for `body_lg`, +3 for a title.
/// `line_base` is the caller's own unscaled design line-height (the value previously passed to
/// `.line_height(...)`, or `MoonTextStyle`'s default 11). Font size is `ui(BODY_TEXT + step)`;
/// line height is [`line_value`] of `line_base`, the way [`line_px`] does it.
///
/// Args:
///     cx: Application context used to read the active Moon scale.
///     step: Local unscaled addition on top of [`BODY_TEXT`].
///     line_base: Unscaled design-reference line height the caller used before migration.
///
/// Returns:
///     Rendered font size and line height, ready for `MoonText::rendered_metrics`.
pub fn text_metrics(cx: &App, step: f32, line_base: f32) -> MoonTextMetrics {
    MoonTextMetrics {
        font_size: ui_value(cx, BODY_TEXT + step),
        line_height: line_value(cx, line_base),
    }
}

/// [`t_body`] plus a local design-px `step`, already scaled through `tokens.ui`, for raw-GPUI
/// `.text_size(...)`.
///
/// `step = 0` is the body text itself.
///
/// Args:
///     cx: Application context used to read the active Moon scale.
///     step: Local unscaled addition on top of [`BODY_TEXT`].
///
/// Returns:
///     The rendered size as `Pixels`.
pub fn t_body_step_px(cx: &App, step: f32) -> Pixels {
    ui_px(cx, BODY_TEXT + step)
}

/// The BASE a font-channel MoonUI prop must be handed so it renders at the body text's
/// `tokens.ui` size plus `step`.
///
/// Inverse of [`font_value`], via [`font_base_for`]. For `MoonSelectorSegment::font_size` and
/// `MoonBadgeSize::Custom`.
///
/// Args:
///     cx: Application context used to read the active tokens.
///     step: Local unscaled addition on top of [`BODY_TEXT`].
///
/// Returns:
///     An unscaled base for a MoonUI component's own size field.
pub fn body_font_base(cx: &App, step: f32) -> f32 {
    font_base_for(cx, ui_value(cx, BODY_TEXT + step))
}

pub fn fit_h_px(cx: &App, base_height: f32, base_line_height: f32, base_pad_y: f32) -> Pixels {
    px(fit_h_value(cx, base_height, base_line_height, base_pad_y))
}

/// Return the drawn height of a dense-strip button, in base px.
///
/// Pinned to `MoonSize::Xs.control_metrics().height`, not [`CONTROL_TIER`]. Two callers need it:
/// a plain `div` sitting BESIDE such a button (a card title) takes the same box so the row's
/// `items_center` centres two equal heights instead of centring a text line box against a taller
/// pill, and the chart's action overlay sizes its own layout from it.
pub fn micro_control_h_value(cx: &App) -> f32 {
    ui_value(cx, MoonSize::Xs.control_metrics().height)
}

/// [`micro_control_h_value`] as `Pixels` — the `*_value`/`*_px` pair every geometry helper in
/// this file ships, because layout arithmetic needs the `f32` and styling needs the `Pixels`.
pub fn micro_control_h(cx: &App) -> Pixels {
    px(micro_control_h_value(cx))
}

// ---- Goal C: dock chrome shared by every panel ----

/// Return the drawn height of an ordinary button at the design's control tier, in base px.
///
/// Reads [`CONTROL_TIER`]'s `control_metrics().height` through [`ui_value`]. This is the
/// ordinary-control family; dense strips use [`micro_control_h_value`].
pub fn action_control_h_value(cx: &App) -> f32 {
    ui_value(cx, CONTROL_TIER.control_metrics().height)
}

/// [`action_control_h_value`] as `Pixels` — the `*_value`/`*_px` pair every geometry helper in
/// this file ships.
pub fn action_control_h_px(cx: &App) -> Pixels {
    px(action_control_h_value(cx))
}

/// Height floor a panel footer row never sits below, so a footer carrying only text never reads
/// shorter than one carrying an ordinary control beside it.
pub fn panel_band_min_h_px(cx: &App) -> Pixels {
    action_control_h_px(cx)
}

/// Glyph a pinned scope chip draws in place of the interactive trigger's dropdown caret.
///
/// MoonUI ships no lock or pin icon — the whole `moon-ui-components-assets` icon set was checked
/// and carries none. A colour emoji such as `🔒` (U+1F512) is drawn from a colour glyph table and
/// therefore ignores `text_color`, which would break the muted-vs-disabled distinction this whole
/// change exists to make, and `▦` (U+25A6) already failed on this Windows font stack (see
/// [`COLUMN_SELECTOR_ICON`]'s doc above). `●` is proven to render here AND already means "pinned"
/// in this app — `panels/chart/render.rs:823` draws `●` for a pinned chart pane and `○` for an
/// unpinned one.
pub const PINNED_SCOPE_GLYPH: &str = "●";

/// Design-unit gap between a pinned scope chip's glyph and its label.
pub const PINNED_SCOPE_GAP: f32 = 4.0;

/// Design-unit horizontal padding a pinned scope chip applies on both sides.
pub const PINNED_SCOPE_PAD_X: f32 = 7.0;

/// The colour a panel footer fact renders at for a shared tone. `Warn` and `Alarm` are
/// deliberately absent: every caller that needs them already has its own literal and this helper
/// only covers the tones more than one footer resolves the same way.
pub fn footer_tone_color(p: MoonPalette, tone: MoonTone) -> u32 {
    match tone {
        MoonTone::Positive => positive_color(p),
        MoonTone::Danger => danger_color(p),
        _ => p.text_soft,
    }
}

/// Width of mono text drawn at the terminal's body size — [`ui_text_width_zoomed`] at
/// [`BODY_TEXT`].
///
/// Exists so a caller measuring text it drew with `t_body()` + [`mono`] cannot reach for
/// `t_body(cx)` as the size argument: that value is ALREADY scaled, and passing it in would scale
/// twice. These five helpers measure TERMINAL text, which follows the control tier; plain
/// [`ui_text_width`] stays on the legacy font channel for mirroring MoonUI-internal text.
pub fn mono_body_text_width(cx: &App, text: &str, weight: f32) -> f32 {
    ui_text_width_zoomed(cx, text, BODY_TEXT, weight, true)
}

/// Width of mono text drawn at the terminal's caption size — the [`t_caption`] partner of
/// [`mono_body_text_width`], and it exists for the same reason: `t_caption(cx)` is already scaled,
/// so passing it in would scale twice. Measures TERMINAL text on the control tier; plain
/// [`ui_text_width`] stays on the legacy font channel for mirroring MoonUI-internal text.
pub fn mono_caption_text_width(cx: &App, text: &str, weight: f32) -> f32 {
    ui_text_width_zoomed(cx, text, BODY_TEXT - 2.0, weight, true)
}

/// Width of mono text drawn at the terminal's title size — the [`t_title`] partner of
/// [`mono_body_text_width`], and it exists for the same reason: `t_title(cx)` is already scaled,
/// so passing it in would scale twice. Measures TERMINAL text on the control tier; plain
/// [`ui_text_width`] stays on the legacy font channel for mirroring MoonUI-internal text.
pub fn mono_title_text_width(cx: &App, text: &str, weight: f32) -> f32 {
    ui_text_width_zoomed(cx, text, BODY_TEXT + 3.0, weight, true)
}

/// Width of UI-face text drawn at the terminal's body size — the [`ui_font`] partner of
/// [`mono_body_text_width`], filling in the same body base.
///
/// A layout that sizes itself from a measured label must measure in the family that label is
/// RENDERED in: prose reads in [`ui_font`], so measuring it with the mono trio above overstates
/// every proportional string and drifts the column it sizes. Measures TERMINAL text on the control
/// tier; plain [`ui_text_width`] stays on the legacy font channel for mirroring MoonUI-internal
/// text.
pub fn ui_body_text_width(cx: &App, text: &str, weight: f32) -> f32 {
    ui_text_width_zoomed(cx, text, BODY_TEXT, weight, false)
}

/// Width of UI-face text drawn at the terminal's caption size — the [`t_caption`] partner of
/// [`ui_body_text_width`], and the [`ui_font`] partner of [`mono_caption_text_width`]. Measures
/// TERMINAL text on the control tier; plain [`ui_text_width`] stays on the legacy font channel for
/// mirroring MoonUI-internal text.
pub fn ui_caption_text_width(cx: &App, text: &str, weight: f32) -> f32 {
    ui_text_width_zoomed(cx, text, BODY_TEXT - 2.0, weight, false)
}
