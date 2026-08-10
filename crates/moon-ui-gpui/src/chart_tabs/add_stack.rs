//! An AddToChart tab appears as one chart list but uses a separate `ChartPanel` for each chart.
//! Extracted from `chart_tabs` as an independent view model; shared stack rendering lives in
//! [`super::stack`]. Used by both the tab strip and detached windows ([`super::windows`]).

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::MoonVirtualListScrollHandle;

use super::stack::{
    COMPACT_STABLE, ChartStackEntry, apply_setting, chart_stack_card, compare_role,
    render_chart_stack, resolve_layout, set_panels_action_btn_pos, set_panels_auto_pin,
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
use moon_core::config::{ChartBucket, ChartTheme};
use moon_core::session::CoreId;

/// AddToChart tab that appears as one chart list while each chart is architecturally a separate
/// `ChartPanel`/`gpu_canvas`/dirty entity. This avoids the old `ChartPanel -> Container.panes`
/// model, where moving the mouse over one chart repainted every overlay.
pub(crate) struct AddChartStack {
    backend: Entity<Backend>,
    /// Group whose Auto rail authorizes actions in every chart owned by this stack.
    workspace_group: String,
    num: u32,
    bucket: ChartBucket,
    epoch: f64,
    theme: ChartTheme,
    charts: Vec<ChartStackEntry>,
    scale: Option<f32>,
    /// Per-tab layout mode (Fit/Scroll; `None` = default Fit).
    layout_mode: Option<StackLayoutMode>,
    /// Fit slot height: 0 = stretch, ≥20 = compress. `None` = default.
    layout_height_fit: Option<u16>,
    /// Scroll slot height. `None` = default.
    layout_height_scroll: Option<u16>,
    /// Whether to show order books on this tab's charts (per window). `None` = enabled by default.
    orderbook_enabled: Option<bool>,
    /// Whether to draw liquidation trades (per window). `None` = enabled by default.
    liquidations_enabled: Option<bool>,
    /// Tab candle/trade display settings (`None` = global default).
    candle_view: Option<moon_core::market::CandleViewCfg>,
    /// Window X scale (px/ms, synchronized with Shift+middle-click; `None` = built-in default).
    /// New charts inherit it, and synchronization applies it to all charts.
    x_ppm: Option<f32>,
    /// Whether to show the management-zone fill (per window). `None` = enabled by default.
    show_zone: Option<bool>,
    /// Whether to auto-pin a chart when placing an order (per window). `None` = disabled by default.
    auto_pin: Option<bool>,
    /// Stack orientation (per window). `None` = default Vertical.
    layout_orientation: Option<StackOrientation>,
    /// Positions of the Cancel Buy / Panic Sell buttons in the chart zone (per window).
    /// `None` = default Right.
    cancel_buy_pos: Option<ChartBtnPos>,
    panic_sell_pos: Option<ChartBtnPos>,
    /// Price-axis position (Left/Right/Hide) for stack charts (per window). `None` = default Left.
    price_axis_pos: Option<PriceAxisPos>,
    /// Time-axis visibility for stack charts (per window). `None` = enabled by default.
    time_axis_visible: Option<bool>,
    /// Line-label visibility for stack charts (per window). `None` = enabled by default.
    line_labels: Option<bool>,
    /// Crosshair-label visibility for stack charts (per window). `None` = enabled by default.
    cursor_labels: Option<bool>,
    /// Whether order-book subscriptions are temporarily suspended after the tab remains unfocused
    /// for over five seconds. Effective order book = `orderbook_enabled ∧ !suspended`, preserving
    /// the user's Order book checkbox. Tabs detached into a window are never suspended because the
    /// window maintains its own demand.
    orderbook_suspended: bool,
    /// Comparison anchor `(core, market)`: the price-leading chart, locked and positioned left.
    /// `None` disables comparison. Active only in horizontal orientation.
    compare_anchor: Option<(CoreId, String)>,
    /// Shared comparison Y window `(center, range)`, tracking the comparison anchor's Y window.
    compare_y: Option<(f32, f32)>,
    /// Broom mode: anchor neighbors show only the order book, hiding chart and price axis.
    compare_orderbook_only: bool,
    /// Runtime-only explicit mode: one manual-order click is replicated across eligible comparison
    /// peers. It resets on restart and is disabled automatically when comparison is unavailable.
    sync_manual_orders_on: bool,
    /// Last SYNC scope published to Backend for toolbar/manual-strategy propagation. Only this same
    /// stack may clear it, preventing inactive stacks from wiping an active comparison context.
    sync_manual_strategy_targets_published: Vec<(CoreId, String)>,
    /// Whether to retain an empty slot after a chart leaves. Automatic AddToChart tabs reuse the
    /// slot in COMPRESS mode for the next detection. CUSTOM tabs set this false: closing a chart
    /// removes its slot immediately and redistributes neighboring charts according to the layout.
    hold_vacated: bool,
    /// Time of the last open-chart count change (arrival, departure, or slot reuse). This debounces
    /// COMPRESS compaction: empty `vacated` slots collapse only after `COMPACT_STABLE` has elapsed
    /// with a stable chart count. See `compact_vacated_if_stable`.
    last_count_change: Instant,
    /// Whether the roughly 1 Hz COMPRESS-compaction debounce timer is armed. It rearms itself like
    /// Main's idle timer.
    compact_timer_armed: bool,
    /// Scroll handle for the vertical `MoonVirtualList` used by the stack's Scroll mode.
    scroll: MoonVirtualListScrollHandle,
}

impl AddChartStack {
    /// Construct one numbered chart stack for a workspace group.
    ///
    /// Args:
    ///     backend: Shared application state and market session.
    ///     workspace_group: Group whose Auto rail authorizes child-chart actions.
    ///     num: Numbered AddToChart or Custom tab identifier.
    ///     bucket: Detect routing bucket owned by the tab.
    ///     epoch: Chart time origin.
    ///     theme: Runtime chart theme.
    ///
    /// Returns:
    ///     An empty stack ready to create group-authorized chart panels.
    pub(super) fn new(
        backend: Entity<Backend>,
        workspace_group: String,
        num: u32,
        bucket: ChartBucket,
        epoch: f64,
        theme: ChartTheme,
    ) -> Self {
        Self {
            backend,
            workspace_group,
            num,
            bucket,
            epoch,
            theme,
            charts: Vec::new(),
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
            orderbook_suspended: false,
            compare_anchor: None,
            compare_y: None,
            compare_orderbook_only: false,
            sync_manual_orders_on: false,
            sync_manual_strategy_targets_published: Vec::new(),
            hold_vacated: true,
            last_count_change: Instant::now(),
            compact_timer_armed: false,
            scroll: MoonVirtualListScrollHandle::new(),
        }
    }

    /// Record an open-chart count change (arrival, departure, or slot reuse), restarting the
    /// COMPRESS-compaction debounce interval that requires five stable seconds before collapsing
    /// empty slots.
    fn touch_count_change(&mut self) {
        self.last_count_change = Instant::now();
    }

    /// Hand the arrival stamp to slot `i`'s chart so its OWN PASS draws and paces the border flash.
    ///
    /// Nothing here notifies or schedules a timer. The earlier version repainted the stack at
    /// 10 Hz for 2.6 s (and before that, every vblank), and each repaint re-rendered every
    /// `ChartPanel` in the tab. The chart's own pass is driven by the vsync clock regardless, so
    /// the flash now costs presents instead of view renders — measured over seven live arrivals,
    /// `chart_render` did not move at all.
    ///
    /// Not free, and worth knowing before adding more of these: a present is a WINDOW present, so
    /// every sibling canvas in that window re-runs its own pass, text shaping included.
    fn flash_arrival(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.charts.get(i) else {
            return;
        };
        let at = entry.arrived_at;
        let accent = moon_ui::MoonPalette::active(cx).accent;
        entry
            .panel
            .clone()
            .update(cx, |p, _| p.set_arrival_pulse(Some(at), accent));
    }

    /// Arm the roughly 1 Hz COMPRESS-compaction debounce timer if needed. It ticks while charts
    /// exist and rearms itself in the callback. Composition/layout events start it, not render.
    /// Patterned after `MainChartStack::arm_idle_timer`.
    fn arm_compact_timer(&mut self, cx: &mut Context<Self>) {
        if self.compact_timer_armed || self.charts.is_empty() {
            return;
        }
        self.compact_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(Duration::from_secs(1)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    crate::diag::bump(&crate::diag::COMPACT_TICK);
                    this.compact_timer_armed = false;
                    this.compact_vacated_if_stable(cx);
                    this.arm_compact_timer(cx);
                })
                .is_ok()
            });
        })
        .detach();
    }

    /// In COMPRESS mode, remove retained empty slots after the open-chart count stays unchanged for
    /// `COMPACT_STABLE`, allowing remaining charts to expand into the freed space. A no-op in other
    /// modes and on custom tabs.
    fn compact_vacated_if_stable(&mut self, cx: &mut Context<Self>) {
        let (_, compress, _) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        if !compress || !self.hold_vacated || self.last_count_change.elapsed() < COMPACT_STABLE {
            return;
        }
        let before = self.charts.len();
        self.charts.retain(|e| !e.vacated);
        if self.charts.len() != before {
            self.sync_compare(cx);
            cx.notify();
        }
    }

    /// Configure whether a custom tab retains empty slots; disabling retention redistributes the
    /// remaining charts as soon as one closes.
    pub(crate) fn set_hold_vacated(&mut self, hold: bool) {
        self.hold_vacated = hold;
    }

    pub(crate) fn compare_anchor(&self) -> Option<(CoreId, String)> {
        self.compare_anchor.clone()
    }

    pub(crate) fn compare_orderbook_only(&self) -> bool {
        self.compare_orderbook_only
    }

    /// Restore comparison state from `charts.json` (anchor plus broom mode) and apply it.
    pub(crate) fn restore_compare(
        &mut self,
        anchor: Option<(CoreId, String)>,
        orderbook_only: bool,
        cx: &mut Context<Self>,
    ) {
        self.compare_anchor = anchor;
        self.compare_orderbook_only = orderbook_only;
        self.sync_compare(cx);
    }

    /// Synchronize comparison mode: consume lock/broom clicks (change or remove the anchor, move it
    /// left, or toggle order-book-only), then impose the shared Y window and flags on panels.
    /// Comparison is disabled in vertical orientation.
    fn sync_compare(&mut self, cx: &mut Context<Self>) {
        sync_compare(
            &mut self.charts,
            &mut self.compare_anchor,
            &mut self.compare_y,
            &mut self.compare_orderbook_only,
            self.layout_orientation,
            cx,
        );
        self.sync_manual_orders(cx);
    }

    /// Synchronize the runtime-only multi-core manual-order mode.
    ///
    /// The mode is available only while comparison has an anchor in a horizontal stack. Every panel
    /// receives the current stack candidates, but the click path performs the final safety filtering
    /// against exact market, chart bundle, connectivity, and selected manual strategy.
    fn sync_manual_orders(&mut self, cx: &mut Context<Self>) {
        let mut clicked = false;
        for e in &self.charts {
            if e.panel
                .update(cx, |p, _| p.take_sync_manual_orders_request())
            {
                clicked = true;
            }
        }
        let horizontal = self
            .layout_orientation
            .unwrap_or(StackOrientation::Vertical)
            .is_horizontal();
        let available = horizontal && self.compare_anchor.is_some() && self.charts.len() > 1;
        if clicked && available {
            self.sync_manual_orders_on = !self.sync_manual_orders_on;
        }
        if !available {
            self.sync_manual_orders_on = false;
        }
        let targets: Vec<(CoreId, String)> = if available {
            self.charts
                .iter()
                .filter(|e| !e.vacated)
                .map(|e| (e.core, e.market.clone()))
                .collect()
        } else {
            Vec::new()
        };
        if self.sync_manual_orders_on {
            self.sync_manual_strategy_targets_published = targets.clone();
            self.backend.update(cx, |b, _| {
                b.set_sync_manual_strategy_targets(targets.clone());
            });
        } else if !self.sync_manual_strategy_targets_published.is_empty() {
            let published = std::mem::take(&mut self.sync_manual_strategy_targets_published);
            self.backend.update(cx, |b, _| {
                b.clear_sync_manual_strategy_targets_if(&published);
            });
        }
        for e in &self.charts {
            e.panel.update(cx, |p, pcx| {
                p.set_sync_manual_orders(self.sync_manual_orders_on, targets.clone(), pcx)
            });
        }
    }

    pub(super) fn add_coin(
        &mut self,
        core: CoreId,
        market: &str,
        ttl_ms: f64,
        cx: &mut Context<Self>,
    ) {
        let (_, compress, _) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );

        // Extend the TTL when this chart already exists.
        if let Some(i) = self
            .charts
            .iter()
            .position(|e| e.core == core && e.market == market)
        {
            if self.charts[i].vacated {
                self.charts[i].vacated = false;
                self.charts[i].arrived_at = Instant::now();
                self.touch_count_change(); // The chart reopened, so reset the debounce interval.
                self.flash_arrival(i, cx);
            }
            let panel = self.charts[i].panel.clone();
            panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
            cx.notify();
            return;
        }

        // In COMPRESS mode, automatic AddToChart tabs place a new chart in the FIRST retained empty
        // slot without moving or resizing neighbors. Custom tabs set `hold_vacated = false` and do
        // not use this path.
        if compress && self.hold_vacated {
            if let Some(i) = self.charts.iter().position(|e| e.vacated) {
                self.charts[i].core = core;
                self.charts[i].market = market.to_string();
                self.charts[i].arrived_at = Instant::now();
                self.charts[i].vacated = false;
                self.touch_count_change(); // A chart reused an empty slot; reset the debounce.
                self.flash_arrival(i, cx);
                let panel = self.charts[i].panel.clone();
                panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
                cx.notify();
                return;
            }
        }

        // Append a new chart; render sorting raises pinned charts in FIT-stretch mode.
        let backend = self.backend.clone();
        let workspace_group = self.workspace_group.clone();
        let num = self.num;
        let bucket = self.bucket.clone();
        let epoch = self.epoch;
        let theme = self.theme.clone();
        let scale = self.scale;
        let panel = cx.new(|cx| {
            ChartPanel::new_addto(backend, workspace_group, num, bucket, epoch, theme, cx)
        });
        // Repaint the stack after any panel change, including a pin toggle: empty-panel pruning and
        // sorting pinned charts to the top happen during render.
        cx.observe(&panel, |this, _, cx| {
            this.prune_or_hold(cx);
            this.sync_compare(cx);
            cx.notify();
        })
        .detach();
        if scale.is_some() {
            panel.update(cx, |panel, pcx| panel.set_scale(scale, pcx));
        }
        // Honor the effective order-book state, including the suspension gate: a new chart must not
        // subscribe while the tab is suspended or the Order book checkbox is cleared.
        panel.update(cx, |panel, pcx| {
            panel.set_orderbook_enabled(self.effective_orderbook(), pcx)
        });
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
        panel.update(cx, |panel, pcx| {
            panel.set_liquidations_enabled(self.liquidations_enabled.unwrap_or(true), pcx)
        });
        {
            let cv = self.candle_view;
            panel.update(cx, |panel, pcx| panel.set_candle_view(cv, pcx));
        }
        if self.x_ppm.is_some() {
            let ppm = self.x_ppm;
            panel.update(cx, |panel, _| panel.set_default_x_ppm(ppm));
        }
        panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
        self.charts
            .push(ChartStackEntry::new(core, market.to_string(), panel));
        self.touch_count_change(); // A new chart appeared; reset the debounce interval.
        self.arm_compact_timer(cx); // Start the compaction timer if it is not already armed.
        self.flash_arrival(self.charts.len() - 1, cx); // Flash the border on the new slot.
        // In comparison mode, a new ticker immediately receives eligibility and the anchor's
        // shared Y window.
        self.sync_compare(cx);
        cx.notify();
    }

    /// Handle any empty chart panel, whether a chart was explicitly closed or its TTL expired.
    /// - **FIT-stretch / Scroll**: remove empty panels immediately; pinning and render-time sorting
    ///   provide stability.
    /// - **COMPRESS (Fit + pixels)**: retain the slot as `vacated`, preserving neighbor positions
    ///   and sizes. Clear ALL slots only when every slot is empty, restoring the default height.
    fn prune_or_hold(&mut self, cx: &App) -> bool {
        let (_, compress, _) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        // Remove empty panels immediately in FIT-stretch/Scroll or on a custom tab
        // (`hold_vacated = false`), redistributing neighbors. Only automatic AddToChart tabs in
        // COMPRESS mode retain a slot.
        if !compress || !self.hold_vacated {
            let before = self.charts.len();
            self.charts.retain(|e| e.panel.read(cx).pane_count() > 0);
            return self.charts.len() != before;
        }
        let mut changed = false;
        for e in self.charts.iter_mut() {
            let empty = e.panel.read(cx).pane_count() == 0;
            if empty != e.vacated {
                e.vacated = empty;
                changed = true;
            }
        }
        if !self.charts.is_empty() && self.charts.iter().all(|e| e.vacated) {
            self.charts.clear();
            changed = true;
        }
        if changed {
            // A chart disappeared and its slot became empty, changing the open count. Reset the
            // debounce so retained empty slots collapse only after five stable seconds.
            self.touch_count_change();
        }
        changed
    }

    pub(crate) fn pane_count(&self, cx: &App) -> usize {
        self.charts
            .iter()
            .filter(|entry| entry.panel.read(cx).pane_count() > 0)
            .count()
    }

    pub(crate) fn scale(&self) -> Option<f32> {
        self.scale
    }

    pub(crate) fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        apply_setting(&mut self.scale, pct, &self.charts, cx, |c, cx| {
            set_panels_scale(c, pct, cx)
        });
    }

    pub(crate) fn orderbook_enabled(&self) -> Option<bool> {
        self.orderbook_enabled
    }

    /// Effective order book = the user's checkbox (`None` → enabled) AND not focus-suspended.
    fn effective_orderbook(&self) -> bool {
        self.orderbook_enabled.unwrap_or(true) && !self.orderbook_suspended
    }

    /// Enable or disable order books for every stack chart (per window), honoring the suspension
    /// gate.
    pub(crate) fn set_orderbook_enabled(&mut self, enabled: Option<bool>, cx: &mut Context<Self>) {
        if self.orderbook_enabled == enabled {
            return;
        }
        self.orderbook_enabled = enabled;
        set_panels_orderbook_enabled(&self.charts, self.effective_orderbook(), cx);
        cx.notify();
    }

    /// Suspend or resume order-book subscriptions according to tab focus. Custom tabs unsubscribe
    /// after more than five seconds away and resume on return. This does not change the user's
    /// Order book checkbox.
    pub(crate) fn set_orderbook_suspended(&mut self, suspended: bool, cx: &mut Context<Self>) {
        if self.orderbook_suspended == suspended {
            return;
        }
        self.orderbook_suspended = suspended;
        set_panels_orderbook_enabled(&self.charts, self.effective_orderbook(), cx);
        self.backend.update(cx, |b, _| b.rebuild_orderbook_wanted());
        cx.notify();
    }

    pub(crate) fn show_zone(&self) -> Option<bool> {
        self.show_zone
    }

    /// Enable or disable the management-zone fill for every stack chart (per window).
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

    /// Set Cancel Buy / Panic Sell button positions for every stack chart (per window).
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

    pub(crate) fn price_axis_pos(&self) -> Option<PriceAxisPos> {
        self.price_axis_pos
    }

    /// Set the price-axis position (Left/Right/Hide) for every stack chart (per window).
    pub(crate) fn set_price_axis_pos(&mut self, pos: Option<PriceAxisPos>, cx: &mut Context<Self>) {
        apply_setting(&mut self.price_axis_pos, pos, &self.charts, cx, |c, cx| {
            set_panels_price_axis_pos(c, pos.unwrap_or_default(), cx)
        });
    }

    pub(crate) fn time_axis_visible(&self) -> Option<bool> {
        self.time_axis_visible
    }

    /// Set time-axis visibility for every stack chart (per window).
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

    /// Set line-label visibility for every stack chart (per window).
    pub(crate) fn set_line_labels(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.line_labels, show, &self.charts, cx, |c, cx| {
            set_panels_line_labels(c, show.unwrap_or(true), cx)
        });
    }

    pub(crate) fn cursor_labels(&self) -> Option<bool> {
        self.cursor_labels
    }

    /// Set crosshair-label visibility for every stack chart (per window).
    pub(crate) fn set_cursor_labels(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.cursor_labels, show, &self.charts, cx, |c, cx| {
            set_panels_cursor_labels(c, show.unwrap_or(true), cx)
        });
    }

    pub(crate) fn liquidations_enabled(&self) -> Option<bool> {
        self.liquidations_enabled
    }

    /// Enable or disable liquidation trades for every stack chart (per window).
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

    pub(crate) fn x_ppm(&self) -> Option<f32> {
        self.x_ppm
    }

    /// Store the window X scale for new charts; when `apply` is true, apply it to all open charts
    /// for Shift+middle-click synchronization. No charts exist yet during startup seeding.
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

    /// Set candle/trade display settings for every stack chart (per window).
    pub(crate) fn set_candle_view(
        &mut self,
        cfg: Option<moon_core::market::CandleViewCfg>,
        cx: &mut Context<Self>,
    ) {
        apply_setting(&mut self.candle_view, cfg, &self.charts, cx, |c, cx| {
            set_panels_candle_view(c, cfg, cx)
        });
    }

    /// Enable or disable automatic pinning on order for every stack chart (per window).
    pub(crate) fn set_auto_pin(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.auto_pin, on, &self.charts, cx, |c, cx| {
            set_panels_auto_pin(c, on.unwrap_or(false), cx)
        });
    }

    pub(crate) fn layout_orientation(&self) -> Option<StackOrientation> {
        self.layout_orientation
    }

    /// Change stack orientation (per window), rebuilding the current display.
    pub(crate) fn set_orientation(
        &mut self,
        orientation: Option<StackOrientation>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_orientation == orientation {
            return;
        }
        self.layout_orientation = orientation;
        // Orientation controls comparison availability; vertical mode disables the lock.
        self.sync_compare(cx);
        cx.notify();
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

    /// Apply the per-tab layout (mode plus separate Fit/Scroll heights) to this stack.
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
        // Slots are retained only in COMPRESS mode. Remove empty slots when switching modes so
        // FIT-stretch and Scroll do not show empty tiles.
        let (_, compress, _) = resolve_layout(mode, height_fit, height_scroll);
        if !compress {
            self.charts.retain(|e| !e.vacated);
        } else {
            self.arm_compact_timer(cx);
        }
        cx.notify();
    }

    /// Current stack ticker list `(core, market)` for custom-tab persistence, excluding empty or
    /// vacated slots.
    pub(crate) fn coins(&self, cx: &App) -> Vec<(CoreId, String)> {
        self.charts
            .iter()
            .filter(|e| !e.vacated && e.panel.read(cx).pane_count() > 0)
            .map(|e| (e.core, e.market.clone()))
            .collect()
    }

    /// Pin every stack chart so charts on a custom tab start pinned.
    pub(crate) fn pin_all(&mut self, cx: &mut Context<Self>) {
        for e in &self.charts {
            e.panel.update(cx, |p, pcx| p.ensure_pinned(pcx));
        }
    }

    pub(super) fn set_scene_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        for entry in &self.charts {
            entry
                .panel
                .update(cx, |panel, _| panel.set_scene_visible(visible));
        }
    }

    fn sync_stack_visible_ordered(
        &mut self,
        range: Range<usize>,
        render_order: &[usize],
        cx: &mut Context<Self>,
    ) {
        for (ix, entry) in self.charts.iter().enumerate() {
            let visible = !entry.vacated
                && render_order
                    .iter()
                    .enumerate()
                    .any(|(display_ix, real_ix)| *real_ix == ix && range.contains(&display_ix));
            entry
                .panel
                .update(cx, |panel, _| panel.set_scene_visible(visible));
        }
    }

    pub(crate) fn close_all_panes(&mut self, cx: &mut Context<Self>) {
        for entry in &self.charts {
            entry
                .panel
                .update(cx, |panel, pcx| panel.close_all_panes(pcx));
        }
        self.charts.clear();
        cx.notify();
    }
}

impl Render for AddChartStack {
    /// Renders every Add/Custom chart as a labelled, guttered stack tile.
    ///
    /// These stacks have no fullscreen mode, so their shared card helper always retains the
    /// separator and never needs the Main stack's position note.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = moon_ui::MoonPalette::active(cx);
        if self.charts.is_empty() {
            // Use an opaque background: a detached window has `Root=NoFill` and no own pass, so
            // without this background the white window backing would show through the logo.
            return div()
                .size_full()
                .bg(rgb(palette.chart_bg))
                .flex()
                .items_center()
                .justify_center()
                .child(crate::design::logo_glow_sized(cx, 220.0))
                .into_any_element();
        }

        // Stack layout comes from the tab (FIT/SCROLL/COMPRESS plus height), otherwise the global
        // default. IMPORTANT: chart slots are TRANSPARENT. The own pass (combo/order book) is a
        // `GpuCanvasLayer::UnderScene` layer beneath the scene; any opaque `.bg()` above the slot
        // covers it. The border serves as the separator.
        let (scroll, compress, cfg_h) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        // Cluster pinned charts at the top ONLY outside COMPRESS, where slots are positionally
        // stable. This is a read-only render order; render does not mutate `self.charts`.
        let mut render_order: Vec<usize> = (0..self.charts.len()).collect();
        if !compress {
            render_order.sort_by_key(|&ix| !self.charts[ix].panel.read(cx).is_pinned());
        }
        let count = self.charts.len();
        let border = rgb(palette.border);
        let base_id = format!("add-chart-stack-{}", self.num);
        let horizontal = self
            .layout_orientation
            .unwrap_or(StackOrientation::Vertical)
            .is_horizontal();
        let entity = cx.entity();
        let p = palette;
        let title_size = crate::design::t_body(cx);
        let visible_order = render_order.clone();
        let panel_order = render_order.clone();
        let tile_order = render_order.clone();
        let role_order = render_order;
        let on_visible_range = cx.processor(move |this, range: Range<usize>, _window, cx| {
            this.sync_stack_visible_ordered(range, &visible_order, cx);
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
            // An empty retained COMPRESS slot maps to `None`, so render shows a transparent tile.
            move |s, ix| {
                let Some(&real_ix) = panel_order.get(ix) else {
                    return None;
                };
                s.charts
                    .get(real_ix)
                    .filter(|e| !e.vacated)
                    .map(|e| e.panel.clone())
            },
            move |s, ix, panel, size, flex, min_w, horizontal, border, _ent| {
                let real_ix = tile_order.get(ix).copied().unwrap_or(ix);
                let (id, label) = match s.charts.get(real_ix) {
                    Some(e) => (
                        format!("add-chart-stack-tile-{}-{}-{}", s.num, e.core, e.market),
                        e.market.clone(),
                    ),
                    None => (
                        format!("add-chart-stack-tile-{}-{ix}", s.num),
                        "Chart".to_string(),
                    ),
                };
                // Add/Custom stacks have no fullscreen mode and need no position note, since each
                // tile already carries its own market name. The gutter still goes through the
                // shared decision: a tab holding a single chart — the ordinary state of a fresh
                // Add tab opened by one detect — would otherwise carry a permanent empty strip
                // under it.
                let mut tile = chart_stack_card(
                    SharedString::from(id),
                    label,
                    panel,
                    p,
                    border,
                    title_size,
                    tile_gutter(false, s.charts.len()),
                    None,
                );
                // Fill width/height across the axis. Along the axis, use flex with a cap (COMPRESS
                // down to size), fixed size without flex, or stretch (FIT). Horizontal uses the X
                // axis (width); vertical uses the Y axis (height).
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
                    // Fixed WITHOUT shrinking (`min = max = v`): in SCROLL, tiles overflow the
                    // container and activate scrolling.
                    tile = if horizontal {
                        tile.w(px(v)).min_w(px(v))
                    } else {
                        tile.h(px(v)).min_h(px(v))
                    };
                }
                // The arrival flash is NOT a child here any more: it is drawn by the chart's own
                // pass (`chartdx::render_state`, `set_arrival_pulse`). A GPUI element would have to
                // be repainted, and repainting this stack re-renders every `ChartPanel` in it.
                tile.into_any_element()
            },
            move |s, ix| {
                let real_ix = role_order.get(ix).copied().unwrap_or(ix);
                compare_role(
                    &s.charts,
                    &s.compare_anchor,
                    s.compare_orderbook_only,
                    real_ix,
                )
            },
            Some(Box::new(on_visible_range)),
        )
    }
}
