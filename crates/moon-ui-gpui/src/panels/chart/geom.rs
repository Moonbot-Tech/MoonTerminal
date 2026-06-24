//! Геометрия/хит-тест панели чарта: перевод оконных координат в локальные device-px,
//! раскладка pane → plot/glass/зона управления, цена по Y. Вынесено из `chart.rs`.

use gpui::*;

use super::ChartPanel;

impl ChartPanel {
    pub(super) fn chart_local(&self, pos: Point<Pixels>) -> Option<((f32, f32), bool)> {
        self.chart.chart_local_from_window_pos(pos)
    }

    /// Раздельные зоны управления (ордера/линии только в зоне стакана). На Add-вкладках и
    /// выносных окнах (`num.is_some()`) — ВСЕГДА вкл: там всегда две зоны (стакан справа +
    /// чарт слева, дабл-клик по чарту → на Main). Галка в Настройках управляет ТОЛЬКО
    /// вкладкой Main (`num.is_none()`).
    pub(super) fn separate_zones(&self, cx: &App) -> bool {
        if self.num.is_some() {
            return true;
        }
        let b = self.backend.read(cx);
        b.preview
            .as_ref()
            .unwrap_or(&b.config)
            .separate_control_zones
    }

    pub(crate) fn window_pos_in_glass_zone(&self, pos: Point<Pixels>) -> bool {
        let Some(((x, y), within)) = self.chart_local(pos) else {
            return false;
        };
        if !within {
            return false;
        }
        let rects = if self.input.pane_rects.is_empty() {
            self.chart.pane_rects()
        } else {
            self.input.pane_rects.clone()
        };
        rects.iter().any(|(_, r)| {
            if x < r.x || x > r.x + r.w || y < r.y || y > r.y + r.h {
                return false;
            }
            let glass_w = moon_chart::GLASS_ZONE_PX.min(r.w * 0.5);
            x >= r.x + r.w - glass_w
        })
    }

    /// Позиция внутри любой pane-области панели, включая glass/orderbook-зону.
    /// Main stack использует это для ПКМ fullscreen ↔ stack: зона стакана не является
    /// отдельным UI-исключением, пока такая настройка явно не вынесена в UI.
    pub(crate) fn window_pos_allows_main_stack_toggle(&self, pos: Point<Pixels>) -> bool {
        let Some(((x, y), within)) = self.chart_local(pos) else {
            return false;
        };
        if !within {
            return false;
        }
        let rects = if self.input.pane_rects.is_empty() {
            self.chart.pane_rects()
        } else {
            self.input.pane_rects.clone()
        };
        local_pos_in_any_pane_rect(x, y, &rects)
    }

    /// Был ли последний ПКМ зум-перетаскиванием цены (а не коротким кликом).
    pub(crate) fn rmb_was_moved(&self) -> bool {
        self.input.rmb_moved()
    }

    fn local_pane_rect(&self, pane: usize) -> Option<moon_chart::view::Rect> {
        self.input
            .pane_rects
            .iter()
            .find(|(idx, _)| *idx == pane)
            .map(|(_, rect)| *rect)
            .or_else(|| {
                self.chart
                    .pane_rects()
                    .into_iter()
                    .find(|(idx, _)| *idx == pane)
                    .map(|(_, rect)| rect)
            })
    }

    fn local_pane_areas(
        &self,
        pane: usize,
    ) -> Option<(moon_chart::view::Rect, moon_chart::view::Rect)> {
        let rect = self.local_pane_rect(pane)?;
        let price_axis_w = moon_chart::PRICE_AXIS_W * self.last_ppp;
        let time_axis_h = moon_chart::TIME_AXIS_H * self.last_ppp;
        let plot_h = (rect.h - time_axis_h).max(1.0);
        let glass_cap = rect.w * 0.5;
        let glass_base = moon_chart::GLASS_ZONE_PX.min(glass_cap);
        let chart_w_base = rect.w - price_axis_w - glass_base;
        let glass_w = if !self.orderbook_enabled {
            0.0
        } else if chart_w_base < glass_base * 2.0 {
            (moon_chart::GLASS_ZONE_PX * 0.8).min(glass_cap)
        } else {
            glass_base
        };
        let plot = moon_chart::view::Rect {
            x: rect.x + price_axis_w,
            y: rect.y,
            w: (rect.w - price_axis_w - glass_w).max(1.0),
            h: plot_h,
        };
        let glass = moon_chart::view::Rect {
            x: rect.x + (rect.w - glass_w).max(1.0),
            y: rect.y,
            w: glass_w,
            h: plot_h,
        };
        Some((plot, glass))
    }

    pub(super) fn local_plot_rect(&self, pane: usize) -> Option<moon_chart::view::Rect> {
        self.local_pane_areas(pane).map(|(plot, _)| plot)
    }

    fn local_glass_rect(&self, pane: usize) -> Option<moon_chart::view::Rect> {
        self.local_pane_areas(pane).map(|(_, glass)| glass)
    }

    /// Зона управления ордерами панели (device-px, как `chart_local`/`pane_rects`). Стакан
    /// виден → его glass-полоса; стакан СКРЫТ → резервируем полосу той же ширины справа поверх
    /// чарта, чтобы место под ордера (и риска границы) оставалось и при свёрнутом стакане.
    fn control_zone_rect(&self, pane: usize) -> Option<moon_chart::view::Rect> {
        if self.orderbook_enabled {
            return self.local_glass_rect(pane).filter(|g| g.w > 0.0);
        }
        let rect = self.local_pane_rect(pane)?;
        let time_axis_h = moon_chart::TIME_AXIS_H * self.last_ppp;
        let plot_h = (rect.h - time_axis_h).max(1.0);
        let w = moon_chart::GLASS_ZONE_PX.min(rect.w * 0.5);
        Some(moon_chart::view::Rect {
            x: rect.x + (rect.w - w).max(1.0),
            y: rect.y,
            w,
            h: plot_h,
        })
    }

    pub(super) fn glass_pane_at(&self, pos: (f32, f32)) -> Option<usize> {
        let pane = self.input.pane_at(pos.0, pos.1)?;
        let zone = self.control_zone_rect(pane)?;
        (zone.w > 0.0
            && pos.0 >= zone.x
            && pos.0 <= zone.x + zone.w
            && pos.1 >= zone.y
            && pos.1 <= zone.y + zone.h)
            .then_some(pane)
    }

    pub(super) fn price_at_pane_y(&self, pane: usize, y: f32) -> Option<f64> {
        let plot = self.local_plot_rect(pane)?;
        if plot.h <= 1.0 {
            return None;
        }
        let (center, range) = self.chart.with_container(|container| {
            container
                .pane(pane)
                .map(|pane| (pane.view.render_center, pane.view.render_range))
        })?;
        if !(range > 0.0) || !center.is_finite() {
            return None;
        }
        let rel_y = ((y - plot.y) / plot.h).clamp(0.0, 1.0);
        let price = center + (0.5 - rel_y) * range;
        (price.is_finite() && price > 0.0).then_some(price as f64)
    }
}

fn local_pos_in_any_pane_rect(x: f32, y: f32, rects: &[(usize, moon_chart::view::Rect)]) -> bool {
    rects
        .iter()
        .any(|(_, r)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
}
