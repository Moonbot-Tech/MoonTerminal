//! Main chart tab: each market owns a separate `ChartPanel`/`gpu_canvas`; the active chart is
//! fullscreen, and right-clicking its chart plot expands the whole stack. The shared stack
//! renderer lives in [`super::stack`].

use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::{
    MoonPalette, MoonRect, MoonTabItem, MoonTabStrip, MoonVirtualListScrollHandle, v_flex,
};

use super::stack::{
    ChartStackEntry, apply_setting, chart_stack_card, compare_role, render_chart_stack,
    resolve_layout, retain_nonempty_panels, set_panels_action_btn_pos, set_panels_auto_pin,
    set_panels_candle_view, set_panels_cursor_labels, set_panels_line_labels,
    set_panels_liquidations, set_panels_orderbook_enabled, set_panels_price_axis_pos,
    set_panels_scale, set_panels_show_zone, set_panels_time_axis_visible, sync_compare,
    tile_gutter,
};
use crate::Backend;
use crate::panels::ChartPanel;
use crate::persistence::chart_persist::{
    ChartBtnPos, PriceAxisPos, StackLayoutMode, StackOrientation,
};
use moon_core::config::ChartTheme;
use moon_core::session::CoreId;

/// Main tab where each market owns a separate `ChartPanel`/`gpu_canvas`.
/// A regular market click in a table opens or focuses it fullscreen. Right-clicking the current
/// chart plot toggles between fullscreen and the whole stack; the order-book/control zone retains
/// its trading actions. Multiple markets never return to one `ChartEngine`.
pub(crate) struct MainChartStack {
    backend: Entity<Backend>,
    group: String,
    epoch: f64,
    theme: ChartTheme,
    charts: Vec<ChartStackEntry>,
    active: Option<usize>,
    show_stack: bool,
    scale: Option<f32>,
    /// Per-tab layout mode (`Fit`/`Scroll`); `None` uses the default `Fit` mode.
    layout_mode: Option<StackLayoutMode>,
    /// Slot height in `Fit` mode: zero stretches, while values of at least 20 compress.
    layout_height_fit: Option<u16>,
    /// Slot height in `Scroll` mode; `None` uses the default.
    layout_height_scroll: Option<u16>,
    /// Per-window order-book visibility for this tab; `None` defaults to enabled.
    orderbook_enabled: Option<bool>,
    /// Per-window liquidation-trade visibility; `None` defaults to enabled.
    liquidations_enabled: Option<bool>,
    /// Candle and trade display settings for the tab; `None` uses the global default.
    candle_view: Option<moon_core::market::CandleViewCfg>,
    /// Window X scale in px/ms, synchronized with Shift+middle-click; `None` uses the built-in default.
    /// New charts inherit it, and synchronization applies it to every chart.
    x_ppm: Option<f32>,
    /// Per-window control-zone fill visibility; `None` defaults to enabled.
    show_zone: Option<bool>,
    /// Per-window automatic chart pinning when an order is placed; `None` defaults to disabled.
    auto_pin: Option<bool>,
    /// Per-window stack orientation; `None` defaults to `Vertical`.
    layout_orientation: Option<StackOrientation>,
    /// Per-window positions of the Cancel Buy and Panic Sell buttons; `None` defaults to `Right`.
    cancel_buy_pos: Option<ChartBtnPos>,
    panic_sell_pos: Option<ChartBtnPos>,
    /// Per-window price-axis position for stack charts; `None` defaults to `Left`.
    price_axis_pos: Option<PriceAxisPos>,
    /// Per-window time-axis visibility for stack charts; `None` defaults to enabled.
    time_axis_visible: Option<bool>,
    /// Per-window line-label visibility for stack charts; `None` defaults to enabled.
    line_labels: Option<bool>,
    /// Per-window crosshair-label visibility for stack charts; `None` defaults to enabled.
    cursor_labels: Option<bool>,
    /// Comparison anchor `(core, market)` that leads the price scale; `None` disables comparison.
    compare_anchor: Option<(CoreId, String)>,
    /// Shared comparison Y range copied from the anchor's current Y window.
    compare_y: Option<(f32, f32)>,
    /// Broom mode makes the anchor's neighbors show only their order books.
    compare_orderbook_only: bool,
    /// Whether the one-shot inactivity auto-close timer is armed.
    /// It ticks at about 1 Hz while configured and charts exist, then rearms itself.
    idle_timer_armed: bool,
    scroll: MoonVirtualListScrollHandle,
}

/// Select one chart for Escape and update Main presentation state for the remaining chart count.
///
/// Args:
///     active: Current active index, updated to the chart replacing the removed slot.
///     show_stack: Current presentation mode, updated to stack when charts remain.
///     chart_count: Number of charts before removal.
///
/// Returns:
///     Index to remove, or `None` when the stack is empty.
fn take_active_close_index(
    active: &mut Option<usize>,
    show_stack: &mut bool,
    chart_count: usize,
) -> Option<usize> {
    if chart_count == 0 {
        *active = None;
        *show_stack = false;
        return None;
    }
    let index = active.filter(|&i| i < chart_count).unwrap_or(0);
    let remaining = chart_count - 1;
    *active = (remaining > 0).then_some(index.min(remaining.saturating_sub(1)));
    *show_stack = remaining > 0;
    Some(index)
}

/// Decide what a right-click on the chart body does to Main's presentation mode.
///
/// Fullscreen means "this chart alone, full bleed"; stack means "all of them, tiled". With a
/// single chart the two render the same content, so flipping into stack presentation changes
/// nothing the user asked for and only shifts the chart by the gutter the tiled layout adds — a
/// vertical jump with no visible cause. Returning to fullscreen FROM the stack stays available at
/// any count, because a stack can be left holding one chart after the others expire.
///
/// Args:
///     show_stack: Current presentation mode.
///     chart_count: Number of charts the stack holds.
///
/// Returns:
///     The new `show_stack` value, or `None` when the gesture has nothing to change.
fn stack_toggle_target(show_stack: bool, chart_count: usize) -> Option<bool> {
    if chart_count < 2 {
        return show_stack.then_some(false);
    }
    Some(!show_stack)
}

/// Resolve an active index after entries were reordered while preserving the active identity.
///
/// Args:
///     item_count: Number of entries after reordering.
///     active_key: Stable identity of the entry that was active before reordering.
///     fallback: Previous numeric index used only when the identity disappeared.
///     matches: Predicate that checks whether an index still owns the active identity.
///
/// Returns:
///     Remapped active index, a bounded fallback, or `None` for an empty stack.
fn remap_active_index<T>(
    item_count: usize,
    active_key: Option<&T>,
    fallback: Option<usize>,
    mut matches: impl FnMut(usize, &T) -> bool,
) -> Option<usize> {
    if item_count == 0 {
        return None;
    }
    active_key
        .and_then(|key| (0..item_count).find(|&ix| matches(ix, key)))
        .or_else(|| Some(fallback.unwrap_or(0).min(item_count - 1)))
}

impl MainChartStack {
    /// Construct a group's Main chart stack and optionally open its initial target.
    ///
    /// Args:
    ///     backend: Shared application state used by chart panels and market publication.
    ///     group: Main window group that owns this stack.
    ///     focus_open: Optional core and market to focus during construction.
    ///     epoch: Chart time origin passed to child panels.
    ///     theme: Runtime chart rendering theme.
    ///     cx: Stack context used to create and observe panels.
    ///
    /// Returns:
    ///     An initialized Main stack.
    pub(super) fn new(
        backend: Entity<Backend>,
        group: String,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            backend,
            group,
            epoch,
            theme,
            charts: Vec::new(),
            active: None,
            show_stack: false,
            scale: None,
            layout_mode: None,
            layout_height_fit: None,
            layout_height_scroll: None,
            orderbook_enabled: None,
            liquidations_enabled: None,
            candle_view: None,
            x_ppm: None,
            show_zone: None,
            auto_pin: None,
            layout_orientation: None,
            cancel_buy_pos: None,
            panic_sell_pos: None,
            price_axis_pos: None,
            time_axis_visible: None,
            line_labels: None,
            cursor_labels: None,
            compare_anchor: None,
            compare_y: None,
            compare_orderbook_only: false,
            idle_timer_armed: false,
            scroll: MoonVirtualListScrollHandle::new(),
        };
        if let Some((core, market)) = focus_open {
            this.open_or_focus(core, market, crate::backend::ChartHistoryScope::Default, cx);
        }
        this
    }

    /// Create and configure one Main chart panel with the stack's current presentation settings.
    ///
    /// Args:
    ///     core: Live core that supplies the market.
    ///     market: Canonical market name to open.
    ///     cx: Stack context used to create the panel and retain its observer.
    ///
    /// Returns:
    ///     The configured chart-panel entity.
    fn create_panel(
        &self,
        core: CoreId,
        market: &str,
        cx: &mut Context<Self>,
    ) -> Entity<ChartPanel> {
        let backend = self.backend.clone();
        let epoch = self.epoch;
        let theme = self.theme.clone();
        let workspace_group = self.group.clone();
        let market = market.to_string();
        let panel = cx.new(|cx| {
            ChartPanel::new_main(
                backend,
                Some(workspace_group),
                Some((core, market)),
                epoch,
                theme,
                cx,
            )
        });
        cx.observe(&panel, |this, _, cx| {
            let mut dirty = this.prune_empty(cx);
            if dirty {
                this.sync_visibility(cx);
                this.sync_backend_open_markets(cx);
            }
            dirty |= this.sync_compare(cx);
            if dirty {
                cx.notify();
            }
        })
        .detach();
        if self.scale.is_some() {
            panel.update(cx, |panel, pcx| panel.set_scale(self.scale, pcx));
        }
        if let Some(en) = self.orderbook_enabled {
            panel.update(cx, |panel, pcx| panel.set_orderbook_enabled(en, pcx));
        }
        if let Some(en) = self.liquidations_enabled {
            panel.update(cx, |panel, pcx| panel.set_liquidations_enabled(en, pcx));
        }
        if self.candle_view.is_some() {
            let cv = self.candle_view;
            panel.update(cx, |panel, pcx| panel.set_candle_view(cv, pcx));
        }
        if self.x_ppm.is_some() {
            let ppm = self.x_ppm;
            panel.update(cx, |panel, _| panel.set_default_x_ppm(ppm));
        }
        if let Some(sz) = self.show_zone {
            panel.update(cx, |panel, pcx| panel.set_show_zone(sz, pcx));
        }
        if let Some(ap) = self.auto_pin {
            panel.update(cx, |panel, pcx| panel.set_auto_pin(ap, pcx));
        }
        panel.update(cx, |panel, pcx| {
            panel.set_action_btn_pos(
                self.cancel_buy_pos.unwrap_or_default(),
                self.panic_sell_pos.unwrap_or_default(),
                pcx,
            )
        });
        panel.update(cx, |panel, pcx| {
            panel.set_price_axis_pos(self.price_axis_pos.unwrap_or_default(), pcx)
        });
        panel.update(cx, |panel, pcx| {
            panel.set_time_axis_visible(self.time_axis_visible.unwrap_or(true), pcx)
        });
        panel.update(cx, |panel, pcx| {
            panel.set_line_labels(self.line_labels.unwrap_or(true), pcx)
        });
        panel.update(cx, |panel, pcx| {
            panel.set_cursor_labels(self.cursor_labels.unwrap_or(true), pcx)
        });
        panel
    }

    /// Synchronize comparison mode while preserving the active market across anchor reordering.
    ///
    /// Args:
    ///     cx: Stack context used to consume comparison requests and update panels.
    ///
    /// Returns:
    ///     Whether the comparison anchor, broom state, or chart order changed.
    fn sync_compare(&mut self, cx: &mut Context<Self>) -> bool {
        let previous_active = self.active;
        let active_key = previous_active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| (entry.core, entry.market.clone()));
        let changed = sync_compare(
            &mut self.charts,
            &mut self.compare_anchor,
            &mut self.compare_y,
            &mut self.compare_orderbook_only,
            self.layout_orientation,
            cx,
        );
        if changed {
            self.active = remap_active_index(
                self.charts.len(),
                active_key.as_ref(),
                previous_active,
                |ix, key| self.charts.get(ix).is_some_and(|entry| entry.is(key)),
            );
        }
        changed
    }

    /// Move fullscreen focus to the next chart cyclically for the `switch_charts` hotkey.
    /// With fewer than two charts there is nothing to switch. This leaves whole-stack mode, just
    /// like regular chart focus, and synchronizes the group's active trading target.
    ///
    /// Args:
    ///     cx: Stack context used to update visibility, open markets, and observers.
    ///
    /// Returns:
    ///     Nothing; stacks with fewer than two charts are unchanged.
    pub(crate) fn cycle_active(&mut self, cx: &mut Context<Self>) {
        if self.charts.len() < 2 {
            return;
        }
        let cur = self.active.unwrap_or(0);
        let next = (cur + 1) % self.charts.len();
        self.active = Some(next);
        self.show_stack = false;
        self.sync_visibility(cx);
        self.sync_backend_open_markets(cx);
        self.arm_idle_timer(cx);
        cx.notify();
    }

    /// Focus an existing Main chart or create it when the core-market pair is not open.
    ///
    /// Args:
    ///     core: Live core that owns the market.
    ///     market: Canonical market name.
    ///     history: Default or published Report durable-history scope for this exact target.
    ///     cx: Stack context used to create panels and publish open markets.
    ///
    /// Returns:
    ///     Nothing; the requested chart becomes the fullscreen active entry.
    pub(super) fn open_or_focus(
        &mut self,
        core: CoreId,
        market: String,
        history: crate::backend::ChartHistoryScope,
        cx: &mut Context<Self>,
    ) {
        let key = (core, market.clone());
        if let Some(index) = self.index_of(&key) {
            self.charts[index].panel.update(cx, |panel, panel_cx| {
                panel.apply_history_scope(core, market.clone(), history.clone(), panel_cx);
            });
            // Already open: this is the same selection the tab row performs, plus the idle timer
            // an explicit open restarts.
            self.select_market(&key, true, cx);
            self.arm_idle_timer(cx);
            return;
        }

        let panel = self.create_panel(core, &market, cx);
        panel.update(cx, |panel, panel_cx| {
            panel.apply_history_scope(core, market.clone(), history, panel_cx);
        });
        self.charts.push(ChartStackEntry::new(core, market, panel));
        self.active = Some(self.charts.len() - 1);
        self.show_stack = false;
        self.sync_visibility(cx);
        self.sync_backend_open_markets(cx);
        // In comparison mode, a new chart immediately receives eligibility and the shared Y range.
        self.sync_compare(cx);
        self.arm_idle_timer(cx);
        cx.notify();
    }

    /// Focus an existing Main chart or replace the active slot for an Auto rail selection.
    ///
    /// Unlike [`Self::open_or_focus`], a missing target never appends beside the current chart.
    /// Replacing the active entry keeps the stack count and position stable while the shared
    /// teardown releases the old chart's panes, market interest, and comparison lock.
    ///
    /// Args:
    ///     core: Selected Auto workspace core.
    ///     market: Canonical target-core market chosen by the parent controller.
    ///     cx: Stack context used to focus or replace the active chart and publish state.
    ///
    /// Returns:
    ///     Nothing; the chosen target becomes the fullscreen active Main entry.
    pub(super) fn replace_or_focus(
        &mut self,
        core: CoreId,
        market: String,
        cx: &mut Context<Self>,
    ) {
        let key = (core, market.clone());
        if self.index_of(&key).is_some() {
            self.select_market(&key, true, cx);
            self.arm_idle_timer(cx);
            return;
        }
        let Some(active) = self.active.filter(|index| *index < self.charts.len()) else {
            self.open_or_focus(core, market, crate::backend::ChartHistoryScope::Default, cx);
            return;
        };
        self.remove_chart_at(active, cx);
        let panel = self.create_panel(core, &market, cx);
        panel.update(cx, |panel, panel_cx| {
            panel.apply_history_scope(
                core,
                market.clone(),
                crate::backend::ChartHistoryScope::Default,
                panel_cx,
            );
        });
        self.charts
            .insert(active, ChartStackEntry::new(core, market, panel));
        self.active = Some(active);
        self.show_stack = false;
        self.sync_visibility(cx);
        self.sync_backend_open_markets(cx);
        self.sync_compare(cx);
        self.arm_idle_timer(cx);
        cx.notify();
    }

    /// Remove panels whose own panes are empty while preserving the active market identity.
    ///
    /// Args:
    ///     cx: Application context used to inspect child-panel state.
    ///
    /// Returns:
    ///     Whether at least one empty panel was removed.
    fn prune_empty(&mut self, cx: &App) -> bool {
        let previous_active = self.active;
        let active_key = previous_active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| (entry.core, entry.market.clone()));
        let changed = retain_nonempty_panels(&mut self.charts, cx);
        self.active = remap_active_index(
            self.charts.len(),
            active_key.as_ref(),
            previous_active,
            |ix, key| self.charts.get(ix).is_some_and(|entry| entry.is(key)),
        );
        if self.active.is_none() {
            self.show_stack = false;
        }
        changed
    }

    /// Arm the one-shot inactivity auto-close timer unless it is already armed.
    /// It ticks at about 1 Hz while `main_idle_close_secs` is positive and charts exist, then rearms
    /// itself in the callback. Chart open/focus events and the timer callback start it, not render.
    fn arm_idle_timer(&mut self, cx: &mut Context<Self>) {
        if self.idle_timer_armed
            || self.charts.is_empty()
            || self.backend.read(cx).main_idle_close_secs() == 0
        {
            return;
        }
        self.idle_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(Duration::from_secs(1)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.idle_timer_armed = false;
                    this.prune_idle(cx);
                    this.arm_idle_timer(cx);
                })
                .is_ok()
            });
        })
        .detach();
    }

    /// Close the active Main chart for Escape in either fullscreen or tiled-stack presentation.
    /// Remaining charts return to stack view with the chart that replaced the removed slot active.
    ///
    /// Args:
    ///     cx: Stack context used to close the panel and publish the new visible/open state.
    ///
    /// Returns:
    ///     Whether a chart closed.
    pub(crate) fn close_active(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(idx) =
            take_active_close_index(&mut self.active, &mut self.show_stack, self.charts.len())
        else {
            return false;
        };
        // Escape has already decided what stays active and that the remainder returns to stack
        // view; this only performs the removal.
        self.remove_chart_at(idx, cx);
        self.publish(cx);
        true
    }

    /// Remove one chart and release everything hanging off it, deciding nothing about what is
    /// active afterwards.
    ///
    /// All single-chart close paths use this teardown so they release the panel's panes and
    /// order-book subscriptions, plus any comparison lock the removed chart anchored. Selection
    /// remains a caller decision because Escape and the tab-row close button choose differently.
    ///
    /// Args:
    ///     idx: Index of the chart to remove; assumed in range.
    ///     cx: Stack context used to close the panel and refresh comparison peers.
    ///
    /// Returns:
    ///     Nothing; selection and publication remain the caller's responsibility.
    fn remove_chart_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        let entry = self.charts.remove(idx);
        entry.panel.update(cx, |p, pcx| p.close_all_panes(pcx));
        let had_compare_anchor = self.compare_anchor.is_some();
        if self
            .compare_anchor
            .as_ref()
            .is_some_and(|key| key.0 == entry.core && key.1 == entry.market)
        {
            self.compare_anchor = None;
            self.compare_orderbook_only = false;
            self.compare_y = None;
        }
        if had_compare_anchor {
            // Refresh comparison peers and clear any locks left by a removed anchor.
            self.sync_compare(cx);
        }
    }

    /// Close one chart chosen from the tab row, keeping the selection where the user left it.
    ///
    /// Unlike Escape this does not change presentation mode: closing a sibling from the row is not
    /// a request to leave fullscreen. The chart that stays active is tracked by IDENTITY rather
    /// than by index, because removing an earlier entry shifts every index after it — following
    /// the number would silently select a different market.
    ///
    /// Args:
    ///     idx: Index of the chart to close.
    ///     cx: Stack context used to release the chart and publish the new state.
    ///
    /// Returns:
    ///     Whether a chart closed.
    fn close_at(&mut self, idx: usize, cx: &mut Context<Self>) -> bool {
        if idx >= self.charts.len() {
            return false;
        }
        // The identity to keep active — unless it is the one going away, in which case the numeric
        // fallback picks up whatever slid into its place.
        let keep = self
            .active
            .filter(|&active| active != idx)
            .and_then(|active| self.charts.get(active))
            .map(|entry| (entry.core, entry.market.clone()));
        let fallback = self.active;
        self.remove_chart_at(idx, cx);
        let charts = &self.charts;
        self.active = remap_active_index(charts.len(), keep.as_ref(), fallback, |ix, key| {
            charts[ix].is(key)
        });
        if self.charts.is_empty() {
            self.show_stack = false;
        }
        self.publish(cx);
        true
    }

    /// Close every chart in the Main stack for the built-in Shift+Escape action.
    /// This releases each panel's order-book subscriptions and clears the stack. Returns whether
    /// anything was available to close.
    pub(crate) fn close_all(&mut self, cx: &mut Context<Self>) -> bool {
        if self.charts.is_empty() {
            return false;
        }
        for entry in self.charts.drain(..) {
            entry.panel.update(cx, |p, pcx| p.close_all_panes(pcx));
        }
        self.active = None;
        self.show_stack = false;
        // The anchor left with the charts. Left set, it names a market that no longer exists, and
        // the next charts opened here are classified as its followers — locked to a price window
        // and, in broom mode, reduced to order books — with no leader on screen to explain it.
        self.compare_anchor = None;
        self.compare_y = None;
        self.compare_orderbook_only = false;
        // Orders highlights and prioritizes rows for the markets Main has open. Nothing else writes
        // that list, so without this it keeps pointing at charts that are gone.
        self.sync_backend_open_markets(cx);
        cx.notify();
        true
    }

    /// Auto-close unpinned charts after the configured window inactivity interval in seconds.
    /// Each deadline is `max(last_input, arrived_at) + interval`, with `arrived_at` used when no
    /// input exists. Closing the active fullscreen chart returns the remainder to stack view and
    /// immediately releases its order-book subscriptions. A focused detached chart window refreshes
    /// group activity because its input does not reach Main directly.
    ///
    /// Args:
    ///     cx: Stack context used to inspect windows, close panels, and publish open markets.
    ///
    /// Returns:
    ///     `true` when at least one idle chart was removed.
    fn prune_idle(&mut self, cx: &mut Context<Self>) -> bool {
        let secs = self.backend.read(cx).main_idle_close_secs();
        if secs == 0 || self.charts.is_empty() {
            return false;
        }
        // Keep Main charts open while any detached chart window in this group is focused. Activity
        // in that separate OS window does not produce mouse movement in Main, so refresh the group
        // timestamp and grant Main a full TTL after the detached window loses focus.
        let group = self.group.clone();
        let chart_handles: Vec<_> = self
            .backend
            .read(cx)
            .detached_chart_windows
            .iter()
            .filter(|(g, _)| *g == group)
            .map(|(_, h)| *h)
            .collect();
        let chart_focused = chart_handles
            .into_iter()
            .any(|h| h.is_active(cx).unwrap_or(false));
        if chart_focused {
            self.backend.update(cx, |b, bcx| {
                b.note_main_input(&group);
                b.focus_auto_workspace(&group, bcx);
            });
            return false;
        }
        let ttl = Duration::from_secs(secs as u64);
        let last_input = self.backend.read(cx).main_input_at(&self.group);
        let now = Instant::now();
        // Inactivity does not close pinned charts, matching `prune_ttl`. Read pin flags through
        // `cx` before filtering to avoid borrowing panels during mutation.
        let pinned: Vec<bool> = self
            .charts
            .iter()
            .map(|e| e.panel.read(cx).is_pinned())
            .collect();
        let expired: Vec<usize> = self
            .charts
            .iter()
            .enumerate()
            .filter(|(ix, e)| {
                if pinned[*ix] {
                    return false;
                }
                let base = match last_input {
                    Some(t) => t.max(e.arrived_at),
                    None => e.arrived_at,
                };
                now.duration_since(base) >= ttl
            })
            .map(|(ix, _)| ix)
            .collect();
        if expired.is_empty() {
            return false;
        }
        let active_closed = self.active.is_some_and(|a| expired.contains(&a));
        // Track the surviving selection by IDENTITY. A numeric index may shift when an earlier
        // chart expires and would then highlight a different market and redirect trading hotkeys.
        let keep = self
            .active
            .filter(|active| !expired.contains(active))
            .and_then(|active| self.charts.get(active))
            .map(|entry| (entry.core, entry.market.clone()));
        let fallback = self.active;
        // Remove from the end to preserve indices. Routed through the shared removal so an expiring
        // comparison anchor tears its lock down with it, exactly as closing one by hand does.
        for &ix in expired.iter().rev() {
            self.remove_chart_at(ix, cx);
        }
        if self.charts.is_empty() {
            self.active = None;
            self.show_stack = false;
        } else {
            // If the active fullscreen chart closed, return the remaining charts to stack view.
            if active_closed && !self.show_stack {
                self.show_stack = true;
            }
            let charts = &self.charts;
            self.active = remap_active_index(charts.len(), keep.as_ref(), fallback, |ix, key| {
                charts[ix].is(key)
            });
        }
        self.publish(cx);
        true
    }

    pub(crate) fn scale(&self) -> Option<f32> {
        self.scale
    }

    pub(crate) fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        apply_setting(&mut self.scale, pct, &self.charts, cx, |c, cx| {
            set_panels_scale(c, pct, cx)
        });
    }

    pub(crate) fn layout_mode(&self) -> Option<StackLayoutMode> {
        self.layout_mode
    }

    pub(crate) fn layout_height_fit(&self) -> Option<u16> {
        self.layout_height_fit
    }

    pub(crate) fn layout_height_scroll(&self) -> Option<u16> {
        self.layout_height_scroll
    }

    /// Apply a per-tab layout mode and separate Fit/Scroll heights to this stack.
    pub(crate) fn set_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_mode == mode
            && self.layout_height_fit == height_fit
            && self.layout_height_scroll == height_scroll
        {
            return;
        }
        self.layout_mode = mode;
        self.layout_height_fit = height_fit;
        self.layout_height_scroll = height_scroll;
        cx.notify();
    }

    pub(crate) fn orderbook_enabled(&self) -> Option<bool> {
        self.orderbook_enabled
    }

    /// Enable or disable the order book for every chart in this stack and window.
    pub(crate) fn set_orderbook_enabled(&mut self, enabled: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(
            &mut self.orderbook_enabled,
            enabled,
            &self.charts,
            cx,
            |c, cx| set_panels_orderbook_enabled(c, enabled.unwrap_or(true), cx),
        );
    }

    pub(crate) fn liquidations_enabled(&self) -> Option<bool> {
        self.liquidations_enabled
    }

    /// Enable or disable liquidation trades for every chart in this stack and window.
    pub(crate) fn set_liquidations_enabled(
        &mut self,
        enabled: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        apply_setting(
            &mut self.liquidations_enabled,
            enabled,
            &self.charts,
            cx,
            |c, cx| set_panels_liquidations(c, enabled.unwrap_or(true), cx),
        );
    }

    pub(crate) fn candle_view(&self) -> Option<moon_core::market::CandleViewCfg> {
        self.candle_view
    }

    /// Store the window X scale for new charts and, when `apply` is true, apply it to all open charts.
    /// Startup seeding stores the scale before any charts are open; Shift+middle-click synchronizes it.
    pub(crate) fn set_x_ppm(&mut self, ppm: Option<f32>, apply: bool, cx: &mut Context<Self>) {
        self.x_ppm = ppm;
        for e in &self.charts {
            e.panel.update(cx, |p, pcx| {
                p.set_default_x_ppm(ppm);
                if apply {
                    if let Some(v) = ppm {
                        p.apply_x_ppm(v, pcx);
                    }
                }
            });
        }
        if apply {
            cx.notify();
        }
    }

    /// Apply per-tab candle and trade display settings to every chart in the stack.
    pub(crate) fn set_candle_view(
        &mut self,
        cfg: Option<moon_core::market::CandleViewCfg>,
        cx: &mut Context<Self>,
    ) {
        apply_setting(&mut self.candle_view, cfg, &self.charts, cx, |c, cx| {
            set_panels_candle_view(c, cfg, cx)
        });
    }

    pub(crate) fn show_zone(&self) -> Option<bool> {
        self.show_zone
    }

    /// Enable or disable the control-zone fill for every chart in this stack and window.
    pub(crate) fn set_show_zone(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.show_zone, show, &self.charts, cx, |c, cx| {
            set_panels_show_zone(c, show.unwrap_or(true), cx)
        });
    }

    pub(crate) fn auto_pin(&self) -> Option<bool> {
        self.auto_pin
    }

    pub(crate) fn action_btn_pos(&self) -> (Option<ChartBtnPos>, Option<ChartBtnPos>) {
        (self.cancel_buy_pos, self.panic_sell_pos)
    }

    pub(crate) fn price_axis_pos(&self) -> Option<PriceAxisPos> {
        self.price_axis_pos
    }

    /// Set the price-axis position for every chart in this stack and window.
    pub(crate) fn set_price_axis_pos(&mut self, pos: Option<PriceAxisPos>, cx: &mut Context<Self>) {
        apply_setting(&mut self.price_axis_pos, pos, &self.charts, cx, |c, cx| {
            set_panels_price_axis_pos(c, pos.unwrap_or_default(), cx)
        });
    }

    pub(crate) fn time_axis_visible(&self) -> Option<bool> {
        self.time_axis_visible
    }

    /// Set time-axis visibility for every chart in this stack and window.
    pub(crate) fn set_time_axis_visible(&mut self, visible: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(
            &mut self.time_axis_visible,
            visible,
            &self.charts,
            cx,
            |c, cx| set_panels_time_axis_visible(c, visible.unwrap_or(true), cx),
        );
    }

    pub(crate) fn line_labels(&self) -> Option<bool> {
        self.line_labels
    }

    /// Set line-label visibility for every chart in this stack and window.
    pub(crate) fn set_line_labels(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.line_labels, show, &self.charts, cx, |c, cx| {
            set_panels_line_labels(c, show.unwrap_or(true), cx)
        });
    }

    pub(crate) fn cursor_labels(&self) -> Option<bool> {
        self.cursor_labels
    }

    /// Set crosshair-label visibility for every chart in this stack and window.
    pub(crate) fn set_cursor_labels(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.cursor_labels, show, &self.charts, cx, |c, cx| {
            set_panels_cursor_labels(c, show.unwrap_or(true), cx)
        });
    }

    /// Set Cancel Buy and Panic Sell button positions for every chart in this stack and window.
    pub(crate) fn set_action_btn_pos(
        &mut self,
        cancel: Option<ChartBtnPos>,
        panic: Option<ChartBtnPos>,
        cx: &mut Context<Self>,
    ) {
        if self.cancel_buy_pos == cancel && self.panic_sell_pos == panic {
            return;
        }
        self.cancel_buy_pos = cancel;
        self.panic_sell_pos = panic;
        set_panels_action_btn_pos(
            &self.charts,
            cancel.unwrap_or_default(),
            panic.unwrap_or_default(),
            cx,
        );
        cx.notify();
    }

    /// Enable or disable automatic pinning on order placement for this stack and window.
    pub(crate) fn set_auto_pin(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.auto_pin, on, &self.charts, cx, |c, cx| {
            set_panels_auto_pin(c, on.unwrap_or(false), cx)
        });
    }

    pub(crate) fn layout_orientation(&self) -> Option<StackOrientation> {
        self.layout_orientation
    }

    /// Change the per-window stack orientation and rebuild the current display.
    pub(crate) fn set_orientation(
        &mut self,
        orientation: Option<StackOrientation>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_orientation == orientation {
            return;
        }
        self.layout_orientation = orientation;
        self.sync_compare(cx);
        cx.notify();
    }

    pub(crate) fn active_target(&self, cx: &App) -> Option<(CoreId, String)> {
        self.active
            .and_then(|ix| self.charts.get(ix))
            .and_then(|entry| entry.panel.read(cx).active_target())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_data_handle(&self, cx: &App) -> Option<crate::chartdx::ChartDataHandle> {
        self.active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| entry.panel.read(cx).debug_data_handle())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_fill_history_to_capacity(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.active else {
            log::warn!("debug fill main chart: no active main chart");
            return false;
        };
        let Some(entry) = self.charts.get(ix) else {
            log::warn!("debug fill main chart: active main chart index is stale");
            return false;
        };
        entry
            .panel
            .update(cx, |panel, pcx| panel.debug_fill_history_to_capacity(pcx))
    }

    pub(super) fn set_scene_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible {
            self.sync_visibility(cx);
        } else {
            for entry in &self.charts {
                entry.panel.update(cx, |panel, _| {
                    panel.set_main_stack_scroll(false);
                    panel.set_scene_visible(false);
                });
            }
        }
    }

    fn sync_visibility(&mut self, cx: &mut Context<Self>) {
        for (ix, entry) in self.charts.iter().enumerate() {
            // In fullscreen, only the active chart is visible. In stack mode, visible tiles set
            // themselves visible in `ChartPanel::render`; offscreen virtual-list entries remain
            // hidden and do not run preparation work.
            let visible = !self.show_stack && Some(ix) == self.active;
            let stack_scroll = self.show_stack;
            entry.panel.update(cx, |panel, _| {
                panel.set_main_stack_scroll(stack_scroll);
                panel.set_scene_visible(visible);
            });
        }
    }

    fn sync_stack_visible_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        if !self.show_stack {
            return;
        }
        for (ix, entry) in self.charts.iter().enumerate() {
            let visible = range.contains(&ix);
            entry.panel.update(cx, |panel, _| {
                panel.set_main_stack_scroll(true);
                panel.set_scene_visible(visible);
            });
        }
    }

    /// Publish the non-vacant markets currently open in this Main stack.
    ///
    /// `ChartTabs` separately owns the anchor-aware trading target, so this child stack must not
    /// publish or persist a competing target.
    ///
    /// Args:
    ///     cx: Stack context used to update the shared Backend.
    ///
    /// Returns:
    ///     Nothing; the Backend's group market list is replaced.
    fn sync_backend_open_markets(&self, cx: &mut Context<Self>) {
        // Publish all non-vacant Main-stack markets so Orders can highlight one row for each.
        let open: Vec<(CoreId, String)> = self
            .charts
            .iter()
            .filter(|e| !e.vacated)
            .map(|e| (e.core, e.market.clone()))
            .collect();
        self.backend
            .update(cx, |b, _| b.set_main_open_markets(&self.group, open));
    }

    /// Republish everything derived from which charts exist and which one is selected.
    ///
    /// Keep these steps together because panel visibility follows the selection, Orders reads the
    /// published open-market list, and the parent only learns of either through the notification.
    ///
    /// Args:
    ///     cx: Stack context used to update child visibility, Backend state, and observers.
    ///
    /// Returns:
    ///     Nothing; all derived state is synchronized before notification.
    fn publish(&mut self, cx: &mut Context<Self>) {
        self.sync_visibility(cx);
        self.sync_backend_open_markets(cx);
        cx.notify();
    }

    /// Select one chart, optionally showing it alone.
    ///
    /// Args:
    ///     key: Core and market of the chart to select.
    ///     fullscreen: Whether to leave stack presentation and show it full bleed.
    ///     cx: Stack context used to publish the new state.
    ///
    /// Returns:
    ///     Whether anything changed.
    fn select_market(
        &mut self,
        key: &(CoreId, String),
        fullscreen: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ix) = self.index_of(key) else {
            return false;
        };
        if self.active == Some(ix) && !(fullscreen && self.show_stack) {
            return false;
        }
        self.active = Some(ix);
        if fullscreen {
            self.show_stack = false;
        }
        self.publish(cx);
        true
    }

    /// Make one chart the active one without changing presentation mode.
    ///
    /// The active chart is also the group's trading target — `ChartTabs::main_chart_target` reads
    /// it — so picking a tab redirects the F1-F6 / S1-S6 hotkeys and cancel-buy to that market.
    /// That is the point of the row, but it means a stray click moves where an order would land.
    ///
    /// The comparison anchor deliberately does NOT follow because it leads the shared price scale,
    /// while Main's trading target follows the selected chart. The two markers therefore represent
    /// separate responsibilities.
    ///
    /// Args:
    ///     key: Core and market of the chart to select.
    ///     cx: Stack context used to publish visibility and wake the parent.
    ///
    /// Returns:
    ///     Nothing; a missing or already-selected market leaves state unchanged.
    fn focus_market(&mut self, key: &(CoreId, String), cx: &mut Context<Self>) {
        self.select_market(key, false, cx);
    }

    /// Select one chart and show it alone, full bleed — the tab row's double-click.
    ///
    /// Args:
    ///     key: Core and market of the chart to show.
    ///     cx: Stack context used to publish visibility and wake the parent.
    ///
    /// Returns:
    ///     Nothing; a missing market leaves state unchanged.
    fn fullscreen_market(&mut self, key: &(CoreId, String), cx: &mut Context<Self>) {
        self.select_market(key, true, cx);
    }

    /// Resolve a chart's identity to its CURRENT index, or `None` once it is gone.
    ///
    /// Every tab-row handler goes through this. A handler fires after the render that built it, and
    /// in between an idle chart can expire or a comparison lock can move its anchor to the front —
    /// both of which renumber the stack while leaving the old index perfectly in range, pointing at
    /// somebody else's market.
    fn index_of(&self, key: &(CoreId, String)) -> Option<usize> {
        self.charts.iter().position(|entry| entry.is(key))
    }

    /// Toggle one clicked chart between fullscreen and whole-stack presentation.
    ///
    /// With a single chart open the gesture is deliberately inert — see [`stack_toggle_target`].
    ///
    /// Args:
    ///     ix: Index of the clicked chart.
    ///     cx: Stack context used to update visibility and notify the parent.
    ///
    /// Returns:
    ///     Nothing; an out-of-range index, or a gesture with nothing to change, is ignored.
    fn toggle_from_chart(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.charts.len() {
            return;
        }
        let Some(show_stack) = stack_toggle_target(self.show_stack, self.charts.len()) else {
            return;
        };
        self.active = Some(ix);
        self.show_stack = show_stack;
        self.publish(cx);
    }

    /// Renders one Main-chart tile in either stacked or fullscreen presentation.
    ///
    /// Fullscreen deliberately keeps the card header so the active market remains identifiable;
    /// only the inter-tile gutter is removed, and a position note appears when siblings exist.
    ///
    /// Args:
    ///     ix: Index of the chart entry being rendered.
    ///     panel: Chart panel placed inside the card.
    ///     size: Optional extent along the stack axis.
    ///     flex: Whether the tile may flex in Compress or Fit layout.
    ///     min_w: Minimum extent along the stack axis while flexing.
    ///     horizontal: Whether the stack axis is horizontal.
    ///     border: Border color for the card.
    ///     entity: Stack entity used by the fullscreen toggle callback.
    ///     palette: Active MoonUI palette.
    ///     fullscreen: Whether this is the single full-bleed active chart.
    ///     title_size: Resolved title font size.
    ///
    /// Returns:
    ///     The sized chart card with its fullscreen toggle handler attached.
    #[allow(clippy::too_many_arguments)]
    fn render_tile(
        &self,
        ix: usize,
        panel: Entity<ChartPanel>,
        size: Option<f32>,
        flex: bool,
        min_w: Option<f32>,
        horizontal: bool,
        border: Rgba,
        entity: Entity<Self>,
        palette: MoonPalette,
        fullscreen: bool,
        title_size: Pixels,
    ) -> Stateful<Div> {
        let panel_for_event = panel.clone();
        let label = self
            .charts
            .get(ix)
            .map(|e| e.market.clone())
            .unwrap_or_else(|| "Chart".to_string());
        // The card — and therefore the market name — is drawn in fullscreen too. Without it the
        // fullscreen view is the stacked view MINUS its only label, so with a single chart the
        // gesture reads as "the coin name disappeared" rather than as a mode change; with several,
        // nothing on screen says which one is showing. Fullscreen drops only the inter-tile gutter,
        // which has nothing left to separate, and gains a position note when there are siblings.
        let trailing = (fullscreen && self.charts.len() > 1)
            .then(|| SharedString::from(format!("{}/{}", ix + 1, self.charts.len())));
        // Tiled, the tab row above says which chart is selected; the card repeats it with an accent
        // frame so the answer is also where the user's eyes already are. Fullscreen shows only the
        // selected chart, so there is nothing to distinguish it from.
        let border = if !fullscreen && Some(ix) == self.active && self.charts.len() > 1 {
            rgb(palette.accent)
        } else {
            border
        };
        let mut tile = chart_stack_card(
            SharedString::from(format!("main-chart-stack-tile-{ix}")),
            label,
            panel,
            palette,
            border,
            title_size,
            tile_gutter(fullscreen, self.charts.len()),
            trailing,
        )
        .on_mouse_up(
            MouseButton::Right,
            move |event: &MouseUpEvent, _window, app| {
                // A short right-click in the panel area exits fullscreen. In the control zone
                // (order book or reserved strip), right-click remains trading-only, while a
                // right-button price drag remains zoom and does not toggle the stack.
                let panel = panel_for_event.read(app);
                if panel.window_pos_allows_main_stack_toggle(event.position)
                    && !panel.window_pos_in_control_zone(event.position, app)
                    && !panel.rmb_was_moved()
                {
                    entity.update(app, |this, cx| this.toggle_from_chart(ix, cx));
                    app.stop_propagation();
                }
            },
        );
        // Fill the cross axis. Along the stack axis use flex with a cap for COMPRESS, a fixed size,
        // or stretching for FIT. Horizontal stacks use X as their axis; vertical stacks use Y.
        tile = if horizontal {
            tile.h_full()
        } else {
            tile.w_full()
        };
        if flex {
            tile = tile.flex_1();
            let m = min_w.unwrap_or(0.0);
            tile = if horizontal {
                tile.min_w(px(m))
            } else {
                tile.min_h(px(m))
            };
            if let Some(v) = size {
                tile = if horizontal {
                    tile.max_w(px(v))
                } else {
                    tile.max_h(px(v))
                };
            }
        } else if let Some(v) = size {
            // Fixed without shrinking (`min == max == v`), so SCROLL tiles overflow and can scroll.
            tile = if horizontal {
                tile.w(px(v)).min_w(px(v))
            } else {
                tile.h(px(v)).min_h(px(v))
            };
        }
        tile
    }
}

impl MainChartStack {
    /// Render the row of per-chart tabs above the stack, or `None` when it would say nothing.
    ///
    /// It exists because the Main tab is a stack of markets under one label: with several charts
    /// open, nothing on screen said which of them was selected — and the selected one is where
    /// trading hotkeys land. Below two charts the row is suppressed: a single tab names a market
    /// the card header already names, at the cost of a strip of chart height.
    ///
    /// The label is the panel's OWN resolved ticker, so the tab and the chart's corner caption
    /// cannot spell the instrument differently; a chart whose catalog has not answered yet falls
    /// back to the raw market key rather than rendering blank.
    ///
    /// Args:
    ///     window: Window whose width the strip is laid out against.
    ///     cx: Stack context, used to read each panel and to build the click handlers.
    ///
    /// Returns:
    ///     The row, or `None` when fewer than two charts are open.
    fn render_tab_row(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Vacated slots are retained placeholders in COMPRESS layout — they hold a position, not a
        // chart, so they get no tab.
        let live: Vec<(CoreId, String, SharedString)> = self
            .charts
            .iter()
            .filter(|entry| !entry.vacated)
            .map(|entry| {
                let label = entry
                    .panel
                    .read(cx)
                    .pane_ticker()
                    .unwrap_or_else(|| entry.market.clone());
                (entry.core, entry.market.clone(), SharedString::from(label))
            })
            .collect();
        if live.len() < 2 {
            return None;
        }
        let active_key = self
            .active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| (entry.core, entry.market.as_str()));
        let strip_h = crate::chart_tabs::chart_tab_strip_h(cx);
        let gap = 4.0_f32;
        let pad_l = 8.0_f32;
        let mut total_w = pad_l;
        let items: Vec<MoonTabItem> = live
            .iter()
            .map(|(core, market, label)| {
                let width = crate::design::tab_width(cx, label.chars().count(), false, true);
                total_w += width + gap;
                MoonTabItem::new(label.clone())
                    .width(width)
                    .selected(active_key == Some((*core, market.as_str())))
                    .closable(true)
            })
            .collect();
        let keys: Rc<Vec<(CoreId, String)>> = Rc::new(
            live.into_iter()
                .map(|(core, market, _)| (core, market))
                .collect(),
        );
        let view = cx.entity();
        // Lay the strip out at its OWN width rather than the window's: `MoonTabStrip` positions
        // every tab absolutely and clips to the bounds it is given, so a stack wider than the
        // window would simply lose its tail. Scrolling the row keeps every chart reachable.
        let strip_w = total_w.max(f32::from(window.viewport_size().width).max(1.0));
        let strip = MoonTabStrip::new("main-chart-tab-row-strip")
            .padding_left(pad_l)
            .gap(gap)
            .bounds(MoonRect::new(0.0, 0.0, strip_w, strip_h))
            .items(items)
            .on_click({
                let keys = keys.clone();
                let view = view.clone();
                move |ix, event, _window, app| {
                    let Some(key) = keys.get(ix).cloned() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        if event.click_count() >= 2 {
                            this.fullscreen_market(&key, cx);
                        } else {
                            this.focus_market(&key, cx);
                        }
                    });
                }
            })
            .on_close({
                let keys = keys.clone();
                move |ix, _event, _window, app| {
                    let Some(key) = keys.get(ix).cloned() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        if let Some(at) = this.index_of(&key) {
                            this.close_at(at, cx);
                        }
                    });
                }
            });
        Some(
            div()
                .id("main-chart-tab-row")
                .flex_none()
                .w_full()
                .h(px(strip_h))
                // `.relative()` is load-bearing: given explicit bounds, `MoonTabStrip` makes its own
                // root ABSOLUTE and places itself against the nearest positioned ancestor. Without
                // this the row would escape to `ChartTabs`'s relative root and paint over the
                // Main/Add tab strip at the very top of the panel. The sibling strip wraps itself
                // the same way for the same reason.
                .relative()
                .overflow_x_scroll()
                .child(strip)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests;

impl Render for MainChartStack {
    /// Renders the per-chart tab row above either the active full-bleed chart or the virtualized
    /// whole-stack layout.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = moon_ui::MoonPalette::active(cx);
        if self.charts.is_empty() {
            return div()
                .relative()
                .size_full()
                .bg(rgb(palette.chart_bg))
                .flex()
                .items_center()
                .justify_center()
                .child(crate::design::logo_glow_sized(cx, 220.0))
                .into_any_element();
        }

        // The row is built first but composed last: it must know the active chart before the body
        // below borrows `self` for the stack layout's closures.
        let tab_row = self.render_tab_row(window, cx);
        let body = self.render_body(palette, cx);
        let Some(tab_row) = tab_row else {
            return body;
        };
        // The body flexes into whatever the row leaves, so the chart loses exactly the row's
        // height and nothing has to subtract it by hand.
        return v_flex()
            .size_full()
            .child(tab_row)
            .child(div().flex_1().w_full().min_h(px(0.0)).child(body))
            .into_any_element();
    }
}

impl MainChartStack {
    /// The chart area itself: one full-bleed chart, or the whole stack tiled.
    fn render_body(&mut self, palette: MoonPalette, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active.unwrap_or(0).min(self.charts.len() - 1);
        if !self.show_stack {
            let panel = self.charts[active].panel.clone();
            let entity = cx.entity();
            return self
                .render_tile(
                    active,
                    panel,
                    None,
                    false,
                    None,
                    false,
                    rgb(palette.border),
                    entity,
                    palette,
                    true,
                    crate::design::t_body(cx),
                )
                .size_full()
                .into_any_element();
        }

        // Stack mode uses the per-tab FIT/SCROLL/COMPRESS layout and height, or the global default.
        let (scroll, compress, cfg_h) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        let count = self.charts.len();
        let border = rgb(palette.border);
        let base_id = format!("main-chart-stack-{}", self.group);
        let horizontal = self
            .layout_orientation
            .unwrap_or(StackOrientation::Vertical)
            .is_horizontal();
        let entity = cx.entity();
        let p = palette;
        let title_size = crate::design::t_body(cx);
        let on_visible_range = cx.processor(|this, range: Range<usize>, _window, cx| {
            this.sync_stack_visible_range(range, cx);
        });
        render_chart_stack(
            &base_id,
            self,
            entity,
            count,
            scroll,
            compress,
            horizontal,
            cfg_h,
            &self.scroll,
            border,
            |s, ix| s.charts.get(ix).map(|e| e.panel.clone()),
            move |s, ix, panel, size, flex, min_w, horizontal, border, ent| {
                s.render_tile(
                    ix, panel, size, flex, min_w, horizontal, border, ent, p, false, title_size,
                )
                .into_any_element()
            },
            |s, ix| compare_role(&s.charts, &s.compare_anchor, s.compare_orderbook_only, ix),
            Some(Box::new(on_visible_range)),
        )
    }
}
