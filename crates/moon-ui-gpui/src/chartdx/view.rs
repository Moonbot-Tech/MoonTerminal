//! Maps `moon_chart::view::ChartView`, whose view math is shared with the reference renderer, to
//! the `ChartViewGpu` constant buffer. This module only prepares uniforms for own-pass layers and
//! performs no drawing.

use moon_chart::view::{ChartView, Rect};

use super::types::ChartViewGpu;

/// The theme-driven part of the shared chart uniform.
///
/// `ChartViewGpu` is bound by every layer in all three backends, so the two fields the appearance
/// settings reach through it are gathered here rather than passed as loose floats: a caller that
/// forgets one gets a compile error instead of a silently stock-looking chart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewStyle {
    /// `ChartTheme::marker_scale` — a multiplier on the trade-cross size, on top of the device
    /// pixel ratio.
    pub marker_scale: f32,
    /// `ChartTheme::trade_volume_alpha` — opacity of the per-trade volume bars.
    pub volume_alpha: f32,
}

impl Default for ViewStyle {
    /// The values that reproduce the appearance before either became configurable.
    fn default() -> Self {
        Self {
            marker_scale: 1.0,
            volume_alpha: 0.34,
        }
    }
}

/// Marker half-size in physical pixels: the logical base, the device scale, and the user's scale.
///
/// `theme_scale` is floored well above zero so a hand-edited `theme.toml` cannot make markers
/// vanish, which would read as trades not arriving rather than as a bad setting.
pub fn marker_half_physical_px(view: &ChartView, marker_scale: f32, theme_scale: f32) -> f32 {
    view.marker_half_px * marker_scale.max(0.1) * theme_scale.max(0.2)
}

pub fn cross_cull_margin_physical_px(view: &ChartView, marker_scale: f32, theme_scale: f32) -> f32 {
    marker_half_physical_px(view, marker_scale, theme_scale).max(7.0) + 1.0
}

#[cfg(test)]
mod tests;

/// Builds the GPU uniform for the current view and chart area in physical pixels. Fields are
/// assigned by name because `ChartViewGpu` uses a different order from `moon_chart::ChartUniform`,
/// so the structure cannot be copied with `memcpy`.
pub fn view_gpu(
    view: &ChartView,
    area: Rect,
    resolution: [f32; 2],
    marker_scale: f32,
    style: ViewStyle,
) -> ChartViewGpu {
    let (view_time0, _window_ms) = view.visible_x(area.w);
    let view_price0 = view.render_center - (area.h * 0.5) / view.px_per_price.max(1e-6);
    ChartViewGpu {
        bounds: [area.x, area.y, area.w, area.h],
        resolution,
        time_to_px: view.px_per_ms,
        view_time0,
        price_to_px: view.px_per_price,
        view_price0,
        marker_half: marker_half_physical_px(view, marker_scale, style.marker_scale),
        pad: 0.0,
        volume_buy_inv: 0.0,
        volume_sell_inv: 0.0,
        // The backends used to overwrite this with a compile-time constant on every upload, which
        // is why the field looked configurable for a long time without being so. They now carry
        // whatever arrives here.
        volume_alpha: style.volume_alpha.clamp(0.0, 1.0),
        _pad2: 0.0,
    }
}
