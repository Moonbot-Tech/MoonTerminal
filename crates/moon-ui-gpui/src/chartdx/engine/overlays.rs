//! Order, figure, news, trade-history and frozen-view overlays.

use super::*;

impl ChartEngine {
    pub fn set_orders(&mut self, orders: OrdersStyle) -> bool {
        if self.orders != orders {
            self.orders = orders;
            let mut data = self.data.borrow_mut();
            data.orders = self.orders.clone();
            data.mark_view_dirty();
            drop(data);
            for pr in &mut self.state.borrow_mut().panes {
                pr.last_order_lines_rev = u64::MAX;
            }
            true
        } else {
            false
        }
    }

    /// Applies the app-wide take-profit switch (`OrdersStyleSet::draw_take_profit`, #1409).
    ///
    /// Invalidates every pane's order geometry the way a style change does, because the line
    /// appears or goes with no change to the order store. Returns whether it changed.
    pub fn set_draw_take_profit(&mut self, draw: bool) -> bool {
        let mut data = self.data.borrow_mut();
        if data.draw_take_profit == draw {
            return false;
        }
        data.draw_take_profit = draw;
        data.mark_view_dirty();
        drop(data);
        for pr in &mut self.state.borrow_mut().panes {
            pr.last_order_lines_rev = u64::MAX;
        }
        true
    }

    /// The take-profit switch this chart last drew under, for the hit test: a line the chart does
    /// not draw must not be grabbable either (`moon_chart::order_geometry::line_kind_drawn`).
    pub fn draws_take_profit(&self) -> bool {
        self.data.borrow().draw_take_profit
    }

    pub fn set_order_visual(
        &mut self,
        highlight: Option<(CoreId, u64)>,
        drag_preview: Option<(CoreId, u64, LineKind, f32)>,
    ) -> bool {
        self.data
            .borrow_mut()
            .set_order_visual(highlight, drag_preview)
    }

    /// Attaches the backend's shared user-figure store when the panel is created.
    pub fn set_figures_store(
        &mut self,
        store: std::rc::Rc<RefCell<moon_core::figures::FigureStore>>,
    ) {
        self.data.borrow_mut().set_figures_store(store);
    }

    /// Sets this panel's figure preview, hover, and selection state.
    pub(crate) fn set_figure_visual(&mut self, visual: super::figures_sync::FigureVisual) -> bool {
        self.data.borrow_mut().set_figure_visual(visual)
    }

    /// Publishes one pane target's auto figures, or removes them with `None`; see
    /// `auto_figures_sync`. The caller forces the userdata rebuild afterwards.
    pub(crate) fn set_auto_figures(
        &mut self,
        core: CoreId,
        market: &str,
        set: Option<std::rc::Rc<super::auto_figures_sync::AutoPaneFigures>>,
    ) -> bool {
        self.data.borrow_mut().set_auto_figures(core, market, set)
    }

    /// The drawn auto line nearest `pos` on `pane`, within `threshold` pixels.
    ///
    /// Reads what the pane last DREW, so the hit matches the picture on screen. Auto lines are
    /// found here only — never through the figure store — which is what keeps them out of
    /// selection and dragging.
    pub(crate) fn auto_line_at(
        &self,
        pane: usize,
        pos: moon_core::figures::proj::PxPoint,
        proj: &dyn moon_core::figures::Proj,
        threshold: f32,
    ) -> Option<super::auto_figures_sync::LineKey> {
        self.state
            .borrow()
            .panes
            .get(pane)?
            .auto
            .line_at(pos, proj, threshold)
    }

    /// Read the drawn auto line `key` of `pane`.
    pub(crate) fn with_auto_line<R>(
        &self,
        pane: usize,
        key: super::auto_figures_sync::LineKey,
        read: impl FnOnce(&super::auto_figures_sync::pane::AutoLineRec) -> R,
    ) -> Option<R> {
        self.state
            .borrow()
            .panes
            .get(pane)?
            .auto
            .line(key)
            .map(read)
    }

    /// Read every auto line `pane` last DREW, in drawing order.
    ///
    /// For tooling that has to point at a line it did not hover first (the private filming
    /// overlay); the hover path keeps asking [`Self::auto_line_at`], which answers for one point.
    /// `None` when the pane does not exist. Compiled only beside that overlay, its one caller.
    #[cfg(uidoc)]
    pub(crate) fn with_auto_lines<R>(
        &self,
        pane: usize,
        read: impl FnOnce(&[super::auto_figures_sync::pane::AutoLineRec]) -> R,
    ) -> Option<R> {
        self.state
            .borrow()
            .panes
            .get(pane)
            .map(|pr| read(&pr.auto.lines))
    }

    /// The figure-tool values the auto line `key` of `pane` is drawn as — what "pin as figure"
    /// copies into the store.
    pub(crate) fn auto_line_figures(
        &self,
        pane: usize,
        key: super::auto_figures_sync::LineKey,
    ) -> Vec<moon_core::figures::Figure> {
        self.state
            .borrow()
            .panes
            .get(pane)
            .map(|pr| pr.auto.line_figures(key))
            .unwrap_or_default()
    }

    /// Set the auto line under the pointer. Returns whether it changed; a change asks for a
    /// present, so an `OnHover` caption appears or goes, and rebuilds nothing.
    pub(crate) fn set_auto_hover(
        &mut self,
        hover: Option<(usize, super::auto_figures_sync::LineKey)>,
    ) -> bool {
        let mut st = self.state.borrow_mut();
        if st.auto_hover == hover {
            return false;
        }
        st.auto_hover = hover;
        st.needs_present = true;
        true
    }

    /// What pane `idx` currently shows, for evaluating its auto-figure keys without copying
    /// candles. `None` until the pane is stamped with that target.
    pub(crate) fn auto_pane_view(
        &self,
        idx: usize,
        core: CoreId,
        market: &str,
    ) -> Option<super::auto_figures_sync::AutoPaneView> {
        self.data.borrow().auto_pane_view(idx, core, market)
    }

    /// Pane `idx`'s closed candles of the chart's own timeframe — the snapshot a detector job
    /// takes with it.
    pub(crate) fn auto_pane_candles(
        &self,
        idx: usize,
        core: CoreId,
        market: &str,
        now_ms: f64,
    ) -> Vec<moon_core::market::ChartCandle> {
        self.data
            .borrow()
            .auto_pane_candles(idx, core, market, now_ms)
    }

    /// Every published auto-figure row of every pane, as `(row index, outcome)` — what the
    /// auto-figures popup reads to mark a row whose last run had too little history, lagged or
    /// found no cache. Read-only and cheap: one `Arc` clone per drawn row, only while that popup
    /// is open.
    pub(crate) fn auto_row_outcomes(
        &self,
    ) -> Vec<(
        usize,
        std::sync::Arc<super::auto_figures_sync::job::AutoRowOutcome>,
    )> {
        self.data
            .borrow()
            .auto_figures
            .iter()
            .flat_map(|(_, set)| set.rows.iter().map(|r| (r.row_ix, r.outcome.clone())))
            .collect()
    }

    /// Sets this panel's news marks and the mark under the cursor. Returns whether anything changed,
    /// so the caller can skip the userdata resync.
    pub(crate) fn set_news_marks(
        &mut self,
        marks: std::rc::Rc<Vec<moon_chart::news_marks::NewsMark>>,
        hovered: Option<usize>,
    ) -> bool {
        self.data.borrow_mut().set_news_marks(marks, hovered)
    }

    /// Sets this panel's warning badges and the one under the cursor. Returns whether anything
    /// changed, so the caller can skip the userdata resync.
    pub(crate) fn set_warn_marks(
        &mut self,
        marks: std::rc::Rc<Vec<moon_chart::news_marks::NewsMark>>,
        hovered: Option<usize>,
    ) -> bool {
        self.data.borrow_mut().set_warn_marks(marks, hovered)
    }

    /// Replace durable closed-trade markers for this exact chart target.
    ///
    /// Args:
    ///     records: Exact-core durable records to compose into userdata.
    ///
    /// Returns:
    ///     Whether the record set changed.
    pub(crate) fn set_trade_history(
        &mut self,
        records: std::rc::Rc<Vec<moon_core::db::ChartTradeRecord>>,
    ) -> bool {
        self.data.borrow_mut().set_trade_history(records)
    }

    /// Replace the admitted core set those markers may widen to.
    ///
    /// Args:
    ///     cores: The panel's admitted set, or `None` for own-core only.
    ///
    /// Returns:
    ///     Whether the set changed.
    pub(crate) fn set_trade_history_cores(
        &mut self,
        cores: Option<std::rc::Rc<super::trade_history_sync::TradeHistoryCores>>,
    ) -> bool {
        self.data.borrow_mut().set_trade_history_cores(cores)
    }

    /// Hand this engine the archived lines of its closed trades, for the "Moonbot lines" style.
    ///
    /// Args:
    ///     lines: Lines by `ReportUID`, as the panel collected them from the trace resolver.
    ///
    /// Returns:
    ///     Whether the map changed.
    pub(crate) fn set_archived_lines(
        &mut self,
        lines: std::rc::Rc<
            std::collections::HashMap<i64, std::sync::Arc<[moon_core::feed::ArchivedOrderTrace]>>,
        >,
    ) -> bool {
        self.data.borrow_mut().set_archived_lines(lines)
    }

    /// The closed trades this engine wants archived lines for, nearest a pane's right edge first.
    /// See `archived_lines`; the caller decides whether the style calls for asking.
    ///
    /// Args:
    ///     cap: Most uids to name.
    pub(crate) fn wanted_trace_uids(&self, cap: usize) -> Vec<i64> {
        self.data.borrow().wanted_trace_uids(cap)
    }

    /// The durable closed-trade history this engine draws, as the panel published it.
    pub(crate) fn trade_history(&self) -> std::rc::Rc<Vec<moon_core::db::ChartTradeRecord>> {
        self.data.borrow().trade_history.clone()
    }

    /// Draw a frozen trade replay instead of the live market source.
    ///
    /// Only the trade window calls this, on the engine it owns. Every other engine leaves the
    /// field `None` and keeps the live path it always had.
    ///
    /// Args:
    ///     series: The frozen series, or `None` to return to the live source.
    pub(crate) fn set_trade_replay(
        &mut self,
        series: Option<std::rc::Rc<moon_core::market::trade_replay::TradeReplaySeries>>,
    ) {
        self.data.borrow_mut().set_trade_replay(series);
    }

    /// Lay the price file's recorded Last and Mark lines over the replay's.
    ///
    /// Only the trade window calls this, on the engine it owns, beside `set_trade_replay`.
    ///
    /// Args:
    ///     lines: The recorded lines, or `None` for the provider's rings alone.
    pub(crate) fn set_stored_prices(
        &mut self,
        lines: Option<std::rc::Rc<moon_core::market::price_recorder::StoredPriceLines>>,
    ) {
        self.data.borrow_mut().set_stored_prices(lines);
    }

    /// Draw a closed trade's archived order lines instead of the live order store.
    ///
    /// Only the trade window calls this, on the engine it owns; see `ChartDataState::frozen_orders`.
    ///
    /// Args:
    ///     store: The archived lines, or `None` to draw none.
    pub(crate) fn set_frozen_orders(
        &mut self,
        store: Option<std::rc::Rc<moon_core::session::order_lines::OrderLineStore>>,
        fit_range: Option<(f32, f32)>,
    ) {
        self.data.borrow_mut().set_frozen_orders(store, fit_range);
    }

    /// Draw a frozen viewer's corridor and modelled trades beside its archived lines.
    ///
    /// Only the trade window calls this, on the engine it owns; see
    /// `ChartDataState::frozen_overlay`.
    ///
    /// Args:
    ///     overlay: What to draw, or `None` for nothing.
    pub(crate) fn set_frozen_overlay(
        &mut self,
        overlay: Option<std::rc::Rc<moon_chart::frozen_overlay::FrozenOverlay>>,
    ) {
        self.data.borrow_mut().set_frozen_overlay(overlay);
    }

    /// Hand this engine the closed trade its captions describe, or take it away.
    ///
    /// Only the trade window calls this, on the engine it owns — the same boundary
    /// [`Self::set_trade_replay`] has, and for the same reason: a live chart has no trade to
    /// describe, so its captions must have nothing to print.
    ///
    /// Args:
    ///     labels: The trade's already-resolved strings, or `None` for a chart describing none.
    ///
    /// Returns:
    ///     Whether anything changed, so the caller can skip the repaint.
    pub(crate) fn set_trade_labels(&mut self, labels: Option<std::rc::Rc<TradeLabels>>) -> bool {
        let mut data = self.data.borrow_mut();
        // By VALUE, not by pointer: the window rebuilds this handle when its trade resolves and
        // hands the same content on every render afterwards, so a pointer test would report a
        // change on each one. `Rc`'s own `PartialEq` takes the pointer shortcut when it can.
        let unchanged = data.trade_labels == labels;
        // Adopted even when unchanged, exactly as `set_chart_labels` adopts an equal handle: the
        // mirror the text pass reads must never hold an older allocation than the data state.
        data.render.borrow_mut().trade_labels = labels.clone();
        data.trade_labels = labels;
        if unchanged {
            return false;
        }
        // The captions are re-resolved by the sync paths, which short-circuit on an unchanged
        // signature — the same invalidation `set_chart_labels` performs, and for the same reason:
        // this window's market and orders never move, so nothing else would wake them.
        data.last_order_sig = u64::MAX;
        data.mark_view_dirty();
        true
    }

    /// Set the trade arrow under the cursor, which draws grown and fully opaque.
    ///
    /// Args:
    ///     hovered: Pane index, mark index within that pane, and whether that end BUYS; `None` for
    ///         no hover.
    ///
    /// Returns:
    ///     Whether anything changed, so the caller can skip the userdata resync.
    pub(crate) fn set_trade_hover(
        &mut self,
        hovered: Option<(usize, usize, bool)>,
    ) -> super::trade_history_sync::TradeHoverChange {
        self.data.borrow_mut().set_trade_hover(hovered)
    }

    /// Read the durable closed trades currently published to this chart.
    ///
    /// The panel does not keep its own copy — it publishes the set here and reads it back — so this
    /// is what a cluster's member indices resolve against.
    ///
    /// Args:
    ///     read: Receives the published records in their published order.
    ///
    /// Returns:
    ///     The closure's value.
    pub(crate) fn with_trade_records<R>(
        &self,
        read: impl FnOnce(&[moon_core::db::ChartTradeRecord]) -> R,
    ) -> R {
        read(&self.data.borrow().trade_history)
    }

    /// The admitted core set published with the durable history, if the panel handed one.
    ///
    /// `None` means own-core only. The `Rc` is cloned; the set is not.
    ///
    /// Returns:
    ///     The published set, or `None` when the panel never handed one.
    pub(crate) fn trade_history_cores(
        &self,
    ) -> Option<std::rc::Rc<super::trade_history_sync::TradeHistoryCores>> {
        self.data.borrow().trade_history_cores.clone()
    }

    /// Read the report axis this engine's closed-trade stamps are currently corrected on.
    ///
    /// Handed to a closure rather than a returning clone, for the same reason
    /// [`Self::with_trade_geometry`] is one: the hover card is rebuilt on every frame the pointer
    /// rests on an arrow, and a `HashMap` clone per frame is avoidable work.
    ///
    /// Args:
    ///     read: Receives the current report axis.
    ///
    /// Returns:
    ///     The closure's value.
    pub(crate) fn with_report_axis<R>(
        &self,
        read: impl FnOnce(&moon_core::db::ReportAxis) -> R,
    ) -> R {
        read(&self.data.borrow().report_axis)
    }

    /// Read one pane's retained trade-arrow geometry.
    ///
    /// Handed to a closure rather than returned, because it holds the clusters and their member
    /// lists: cloning it on every pointer move — which is what a hit test does — would allocate a
    /// vector per cluster for a read that touches only their positions.
    ///
    /// Args:
    ///     pane: Pane index to read.
    ///     read: Receives the pane's retained geometry.
    ///
    /// Returns:
    ///     The closure's value, or `None` when that pane does not exist.
    /// Mark one pane's trade geometry stale so the next unforced order sync rebuilds only it.
    ///
    /// Args:
    ///     pane: Pane index whose built span no longer covers the cursor.
    pub(crate) fn invalidate_trade_pane(&mut self, pane: usize) {
        self.data.borrow_mut().dirty_trade_pane(pane);
    }

    pub(crate) fn with_trade_geometry<R>(
        &self,
        pane: usize,
        read: impl FnOnce(&super::trade_history_sync::TradeGeometry) -> R,
    ) -> Option<R> {
        self.state
            .borrow()
            .panes
            .get(pane)
            .map(|pr| read(&pr.trade_geometry))
    }
}
