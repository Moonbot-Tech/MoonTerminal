//! Data-table row and column metrics.

use super::*;

/// Return the effective font-scaled `MoonDataTable` row height.
///
/// This delegates to the component's own `table_row_height()` tier metric, so Standard renders
/// today's number exactly and wrappers computing natural table height do not clip rows at large
/// font settings or drift from the tier the table itself draws.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The rendered row height in pixels.
pub fn table_row_h(cx: &App) -> f32 {
    MoonTheme::active_tokens(cx).table_row_height()
}

/// Return the effective font-scaled `MoonDataTable` header height.
///
/// This delegates to the component's own `table_header_height()` tier metric, so Standard renders
/// today's number exactly and wrappers stay aligned with the header the table itself draws.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The rendered header height in pixels.
pub fn table_head_h(cx: &App) -> f32 {
    MoonTheme::active_tokens(cx).table_header_height()
}

/// The unfitted `MoonDataTable` header-base tier metric, in `Pixels`, for a caller that draws its
/// own header row rather than letting `MoonDataTable` draw it. Standard renders today's base
/// number exactly; fitting remains the caller's responsibility because this helper does not apply
/// the component's font-height adjustment.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The tier's header base put through the UI-zoom channel.
pub fn table_head_base_px(cx: &App) -> Pixels {
    ui_px(cx, MoonTheme::active_tokens(cx).table_header_base())
}

/// Return the current text-container width scale relative to the theme base.
///
/// [`ui_value`] of [`BODY_TEXT`], divided by [`base_text`]: 14 / 11 at the default typography.
/// Identical to MoonUI's `font_width_scale` at the design's font delta.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The active scaled base font size divided by the unscaled base.
pub fn font_scale(cx: &App) -> f32 {
    ui_value(cx, BODY_TEXT) / base_text(cx)
}

/// Scale a fixed text-container width with the terminal's text channel and return `Pixels`.
///
/// Args:
///     cx: Application context used to calculate [`font_scale`].
///     base: Design-reference width; the scale is 14 / 11 at the default typography.
///
/// Returns:
///     The font-scaled width as `Pixels`.
pub fn font_w_px(cx: &App, base: f32) -> Pixels {
    px(font_w(cx, base))
}

/// Scale a fixed text-container width with the terminal's text channel and return raw pixels as
/// `f32`.
///
/// Use this for MoonUI builders that apply raw `px(...)` without their own scaling, including
/// `menu_width`, `trigger_width`, and `MoonButton::width`.
///
/// Args:
///     cx: Application context used to calculate [`font_scale`].
///     base: Design-reference width; the scale is 14 / 11 at the default typography.
///
/// Returns:
///     The font-scaled raw pixel width.
pub fn font_w(cx: &App, base: f32) -> f32 {
    base * font_scale(cx)
}

// Radius tokens come from `MoonMetrics::TERMINAL`, rather than local numeric values. Avoid raw
// `px(N)` in `.rounded()`. Pill values such as `SEL_H / 2.0` or `999.0` describe shape, not a radius
// tier, and are outside this rule. `*_BASE` values are unscaled tokens for MoonUI builders that
// scale internally; `r_*` functions return ready-to-use `Pixels` for raw GPUI `.rounded()`. Mixing
// them applies scaling twice.
//
// `MoonMetrics` exposes these two shared radius tokens and no shared small-radius token for chips or
// swatches; a third `radius_sm` metric has been requested upstream.
/// Unscaled shared `container_radius` token for MoonUI builders that apply UI scaling internally.
pub const R_CONTAINER_BASE: f32 = M.container_radius;

/// Return the control-tier radius for buttons, cards, popups, and panels.
///
/// Args:
///     cx: Application context used to apply UI scaling.
///
/// Returns:
///     The ready-to-use raw-GPUI radius.
pub fn r_button(cx: &App) -> Pixels {
    ui_px(cx, CONTROL_TIER.control_metrics().radius)
}

/// Return the scaled MoonUI `container_radius`, default 8, for dialogs, modals, and containers.
///
/// Args:
///     cx: Application context used to apply UI scaling.
///
/// Returns:
///     The ready-to-use raw-GPUI radius.
pub fn r_container(cx: &App) -> Pixels {
    ui_px(cx, R_CONTAINER_BASE)
}
