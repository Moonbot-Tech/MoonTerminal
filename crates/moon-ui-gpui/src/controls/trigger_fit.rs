//! Closed-menu widths that hold the chosen word whole.
//!
//! A closed `MoonDropdown` shows only its chosen label, so a fixed trigger narrower than that
//! label hides the very answer the trader is looking for. Most menus fix this on their own with
//! `MoonDropdown::fit_trigger_width`; this module is for the hosts that also have to SIZE around
//! the result — a popup column, a table column, a key cell — and must know the trigger's width
//! before the trigger is drawn.
//!
//! Every width here is measured through `MoonDropdown::fitted_trigger_label`, the exact function
//! the component fits with, so a host and the trigger it holds cannot disagree by a rounding.

use gpui::App;
use moon_ui::{MoonButtonSize, MoonDropdown};

#[cfg(test)]
pub(crate) mod probe;

/// Upper bound handed to the measurement when the natural width is wanted.
const UNBOUNDED: f32 = 4000.0;
/// Design width used to read the component's design-to-rendered scale.
const SCALE_PROBE: f32 = 1000.0;

/// Rendered width a caret trigger needs to show `label` whole, padding and caret included.
///
/// Args:
///     cx: Application context supplying the active theme's text metrics.
///     label: The chosen label, without a caret.
///     size: The trigger's `MoonButtonSize`.
///
/// Returns:
///     The rendered width in pixels.
pub(crate) fn natural_px(cx: &App, label: &str, size: MoonButtonSize) -> f32 {
    MoonDropdown::fitted_trigger_label(cx, label, size, 0.0, UNBOUNDED).1
}

/// Rendered width of the widest of `labels` on a caret trigger, never below `floor` design units.
///
/// Args:
///     cx: Application context supplying the active theme's text metrics.
///     labels: Every label the trigger can show.
///     size: The trigger's `MoonButtonSize`.
///     floor: Design-unit width the trigger never goes below.
///
/// Returns:
///     The rendered width in pixels.
pub(crate) fn widest_px<'a>(
    cx: &App,
    labels: impl IntoIterator<Item = &'a str>,
    size: MoonButtonSize,
    floor: f32,
) -> f32 {
    labels
        .into_iter()
        .map(|label| natural_px(cx, label, size))
        .fold(floor * design_scale(cx, size), f32::max)
}

/// The factor the component turns a design-unit `fit_trigger_width` bound into pixels with.
///
/// Read from the component itself rather than recomputed: MoonUI scales the bounds by its
/// font channel at the trigger's own font size, which is neither `design::font_w` nor the `ui`
/// channel once the Font slider leaves zero.
///
/// Args:
///     cx: Application context supplying the active theme's text metrics.
///     size: The trigger's `MoonButtonSize`.
///
/// Returns:
///     Rendered pixels per design unit.
pub(crate) fn design_scale(cx: &App, size: MoonButtonSize) -> f32 {
    MoonDropdown::fitted_trigger_label(cx, "", size, SCALE_PROBE, SCALE_PROBE).1 / SCALE_PROBE
}

/// Design-unit width, for `fit_trigger_width`, of the widest of `labels`, never below `floor`.
///
/// A column of several triggers passes this as BOTH bounds, so every row of the column draws at
/// the one width the longest label needs and the column stays straight.
///
/// Args:
///     cx: Application context supplying the active theme's text metrics.
///     labels: Every label the column shows.
///     size: The triggers' `MoonButtonSize`.
///     floor: Design-unit width the column never goes below.
///
/// Returns:
///     The design-unit width, rounded up to a whole unit.
pub(crate) fn widest_design<'a>(
    cx: &App,
    labels: impl IntoIterator<Item = &'a str>,
    size: MoonButtonSize,
    floor: f32,
) -> f32 {
    (widest_px(cx, labels, size, floor) / design_scale(cx, size).max(f32::EPSILON)).ceil()
}
