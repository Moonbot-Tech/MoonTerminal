//! Central `DockArea` chart panel with an own-pass GPU renderer and input handling. As a dock panel
//! it can be detached into a window. Main markets come from focus or `Backend.open_request`,
//! detection ingestion populates numbered AddToChart panels, and manual selection or restoration
//! populates numbered Custom panels.
//!
//! `ChartEngine.canvas()` supplies a GPUI `gpu_canvas` below the scene, rendering composed layers
//! directly into GPUI's backbuffer without readback; prepare updates the view and uploads new ticks.
//! Axis and readout text uses retained `gpu_canvas` text, while crosshair lines use the native
//! chartdx cursor layer.
//!
//! Responsibilities are split between state, lifecycle, constructors, and traits here; geometry and
//! hit-testing in [`geom`]; market refcounts and TTL/auto-live timers in [`refs`]; manual trading and
//! order dragging in [`trade`]; rendering in [`render`]; and slot wheel, mouse, and hover handling in
//! [`render_input`].

mod figures;
mod geom;
mod news;
mod refs;
mod render;
mod render_input;
mod trade;
mod warn;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::{MoonBackgroundPolicy, Panel, PanelEvent};

use rust_i18n::t;

use crate::Backend;
use crate::chartdx::ChartEngine;
use crate::chartdx::input;
use moon_chart::container::ContainerKind;
use moon_chart::paint::now_unix_ms;
use moon_core::config::{ChartBucket, ChartTheme, OrdersStyleSet};
use moon_core::session::CoreId;

use trade::{OrderDrag, OrderHoverKey, PendingOrderDrag};

#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(windows)]
fn monitor_refresh_hz() -> u32 {
    unsafe {
        let mut mode = DEVMODEW::default();
        mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        if EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut mode).as_bool()
            && mode.dmDisplayFrequency > 1
        {
            mode.dmDisplayFrequency
        } else {
            60
        }
    }
}

#[cfg(not(windows))]
fn monitor_refresh_hz() -> u32 {
    60
}

fn chart_bootstrap_present_rate_hz() -> f32 {
    let refresh = monitor_refresh_hz().clamp(30, 360);
    refresh as f32
}

const DEBUG_HISTORY_FILL_SPAN_MS: i64 = 3_600_000;

#[derive(Clone, PartialEq)]
struct ChartSettingsSig {
    theme: ChartTheme,
    orders: OrdersStyleSet,
    follow: bool,
}

fn chart_settings_sig(backend: &Backend) -> ChartSettingsSig {
    let effective = backend.preview.as_ref().unwrap_or(&backend.config);
    ChartSettingsSig {
        theme: effective.chart_theme().clone(),
        orders: effective.orders.clone(),
        follow: backend.follow,
    }
}

pub struct ChartPanel {
    backend: Entity<Backend>,
    chart: ChartEngine,
    input: input::ChartInput,
    market: Option<String>,
    /// Price scale for this panel, where `None` means Auto. It is per tab rather than global, edited
    /// by the active-tab toolbar or detached-window header and applied during rendering.
    scale: Option<f32>,
    /// Whether this panel shows order books. This per-window/tab setting is applied through the
    /// engine's `set_orderbook_enabled`; it defaults to enabled.
    orderbook_enabled: bool,
    /// Whether this panel renders liquidation trades. This per-window/tab setting is applied through
    /// the engine's `set_liquidations_enabled`; it defaults to enabled.
    liquidations_enabled: bool,
    /// Per-window/tab candle and trade display settings. `None` follows the global
    /// `layout.candle_view` default; the effective value is applied during rendering.
    candle_view: Option<moon_core::market::CandleViewCfg>,
    /// Whether to dim-fill the reserved control zone when zones are separate and the order book is
    /// hidden. This is per window/tab, applied during rendering, and enabled by default.
    show_zone: bool,
    /// Whether a successful long or short order automatically pins its chart. Per window/tab and
    /// disabled by default.
    auto_pin: bool,
    /// Per-window/tab positions of the market-action buttons in the chart area; defaults to Right.
    cancel_buy_pos: crate::persistence::chart_persist::ChartBtnPos,
    panic_sell_pos: crate::persistence::chart_persist::ChartBtnPos,
    /// Per-window/tab price-axis position. It is applied through the engine's
    /// `set_price_axis_pos` and affects layout and hit-testing; defaults to Left.
    price_axis_pos: crate::persistence::chart_persist::PriceAxisPos,
    /// Per-window/tab time-axis visibility. It is applied through the engine's
    /// `set_time_axis_visible` and affects plot height in layout and hit-testing; enabled by default.
    time_axis_visible: bool,
    /// Whether order-line labels are shown for this window/tab; enabled by default.
    line_labels: bool,
    /// Whether crosshair cursor-readout labels are shown; enabled by default.
    cursor_labels: bool,
    /// Number of the containing AddToChart or Custom panel, or `None` for Main.
    num: Option<u32>,
    /// Markets retained by this panel. Backend aggregates ownership counts across panels to derive
    /// `desired`.
    registered_markets: HashSet<(CoreId, String)>,
    /// Markets for which this panel retains an order-book reference: equal to `registered_markets`
    /// while the book is enabled and empty otherwise. Backend derives `desired_orderbook` from them.
    registered_orderbook: HashSet<(CoreId, String)>,
    /// Backend market-reference epoch captured when this panel was constructed. If it ever changes,
    /// `sync_market_ref_epoch` clears this panel's local ownership sets. The current runtime
    /// initializes the epoch once and neither advances it nor clears the registry after startup.
    market_ref_epoch: u64,
    /// Latest market-data signature copied during backend observation or explicit data sync.
    /// Notification throttling uses `last_axis_notify_data_sig` instead.
    data_sig: u64,
    /// UI settings applied during rendering. Cached dock panels are no longer awakened by every
    /// top-down Shell render, so a changed signature must notify this panel directly.
    settings_sig: ChartSettingsSig,
    /// Whether to present smoothly at vsync as a focused chart instead of adapting to observed data.
    /// Main starts true; numbered AddToChart and Custom panels start false.
    fast: bool,
    /// Whether the panel is present in this window's GPUI scene. Hidden tabs skip CPU data prepare
    /// because their `gpu_canvas` will not be queried or drawn.
    scene_visible: bool,
    /// Whether the panel is a Main-stack tile. The outer ScrollBox owns wheel events in this mode;
    /// fullscreen and numbered AddToChart or Custom panels retain normal chart zoom.
    main_stack_scroll: bool,
    /// Last market-data signature that passed the throttled axis-overlay notification gate.
    last_axis_notify_data_sig: u64,
    /// Whether comparison is eligible because the tab is horizontal, enabling the lock button.
    compare_eligible: bool,
    /// Whether this chart is the comparison anchor with the active lock and leading price scale.
    is_compare_anchor: bool,
    /// Pending lock-button request consumed by the stack's observer.
    compare_lock_pending: bool,
    /// Comparison anchor's imposed Y window as `(center, range)`, or `None` for an independent Y
    /// range. Applied through the engine's `set_locked_y` during rendering.
    locked_y: Option<(f32, f32)>,
    /// Book-only mode for anchor peers: hide the plot and price axis while keeping the order book.
    orderbook_only: bool,
    /// Pending broom-button request consumed by the stack observer to toggle anchor peers.
    compare_broom_pending: bool,
    /// Tab-level broom state supplied by the stack to highlight the anchor's broom button.
    compare_broom_on: bool,
    /// Ghost-cursor handles for comparison peers. With the lock active, the stack gives each panel
    /// weak handles to the other engines; mouse movement sends them the cursor price without a GPUI
    /// notification because each peer schedules its own present. Empty means comparison is inactive.
    ghost_peers: Vec<crate::chartdx::ChartGhostCursor>,
    view_dirty: bool,
    last_adaptive_notify_at: Option<Instant>,
    /// Last window scale factor recorded during rendering. The data-prepare path has no `Window`, so
    /// it reuses this value between infrequent DPI changes.
    last_ppp: f32,
    /// Whether a one-shot timer is armed for the nearest unpinned pane TTL deadline in a numbered
    /// AddToChart or Custom panel. Custom panes are normally pinned after population. Time-based
    /// expiry must not depend on backend data observations.
    ttl_timer_armed: bool,
    /// Whether the one-shot auto-live timer is armed after a pan. Prepare does not tick while idle,
    /// so the own-pass camera must return to live on a wall-clock timer.
    auto_live_timer_armed: bool,
    order_drag: Option<OrderDrag>,
    pending_order_drag: Option<PendingOrderDrag>,
    order_hover: Option<OrderHoverKey>,
    /// Point of the most recent order-line hover hit-test. The Delphi-style movement threshold keeps
    /// subpixel raw mouse movement from scanning the lines again.
    order_hover_probe: Option<(f32, f32)>,
    /// News marks for this chart's coin plus their hover state; see [`news`].
    news: news::NewsState,
    warn: warn::WarnState,
    /// Figure-drawing state for this panel: pencil draft, hover, and drag.
    fig_draft: Option<figures::FigDraft>,
    /// Screen point where the latest draft node was placed. Releasing sufficiently far away
    /// distinguishes a drag-release gesture from a click and completes the figure.
    fig_draw_down: Option<(f32, f32)>,
    fig_hover: Option<u64>,
    fig_drag: Option<figures::FigDrag>,
    /// Whether right-button down opened an order context menu, suppressing the matching button-up
    /// so the Main-stack parent cannot interpret it as a fullscreen toggle.
    suppress_rmb_up: bool,
    /// Unix-millisecond time of the last pane close-button action. After a fullscreen chart closes,
    /// another chart takes its place and the OS can deliver the second click of the double-click to
    /// that new chart, accidentally placing an order. Placement clicks within `ORDER_SUPPRESS_MS`
    /// are ignored; zero means no recent close.
    last_pane_close_ms: f64,
    focus: FocusHandle,
}

/// Milliseconds to suppress order placement after closing a pane, covering a likely second click on
/// the replacement fullscreen chart.
const ORDER_SUPPRESS_MS: f64 = 500.0;

impl ChartPanel {
    fn sync_orders_from_backend_notify(&mut self, cx: &mut Context<Self>) -> bool {
        crate::diag::bump(&crate::diag::CHART_ORDER_SYNC);
        {
            let b = self.backend.read(cx);
            self.chart.sync_orders_if_visible(&b.session, false);
        }
        // Pencil and selection state changes in the tab strip or through hotkeys reach this panel
        // through the backend observer; propagate them into the figure engine.
        self.sync_fig_visual(cx);
        // News arrives on the same observer; the signature gate makes the common case a no-op. Its
        // gems are own-pass and present themselves, so a news item never wakes the GPUI scene — an
        // open hover card catches up on the next repaint.
        self.sync_news_marks(cx);
        self.sync_warn_marks(cx);
        self.clear_settled_order_drag_preview(cx) && self.apply_order_visual(cx)
    }

    pub fn new(
        backend: Entity<Backend>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_main(backend, focus_open, epoch, theme, cx)
    }

    pub fn new_main(
        backend: Entity<Backend>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut chart = ChartEngine::new(epoch, theme);
        chart.set_market_source(Some(backend.read(cx).session.market_source()));
        chart.set_figures_store(backend.read(cx).figures.clone());
        let market_ref_epoch = backend.read(cx).chart_market_refs_epoch;
        let mut market = None;
        let mut registered_markets = HashSet::new();
        let mut registered_orderbook = HashSet::new();
        if let Some((core, m)) = focus_open {
            chart.open(core, &m);
            market = Some(m.clone());
            registered_markets.insert((core, m.clone()));
            // The order book starts enabled, so retain its demand reference immediately.
            registered_orderbook.insert((core, m.clone()));
            backend.update(cx, |b, _| {
                b.retain_chart_market(core, &m);
                b.retain_chart_orderbook(core, &m);
            });
        }
        let settings_sig = {
            let b = backend.read(cx);
            chart_settings_sig(&b)
        };
        // Notify for setting changes and infrequent axis text. Frequent market data bypasses GPUI
        // notification because `gpu_canvas.frame()` reads MarketDataSource directly. A local
        // one-shot timer handles time-based pane TTL independently of backend observations.
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::CHART_OBS_FIRE);
            let now = Instant::now();
            let (sig, settings_sig) = {
                let b = backend.read(cx);
                (
                    this.chart.notify_signature(&b.session),
                    chart_settings_sig(&b),
                )
            };
            if settings_sig != this.settings_sig {
                this.settings_sig = settings_sig;
                this.view_dirty = true;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            if this.sync_orders_from_backend_notify(cx) {
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            this.data_sig = sig;
            // Throttle notification because `gpu_canvas` presents data itself. GPUI notification is
            // needed only for the top-down axis overlay and also wakes Orders, so cap it at 4 Hz for
            // fast panels and 1 Hz for numbered AddToChart and Custom panels.
            // `gpu_canvas.frame()` handles frequent GPU data and state updates without marking GPUI
            // dirty.
            let floor = if this.fast {
                Duration::from_millis(250)
            } else {
                Duration::from_millis(1_000)
            };
            let notify_due = this
                .last_adaptive_notify_at
                .is_none_or(|last| now.duration_since(last) >= floor);
            if sig != this.last_axis_notify_data_sig && notify_due {
                this.last_axis_notify_data_sig = sig;
                this.last_adaptive_notify_at = Some(now);
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();
        let chart_handle = chart.data_handle();
        backend.update(cx, |b, _| b.register_chart_consumer(chart_handle));
        cx.on_release(|this, cx| {
            this.release_all_market_refs(cx);
        })
        .detach();
        Self {
            backend,
            chart,
            input: input::ChartInput::default(),
            market,
            scale: None,
            orderbook_enabled: true,
            liquidations_enabled: true,
            candle_view: None,
            show_zone: true,
            auto_pin: false,
            cancel_buy_pos: Default::default(),
            panic_sell_pos: Default::default(),
            price_axis_pos: Default::default(),
            time_axis_visible: true,
            line_labels: true,
            cursor_labels: true,
            num: None,
            registered_markets,
            registered_orderbook,
            market_ref_epoch,
            data_sig: 0,
            settings_sig,
            fast: true,
            scene_visible: false,
            main_stack_scroll: false,
            last_axis_notify_data_sig: u64::MAX,
            compare_eligible: false,
            is_compare_anchor: false,
            compare_lock_pending: false,
            locked_y: None,
            orderbook_only: false,
            compare_broom_pending: false,
            compare_broom_on: false,
            ghost_peers: Vec::new(),
            view_dirty: true,
            last_adaptive_notify_at: None,
            last_ppp: 1.0,
            ttl_timer_armed: false,
            auto_live_timer_armed: false,
            order_drag: None,
            pending_order_drag: None,
            order_hover: None,
            order_hover_probe: None,
            news: news::NewsState::default(),
            warn: warn::WarnState::default(),
            fig_draft: None,
            fig_draw_down: None,
            fig_hover: None,
            fig_drag: None,
            suppress_rmb_up: false,
            last_pane_close_ms: 0.0,
            focus: cx.focus_handle(),
        }
    }

    pub fn active_target(&self) -> Option<(CoreId, String)> {
        self.chart.active_target()
    }

    /// Builds numbered AddToChart or Custom panel `num`. Detections populate AddToChart panels and
    /// manual selection or restoration populates Custom panels through [`Self::add_coin`]. It needs
    /// no `Window`, allowing deferred restoration of detached windows from data alone.
    pub fn new_addto(
        backend: Entity<Backend>,
        num: u32,
        bucket: ChartBucket,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut chart = ChartEngine::new_kind(epoch, theme, ContainerKind::Chart { num, bucket });
        chart.set_market_source(Some(backend.read(cx).session.market_source()));
        chart.set_figures_store(backend.read(cx).figures.clone());
        let market_ref_epoch = backend.read(cx).chart_market_refs_epoch;
        let settings_sig = {
            let b = backend.read(cx);
            chart_settings_sig(&b)
        };
        cx.observe(&backend, |this, backend, cx| {
            let now = Instant::now();
            let (sig, settings_sig) = {
                let b = backend.read(cx);
                (
                    this.chart.notify_signature(&b.session),
                    chart_settings_sig(&b),
                )
            };
            if settings_sig != this.settings_sig {
                this.settings_sig = settings_sig;
                this.view_dirty = true;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            if this.sync_orders_from_backend_notify(cx) {
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            this.data_sig = sig;
            // Numbered AddToChart and Custom panels are background charts, so cap GPUI notification
            // and their top-down Orders redraw at 1 Hz. `gpu_canvas.frame()` handles frequent GPU
            // data and state without notification, while the local TTL timer performs time-based
            // pruning of unpinned panes.
            let notify_due = this
                .last_adaptive_notify_at
                .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(1_000));
            if sig != this.last_axis_notify_data_sig && notify_due {
                this.last_axis_notify_data_sig = sig;
                this.last_adaptive_notify_at = Some(now);
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();
        let chart_handle = chart.data_handle();
        backend.update(cx, |b, _| b.register_chart_consumer(chart_handle));
        cx.on_release(|this, cx| {
            this.release_all_market_refs(cx);
        })
        .detach();
        Self {
            backend,
            chart,
            input: input::ChartInput::default(),
            market: None,
            scale: None,
            orderbook_enabled: true,
            liquidations_enabled: true,
            candle_view: None,
            show_zone: true,
            auto_pin: false,
            cancel_buy_pos: Default::default(),
            panic_sell_pos: Default::default(),
            price_axis_pos: Default::default(),
            time_axis_visible: true,
            line_labels: true,
            cursor_labels: true,
            num: Some(num),
            registered_markets: HashSet::new(),
            registered_orderbook: HashSet::new(),
            market_ref_epoch,
            data_sig: 0,
            settings_sig,
            fast: false,
            scene_visible: false,
            main_stack_scroll: false,
            last_axis_notify_data_sig: u64::MAX,
            compare_eligible: false,
            is_compare_anchor: false,
            compare_lock_pending: false,
            locked_y: None,
            orderbook_only: false,
            compare_broom_pending: false,
            compare_broom_on: false,
            ghost_peers: Vec::new(),
            view_dirty: true,
            last_adaptive_notify_at: None,
            last_ppp: 1.0,
            ttl_timer_armed: false,
            auto_live_timer_armed: false,
            order_drag: None,
            pending_order_drag: None,
            order_hover: None,
            order_hover_probe: None,
            news: news::NewsState::default(),
            warn: warn::WarnState::default(),
            fig_draft: None,
            fig_draw_down: None,
            fig_hover: None,
            fig_drag: None,
            suppress_rmb_up: false,
            last_pane_close_ms: 0.0,
            focus: cx.focus_handle(),
        }
    }

    /// Returns the number of open chart panes for the tab's count badge.
    pub fn pane_count(&self) -> usize {
        self.chart.pane_count()
    }

    /// Returns whether any pane is pinned. The stack sorts pinned charts first, and
    /// `prune_ttl` skips them.
    pub fn is_pinned(&self) -> bool {
        (0..self.chart.pane_count()).any(|i| self.chart.pane_pinned(i))
    }

    /// Idempotently pins every pane, as required for charts restored into a custom tab. Pinning
    /// cancels TTL auto-close, so the deadline timer is re-armed for any remaining unpinned pane.
    pub fn ensure_pinned(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for i in 0..self.chart.pane_count() {
            if !self.chart.pane_pinned(i) && self.chart.toggle_pane_pin(i) {
                changed = true;
            }
        }
        if changed {
            self.view_dirty = true;
            self.arm_ttl_timer(cx);
            cx.notify();
        }
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub fn debug_data_handle(&self) -> crate::chartdx::ChartDataHandle {
        self.chart.data_handle()
    }

    /// Mark whether this panel is part of the currently rendered GPUI scene. This only gates
    /// CPU-side data prepare; the `gpu_canvas` element lifetime is still owned by GPUI scene replay.
    pub fn set_scene_visible(&mut self, visible: bool) {
        self.scene_visible = visible;
        self.chart.set_scene_visible(visible);
    }

    /// Main stack mode scrolls the list of chart tiles. The chart itself must not consume wheel
    /// events there, otherwise the outer ScrollBox cannot move.
    pub fn set_main_stack_scroll(&mut self, enabled: bool) {
        self.main_stack_scroll = enabled;
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
        if self.clear_settled_order_drag_preview(cx) && self.apply_order_visual(cx) {
            cx.notify();
        }
    }

    /// Sets this panel's price scale, where `None` means Auto. Rendering applies it through the
    /// engine's `set_scale`.
    pub fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        if self.scale != pct {
            self.scale = pct;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Enables or disables this window/tab's order book. Rendering applies the engine flag, and the
    /// backend order-book reference is synchronized for demand-driven subscription.
    pub fn set_orderbook_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.orderbook_enabled != enabled {
            self.orderbook_enabled = enabled;
            self.view_dirty = true;
            self.sync_orderbook_refs(cx);
            cx.notify();
        }
    }

    /// Enables or disables liquidation trades for this window/tab. Rendering applies the engine
    /// flag, whose change resets the composed layer to add or remove liquidation crosses.
    pub fn set_liquidations_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.liquidations_enabled != enabled {
            self.liquidations_enabled = enabled;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Sets this window/tab's candle and trade display settings. `None` uses the global default;
    /// rendering applies the effective value through the engine's `set_candle_view`.
    pub fn set_candle_view(
        &mut self,
        cfg: Option<moon_core::market::CandleViewCfg>,
        cx: &mut Context<Self>,
    ) {
        if self.candle_view != cfg {
            self.candle_view = cfg;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Handles Shift+middle-click by requesting Moonbot-style time-axis synchronization within this
    /// OS window. Backend carries the hovered pane's scale with the window handle; the tab strip or
    /// detached host applies it to that window's stacks and persists it in group layout or the
    /// detached tab specification.
    pub(super) fn sync_x_scale_window(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let Some(ppm) = self.chart.pane_x_ppm(self.input.hovered_pane) else {
            return false;
        };
        let handle = window.window_handle();
        self.backend.update(cx, |b, bcx| {
            b.chart_x_sync = Some((handle, ppm));
            b.chart_x_sync_rev = b.chart_x_sync_rev.wrapping_add(1);
            bcx.notify();
        });
        true
    }

    /// Sets the default time-axis scale for new panes, supplied by the owning stack. `None` selects
    /// the built-in time-window default.
    pub fn set_default_x_ppm(&mut self, ppm: Option<f32>) {
        self.chart.set_default_x_ppm(ppm);
    }

    /// Applies a synchronized time-axis scale to every open pane in this panel.
    pub fn apply_x_ppm(&mut self, ppm: f32, cx: &mut Context<Self>) {
        if self.chart.set_x_ppm_all(ppm, now_unix_ms()) {
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Controls the per-window/tab fill over the reserved control zone when the book is hidden.
    /// This affects only the UI overlay, not the engine.
    pub fn set_show_zone(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_zone != show {
            self.show_zone = show;
            cx.notify();
        }
    }

    /// Sets the per-window/tab auto-pin flag. [`Self::try_place_order_click`] performs the pin after
    /// a successful order.
    pub fn set_auto_pin(&mut self, on: bool, _cx: &mut Context<Self>) {
        self.auto_pin = on;
    }

    /// Sets the per-window/tab positions of Cancel Buy and Panic Sell in the chart area.
    pub fn set_action_btn_pos(
        &mut self,
        cancel_buy: crate::persistence::chart_persist::ChartBtnPos,
        panic_sell: crate::persistence::chart_persist::ChartBtnPos,
        cx: &mut Context<Self>,
    ) {
        if self.cancel_buy_pos != cancel_buy || self.panic_sell_pos != panic_sell {
            self.cancel_buy_pos = cancel_buy;
            self.panic_sell_pos = panic_sell;
            cx.notify();
        }
    }

    /// Sets comparison eligibility for a horizontal tab, controlling lock-button visibility.
    pub fn set_compare_eligible(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.compare_eligible != on {
            self.compare_eligible = on;
            cx.notify();
        }
    }

    /// Marks this chart as the stack-controlled comparison anchor and highlights its lock.
    pub fn set_compare_anchor(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.is_compare_anchor != on {
            self.is_compare_anchor = on;
            cx.notify();
        }
    }

    /// Records a lock-button request and notifies the stack observer that consumes it.
    fn request_compare_lock(&mut self, cx: &mut Context<Self>) {
        self.compare_lock_pending = true;
        cx.notify();
    }

    /// Takes and clears the pending lock-button request for the stack observer.
    pub fn take_compare_lock_request(&mut self) -> bool {
        std::mem::take(&mut self.compare_lock_pending)
    }

    /// Records a broom-button request for the stack observer to apply to anchor peers.
    fn request_compare_broom(&mut self, cx: &mut Context<Self>) {
        self.compare_broom_pending = true;
        cx.notify();
    }

    /// Takes and clears the pending broom-button request.
    pub fn take_compare_broom_request(&mut self) -> bool {
        std::mem::take(&mut self.compare_broom_pending)
    }

    /// Enables book-only broom mode: rendering hides the plot and price axis and expands the book.
    pub fn set_orderbook_only(&mut self, only: bool, cx: &mut Context<Self>) {
        if self.orderbook_only != only {
            self.orderbook_only = only;
            self.view_dirty = true;
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
    pub fn set_arrival_pulse(&mut self, at: Option<std::time::Instant>, accent: u32) {
        self.chart.set_arrival_pulse(at, accent);
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
        self.release_market_refs_except(Some((core, market)), cx);
        self.chart.push_auto(core, market, ttl_ms, now_unix_ms());
        self.retain_market_ref(core, market, cx);
        self.view_dirty = true;
        self.arm_ttl_timer(cx);
        // Notify the panel itself. ChartTabs renders an attached strip tab, but a detached panel
        // lives in another window and would not display the newly detected market without this.
        cx.notify();
    }

    /// Closes a pane through its close button. When no remaining pane uses its market, release this
    /// panel's market ownership and order-book demand; backend refcounts decide whether the market
    /// remains desired by another panel.
    fn remove_pane(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some((core, market)) = self.chart.remove_pane(idx) else {
            return;
        };
        // The replacement chart becomes fullscreen; suppress the second close-button click so it
        // cannot place an order on that chart.
        self.last_pane_close_ms = moon_chart::paint::now_unix_ms();
        self.view_dirty = true;
        if !self.chart.uses_market(core, &market) {
            self.release_market_ref(core, &market, cx);
        }
        cx.notify();
    }

    /// Toggles a pane's pin. Pinning exempts it from TTL auto-close; unpinning restores its deadline,
    /// which may already have elapsed, so the deadline timer is re-armed.
    fn toggle_pin(&mut self, idx: usize, cx: &mut Context<Self>) {
        if self.chart.toggle_pane_pin(idx) {
            self.view_dirty = true;
            self.arm_ttl_timer(cx);
            cx.notify();
        }
    }

    /// Closes every market in this panel and releases their market and order-book references.
    pub fn close_all_panes(&mut self, cx: &mut Context<Self>) {
        let removed = self.chart.clear_panes();
        if removed.is_empty() {
            return;
        }
        self.view_dirty = true;
        for (core, market) in removed {
            self.release_market_ref(core, &market, cx);
        }
        cx.notify();
    }

    fn mark_input_changed(&mut self, cx: &mut Context<Self>) {
        self.chart.sync_follow_from_views();
        let follow = self.chart.follow();
        self.backend.update(cx, |b, bcx| {
            if b.follow != follow {
                b.follow = follow;
                bcx.notify();
            }
        });
        self.view_dirty = true;
        // If the input moved a pane into manual mode, arm its wall-clock return to live.
        self.arm_auto_live_timer(cx);
    }

    /// Returns the tab caption. Numbered AddToChart and Custom panels use `N · market`, falling back
    /// to the localized numbered label; Main uses the active market and then `Main`. Group and core
    /// belong to ChartTabs or DetachedChartHost and are not available here.
    pub fn title_text(&self) -> String {
        let market = self
            .chart
            .active_market()
            .filter(|m| !m.is_empty())
            .or_else(|| self.market.clone());
        if let Some(n) = self.num {
            return match market {
                Some(m) => format!("{n} · {m}"),
                None => t!("chartwin.tab_title", n = n).to_string(),
            };
        }
        market.unwrap_or_else(|| "Main".into())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub fn debug_fill_history_to_capacity(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((core, market)) = self.chart.active_target() else {
            log::warn!("debug history fill: current chart has no active market");
            return false;
        };
        let now_ms = now_unix_ms();
        log::info!(
            "debug history fill: requesting core={} market={market} span_ms={DEBUG_HISTORY_FILL_SPAN_MS}",
            moon_core::feed::core_label(core)
        );
        let filled = self
            .backend
            .read(cx)
            .session
            .diag_fill_market_history_to_capacity(
                core,
                &market,
                now_ms.round() as i64,
                DEBUG_HISTORY_FILL_SPAN_MS,
            );
        if !filled {
            log::warn!(
                "debug history fill: failed core={} market={market}",
                moon_core::feed::core_label(core)
            );
            return false;
        }
        self.chart.force_history_reupload();
        self.view_dirty = true;
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
        log::info!(
            "debug history fill: force reupload core={} market={market}",
            moon_core::feed::core_label(core)
        );
        true
    }
}

impl EventEmitter<PanelEvent> for ChartPanel {}
impl Focusable for ChartPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ChartPanel {
    fn panel_name(&self) -> &'static str {
        "Chart"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.title_text())
    }
    fn background_policy(&self, _cx: &App) -> MoonBackgroundPolicy {
        MoonBackgroundPolicy::NoFill
    }
}
