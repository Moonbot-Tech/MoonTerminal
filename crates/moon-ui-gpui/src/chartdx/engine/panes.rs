//! Pane lifecycle, targets, history invalidation and axis snapshots.

use super::*;

impl ChartEngine {
    /// Opens a market in the full-screen pane.
    pub fn open(&mut self, core: CoreId, market: &str) {
        self.container
            .borrow_mut()
            .open_manual(core, market, self.epoch);
        self.data.borrow_mut().mark_view_dirty();
    }

    /// Opens or extends an AddToChart market in this pane with a TTL.
    pub fn push_auto(&mut self, core: CoreId, market: &str, ttl_ms: f64, now_ms: f64) {
        self.container
            .borrow_mut()
            .push_auto(core, market, now_ms, ttl_ms, self.epoch);
        self.data.borrow_mut().mark_view_dirty();
    }

    /// Removes expired AddToChart panes and returns their markets.
    pub fn prune_ttl(&mut self, now_ms: f64) -> Vec<(CoreId, String)> {
        let removed = self.container.borrow_mut().prune_ttl(now_ms);
        if !removed.is_empty() {
            self.data.borrow_mut().mark_view_dirty();
        }
        removed
    }

    pub fn stalest_detect_ms(&self) -> Option<f64> {
        self.container.borrow().stalest_detect_ms()
    }

    pub fn next_ttl_deadline_ms(&self) -> Option<f64> {
        self.container.borrow().next_ttl_deadline_ms()
    }

    pub fn with_container_mut<R>(&mut self, f: impl FnOnce(&mut Container) -> R) -> R {
        let out = f(&mut self.container.borrow_mut());
        self.data.borrow_mut().mark_view_dirty();
        out
    }

    pub fn with_container<R>(&self, f: impl FnOnce(&Container) -> R) -> R {
        f(&self.container.borrow())
    }

    pub fn remove_pane(&mut self, idx: usize) -> Option<(CoreId, String)> {
        let removed = self.container.borrow_mut().remove_pane(idx);
        if removed.is_some() {
            self.data.borrow_mut().mark_view_dirty();
        }
        removed
    }

    pub fn uses_market(&self, core: CoreId, market: &str) -> bool {
        self.container.borrow().uses_market(core, market)
    }

    /// Returns whether pane `idx` can be pinned; only AddToChart panes with a TTL qualify.
    pub fn pane_is_pinnable(&self, idx: usize) -> bool {
        self.container.borrow().is_pinnable(idx)
    }

    pub fn pane_pinned(&self, idx: usize) -> bool {
        self.container.borrow().is_pinned(idx)
    }

    /// Toggles pinning for pane `idx`, disabling or restoring TTL auto-close. Returns true on change.
    pub fn toggle_pane_pin(&mut self, idx: usize) -> bool {
        let changed = self.container.borrow_mut().toggle_pin(idx).is_some();
        if changed {
            self.data.borrow_mut().mark_view_dirty();
        }
        changed
    }

    pub fn clear_panes(&mut self) -> Vec<(CoreId, String)> {
        let removed = self.container.borrow_mut().clear_panes();
        if !removed.is_empty() {
            self.data.borrow_mut().mark_view_dirty();
        }
        removed
    }

    /// Returns the active full-screen or first pane's core and market.
    pub fn active_target(&self) -> Option<(CoreId, String)> {
        let container = self.container.borrow();
        container.pane(0).map(|p| (p.core, p.market.clone()))
    }

    /// Returns a pane's core and market by index for chart overlay actions such as Panic Sell and
    /// Cancel Buy that are bound to a specific slot.
    pub fn pane_target(&self, idx: usize) -> Option<(CoreId, String)> {
        self.container
            .borrow()
            .pane(idx)
            .map(|p| (p.core, p.market.clone()))
    }

    /// The `market_currency` a pane resolved for its market, or `None` while the catalogue has not
    /// named it yet.
    ///
    /// The identity the CORE's own lists are matched against — see `PaneRender::coin`. Read from
    /// the pane rather than resolved by the caller: the label was taken once, when the market was
    /// assigned, and taking the market-source lock again per pane per frame is what caching it
    /// avoided.
    ///
    /// Args:
    ///     idx: Pane index, as `pane_target` reports them.
    ///
    /// Returns:
    ///     The coin, or `None` for a pane with no market or an unresolved catalogue.
    pub fn pane_coin(&self, idx: usize) -> Option<String> {
        let data = self.data.borrow();
        let render = data.render.borrow();
        let coin = render.panes.get(idx)?.coin.clone();
        (!coin.is_empty()).then_some(coin)
    }

    /// Returns the active full-screen or first pane's market for the tab label.
    pub fn active_market(&self) -> Option<String> {
        self.active_target().map(|(_, market)| market)
    }

    /// Force the next prepare to rebuild resident GPU history from MoonProto.
    /// Does not change the visible time window: viewport scale and retained
    /// data capacity are independent.
    pub fn force_history_reupload(&mut self) {
        let mut st = self.state.borrow_mut();
        for pr in &mut st.panes {
            pr.history_cursor.reset();
            pr.resident_left_rel = f32::NAN;
            pr.pan_reset_cam_px = i64::MIN;
            pr.cached_tick_price = None;
            pr.price_scan_window = None;
            pr.gpu_prepare_dirty = true;
        }
        st.needs_present = true;
        st.base_dirty = true;
        self.data.borrow_mut().mark_view_dirty();
    }

    /// Invalidate retained axis and readout text after the selected display zone changes.
    ///
    /// Returns:
    ///     Nothing; the next frame rebuilds display-time text from retained data.
    pub fn invalidate_display_time(&mut self) {
        self.data.borrow_mut().mark_view_dirty();
    }

    pub fn pane_count(&self) -> usize {
        self.container.borrow().pane_count()
    }

    /// Return axis snapshots for visible panes after preparation.
    ///
    /// Returns:
    ///     Visible panes as `(index, device-pixel rectangle, snapshot)` tuples.
    pub fn axis_panes(&self) -> Vec<(usize, Rect, AxisSnapshot)> {
        let container = self.container.borrow();
        container
            .layout({
                let data = self.data.borrow();
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: data.w.max(1) as f32,
                    h: data.h.max(1) as f32,
                }
            })
            .into_iter()
            .filter_map(|(idx, rect)| {
                let v = &container.pane(idx)?.view;
                Some((
                    idx,
                    rect,
                    AxisSnapshot {
                        px_per_ms: v.px_per_ms,
                        right_margin_frac: v.right_margin_frac,
                        render_center: v.render_center,
                        render_range: v.render_range,
                        epoch_ms: v.epoch_ms,
                        right_time_ms: v.right_time_ms,
                    },
                ))
            })
            .collect()
    }
}
