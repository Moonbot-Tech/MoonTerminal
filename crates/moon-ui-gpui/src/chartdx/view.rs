//! Маппинг `moon_chart::view::ChartView` (математика вида, общая с эталоном) → наш cbuffer
//! `ChartViewGpu`. Никакого рисования — только подготовка uniform для own-pass слоёв.

use moon_chart::view::{ChartView, Rect};

use super::types::{ChartViewGpu, DEFAULT_VOLUME_ALPHA};

/// Собирает GPU-юнформ для текущего вида и чарт-области (физ. px). Поля заполняются ПО ИМЕНАМ
/// (порядок в `ChartViewGpu` отличается от `moon_chart` ChartUniform — нельзя memcpy).
pub fn view_gpu(view: &ChartView, area: Rect, resolution: [f32; 2]) -> ChartViewGpu {
    let (view_time0, _window_ms) = view.visible_x(area.w);
    let view_price0 = view.render_center - (area.h * 0.5) / view.px_per_price.max(1e-6);
    ChartViewGpu {
        bounds: [area.x, area.y, area.w, area.h],
        resolution,
        time_to_px: view.px_per_ms,
        view_time0,
        price_to_px: view.px_per_price,
        view_price0,
        marker_half: view.marker_half_px,
        pad: 0.0,
        volume_buy_inv: 0.0,
        volume_sell_inv: 0.0,
        volume_alpha: DEFAULT_VOLUME_ALPHA,
        _pad2: 0.0,
    }
}
