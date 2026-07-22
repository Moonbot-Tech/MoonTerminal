//! GPUI chart-area input state, ported from the removed egui implementation without winit types.
//! Wheel input uses a plain `dy: f32`, and mouse buttons use the local [`Btn`] enum. All stored
//! coordinates are device pixels, matching the chart renderer and [`ChartView`]. The `ChartPanel`
//! handlers obtain them through `ChartEngine::chart_local_from_window_pos`, which converts logical
//! window coordinates as `(position - slot_origin) * scale_factor`. This layer mutates chart views
//! immediately; callers own event propagation, GPUI notification, and pending Main-open handoff.

use crate::chartdx::pane::Container;
use moon_chart::paint::now_unix_ms;
use moon_chart::view::{ChartView, Rect};
use moon_chart::{GLASS_ZONE_PX, PRICE_AXIS_W};
use moon_core::session::CoreId;

/// Mouse button subset used by chart navigation instead of `winit::MouseButton`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Left,
    Right,
}

const WHEEL_THRESHOLD: f32 = 100.0;
/// Pixel distance that doubles or halves X scale for precise macOS trackpad input.
///
/// Precise zoom uses `2^(pixels / WHEEL_PX_PER_2X)` instead of the discrete wheel threshold, which
/// caused inertial pixel deltas to trigger repeated jumps and axis jitter.
const WHEEL_PX_PER_2X: f32 = 300.0;
const ANCHOR_BREAK_PCT: f32 = 0.10;
const RMB_ZOOM_START_PX: f32 = 4.0;

#[derive(Default)]
pub struct ChartInput {
    /// Crosshair position in device pixels, or `None` outside the chart zone.
    pub cursor: Option<(f32, f32)>,
    /// Most recent pointer position in device pixels.
    pub last_ptr: (f32, f32),
    /// Container index of the pane under the cursor.
    pub hovered_pane: Option<usize>,
    /// Pane layout from the previous render, in device pixels, used for input hit testing.
    pub pane_rects: Vec<(usize, Rect)>,
    /// Price-axis position published during render.
    ///
    /// This controls plot width and the left offset used to calculate `cursor_x`; its default is
    /// `Left`, matching the panel default.
    pub price_axis_pos: crate::chart_persist::PriceAxisPos,
    /// Market queued by an eligible chart double-click for the caller to take and open on Main.
    pub pending_to_main: Option<(CoreId, String)>,

    lmb_down: bool,
    lmb_x_active: bool,
    drag_pane: Option<usize>,
    drag_accum: (f32, f32),
    wheel_accum: f32,
    wheel_pane: Option<usize>,
    rmb_down: bool,
    /// Whether right-button movement crossed the price-zoom drag threshold.
    rmb_moved: bool,
    rmb_start_y: f32,
    rmb_start_range: f32,
    rmb_start_center: f32,
    /// Time and position of the previous left-button press for double-click detection.
    last_lmb_ms: f64,
    last_lmb_pos: (f32, f32),
}

impl ChartInput {
    fn plot_metrics_for(&self, pane: Option<usize>, fallback_w: f32, ppp: f32) -> (f32, f32) {
        let Some(idx) = pane else {
            return (
                fallback_w.max(1.0),
                self.last_ptr.0.clamp(0.0, fallback_w.max(1.0)),
            );
        };
        let Some((_, r)) = self.pane_rects.iter().find(|(i, _)| *i == idx) else {
            return (
                fallback_w.max(1.0),
                self.last_ptr.0.clamp(0.0, fallback_w.max(1.0)),
            );
        };
        use crate::chart_persist::PriceAxisPos;
        let price_axis_w = if matches!(self.price_axis_pos, PriceAxisPos::Hide) {
            0.0
        } else {
            PRICE_AXIS_W * ppp
        };
        let glass_w = GLASS_ZONE_PX.min(r.w * 0.5);
        let plot_w = (r.w - price_axis_w - glass_w).max(1.0);
        // Reserve a left offset only for a left-side axis; right-side or hidden axes start at the slot edge.
        let left_off = if matches!(self.price_axis_pos, PriceAxisPos::Left) {
            price_axis_w
        } else {
            0.0
        };
        let cursor_x = (self.last_ptr.0 - r.x - left_off).clamp(0.0, plot_w);
        (plot_w, cursor_x)
    }

    /// Return the hovered pane's mutable view for pan or zoom operations.
    ///
    /// Returns `None` when no pane is hovered or the container has no matching view.
    pub fn hovered_view_mut<'c>(&self, container: &'c mut Container) -> Option<&'c mut ChartView> {
        self.view_mut(container, self.hovered_pane)
    }

    fn view_mut<'c>(
        &self,
        container: &'c mut Container,
        pane: Option<usize>,
    ) -> Option<&'c mut ChartView> {
        let idx = pane?;
        container.view_mut(idx)
    }

    /// Queue the hovered pane's market after a chart-area double-click.
    fn try_dblclick_to_main(&mut self, container: &Container) {
        let Some(idx) = self.hovered_pane else { return };
        let Some((_, r)) = self.pane_rects.iter().find(|(i, _)| *i == idx) else {
            return;
        };
        // Ignore double-clicks in the right-side order-book/glass zone.
        let glass_w = GLASS_ZONE_PX.min(r.w * 0.5);
        if self.last_ptr.0 >= r.x + r.w - glass_w {
            return;
        }
        self.pending_to_main = container.target(idx);
    }

    /// Apply wheel X zoom or X pan to the hovered pane.
    ///
    /// `dy` is a line delta for discrete input or a pixel delta when `precise` is true. `pan`
    /// selects Shift/Alt wheel panning instead of zooming, and `gate_ok` requires the pointer to be
    /// inside the chart zone. Returns whether a view changed and needs presentation.
    pub fn wheel(
        &mut self,
        dy: f32,
        precise: bool,
        pan: bool,
        gate_ok: bool,
        container: &mut Container,
        fallback_w: f32,
        ppp: f32,
    ) -> bool {
        if !gate_ok || dy == 0.0 {
            return false;
        }
        if self.wheel_pane != self.hovered_pane {
            self.wheel_accum = 0.0;
            self.wheel_pane = self.hovered_pane;
        }
        let (plot_w, cursor_x) = self.plot_metrics_for(self.hovered_pane, fallback_w, ppp);
        let now = now_unix_ms();
        if let Some(view) = self.hovered_view_mut(container) {
            if pan {
                // Precise panning follows gesture pixels; a discrete line event uses a fixed
                // 60-device-pixel step in its sign direction.
                let dx = if precise { -dy } else { -dy.signum() * 60.0 };
                view.pan_x_px(dx, now, plot_w);
            } else if precise {
                // Precise macOS trackpad input zooms proportionally to gesture magnitude without
                // threshold accumulation, avoiding repeated jumps from inertial pixel deltas.
                self.wheel_accum = 0.0;
                let factor = 2f32.powf(dy / WHEEL_PX_PER_2X);
                view.zoom_x_at(factor, plot_w, cursor_x, now);
            } else {
                // Accumulate discrete wheel lines and apply a 2x or 0.5x step at the threshold.
                self.wheel_accum += dy * 40.0;
                if self.wheel_accum.abs() < WHEEL_THRESHOLD {
                    return false;
                }
                // Terminal UX: wheel up zooms in, wheel down zooms out.
                let factor = if self.wheel_accum > 0.0 { 2.0 } else { 0.5 };
                self.wheel_accum = 0.0;
                view.zoom_x_at(factor, plot_w, cursor_x, now);
            }
            return true;
        }
        false
    }

    /// Update chart-navigation state for a left or right mouse-button transition.
    ///
    /// `gate_ok` restricts presses to the chart zone but does not block releases, allowing held
    /// state to clear outside that zone. A right-button drag zooms price; a short right click does
    /// not toggle anything in this layer because one `ChartEngine` no longer owns tiled mode. The
    /// caller may still use [`rmb_moved`](Self::rmb_moved) to gate a parent stack toggle.
    /// `allow_dbl_to_main` permits an eligible left double-click to fill `pending_to_main`.
    /// The return value reports a view change; the caller handles the pending market separately.
    pub fn mouse_button(
        &mut self,
        button: Btn,
        pressed: bool,
        gate_ok: bool,
        allow_dbl_to_main: bool,
        container: &mut Container,
        ppp: f32,
        fallback_w: f32,
    ) -> bool {
        let mut changed = false;
        match button {
            Btn::Left => {
                if pressed {
                    if !gate_ok {
                        return false; // Do not start chart dragging from surrounding UI controls.
                    }
                    let now = now_unix_ms();
                    let (px, py) = self.last_ptr;
                    let dbl = now - self.last_lmb_ms < 400.0
                        && (px - self.last_lmb_pos.0).abs() < 28.0
                        && (py - self.last_lmb_pos.1).abs() < 28.0;
                    self.last_lmb_ms = now;
                    self.last_lmb_pos = (px, py);
                    if dbl && allow_dbl_to_main {
                        self.try_dblclick_to_main(container);
                    }
                    self.lmb_down = true;
                    self.lmb_x_active = false;
                    self.drag_pane = self.hovered_pane;
                    self.drag_accum = (0.0, 0.0);
                } else {
                    if self.lmb_down && self.lmb_x_active {
                        let target = self.drag_pane;
                        let (plot_w, _) = self.plot_metrics_for(target, fallback_w, ppp);
                        let now = now_unix_ms();
                        if let Some(view) = self.view_mut(container, target) {
                            changed |= view.snap_to_live_if_near(now, plot_w);
                        }
                    }
                    self.lmb_down = false;
                    self.lmb_x_active = false;
                    if !self.rmb_down {
                        self.drag_pane = None;
                    }
                }
            }
            Btn::Right => {
                if pressed {
                    if !gate_ok {
                        return false;
                    }
                    self.rmb_down = true;
                    self.rmb_moved = false;
                    self.drag_pane = self.hovered_pane;
                    self.rmb_start_y = self.last_ptr.1;
                    let snap = self
                        .view_mut(container, self.drag_pane)
                        .map(|v| (v.render_range, v.render_center));
                    if let Some((r, c)) = snap {
                        self.rmb_start_range = r;
                        self.rmb_start_center = c;
                    }
                } else {
                    self.rmb_down = false;
                    if !self.lmb_down {
                        self.drag_pane = None;
                    }
                }
            }
        }
        changed
    }

    /// Apply the navigation portion of a pointer-drag event and update `last_ptr`.
    ///
    /// Left drag pans X/Y, while right drag vertically zooms price from the press-time range and
    /// center snapshot. Returns whether the event actually changed a view and needs presentation.
    pub fn pointer_drag(
        &mut self,
        x: f32,
        y: f32,
        container: &mut Container,
        ppp: f32,
        fallback_w: f32,
    ) -> bool {
        let dx = x - self.last_ptr.0;
        let dy = y - self.last_ptr.1;
        self.last_ptr = (x, y);
        let mut changed = false;

        // Left drag pans time horizontally and price vertically.
        if self.lmb_down {
            let target = self.drag_pane.or(self.hovered_pane);
            let (plot_w, _) = self.plot_metrics_for(target, fallback_w, ppp);
            self.drag_accum.0 += dx;
            self.drag_accum.1 += dy;
            let now = now_unix_ms();
            if let Some(view) = self.view_mut(container, target) {
                if dy != 0.0 {
                    view.pan_y_px(dy, now);
                    changed = true;
                }
                if !self.lmb_x_active
                    && self.drag_accum.0.abs() >= plot_w * ANCHOR_BREAK_PCT
                    && self.drag_accum.0.abs() >= self.drag_accum.1.abs()
                {
                    self.lmb_x_active = true;
                }
                if self.lmb_x_active && dx != 0.0 {
                    view.pan_x_px(dx, now, plot_w);
                    changed = true;
                }
            }
        }

        // Right drag zooms price vertically from the range and center captured on press.
        if self.rmb_down {
            let cum = y - self.rmb_start_y;
            if cum.abs() > RMB_ZOOM_START_PX {
                self.rmb_moved = true;
            }
            if self.rmb_moved {
                let (c, r) = (self.rmb_start_center, self.rmb_start_range);
                let now = now_unix_ms();
                if let Some(view) = self.view_mut(container, self.drag_pane.or(self.hovered_pane)) {
                    view.rmb_zoom(c, r, cum, now);
                    changed = true;
                }
            }
        }

        changed
    }

    /// Reconcile held-button state from a mouse-move event.
    ///
    /// GPUI delivers `mouse_up` only over the element, so a release outside the slot can leave drag
    /// state stuck. The move event reports the currently pressed button and clears stale state here.
    pub fn sync_pressed(&mut self, left_held: bool, right_held: bool) {
        if !left_held {
            self.lmb_down = false;
            self.lmb_x_active = false;
        }
        if !right_held {
            self.rmb_down = false;
        }
        if !left_held && !right_held {
            self.drag_pane = None;
        }
    }

    /// Return whether right-button movement crossed the price-zoom threshold since its press.
    ///
    /// The parent Main stack uses this to distinguish a zoom drag from the short right click that
    /// exits fullscreen.
    pub fn rmb_moved(&self) -> bool {
        self.rmb_moved
    }

    /// Return the first pane whose cached rectangle contains the device-pixel point.
    pub fn pane_at(&self, x: f32, y: f32) -> Option<usize> {
        self.pane_rects
            .iter()
            .find(|(_, r)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
            .map(|(i, _)| *i)
    }
}
