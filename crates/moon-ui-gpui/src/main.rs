// GUI application: suppress the console window at startup to avoid a black-window flash.
// A true debug build (`debug_assertions = true`) keeps the console for `env_logger` output;
// regular and release builds have no console, while configuration controls file logging through
// `applog::set_file_logging`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! MoonTerminal GPUI application shell.
//!
//! The crate-wide [`Backend`] owns the `moon-core` session manager, persistence state,
//! window registries, and chart coordination state. [`startup`] initializes logging,
//! crash handlers, configuration, and GPUI; starts the feed-wake and coordination loops;
//! and opens one primary window for every active configured group, or a synthetic default group
//! when no configured group is active.
//!
//! This crate root contains the module declarations, the shared [`Backend`] state whose private
//! fields are visible to descendant modules, and a thin [`main`] function. [`Backend`] methods
//! live in [`backend`], while the application startup and lifecycle live in [`startup`].

mod analytics;
mod backend;
mod chart_tabs;
mod chartdx;
mod chrome;
mod controls;
mod core_order;
mod crash;
mod debug_window;
mod design;
mod detached;
mod diag;
mod display_text;
mod firetest;
mod group_window;
mod hotkeys;
mod load_state;
mod media;
mod panels;
mod persistence;
mod screener;
mod settings;
mod shell;
mod startup;
mod strategies;
mod windowing;

pub(crate) use startup::install_moon_theme_for_config;

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gpui::*;

use chartdx::ChartDataHandle;
use persistence::chart_persist;

use moon_ui::{DockAreaState, Root};

use moon_core::config::{AppConfig, WindowLayout};
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::session::{CoreId, SessionManager};

// Localization: load the root `locales/*.yml` files relative to this crate's manifest.
// `t!("key")` reads a string from that set; `rust_i18n::set_locale` selects the global locale
// shared with MoonUI. Fall back to English when the selected locale has no matching key.
rust_i18n::i18n!("../../locales", fallback = "en");

/// Request to apply a layout to every tab and window in a group, originating from a detached chart.
pub(crate) struct ChartApplyAll {
    pub group: String,
    /// Whether to include the Main tab: `true` from its popup for every window, `false` from charts.
    pub include_main: bool,
    pub mode: Option<chart_persist::StackLayoutMode>,
    pub height_fit: Option<u16>,
    pub height_scroll: Option<u16>,
    /// Copy every relevant setting from the source tab, including price scale and order book state.
    pub scale: Option<f32>,
    pub orderbook: Option<bool>,
    pub liquidations: Option<bool>,
    pub show_zone: Option<bool>,
    pub auto_pin: Option<bool>,
    pub orientation: Option<chart_persist::StackOrientation>,
    pub cancel_pos: Option<chart_persist::ChartBtnPos>,
    pub panic_pos: Option<chart_persist::ChartBtnPos>,
    pub price_axis_pos: Option<chart_persist::PriceAxisPos>,
    pub time_axis_visible: Option<bool>,
    pub line_labels: Option<bool>,
    pub cursor_labels: Option<bool>,
}

/// Shared backend stored in one `Entity`, drained by coordination loops, and notifying UI observers.
struct Backend {
    session: SessionManager,
    /// Shared time origin for sessions and chart views, expressed as epoch milliseconds.
    /// This is reused when sessions are recreated after settings are saved and restarted.
    /// Ported from egui's `App.epoch_ms`.
    epoch: f64,
    /// Report database handle for typed `Event::Report` replication into SQLite.
    /// The session uses `tx` across starts and reconnects; the Report panel uses
    /// `generation` to trigger reads. `None` means the database is unavailable.
    reports: Option<moon_core::db::ReportsHandle>,
    metrics: Metrics,
    snap: MetricsSnapshot,
    /// Desired open markets as `(core, market)`, derived from `chart_market_refs`.
    /// Chart panels retain ownership counts rather than mutating this list directly.
    desired: Vec<(CoreId, String)>,
    chart_market_refs: HashMap<(CoreId, String), usize>,
    chart_market_refs_epoch: u64,
    /// Markets requiring an order book are tracked through effective order-book consumers.
    /// This count parallels `chart_market_refs` and includes visible panels plus inactive custom-tab
    /// references retained for an approximately five-second grace period. `desired_orderbook` is the
    /// derived list passed separately to `set_open`, avoiding a subscription after no consumer remains.
    chart_orderbook_refs: HashMap<(CoreId, String), usize>,
    desired_orderbook: Vec<(CoreId, String)>,
    desired_open_dirty: bool,
    last_open_sync: Instant,
    /// Main fullscreen chart target by group. Panels such as Orders use this for
    /// "current market"; AddToChart stacks are deliberately not part of that filter.
    main_chart_targets: HashMap<String, (CoreId, String)>,
    /// Markets open in each group's Main-tab stack: `group -> [(core, market)]`.
    /// The Orders view highlights one row for each pair the user opened on Main.
    main_open_markets: HashMap<String, Vec<(CoreId, String)>>,
    /// Manual active-trading-core selection in the header, mapped from group to core.
    /// This sticky override suppresses automatic fullscreen-chart following while the core remains
    /// in the group, until the user opens a fullscreen chart for another core. See `active_trade_core`.
    trade_core_override: HashMap<String, CoreId>,
    /// Active committed in-memory configuration, including theme, order style, and servers.
    /// Dirty edits may remain ahead of disk until the debounced persistence path saves them.
    config: AppConfig,
    /// Settings-window draft, present while the window is open.
    /// Group windows render charts with this draft for live preview; Save commits it to `config`
    /// and disk, while closing without saving discards it and restores `config`.
    /// This mirrors egui's `SettingsState.draft`.
    preview: Option<AppConfig>,
    /// Request to open a market on Main, consumed by the target group's Shell.
    /// Ported from egui's `open_detect -> host` path.
    open_request: Option<(CoreId, String)>,
    /// Revision of `open_request`, allowing `ChartTabs` to wake for a specific request rather than
    /// relying on a fallback backend render.
    open_request_rev: u64,
    /// Whether an `open_request` should activate the Main window.
    /// Sources that should raise Main, currently chart double-clicks and Alerts coin clicks, set
    /// this to `true`; other request sources open the market without raising it. Every request sets it.
    open_request_activate: bool,
    /// Request to open a market in a new custom comparison tab from a detect context menu.
    /// The detect's market anchors the tab alongside that market from other group cores,
    /// deduplicated by exchange, with lock and clear controls. See `open_compare_tab`.
    open_compare_request: Option<(CoreId, String)>,
    /// Revision of `open_compare_request`, waking `ChartTabs` through its signature like
    /// `open_request_rev`.
    open_compare_request_rev: u64,
    /// Diagnostic chart auto-open for runtime counters, disabled by default and enabled only by
    /// the `MOON_RENDER_DIAG_OPEN_FIRST_MARKET` environment variable.
    diag_open_first_market: bool,
    diag_open_done: bool,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    diag_open_10_btc: bool,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    diag_open_10_btc_done: bool,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_fill_main_chart_group: Option<String>,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_fill_main_chart_rev: u64,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_main_chart_handles: HashMap<String, ChartDataHandle>,
    /// Window layout with per-group geometry, loaded at startup and saved after changes through
    /// the debounced coordination loop. Ported from egui's `WindowLayout` and `layout.toml`.
    layout: WindowLayout,
    layout_dirty: bool,
    /// Per-group detect-strip presentation: dimensions, chart, rail, and size slots.
    /// Stored in the portable `detects_view.toml` and saved immediately because the file is small.
    detects_view: moon_core::config::DetectViewFile,
    /// Cache of the default header-ticker source when no choice is saved, as `(core, market)`.
    /// Resolved lazily from exact BTCUSDT or UBTCUSDC matches, then the first broader `BTC` search
    /// result as a fallback, and not persisted.
    header_ticker_default: Option<(CoreId, String)>,
    last_header_ticker_refresh: Option<Instant>,
    /// Dock layouts mapped from group to `DockAreaState`, loaded at startup and saved to
    /// `docks.json` after `DockEvent::LayoutChanged` through the same debounce loop.
    dock_states: HashMap<String, DockAreaState>,
    dock_dirty: bool,
    /// Y-axis price scale of the window's active chart; `None` means automatic scaling.
    /// Scale is stored per tab, so this field mirrors the active tab for the toolbar. A toolbar
    /// selection increments `price_scale_rev`, causing `ChartTabs` to update the active panel.
    price_scale: Option<f32>,
    /// Window group addressed by the most recent scale request.
    price_scale_group: Option<String>,
    /// Addressable scale-request revision, incremented by dropdown changes, scale hotkeys, and FireTest.
    /// `ChartTabs` applies `price_scale` to the active panel when this grows, not on every frame.
    price_scale_rev: u64,
    /// Window group whose Main stack should switch its active chart for the `switch_charts` hotkey.
    switch_charts_group: Option<String>,
    /// Active-chart switch-request revision, incremented on every hotkey press.
    /// The addressed group's `ChartTabs` advances the Main-stack chart when this grows.
    switch_charts_rev: u64,
    /// Global close-all-charts request revision for the built-in Shift+Esc binding.
    /// Every `ChartTabs` closes its Main stack when this grows; any window may increment it.
    close_all_charts_rev: u64,
    /// Window group whose Main stack should close its fullscreen chart for the built-in Esc binding.
    close_active_chart_group: Option<String>,
    /// Fullscreen-chart close-request revision, incremented on Esc.
    /// The addressed group's `ChartTabs` closes only its current fullscreen Main chart when this grows.
    close_active_chart_rev: u64,
    /// Toolbar live-follow state: `true` tracks the present; `false` pauses the view.
    follow: bool,
    /// Selected manual-order size preset per core, as an F1-F6 button index in `0..=5`.
    /// Bases differ between cores, such as BTC versus USDT, so values and selection are per-core.
    /// `PlaceOrder` uses `ServerConfig::order_sizes_or_default(base)[sel]`; absence means default.
    order_size_sel: HashMap<CoreId, usize>,
    /// Broad toolbar and order-controls revision used to trigger notification and redraw.
    /// It increments for hotkeys, size edits and wheel changes, fixed-sell slot changes, and
    /// fixed-sell percentage edits, as well as direct size selection.
    order_size_rev: u64,
    /// Request to edit a size-button value inline after a toolbar double-click, as
    /// `(core, F1-F6 index)`. Shell consumes it during render, opens and focuses an input over
    /// the button, then writes to `ServerConfig.order_sizes` and saves on blur or Enter.
    order_size_edit_req: Option<(CoreId, usize)>,
    /// Request to edit a fixed-sell preset inline after an S-button double-click, as
    /// `(core, S1-S6 index)`. Shell sends `SetFixedSellPct` to the core on blur or Enter.
    sell_edit_req: Option<(CoreId, usize)>,
    /// Optimistic local cache of fixed-sell percentages, keyed by `(core, index)`.
    /// Wheel and edit actions update it immediately for responsive display while sending to the core.
    /// Otherwise the value would update only after the server echo because `send_settings` does not
    /// modify the local snapshot, making sell changes appear unresponsive.
    sell_pct_local: HashMap<(CoreId, usize), f64>,
    /// Optimistic local fixed-sell slot selection. `Some(slot)` highlights S1-S6, while `None`
    /// highlights the primary TP. Without it, clicks would visually wait for the core's settings echo.
    sell_slot_local: HashMap<CoreId, Option<usize>>,
    /// Optimistic local manual-strategy selection as `(enabled, id)`, keeping the header toggle and
    /// picker responsive until the core echoes its settings.
    manual_strat_local: HashMap<CoreId, (bool, u64)>,
    /// Locally armed Panic Sell state by `(core, market)`, providing immediate button highlighting
    /// and on/off state without waiting for a core echo.
    panic_armed: HashSet<(CoreId, String)>,
    /// Backend-level notify is only for slow GPUI chrome/status/overlays. High-rate chart
    /// data goes straight into retained chart handles and must not dirty the whole tree.
    backend_dirty_since_notify: bool,
    last_backend_notify: Option<Instant>,
    /// Core reconnect requests from the Connections button, drained into `session.reconnect`.
    /// Ported from egui's `SettingsActions.reconnect`.
    reconnect_request: Vec<CoreId>,
    /// Requests from the eye button to show a group window, drained by opening or focusing it.
    /// Ported from egui's `SettingsActions.show_group`.
    show_group_request: Vec<String>,
    /// Open group windows mapped from group to handle, used for eye-button focus and deduplication.
    group_windows: HashMap<String, WindowHandle<Root>>,
    /// Floating Settings tool window handle for deduplication and focus.
    settings_window: Option<WindowHandle<Root>>,
    /// Application-wide Strategies OS window handle for deduplication and focus.
    strategies_window: Option<WindowHandle<Root>>,
    /// Request to show a strategy in the Strategies window as `(core, strategy id)`.
    /// Set from a chart order-line context menu or an Orders-table strategy cell, then consumed by
    /// `StrategiesView` during render to disable the active-only filter, expand, and select the row.
    strategies_goto: Option<(CoreId, u64)>,
    /// Global singleton Assets window for all cores, retained for deduplication and focus.
    assets_window: Option<WindowHandle<Root>>,
    /// Singleton Screener window covering all exchanges with provider deduplication.
    screener_window: Option<WindowHandle<Root>>,
    /// Singleton Analytics window containing report analyzers, retained for deduplication and focus.
    analytics_window: Option<WindowHandle<Root>>,
    /// Built-in debug scenario runner (`--debug-script chart-smoke`). None in normal app runs.
    firetest: Option<firetest::Runtime>,
    /// Detached dock panels, recording panel identity, source group, and window geometry.
    /// Loaded at startup and saved after changes; ported from egui's `WindowLayout.detached`.
    detached: Vec<detached::DetachedSpec>,
    detached_dirty: bool,
    /// Requests to return a panel to its dock after its detached window closes, as
    /// `(group, panel_name)`. The group's Shell consumes each request, adds the panel back to its
    /// `DockArea`, and removes the detached specification.
    repin_request: Vec<(String, String)>,
    /// Requests to return a chart tab to its strip after its detached window closes, as
    /// `(group, number, bucket)`. The group's `ChartTabs` consumes each request and reattaches it.
    chart_repin_request: Vec<(String, u32, moon_core::config::ChartBucket)>,
    /// Apply-layout-to-all requests from detached chart windows, which cannot access group stacks.
    /// The group's `ChartTabs` consumes them. Chart-originated requests use `include_main = false`;
    /// requests originating within `ChartTabs` are applied directly without this queue.
    chart_apply_all: Vec<ChartApplyAll>,
    /// Apply-candle-settings-to-all requests from detached windows, carrying
    /// `(group, config, source-window X scale)` and consumed by the group's tab strip.
    chart_candle_apply_all: Vec<(String, moon_core::market::CandleViewCfg, Option<f32>)>,
    /// X-scale synchronization request from Shift+middle-click on a chart, carrying the source
    /// OS window and pixels per millisecond. The owner of that same window, either group `ChartTabs`
    /// or `DetachedChartHost`, applies and saves the scale only for charts in that window.
    chart_x_sync: Option<(gpui::AnyWindowHandle, f32)>,
    /// Revision of `chart_x_sync`, incremented for every gesture so window owners react once.
    chart_x_sync_rev: u64,
    /// Chart tabs detached into OS windows, stored as `(group, window handle)`.
    /// Closing a group window closes its detached charts; closing a detached window removes it by
    /// `window_id`. This is separate from `detached`, which tracks detached dock panels.
    detached_chart_windows: Vec<(String, WindowHandle<Root>)>,
    /// Time of the last active input in each group's primary window, updated by mouse movement while
    /// focused. Main's inactivity timeout, configured by `main_idle_close_secs`, measures from this
    /// value. It stops advancing when the window loses focus or the mouse stops, then charts close
    /// after the configured delay. See Shell's `on_mouse_move`.
    last_main_input: std::collections::HashMap<String, std::time::Instant>,
    /// Per-core local toggle for excluding blacklisted markets from market-delta calculation.
    /// The core provides no readback for this local Active Lib action, so the UI retains the choice;
    /// the default is disabled.
    exclude_bl_delta: std::collections::HashMap<CoreId, bool>,
    /// Singleton debug-window handle used for deduplication and focus.
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_window: Option<WindowHandle<Root>>,
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    debug_chart_windows: Vec<WindowHandle<Root>>,
    /// Visible chart consumers for account/order overlays. Live market frames pull
    /// `MarketDataSource` directly from `gpu_canvas.frame()`.
    chart_consumers: Vec<ChartDataHandle>,
    /// Persistent chart-tab state, including per-tab scale and detached-window geometry, in
    /// `charts.json`. The coordination loop saves it after `chart_specs_dirty`; see `chart_persist`.
    chart_specs: Vec<chart_persist::ChartTabSpec>,
    chart_specs_dirty: bool,
    /// User chart figures keyed by core and market, shared by every panel as an `Rc` cloned into
    /// chart engines. Persisted to `figures.json`; the coordination loop debounces saves via the
    /// store's `dirty` flag.
    figures: std::rc::Rc<std::cell::RefCell<moon_core::figures::FigureStore>>,
    /// Whether drawing mode is enabled, represented by the pressed pencil button and enabled by
    /// default. In this mode the platform secondary modifier plus left-click, Ctrl on Windows and
    /// Linux or Cmd on macOS, draws with `fig_tool` or selects and moves an existing figure.
    /// When disabled, figures are hidden and frozen.
    fig_draw_mode: bool,
    /// Selected drawing tool used by Ctrl+left-click and chosen from the pencil popup.
    fig_tool: moon_core::figures::FigureTool,
    /// Style for new figures, including color, thickness, and dash pattern, edited in the pencil popup.
    fig_style: moon_core::figures::DrawStyle,
    /// Application-wide selected figure as `(core, market, id)`, used for chart highlighting and
    /// handles as well as Shell deletion and alert hotkeys.
    fig_selected: Option<(CoreId, String, u64)>,
    /// Application-wide chart panel under the cursor, set and cleared by infrequent `on_hover`
    /// enter and leave events. Cursor-dependent hotkeys such as `new_long` and `new_short` use it
    /// to place an order at the pointer price regardless of focus. The weak handle may expire.
    hovered_chart: Option<WeakEntity<crate::panels::ChartPanel>>,
    /// Last observed aggregate revision of server-side chart alerts, gating remote-figure
    /// reconciliation in the feed-drain path.
    last_chart_alerts_activity: u64,
    /// Last processed detect sequence per core, used for detect and alert sound traversal.
    /// It may be seeded or advanced after observing a detect without playing a sound.
    last_detect_seq: std::collections::HashMap<CoreId, u64>,
    /// Last observed `detects_rev` per core, gating `play_detect_sounds`.
    /// The drain wakes hundreds of times per second while detects change infrequently; without this
    /// gate, every wake would scan as many as 2,000 detects per core.
    last_detect_rev: std::collections::HashMap<CoreId, u64>,
    /// Default sound for an alert without a strategy, selected in the Alerts panel.
    /// Stored as a WAV filename stem; see `sound` and `detect_sound`.
    default_alert_sound: String,
    /// Configuration changed in memory and awaiting a debounced save.
    /// Frequent edits such as mouse-wheel order-size changes write to disk once per coordination
    /// tick; the drain calls `config.save()` and clears this flag.
    config_dirty: bool,
    /// Whether `on_app_quit` is shutting down the application.
    /// Detached windows must not repin while exiting, or their detached state would be cleared and
    /// not restored on the next start; the repin drain checks this flag.
    quitting: bool,
}

fn main() -> anyhow::Result<()> {
    startup::run()
}
