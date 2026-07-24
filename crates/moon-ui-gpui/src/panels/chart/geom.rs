//! Chart-panel geometry and hit-testing: window coordinates to local device pixels, pane layout
//! into plot, glass, and control zones, and Y coordinate to price conversion. Numbered AddToChart
//! and Custom panels share these paths. Split from `chart.rs`.

use gpui::*;
use moon_core::figures::FigNode;

use super::ChartPanel;

/// Mapping between one pane's plot data coordinates (time/price) and its device pixels.
///
/// Shared by every interactive chart layer — figures place their nodes through it, news marks place
/// their gems — so a single description of "where does this moment sit on screen" serves all of them
/// and cannot drift between two copies.
pub(super) struct PaneMap {
    pub plot: moon_chart::view::Rect,
    pub epoch_ms: f64,
    pub left_rel: f32,
    pub window_ms: f32,
    pub center: f32,
    pub range: f32,
}

impl PaneMap {
    pub fn time_at_x(&self, x: f32) -> f64 {
        let rel = self.left_rel + (x - self.plot.x) / self.plot.w.max(1.0) * self.window_ms;
        self.epoch_ms + rel as f64
    }
    pub fn x_of_time(&self, time_ms: f64) -> f32 {
        let rel = (time_ms - self.epoch_ms) as f32;
        self.plot.x + (rel - self.left_rel) / self.window_ms.max(1e-3) * self.plot.w
    }
    pub fn price_at_y(&self, y: f32) -> f64 {
        let rel_y = ((y - self.plot.y) / self.plot.h.max(1.0)).clamp(0.0, 1.0);
        (self.center + (0.5 - rel_y) * self.range) as f64
    }
    pub fn y_of_price(&self, price: f64) -> f32 {
        let rel_y = 0.5 - (price as f32 - self.center) / self.range.max(1e-9);
        self.plot.y + rel_y * self.plot.h
    }
    pub fn node_at(&self, pos: (f32, f32)) -> FigNode {
        FigNode {
            time_ms: self.time_at_x(pos.0),
            price: self.price_at_y(pos.1),
        }
    }
}

impl ChartPanel {
    pub(super) fn chart_local(&self, pos: Point<Pixels>) -> Option<((f32, f32), bool)> {
        self.chart.chart_local_from_window_pos(pos)
    }

    /// Returns whether trading controls are confined to the order-book zone. Numbered AddToChart and
    /// Custom panels, including detached ones, always separate the book on the right from chart
    /// navigation on the left. The Settings toggle controls only Main, where `num` is `None`.
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

    /// Returns whether a window position lies in the trading control zone while zones are separate:
    /// the visible order book or the reserved right strip when it is hidden. This is the same
    /// `control_zone_rect` used for order placement. Trading actions, order dragging, order menus,
    /// and hotkeys remain active there, while pan, zoom, Main navigation, and fullscreen toggling are
    /// suppressed. Returns false when Main uses unified zones.
    pub(crate) fn window_pos_in_control_zone(&self, pos: Point<Pixels>, cx: &App) -> bool {
        if !self.separate_zones(cx) {
            return false;
        }
        let Some((local, within)) = self.chart_local(pos) else {
            return false;
        };
        within && self.glass_pane_at(local).is_some()
    }

    /// Returns whether a position is inside any pane rectangle, including its glass/order-book zone.
    /// This method performs only the pane-bounds test; the Main-stack caller separately excludes
    /// positions for which [`Self::window_pos_in_control_zone`] is true before toggling fullscreen.
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

    /// Returns whether the latest right-button gesture moved the price scale rather than clicking.
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
        // Approximate the ordinary pane split for input hit-testing: Left, Right, or Hide shifts the
        // plot, book, and axis gutter. Broom mode hides the local axis, but unlike ChartDataState
        // this helper still derives glass width from `orderbook_enabled` and does not model the
        // engine's full-width book-only rendering.
        use crate::persistence::chart_persist::PriceAxisPos;
        let axis_pos = if self.orderbook_only {
            PriceAxisPos::Hide
        } else {
            self.price_axis_pos
        };
        let price_axis_w = if matches!(axis_pos, PriceAxisPos::Hide) {
            0.0
        } else {
            moon_chart::PRICE_AXIS_W * self.last_ppp
        };
        let time_axis_h = if self.time_axis_visible {
            moon_chart::TIME_AXIS_H * self.last_ppp
        } else {
            0.0
        };
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
        let axis_on_left = matches!(axis_pos, PriceAxisPos::Left);
        let chart_x = if axis_on_left {
            rect.x + price_axis_w
        } else {
            rect.x
        };
        let chart_w = (rect.w - price_axis_w - glass_w).max(1.0);
        let glass_x = if matches!(axis_pos, PriceAxisPos::Right) {
            chart_x + chart_w
        } else {
            rect.x + (rect.w - glass_w).max(1.0)
        };
        let plot = moon_chart::view::Rect {
            x: chart_x,
            y: rect.y,
            w: chart_w,
            h: plot_h,
        };
        let glass = moon_chart::view::Rect {
            x: glass_x,
            y: rect.y,
            w: glass_w,
            h: plot_h,
        };
        Some((plot, glass))
    }

    pub(super) fn local_plot_rect(&self, pane: usize) -> Option<moon_chart::view::Rect> {
        self.local_pane_areas(pane).map(|(plot, _)| plot)
    }

    /// Build a pane's plot mapping, or return `None` when the pane has no valid view.
    pub(super) fn pane_map(&self, pane: usize) -> Option<PaneMap> {
        let plot = self.local_plot_rect(pane)?;
        let (epoch_ms, left_rel, window_ms, center, range) =
            self.chart.with_container(|container| {
                container.pane(pane).map(|p| {
                    let (l, w) = p.view.visible_x(plot.w);
                    (
                        p.view.epoch_ms,
                        l,
                        w,
                        p.view.render_center,
                        p.view.render_range,
                    )
                })
            })?;
        if !(range > 0.0) || window_ms <= 0.0 {
            return None;
        }
        Some(PaneMap {
            plot,
            epoch_ms,
            left_rel,
            window_ms,
            center,
            range,
        })
    }

    fn local_glass_rect(&self, pane: usize) -> Option<moon_chart::view::Rect> {
        self.local_pane_areas(pane).map(|(_, glass)| glass)
    }

    /// Returns a pane's order-control zone in device pixels. With the book visible on a narrow pane,
    /// the local glass width is `(GLASS_ZONE_PX * 0.8).min(rect.w * 0.5)`. With the book hidden it
    /// reserves the full capped base width, `GLASS_ZONE_PX.min(rect.w * 0.5)`, over the chart's right
    /// edge so order interaction and the boundary marker remain available.
    fn control_zone_rect(&self, pane: usize) -> Option<moon_chart::view::Rect> {
        if self.orderbook_enabled {
            return self.local_glass_rect(pane).filter(|g| g.w > 0.0);
        }
        let rect = self.local_pane_rect(pane)?;
        let time_axis_h = if self.time_axis_visible {
            moon_chart::TIME_AXIS_H * self.last_ppp
        } else {
            0.0
        };
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
