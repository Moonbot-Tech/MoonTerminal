//! Маппинг `moon_chart::view::ChartView` (математика вида, общая с эталоном) → наш cbuffer
//! `ChartViewGpu`. Никакого рисования — только подготовка uniform для own-pass слоёв.

use moon_chart::view::{ChartView, Rect};

use super::types::{ChartViewGpu, DEFAULT_VOLUME_ALPHA};

pub fn marker_half_physical_px(view: &ChartView, marker_scale: f32) -> f32 {
    view.marker_half_px * marker_scale.max(0.1)
}

pub fn cross_cull_margin_physical_px(view: &ChartView, marker_scale: f32) -> f32 {
    marker_half_physical_px(view, marker_scale).max(7.0) + 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_cull_margin_matches_shader_margin() {
        let mut view = ChartView::new(0.0);
        view.marker_half_px = 3.5;
        assert_eq!(cross_cull_margin_physical_px(&view, 1.0), 8.0);
        assert_eq!(cross_cull_margin_physical_px(&view, 3.0), 11.5);
    }
}

/// Собирает GPU-юнформ для текущего вида и чарт-области (физ. px). Поля заполняются ПО ИМЕНАМ
/// (порядок в `ChartViewGpu` отличается от `moon_chart` ChartUniform — нельзя memcpy).
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
        _pad2: 0.0,
    }
}
