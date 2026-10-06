//! Chart panel attach operations.

use super::*;

impl ChartPanel {
    /// Main stack mode scrolls the list of chart tiles. The chart itself must not consume wheel
    /// events there, otherwise the outer ScrollBox cannot move.
    pub fn set_main_stack_scroll(&mut self, enabled: bool) {
        self.main_stack_scroll = enabled;
    }

    /// Draw a frozen trade replay on this panel instead of the live market source.
    ///
    /// Used only by the trade-detail window, which owns its panel outright. Every other panel
    /// leaves the engine's replay slot empty and keeps the live path unchanged.
    ///
    /// Args:
    ///     series: The frozen series, or `None` to return this panel to the live source.
    ///     cx: Panel context.
    pub(crate) fn attach_trade_replay(
        &mut self,
        series: Option<std::rc::Rc<moon_core::market::trade_replay::TradeReplaySeries>>,
        cx: &mut Context<Self>,
    ) {
        self.chart.set_trade_replay(series);
        self.view_dirty = true;
        cx.notify();
    }

    /// Hand this panel the archived order lines of the trade it shows, or take them away.
    ///
    /// The window's third publication beside the replay and the trade history, reaching the engine
    /// this panel owns and nothing else, for the reason the other two do.
    ///
    /// Args:
    ///     store: The lines, already built by `OrderLineStore::archived`, or `None` for no orders.
    ///     fit_range: The price band the auto-Y fit must include beside the visible prices, or
    ///         `None` to fit the prices alone.
    ///     cx: Panel context.
    pub(crate) fn attach_frozen_orders(
        &mut self,
        store: Option<std::rc::Rc<moon_core::session::order_lines::OrderLineStore>>,
        fit_range: Option<(f32, f32)>,
        cx: &mut Context<Self>,
    ) {
        self.chart.set_frozen_orders(store, fit_range);
        self.view_dirty = true;
        cx.notify();
    }

    /// Hand this panel what its frozen picture draws beside the archived lines — the entry
    /// corridor, modelled trades — or take it away.
    ///
    /// Args:
    ///     overlay: What to draw, or `None` for nothing.
    ///     cx: Panel context.
    pub(crate) fn attach_frozen_overlay(
        &mut self,
        overlay: Option<std::rc::Rc<moon_chart::frozen_overlay::FrozenOverlay>>,
        cx: &mut Context<Self>,
    ) {
        self.chart.set_frozen_overlay(overlay);
        self.view_dirty = true;
        cx.notify();
    }

    /// Hand this panel the archived lines its frozen store was built from, by `ReportUID`.
    ///
    /// The trade window's companion to [`Self::attach_frozen_orders`]: the store is what the
    /// order pass DRAWS, this map is what the arrows pass ASKS — which end of a trade has a line,
    /// so the arrow of that end is not drawn on top of it (`archived_line_ends`). A live chart
    /// fills the same map from the resolver in `trace_lines`; a historical viewer never runs that
    /// path and has to be handed it.
    ///
    /// Args:
    ///     lines: The resolved lines by `ReportUID`, subject and drawn neighbours alike.
    ///     cx: Panel context.
    pub(crate) fn attach_archived_lines(
        &mut self,
        lines: std::rc::Rc<
            std::collections::HashMap<i64, std::sync::Arc<[moon_core::feed::ArchivedOrderTrace]>>,
        >,
        cx: &mut Context<Self>,
    ) {
        if self.chart.set_archived_lines(lines) {
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Hand this panel the closed trade its captions describe.
    ///
    /// The window's second publication, beside the replay and the trade history: those two put the
    /// PICTURE on the chart, this one puts the trade's own facts into the captions beside it. Like
    /// them it reaches the engine this panel owns and nothing else, so a live chart is structurally
    /// incapable of printing a trade it was never handed.
    ///
    /// Args:
    ///     labels: The trade's already-resolved strings, or `None` to describe no trade.
    ///     cx: Panel context.
    pub(crate) fn attach_trade_labels(
        &mut self,
        labels: Option<std::rc::Rc<crate::chartdx::TradeLabels>>,
        cx: &mut Context<Self>,
    ) {
        if !self.chart.set_trade_labels(labels) {
            return;
        }
        self.view_dirty = true;
        cx.notify();
    }

    /// Place the viewport on an absolute millisecond interval.
    ///
    /// The same primitive the Report's main-chart focus already uses; exposed so the trade window
    /// can frame the interval its rows actually cover without reaching into the engine itself.
    ///
    /// Args:
    ///     from_ms: Interval start in Unix milliseconds.
    ///     to_ms: Interval end in Unix milliseconds.
    ///     padding_fraction: Breathing room added on each side. Zero when the caller has already
    ///         built its own context into the interval, which is what the trade window does -
    ///         adding more here would push the frame past the edges its data was fetched for.
    pub(crate) fn show_time_range(&mut self, from_ms: i64, to_ms: i64, padding_fraction: f32) {
        self.chart
            .show_time_range(from_ms as f64, to_ms as f64, padding_fraction);
        self.view_dirty = true;
    }

    /// Release every live market reference this panel took, without dropping the panel.
    ///
    /// The trade window calls this when it closes: a frozen viewer must not leave the application
    /// subscribed to a market it opened only to look at the past.
    ///
    /// Args:
    ///     cx: Application context.
    pub(crate) fn release_market_refs(&mut self, cx: &mut App) {
        self.release_all_market_refs(cx);
    }

    pub(crate) fn sync_orders_if_visible(&mut self, cx: &mut Context<Self>, force: bool) {
        if !self.scene_visible {
            return;
        }
        {
            let b = self.backend.read(cx);
            self.data_sig = self.chart.notify_signature(&b.session);
            self.chart.sync_orders_if_visible(&b.session, force);
        }
        // The view may have moved: the trades nearest the right edge are a different set now.
        // Rate-limited inside, so a pan costs one ranking every few hundred milliseconds at most.
        self.request_trace_lines(false, cx);
        if self.clear_settled_order_drag_preview(cx) && self.apply_order_visual(cx) {
            cx.notify();
        }
    }
}
