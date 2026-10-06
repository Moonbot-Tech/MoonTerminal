//! Central `DockArea` chart panel with an own-pass GPU renderer and input handling. As a dock panel
//! it can be detached into a window. Main markets come from focus or `Backend::open_on_main` through
//! its atomic `open_main_request`, detection ingestion populates numbered AddToChart panels, and
//! manual selection or restoration populates numbered Custom panels.
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

mod arb_open;
mod click_series;
mod figures;
mod filter_headers;
mod geom;
mod market_actions;
mod news;
mod refs;
mod render;
mod render_input;
mod report_trades;
mod ruler;
pub(crate) mod shot;
#[cfg(test)]
mod tests;
mod trace_lines;
mod trade;
mod trade_history_hover;
mod volume_menu;
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

use click_series::ClickSeries;
use trade::{OrderDrag, OrderHoverKey, PendingOrderDrag};

mod attach;
mod axes_controls;
mod construct;
mod controls;
mod introspect;
mod lifecycle;
mod settings_sig;

use settings_sig::ChartSettingsSig;
use settings_sig::{
    DEBUG_HISTORY_FILL_SPAN_MS, chart_bootstrap_present_rate_hz, chart_settings_sig,
};

pub struct ChartPanel {
    backend: Entity<Backend>,
    /// Which kind's defaults this panel follows when it has no override of its own.
    ///
    /// Owned by the STACK, which is the thing that knows whether it sits in a window and whether
    /// the anchor lock is on; pushed down through [`Self::set_default_kind`] whenever either moves.
    default_kind: moon_core::config::ChartTabKind,
    /// Group whose Auto rail authorizes chart navigation and trading; diagnostics stay unscoped.
    workspace_group: Option<String>,
    chart: ChartEngine,
    input: input::ChartInput,
    market: Option<String>,
    /// Price scale for this panel, where `None` means Auto. It is per tab rather than global, edited
    /// by the active-tab toolbar or detached-window header and applied during rendering.
    scale: Option<f32>,
    /// Whether this panel shows order books. This per-window/tab setting is applied through the
    /// engine's `set_orderbook_enabled`; it defaults to enabled.
    orderbook_enabled: bool,
    /// Per-window/tab candle and trade display settings. `None` follows the global
    /// `layout.candle_view` default; the effective value is applied during rendering.
    candle_view: Option<moon_core::market::CandleViewCfg>,
    /// Per-window/tab chart-drawing settings: trade-arrow size, connector thickness, which closed
    /// trades are drawn, the closed order's sell line, the trade-mark size and the bottom volume
    /// band. `None` follows the global `layout.chart_graphics` default; the effective value is
    /// applied during rendering.
    chart_graphics: Option<moon_core::config::ChartGraphicsCfg>,
    /// Per-window/tab chart captions: which figures print beside the plot, where and how. `None`
    /// follows the global `layout.chart_labels` default; the effective value is applied during
    /// rendering.
    chart_labels: Option<moon_core::config::ChartLabelsCfg>,
    /// Captions edited by this panel's own right-click menu, waiting to be PERSISTED.
    ///
    /// The panel has already applied them to itself; this is the copy its stack hands to the host
    /// that owns the tab spec. See `volume_menu` for why the write cannot happen here.
    pending_labels: Option<moon_core::config::ChartLabelsCfg>,
    /// Whether a hidden order book still leaves an order zone — the reserved strip along the right
    /// edge, dim-filled as a marker. Off, together with a hidden book, the pane has no order zone
    /// at all and no order gesture acts on it (`order_gestures_allowed`). Per window/tab, enabled
    /// by default.
    show_zone: bool,
    /// Whether a successful long or short order automatically pins its chart. Per window/tab and
    /// disabled by default.
    auto_pin: bool,
    /// Whether this panel last handed its panes a market-button state.
    ///
    /// A latch, not a cache: the state is held per PANE, so a chart that stops drawing buttons —
    /// its last one deleted, its pane gone book-only — has to clear what it pushed, and a chart
    /// that never drew one must not walk its panes on every render to clear nothing.
    market_actions_pushed: bool,
    /// Whether this panel is a HISTORICAL VIEWER rather than a live chart.
    ///
    /// Set once at construction and never afterwards, because it is a property of the WINDOW this
    /// panel was built for, not a preference. It is what the trade-detail window uses: that window
    /// shows what the market did around a trade that already closed, so everything belonging to
    /// LIVE trading is out of place in it.
    ///
    /// Two things are structurally absent while it is set, and both are removals rather than
    /// fences:
    ///
    /// * the ORDER BOOK, which describes the market RIGHT NOW and has nothing to say about a trade
    ///   that closed hours ago — it also costs the window roughly a fifth of its width;
    /// * the MARKET BUTTONS — `Cancel Buy`, `Panic Sell`, the temporary-ban lock — which send
    ///   commands to a real core against real money. A user reading a closed trade has no reason to
    ///   expect a live weapon under the cursor. A disabled or hidden button would still say
    ///   "trading happens here", so they print nothing at all: they are captions now, and the
    ///   engine answers `draws_live_market()` for them rather than trusting what it is handed —
    ///   see `ChartEngine::set_pane_actions`.
    ///
    /// It is FALSE for every other panel — Main, the stacks, detached chart windows, group windows
    /// and the Profit Monitor all keep their book and their trading controls unchanged.
    historical: bool,
    /// Last ChartText request this panel sent, so a stable target does not re-issue the command.
    last_chart_text_sent: Option<(CoreId, String)>,
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
    /// The camera moved (pan, wheel, zoom) and nothing else did: the overlays sync UNFORCED, so
    /// each layer rebuilds only when its own signature says the new view changed it.
    camera_dirty: bool,
    last_adaptive_notify_at: Option<Instant>,
    /// Last window scale factor recorded during rendering. The data-prepare path has no `Window`, so
    /// it reuses this value between infrequent DPI changes.
    last_ppp: f32,
    /// Wheel travel over a label column not yet paid out as a whole scroll step.
    label_wheel_accum: f32,
    /// The (pane, label row) `label_wheel_accum` was gathered over; another target starts it afresh.
    label_wheel_target: Option<(usize, usize)>,
    /// Whether a one-shot timer is armed for the nearest unpinned pane TTL deadline in a numbered
    /// AddToChart or Custom panel. Custom panes are normally pinned after population. Time-based
    /// expiry must not depend on backend data observations.
    ttl_timer_armed: bool,
    order_drag: Option<OrderDrag>,
    pending_order_drag: Option<PendingOrderDrag>,
    /// A left press in the book zone that has not yet become a chart pan or an order click.
    ///
    /// Held only while `chart_pan_in_book_zone` is on and the press missed every order line. A
    /// move past the pan threshold starts chart navigation and clears this; a still release
    /// replays the order-click gestures at the origin.
    book_zone_press: Option<render_input::BookZonePress>,
    order_hover: Option<OrderHoverKey>,
    /// Point of the most recent order-line hover hit-test. The Delphi-style movement threshold keeps
    /// subpixel raw mouse movement from scanning the lines again.
    order_hover_probe: Option<(f32, f32)>,
    /// When the last drag-driven `cx.notify()` went out, for the pacing in `render_input`.
    drag_notify_at: Option<Instant>,
    /// Whether a drag move was paced away and still owes the GPUI tree a repaint.
    ///
    /// Without this the LAST move of a gesture could be the one the pacer dropped, leaving the
    /// side controls a frame behind where the user let go.
    drag_notify_pending: bool,
    /// Last position the FIGURE hover hit-test ran at; see `trade::hover_probe_due`.
    fig_hover_probe: Option<(f32, f32)>,
    /// Last position the draft preview followed the cursor to, under the same threshold.
    fig_draft_probe: Option<(f32, f32)>,
    /// News marks for this chart's coin plus their hover state; see [`news`].
    news: news::NewsState,
    warn: warn::WarnState,
    /// Runtime-only durable history for this exact Main core and market.
    report_trades: report_trades::ReportTradesState,
    /// The closed trades' archived lines for the "Moonbot lines" style — see [`trace_lines`].
    trace_lines: trace_lines::TraceLinesState,
    /// Hover over a drawn closed-trade arrow, and the card it opens; see [`trade_history_hover`].
    trade_hover: trade_history_hover::TradeHoverState,
    /// The percent ruler while the left button holds it; see [`ruler`].
    ruler: Option<ruler::RulerHold>,
    /// Figure-drawing state for this panel: draft, hover, and drag.
    fig_draft: Option<figures::FigDraft>,
    fig_hover: Option<u64>,
    fig_drag: Option<figures::FigDrag>,
    /// Per-figure settings panel: the figure it edits and the point it was opened at, or `None`
    /// when it is closed. One at a time on purpose — it is opened from a right-click on a figure,
    /// and a second panel would edit a figure the first one is already showing.
    ///
    /// The point is the chart's, not the panel's: it is where THIS host puts the frame, in WINDOW
    /// coordinates — the same point the figure's context menu was opened at — and it is passed to
    /// the renderer rather than carried inside the target.
    fig_settings: Option<(crate::figstyle::FigStyleTarget, Point<Pixels>)>,
    /// Figure-store revision this panel last rebuilt geometry for. An edit bumps the store but no
    /// order does, and the userdata rebuild is gated on the order signature — this is what makes a
    /// restyled figure repaint at once instead of on the next order tick.
    last_fig_store_rev: u64,
    /// `Backend::panic_rev` this panel last repainted for. The Panic Sell / Stop Panic control is
    /// a GPUI element built in `Render`, and nothing else in this observer has a reason to move for
    /// a panic state change; without this the label waits for an unrelated market tick (250 ms /
    /// 1000 ms / unbounded on a quiet market).
    last_panic_rev: u64,
    /// `Backend::fav_rev` this panel last repainted for. The favourite star is an overlay element
    /// on the same terms as the panic control above; see `last_panic_rev`.
    last_fav_rev: u64,
    /// Whether right-button down opened an order context menu, suppressing the matching button-up
    /// so the Main-stack parent cannot interpret it as a fullscreen toggle.
    suppress_rmb_up: bool,
    /// Clicks this panel itself received, which is what mouse gestures are matched against. The
    /// window's native count alone would let a press whose predecessor landed on another chart —
    /// or on the × that closed one — act as a double click here.
    click_series: ClickSeries,
    focus: FocusHandle,
}

/// Whether a live-data axis notice may wake this chart's window.
///
/// Hidden charts never wake, however long the last wake was. A visible chart
/// wakes only when the signature changed and the previous wake is missing or
/// at least `floor` old.
///
/// Args:
///     visible: Whether this panel's scene is on screen.
///     changed: Whether the data signature differs from the last axis wake.
///     last: Time of the previous axis wake, if one has happened.
///     now: Observation time.
///     floor: Minimum gap between axis wakes.
///
/// Returns:
///     `true` only when the caller should stamp the wake and notify.
fn chart_axis_notify_due(
    visible: bool,
    changed: bool,
    last: Option<Instant>,
    now: Instant,
    floor: Duration,
) -> bool {
    visible && changed && last.is_none_or(|t| now.saturating_duration_since(t) >= floor)
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
