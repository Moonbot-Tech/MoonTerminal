//! Chart panel axes controls operations.

use super::*;

impl ChartPanel {
    /// Enables book-only broom mode: rendering hides the plot and price axis and expands the book.
    ///
    /// The book-reference sync runs for the same reason [`Self::set_orderbook_enabled`] runs it:
    /// this mode draws the book whether or not that toggle is set, and depth arrives only for a
    /// subscribed market.
    pub fn set_orderbook_only(&mut self, only: bool, cx: &mut Context<Self>) {
        if self.orderbook_only != only {
            self.orderbook_only = only;
            self.view_dirty = true;
            self.sync_orderbook_refs(cx);
            self.drop_order_hover_if_disallowed(cx);
            cx.notify();
        }
    }

    /// Sets this window/tab's price-axis position. Rendering applies the engine flag, which also
    /// affects plot, order-book, and gutter layout and hit-testing.
    pub fn set_price_axis_pos(
        &mut self,
        pos: crate::persistence::chart_persist::PriceAxisPos,
        cx: &mut Context<Self>,
    ) {
        if self.price_axis_pos != pos {
            self.price_axis_pos = pos;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Sets this window/tab's time-axis visibility. Rendering applies the engine flag, and layout
    /// and hit-testing adjust the plot height.
    pub fn set_time_axis_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.time_axis_visible != visible {
            self.time_axis_visible = visible;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Sets order-line label visibility for this window/tab. Rendering applies the engine flag;
    /// only text visibility changes, not layout.
    pub fn set_line_labels(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.line_labels != show {
            self.line_labels = show;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Sets crosshair cursor-readout label visibility, applied through the engine during rendering.
    pub fn set_cursor_labels(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.cursor_labels != show {
            self.cursor_labels = show;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Sets the stack-owned tab broom state used to highlight the anchor's broom button.
    pub fn set_compare_broom_on(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.compare_broom_on != on {
            self.compare_broom_on = on;
            cx.notify();
        }
    }

    /// Returns this panel engine's weak ghost-cursor handle for distribution to stack peers.
    pub fn ghost_cursor_handle(&self) -> crate::chartdx::ChartGhostCursor {
        self.chart.ghost_cursor()
    }

    /// Replaces the comparison-peer list as directed by the stack's `apply_compare`. An empty list
    /// means comparison is inactive and clears this panel's own possibly stale ghost cursor. This
    /// plumbing bypasses the GPUI tree and needs no notification.
    pub fn set_ghost_peers(&mut self, peers: Vec<crate::chartdx::ChartGhostCursor>) {
        if peers.is_empty() && !self.ghost_peers.is_empty() {
            self.chart.clear_ghost_cursor();
        }
        self.ghost_peers = peers;
    }

    /// Returns this panel's latest price, used by the comparison anchor to drive peer deltas.
    pub fn last_price(&self) -> Option<f64> {
        self.chart.last_price()
    }

    /// The panel's market as its own corner caption spells it, or `None` before the catalog
    /// answers. See [`crate::chartdx::ChartEngine::pane_ticker`] for why it is read, not resolved.
    pub fn pane_ticker(&self) -> Option<String> {
        self.chart.pane_ticker()
    }

    /// Starts the accent border flash for a chart that just arrived in a stack slot.
    ///
    /// Deliberately takes no `cx` and issues no notify: the flash is drawn and paced by the chart's
    /// own pass. Repainting the owning stack instead would re-render every chart panel in the tab
    /// ten times a second, which is the cost this exists to avoid.
    pub fn set_arrival_pulse(&mut self, at: Option<std::time::Instant>, accent: u32, hold: bool) {
        self.chart.set_arrival_pulse(at, accent, hold);
    }

    /// Supplies the comparison anchor's latest price for the large broom-mode delta below a peer's
    /// corner caption. The engine schedules a present when the value changes, so no notify is needed.
    pub fn set_compare_ref_price(&mut self, price: Option<f64>) {
        self.chart.set_compare_ref_price(price);
    }

    /// Returns the current Y window as `(center, range)`, used as the anchor window by the stack, or
    /// `None` when no pane exists.
    pub fn y_window(&self) -> Option<(f32, f32)> {
        self.chart.y_window()
    }

    /// Applies or clears the comparison anchor's locked Y window. Rendering reapplies a lock each
    /// frame; clearing it once restores this panel's configured or automatic price scale.
    pub fn set_locked_y(&mut self, window: Option<(f32, f32)>, cx: &mut Context<Self>) {
        if self.locked_y == window {
            return;
        }
        let exiting = window.is_none();
        self.locked_y = window;
        if exiting {
            // Leaving comparison must restore this panel's configured or automatic scale immediately.
            self.chart.reapply_scale(self.scale);
        }
        self.view_dirty = true;
        cx.notify();
    }

    /// Opens or refreshes a market in this numbered AddToChart or Custom panel with the supplied
    /// TTL. Custom callers normally pin their panes after population to disable expiry.
    pub fn add_coin(&mut self, core: CoreId, market: &str, ttl_ms: f64, cx: &mut Context<Self>) {
        // A retained empty slot keeps its panel and takes the next detection here, so the presses
        // that belonged to the previous coin must not chain into a double click that trades this
        // one. Only on a market CHANGE: this is also the path that extends the TTL of the market
        // already shown, and resetting there would swallow the user's own second click.
        if !self.chart.uses_market(core, market) {
            self.click_series.reset();
        }
        self.release_market_refs_except(Some((core, market)), cx);
        self.chart.push_auto(core, market, ttl_ms, now_unix_ms());
        self.retain_market_ref(core, market, cx);
        self.view_dirty = true;
        self.arm_ttl_timer(cx);
        // Notify the panel itself. ChartTabs renders an attached strip tab, but a detached panel
        // lives in another window and would not display the newly detected market without this.
        cx.notify();
    }

    /// Point this panel at another kind's defaults.
    ///
    /// Called by the stack when it is detached, repinned, or when the anchor lock goes on or off.
    /// The effective values are recomputed HERE rather than waited for: the settings signature is
    /// otherwise only rebuilt on a backend notification, and a panel that has just changed kind
    /// would keep drawing the old kind's default until something unrelated woke it.
    pub fn set_default_kind(
        &mut self,
        kind: moon_core::config::ChartTabKind,
        cx: &mut Context<Self>,
    ) {
        if self.default_kind == kind {
            return;
        }
        self.default_kind = kind;
        let settings_sig = {
            let b = self.backend.read(cx);
            chart_settings_sig(
                b,
                self.chart_graphics,
                self.candle_view,
                self.chart_labels.clone(),
                kind,
            )
        };
        if settings_sig == self.settings_sig {
            return;
        }
        self.settings_sig = settings_sig;
        self.view_dirty = true;
        // For the reason the backend observer does it: the durable trade-history query is narrowed
        // by the drawn trade kinds, and the admitted core set follows the kind's stored
        // `history_all_cores`. Both live in the graphics settings this just changed. This method
        // stamps `settings_sig` itself, so the observer branch that would have re-read them does
        // not run.
        self.requery_trade_history_on_trade_kinds(cx);
        self.requery_trade_history_on_core_scope(cx);
        // The style may have flipped to lines: resolve and hand over what is already resolved.
        // The engine's own graphics update happens on render, before its next order pass.
        self.request_trace_lines(true, cx);
        self.sync_trace_lines(true, cx);
        cx.notify();
    }
}
