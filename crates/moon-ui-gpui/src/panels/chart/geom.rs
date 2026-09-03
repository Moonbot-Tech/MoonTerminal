//! Chart-panel geometry and hit-testing: window coordinates to local device pixels, pane layout
//! into plot, glass, and control zones, and Y coordinate to price conversion. Numbered AddToChart
//! and Custom panels share these paths. Split from `chart.rs`.

use gpui::*;
use moon_core::figures::proj::PxPoint;
use moon_core::figures::{FigNode, Proj};

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

/// The projection every figure hit test runs through. Implementing the core trait here — rather
/// than duplicating the arithmetic in `moon-core` — keeps ONE description of where a moment sits
/// on screen: figures, news marks and warning badges all read this map.
impl Proj for PaneMap {
    fn time_at_x(&self, x: f32) -> f64 {
        let rel = self.left_rel + (x - self.plot.x) / self.plot.w.max(1.0) * self.window_ms;
        self.epoch_ms + rel as f64
    }
    fn x_of_time(&self, time_ms: f64) -> f32 {
        let rel = (time_ms - self.epoch_ms) as f32;
        self.plot.x + (rel - self.left_rel) / self.window_ms.max(1e-3) * self.plot.w
    }
    fn price_at_y(&self, y: f32) -> f64 {
        // Clamped, because this answers for a POINTER: a click cannot mean a price outside the
        // plot the user is looking at. The arithmetic itself lives once, in `price_at_rel`.
        self.price_at_rel(self.rel_y(y).clamp(0.0, 1.0))
    }
    fn y_of_price(&self, price: f64) -> f32 {
        let rel_y = 0.5 - (price as f32 - self.center) / self.range.max(1e-9);
        self.plot.y + rel_y * self.plot.h
    }
}

impl PaneMap {
    /// Where a pixel sits down the plot: 0 at its top edge, 1 at its bottom, outside that beyond
    /// them.
    fn rel_y(&self, y: f32) -> f32 {
        (y - self.plot.y) / self.plot.h.max(1.0)
    }

    /// The price at a relative position down the plot. The ONE statement of the Y mapping; both
    /// readings below are this function with and without a clamp on its argument.
    fn price_at_rel(&self, rel_y: f32) -> f64 {
        (self.center + (0.5 - rel_y) * self.range) as f64
    }

    /// Node under a pixel WITHOUT clamping the price to the visible range.
    ///
    /// [`Proj::price_at_y`] clamps, which is right for a pointer. It is wrong for a point a tool
    /// DERIVES — a triangle's apex is raised as far from its base as the base is long, so any base
    /// wider than the plot is tall puts it off screen, and clamping would stick the vertex to the
    /// top of the view and make the stored figure depend on the Y zoom it was drawn at.
    pub(super) fn node_at_unclamped(&self, p: PxPoint) -> FigNode {
        FigNode {
            time_ms: self.time_at_x(p.0),
            price: self.price_at_rel(self.rel_y(p.1)),
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

    /// Returns whether a window position lies over the order book, or over the strip reserved for
    /// it, whatever the zone setting says: the Main stack asks this to hand the WHEEL to the stack
    /// instead of zooming the chart under it. It reads the same rectangle as
    /// [`Self::control_zone_rect`], so a book-only broom pane — book across its whole width — gives
    /// the stack the wheel everywhere on it rather than only along its right edge.
    pub(crate) fn window_pos_in_glass_zone(&self, pos: Point<Pixels>) -> bool {
        let Some(((x, y), within)) = self.chart_local(pos) else {
            return false;
        };
        if !within {
            return false;
        }
        self.with_pane_rects(|rects| {
            rects.iter().any(|(_, r)| {
                if x < r.x || x > r.x + r.w || y < r.y || y > r.y + r.h {
                    return false;
                }
                // Everything from the zone's left edge rightwards, over the pane's FULL height: the
                // wheel belongs to the stack over the book, over an axis gutter beside it and over
                // the time-axis band beneath it alike. Only that left edge is a boundary here.
                x >= self.control_zone_of(*r).x
            })
        })
    }

    /// Returns whether a window position is closed to chart gestures — pan, zoom, the open-on-Main
    /// double click, fullscreen toggling — because it belongs to trading instead.
    ///
    /// Two different reasons answer yes, and they are separate questions. A book-only broom pane
    /// says yes over ALL of it: there is no plot on it to pan or open, so nothing there can mean
    /// chart, and the Settings toggle has no say — it governs whether to split a pane that HAS
    /// both. An ordinary pane says yes inside its control zone while that toggle is on, where
    /// trading actions, order dragging, order menus and hotkeys stay live.
    pub(crate) fn window_pos_in_control_zone(&self, pos: Point<Pixels>, cx: &App) -> bool {
        // Cheapest first: an ordinary pane under unified zones answers no without touching geometry,
        // and that is the common case on Main.
        if !self.orderbook_only && !self.separate_zones(cx) {
            return false;
        }
        let Some((local, within)) = self.chart_local(pos) else {
            return false;
        };
        // On no pane at all — an empty stack slot, the gap between panes — the answer is no. Chart
        // gestures have nothing to act on there either, but claiming the point would swallow the
        // press instead of leaving it to whoever owns that space.
        within
            && self.pane_at_with_fallback(local).is_some()
            && self.chart_gesture_pane_at(local).is_none()
    }

    /// The pane holding a local point, from the render's published rectangles or, before the first
    /// render publishes any, the engine's current layout.
    ///
    /// `ChartInput::pane_at` answers the same question WITHOUT that fallback; every hit test in
    /// this file goes through here so the two halves of one gesture cannot disagree on a chart
    /// whose first frame has not landed.
    pub(super) fn pane_at_with_fallback(&self, pos: (f32, f32)) -> Option<usize> {
        self.with_pane_rects(|rects| local_pane_rect_at(pos.0, pos.1, rects))
            .map(|(idx, _)| idx)
    }

    /// The pane whose CHART SPACE holds this point, or `None` when the point belongs to trading —
    /// the order book, the strip reserved for it, or anywhere on a book-only broom pane.
    ///
    /// The one statement of "this is chart, not book", so a gesture added later cannot get the
    /// question half right: figure drawing, figure hit testing and the chart-space order-cross gate
    /// all ask it, and each of them used to spell it out again.
    pub(super) fn chart_gesture_pane_at(&self, pos: (f32, f32)) -> Option<usize> {
        let pane = self.pane_at_with_fallback(pos)?;
        if self.orderbook_only || self.glass_pane_at(pos).is_some() {
            return None;
        }
        Some(pane)
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
        self.with_pane_rects(|rects| local_pos_in_any_pane_rect(x, y, rects))
    }

    /// Returns whether the latest right-button gesture moved the price scale rather than clicking.
    pub(crate) fn rmb_was_moved(&self) -> bool {
        self.input.rmb_moved()
    }

    /// Run `f` over this panel's pane rectangles in device pixels.
    ///
    /// Input arrives before the first render has published `input.pane_rects` — a wheel event over
    /// a freshly opened chart — so the engine's current layout stands in for them. Borrowed rather
    /// than returned: hit testing runs on the pointer path, and the steady-state branch must not
    /// allocate a vector per event.
    fn with_pane_rects<R>(&self, f: impl FnOnce(&[(usize, moon_chart::view::Rect)]) -> R) -> R {
        if self.input.pane_rects.is_empty() {
            f(&self.chart.pane_rects())
        } else {
            f(&self.input.pane_rects)
        }
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

    /// This pane's areas as the ENGINE lays them out — the same call `prepare` makes, so hit
    /// testing cannot answer for a layout that was never drawn. The copy that used to live here
    /// derived the book's width from the Order Book toggle alone and never learned about book-only
    /// broom mode, where the book takes the whole pane.
    fn local_pane_areas(&self, rect: moon_chart::view::Rect) -> crate::chartdx::PaneAreas {
        crate::chartdx::pane_layout(
            rect,
            self.orderbook_only,
            self.orderbook_enabled,
            self.time_axis_visible,
            self.price_axis_pos,
            self.last_ppp,
        )
    }

    pub(super) fn local_plot_rect(&self, pane: usize) -> Option<moon_chart::view::Rect> {
        Some(self.local_pane_areas(self.local_pane_rect(pane)?).plot)
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

    /// Whether this panel DRAWS an order book, which is not the same question as its Order Book
    /// toggle: broom mode draws one regardless, exactly as `ChartDataState` decides it.
    pub(super) fn orderbook_drawn(&self) -> bool {
        self.orderbook_enabled || self.orderbook_only
    }

    pub(super) fn control_zone_rect(&self, pane: usize) -> Option<moon_chart::view::Rect> {
        Some(self.control_zone_of(self.local_pane_rect(pane)?))
    }

    /// A pane's order-control zone in device pixels, taken from the pane rectangle the caller
    /// already holds.
    ///
    /// The book's OWN area whenever one is drawn, so a cramped pane's narrowed book and a book-only
    /// broom pane's full-width one are each exactly the zone they look like. With no book at all it
    /// reserves `GLASS_ZONE_PX.min(rect.w * 0.5)` over the chart's right edge instead, so order
    /// interaction and the boundary marker still have somewhere to live.
    pub(super) fn control_zone_of(&self, rect: moon_chart::view::Rect) -> moon_chart::view::Rect {
        let areas = self.local_pane_areas(rect);
        if self.orderbook_drawn() {
            return areas.glass;
        }
        let w = moon_chart::GLASS_ZONE_PX.min(rect.w * 0.5);
        moon_chart::view::Rect {
            x: rect.x + (rect.w - w).max(0.0),
            y: rect.y,
            w,
            h: areas.plot.h,
        }
    }

    pub(super) fn glass_pane_at(&self, pos: (f32, f32)) -> Option<usize> {
        let pane = self.pane_at_with_fallback(pos)?;
        let zone = self.control_zone_rect(pane)?;
        // A zone of zero width is a pane with no book and no reserved strip; nothing to be inside.
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
    local_pane_rect_at(x, y, rects).is_some()
}

/// The pane rectangle holding a point, if any.
fn local_pane_rect_at(
    x: f32,
    y: f32,
    rects: &[(usize, moon_chart::view::Rect)],
) -> Option<(usize, moon_chart::view::Rect)> {
    rects
        .iter()
        .find(|(_, r)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
        .copied()
}
