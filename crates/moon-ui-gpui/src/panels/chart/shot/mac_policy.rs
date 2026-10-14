//! Fail-closed decisions and coordinate conversion for the asynchronous macOS shot.

use std::time::Instant;

/// Whether this shot still owns a live, proven exchange-caption frame.
pub(super) fn publishable(
    generation: u64,
    current: u64,
    drawn: bool,
    deadline: Instant,
    now: Instant,
) -> bool {
    generation == current && drawn && now < deadline
}

/// Convert a bottom-left window-base rectangle to ScreenCaptureKit's top-left points.
/// Returns no rectangle for clipped slots, invalid geometry, or a titlebar-offset mismatch.
pub(super) fn source_rect(rect: [f64; 4], window: [f64; 2]) -> Option<[f64; 4]> {
    let [x, y, width, height] = rect;
    let [window_width, window_height] = window;
    if rect.into_iter().chain(window).any(|v| !v.is_finite())
        || x < 0.0
        || y < 0.0
        || width <= 0.0
        || height <= 0.0
        || x + width > window_width
        || y + height > window_height
    {
        return None;
    }
    Some([x, window_height - y - height, width, height])
}

/// Convert the validated logical crop to physical pixels without scaling the whole window to it.
/// Single-window ScreenCaptureKit ignores sourceRect, so cropping must happen after the read.
pub(super) fn crop_pixels(source: [f64; 4], scale: f64, image: [u32; 2]) -> Option<[u32; 4]> {
    if !scale.is_finite() || scale <= 0.0 || source.into_iter().any(|v| !v.is_finite() || v < 0.0) {
        return None;
    }
    let [x, y, width, height] = source.map(|v| (v * scale).round() as u32);
    if width == 0
        || height == 0
        || x.checked_add(width)? > image[0]
        || y.checked_add(height)? > image[1]
    {
        return None;
    }
    Some([x, y, width, height])
}

#[cfg(test)]
mod tests;
