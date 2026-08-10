//! Maps `moon_chart::view::ChartView`, whose view math is shared with the reference renderer, to
//! the `ChartViewGpu` constant buffer. This module only prepares uniforms for own-pass layers and
//! performs no drawing.

use moon_chart::view::{ChartView, Rect};

use super::types::{ChartViewGpu, DEFAULT_VOLUME_ALPHA};

pub fn marker_half_physical_px(view: &ChartView, marker_scale: f32) -> f32 {
    view.marker_half_px * marker_scale.max(0.1)
}

pub fn cross_cull_margin_physical_px(view: &ChartView, marker_scale: f32) -> f32 {
    marker_half_physical_px(view, marker_scale).max(7.0) + 1.0
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
        marker_half: marker_half_physical_px(view, marker_scale),
        pad: 0.0,
        volume_buy_inv: 0.0,
        volume_sell_inv: 0.0,
        volume_alpha: DEFAULT_VOLUME_ALPHA,
        volume_height_frac: 0.18,
        price_line: [0.82, 0.60, 0.36, 0.82],
        mark_price_line: [0.42, 0.72, 1.00, 0.78],
        price_line_width: 1.7,
        volume_style: 1.0,
        _pad3: [0.0; 2],
    }
}
