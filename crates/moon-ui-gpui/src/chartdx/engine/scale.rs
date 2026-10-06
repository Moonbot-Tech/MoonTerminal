//! Shared price and time scales and market graphics settings.

use super::*;

impl ChartEngine {
    /// Applies a price-axis scale to every pane and stores it in the container. None selects Auto.
    pub fn set_scale(&mut self, pct: Option<f32>) -> bool {
        if self.scale == pct {
            return false;
        }
        self.scale = pct;
        self.container.borrow_mut().set_scale(pct);
        self.data.borrow_mut().mark_view_dirty();
        true
    }

    /// Returns the first pane's current Y window `(center, range)` as the comparison-mode anchor.
    pub fn y_window(&self) -> Option<(f32, f32)> {
        self.container
            .borrow()
            .panes()
            .first()
            .map(|p| p.view.y_window())
    }

    /// Forces the anchor-locked comparison Y window onto every engine pane. Returns true on change.
    pub fn set_locked_y(&mut self, center: f32, range: f32) -> bool {
        let mut changed = false;
        for p in self.container.borrow_mut().panes_mut() {
            changed |= p.view.set_y_window(center, range);
        }
        if changed {
            self.data.borrow_mut().mark_view_dirty();
        }
        changed
    }

    /// Reapplies the tab scale after leaving comparison lock, bypassing the unchanged `self.scale`
    /// cache that would make a normal `set_scale` call a no-op. None selects Auto.
    pub fn reapply_scale(&mut self, pct: Option<f32>) {
        self.scale = pct;
        self.container.borrow_mut().set_scale(pct);
        self.data.borrow_mut().mark_view_dirty();
    }

    /// Enables or disables the per-window order book for every engine pane. Returns true on change.
    pub fn set_orderbook_enabled(&mut self, enabled: bool) -> bool {
        let mut data = self.data.borrow_mut();
        if data.orderbook_enabled == enabled {
            return false;
        }
        data.orderbook_enabled = enabled;
        data.mark_view_dirty();
        true
    }

    /// Stores the X scale for new panes in pixels per millisecond. None uses the built-in default.
    pub fn set_default_x_ppm(&mut self, ppm: Option<f32>) {
        self.data.borrow_mut().default_x_ppm = ppm;
    }

    /// Returns the X scale in pixels per millisecond for pane `idx`, or the first pane as fallback.
    pub fn pane_x_ppm(&self, idx: Option<usize>) -> Option<f32> {
        let container = self.container.borrow();
        let panes = container.panes();
        let pane = idx.and_then(|i| panes.get(i)).or_else(|| panes.first())?;
        Some(pane.view.px_per_ms)
    }

    /// Forces the X scale onto every engine pane for Shift+middle-click synchronization. Returns
    /// true on change.
    pub fn set_x_ppm_all(&mut self, ppm: f32, now_ms: f64) -> bool {
        let mut changed = false;
        for p in self.container.borrow_mut().panes_mut() {
            changed |= p.view.set_px_per_ms_sync(ppm, now_ms);
        }
        if changed {
            self.data.borrow_mut().mark_view_dirty();
        }
        changed
    }

    /// Applies candle and trade display settings — timeframe, mode, trade zone, outline, the two
    /// price lines and the MoonShot corridor — to every engine pane. Returns true on change and
    /// forces history resynchronization.
    ///
    /// The corridor flag feeds ORDER geometry rather than history, and BOTH rebuild gates are blind
    /// to it: the outer order signature short-circuits on an otherwise idle chart (hence
    /// `last_order_sig`, as in `set_chart_graphics`), and the per-pane gate in
    /// `sync_orders_from_session` folds no candle input either (hence `last_order_lines_rev`, as in
    /// `set_orders`). Clearing only the outer one would stamp the signature and leave the stale
    /// corridor on the pane. The panel's `view_dirty` forces the same resync today; these two keep
    /// the flag correct for a caller that does not travel that path — on a pane that HAS order
    /// data, which is the only pane that draws a corridor to begin with.
    pub fn set_candle_view(&mut self, cfg: moon_core::market::CandleViewCfg) -> bool {
        let mut data = self.data.borrow_mut();
        if data.candle_view == cfg {
            return false;
        }
        data.candle_view = cfg;
        data.last_order_sig = u64::MAX;
        // Mirrored into the text pass for the same reason `chart_labels` is: a countdown caption
        // set to `Авто` counts down to THIS timeframe, and the caption pass is handed what it
        // prints rather than reaching back into the data state for it.
        data.render.borrow_mut().chart_tf_ms = cfg.tf_ms();
        data.mark_view_dirty();
        drop(data);
        for pr in &mut self.state.borrow_mut().panes {
            pr.last_order_lines_rev = u64::MAX;
        }
        true
    }

    /// Applies the chart graphics settings — trade-history arrow size, connector thickness,
    /// trade-kind visibility, the trade-mark size and the bottom volume band — to every engine pane.
    /// Returns true on change.
    ///
    /// The value is the owning panel's: its own per-tab override, or the `layout.chart_graphics`
    /// default when it has none.
    ///
    /// The order and trade-history geometry is baked in the userdata layer, so a change has to
    /// invalidate it: `mark_view_dirty` drives the panel's forced resynchronization, and
    /// `last_order_sig` is reset for the same reason `set_last_ppp` resets it — the outer order
    /// signature short-circuits the per-surface ones on an otherwise idle chart.
    pub fn set_chart_graphics(&mut self, cfg: moon_core::config::ChartGraphicsCfg) -> bool {
        // NORMALIZED before it is stored: a hand-edited `nan` would make the equality guard below
        // report a change on every frame. See `normalize_chart_graphics`.
        let cfg = moon_chart::normalize_chart_graphics(cfg);
        let mut data = self.data.borrow_mut();
        if data.chart_graphics == cfg {
            return false;
        }
        data.chart_graphics = cfg;
        data.last_order_sig = u64::MAX;
        data.mark_view_dirty();
        true
    }

    /// Apply the report axis that corrects this engine's closed-trade stamps on every pane.
    ///
    /// Args:
    ///     axis: Current per-core report-time offsets and display zone from the backend.
    ///
    /// Returns:
    ///     `true` when the axis changed and trade-history geometry was invalidated.
    pub fn set_report_axis(&mut self, axis: moon_core::db::ReportAxis) -> bool {
        self.data.borrow_mut().set_report_axis(axis)
    }
}
