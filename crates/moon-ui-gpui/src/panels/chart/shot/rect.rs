//! The chart-slot rectangle in the coordinates a screen capture needs.
//!
//! Kept apart from the platform arms because it is the one part of the shot that is pure
//! arithmetic and therefore the one part a test can pin. Both arms take their rectangle from here.

use gpui::{Bounds, Pixels};

/// A capture rectangle in PHYSICAL pixels, relative to the window's client area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShotRect {
    /// Left edge, client-relative.
    pub(crate) x: i32,
    /// Top edge, client-relative.
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Convert the chart engine's published slot geometry into a client-relative capture rectangle.
///
/// The SIZE is taken from the engine rather than recomputed. `chartdx::data_state::state::
/// apply_slot_geometry` derives the render target's dimensions as `round(logical * scale).max(1)`
/// and everything the chart draws is laid out against exactly those numbers, so recomputing them
/// here would be a second copy of the same arithmetic, free to drift by a pixel and reliably
/// noticed only as a hairline of the wrong colour down one edge of the screenshot.
///
/// The ORIGIN is rounded here, because the engine keeps it unrounded (it multiplies without
/// rounding, and only the render state consumes it). The capture can therefore sit up to one
/// physical pixel off the renderer's sub-pixel origin. That is deliberate and harmless for an
/// image; do not "fix" it by rounding the size to match, which would reintroduce the drift the
/// paragraph above avoids.
///
/// Args:
///     bounds: The slot in window-relative LOGICAL pixels, as published by the GPU canvas.
///     scale: Device pixels per logical pixel for the window the slot is in.
///     device_size: The slot's size in DEVICE pixels, already computed by the engine.
///
/// Returns:
///     The rectangle to capture, or `None` when it has no area to capture.
pub(super) fn slot_capture_rect(
    bounds: Bounds<Pixels>,
    scale: f32,
    device_size: (u32, u32),
) -> Option<ShotRect> {
    let (width, height) = device_size;
    if width == 0 || height == 0 {
        return None;
    }
    // A non-finite or non-positive scale means the window never reported one; there is no sane
    // rectangle to derive and a silent `as i32` of NaN is zero, which would capture the wrong
    // corner of the screen rather than nothing.
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let x = f32::from(bounds.origin.x) * scale;
    let y = f32::from(bounds.origin.y) * scale;
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    Some(ShotRect {
        x: x.round() as i32,
        y: y.round() as i32,
        width,
        height,
    })
}
