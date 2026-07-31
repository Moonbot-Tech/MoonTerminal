//! The chart's real on-screen geometry, reported by the chart element during render.
//!
//! The mouse storm targets these bounds instead of guessing at a window rectangle: FireTest must
//! drive the true OS input path over the true chart area, or it would measure nothing.

/// Where the live chart actually sits, in the units each platform's storm needs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChartProbe {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(super) hwnd: Option<isize>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) screen_left: f32,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) screen_top: f32,
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(super) scale_factor: f32,
}

impl ChartProbe {
    /// Build a probe, rejecting geometry too small or too degenerate to storm over.
    pub(crate) fn new(
        hwnd: Option<isize>,
        screen_left: f32,
        screen_top: f32,
        left: f32,
        top: f32,
        width: f32,
        height: f32,
        scale_factor: f32,
    ) -> Option<Self> {
        (width >= 80.0
            && height >= 80.0
            && scale_factor > 0.0
            && screen_left.is_finite()
            && screen_top.is_finite())
        .then_some(Self {
            hwnd,
            screen_left,
            screen_top,
            left,
            top,
            width,
            height,
            scale_factor,
        })
    }
}
