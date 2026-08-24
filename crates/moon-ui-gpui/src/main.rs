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
mod conn_diag;
mod controls;
mod core_order;
mod design;
mod diag;
mod diagnostics;
mod display_text;
mod figstyle;
mod firetest;
mod hotkeys;
mod load_state;
mod media;
mod order_math;
mod panels;
mod persistence;
mod pulse;
mod screener;
mod settings;
mod shell;
mod startup;
mod strategies;
mod trade_window;
mod ui_session;
// The UI-control atlas, kept OUT of this repository: a crawl that is not published, plus the
// trade fixtures it runs against. `build.rs` defines `uidoc` only when the overlay is on disk, so
// a clone without it compiles this crate unchanged and no feature promises what is missing.
#[cfg(uidoc)]
#[path = "../../../private/uidoc/mod.rs"]
mod uidoc;
mod update;
mod valuation_health;
mod window;
mod workspace;

pub(crate) use startup::install_moon_theme_for_config;

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gpui::*;

use chartdx::ChartDataHandle;
use persistence::chart_persist;
use ui_session::UiSessionState;
use window::detached;

use moon_ui::{DockAreaState, DockTopologyByName, Root};

use moon_core::config::{AppConfig, WindowLayout};
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::session::{CoreId, SessionManager};

// Localization: load the root `locales/*.yml` files relative to this crate's manifest.
// `t!("key")` reads a string from that set; `rust_i18n::set_locale` selects the global locale
// shared with MoonUI. Fall back to English when the selected locale has no matching key.
rust_i18n::i18n!("../../locales", fallback = "en");

/// Shared backend stored in one `Entity`, drained by coordination loops, and notifying UI observers.
struct Backend {
    /// Process-wide self-update state shared by every group window.
    updater: Entity<update::UpdateController>,
    session: SessionManager,
    /// Shared time origin for sessions and chart views, expressed as epoch milliseconds.
    /// This is reused when sessions are recreated after settings are saved and restarted.
    /// Ported from egui's `App.epoch_ms`.
    epoch: f64,
    /// Report database handle for typed `Event::Report` replication into SQLite.
    /// The session uses `tx` across starts and reconnects; the Report panel uses
    /// `generation` to trigger reads. `None` means the database is unavailable.
    reports: Option<moon_core::db::ReportsHandle>,
    /// Historical and current-rate quote-to-USDT worker with its independently committed data
    /// generation.
    valuation: Option<moon_core::db::valuation::ValuationHandle>,
    /// Dedicated wake channel for report-derived consumers.
    report_revision: Entity<ReportRevision>,
    /// Dedicated wake channel for consumers that must retry after retained market data changes.
    market_data_revision: Entity<MarketDataRevision>,
    /// Dedicated wake channel for surfaces whose civil-time meaning follows the header clock.
    display_time_revision: Entity<DisplayTimeRevision>,
    /// Dedicated wake channel for every effective workspace-scope transition.
    workspace_revision: Entity<workspace::WorkspaceRevision>,
    /// Dedicated wake channel for shared Auto dock-topology and rail-width transitions.
    auto_workspace_layout_revision: Entity<workspace::AutoWorkspaceLayoutRevision>,
    /// Cores broadcast by the Profit Monitor's core click; empty means every core, as in a panel's
    /// own retained filter. Process-lifetime like those filters, and never serialized.
    core_filter: HashSet<CoreId>,
    /// Dedicated wake channel for `core_filter`, observed only by the panels that own a core
    /// selector and by the monitor that publishes it.
    core_filter_revision: Entity<CoreFilterRevision>,
    /// Last live Auto group to own Analytics and Strategies scope; never serialized.
    workspace_focus: Option<workspace::WorkspaceFocus>,
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
    /// Active committed in-memory configuration, including theme, order style, and servers.
    /// Dirty edits may remain ahead of disk until the debounced persistence path saves them.
    config: AppConfig,
    /// Settings-window draft, present while the window is open.
    /// Group windows render charts with this draft for live preview; Save commits it to `config`
    /// and disk, while closing without saving discards it and restores `config`.
    /// This mirrors egui's `SettingsState.draft`.
    preview: Option<AppConfig>,
    /// Atomic Main-open request identity: target, owning group, revision, activation, and pending
    /// state move together so no producer or consumer can observe mismatched parallel fields.
    open_main_request: backend::OpenMainRequest,
    /// Latest causally ordered ChartTabs/Report reveal requested independently for each Auto group.
    auto_workspace_surface_requests: workspace::AutoWorkspaceSurfaceRequests,
    /// Request to open a market in a new custom comparison tab from a detect context menu.
    /// The detect's market anchors the tab alongside that market from other group cores,
    /// deduplicated by exchange, with lock and clear controls. See `open_compare_tab`.
    open_compare_request: Option<backend::OpenCompareRequest>,
    /// Revision of `open_compare_request`, waking `ChartTabs` through its signature like the
    /// atomic Main-open request revision.
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
    /// Cached empty-field coin suggestions, one entry per search field's core scope.
    ///
    /// Building a suggestion list walks every market of every provider, so it runs when a popup
    /// OPENS and never at render. One entry per key, not a single slot: the tab strip and a
    /// detached window can hold popups open at the same time and would otherwise blank each other.
    coin_suggest: HashMap<
        (String, Option<moon_core::config::ChartBucket>),
        crate::controls::coin_search::CoinSuggestEntry,
    >,
    /// Process-lifetime window state that survives view replacement but is never serialized.
    ui_session: UiSessionState,
    /// Per-group detect-strip presentation: dimensions, chart, rail, and size slots.
    /// Stored in the portable `detects_view.toml` and saved immediately because the file is small.
    detects_view: moon_core::config::DetectViewFile,
    /// Global News-panel tag settings: per-tag colours + the tag-visibility filter. Stored in the
    /// small portable `news_tags.json` and saved immediately on change.
    news_tag_settings: moon_core::config::NewsTagSettings,
    /// Global arbitrage roster for the chart's arbitrage column: which venues, in what order, under
    /// what name and colour. Stored in the portable `arb_view.toml` and saved immediately, like the
    /// two above — it is a small file and the window that edits it has no Apply button.
    ///
    /// Behind an `Rc` because every chart panel hands the SAME handle to its engine on render, and
    /// the pointer test there is what keeps a roster of twenty venues from being compared per frame.
    arb_view: std::rc::Rc<moon_core::config::ArbViewCfg>,
    /// Dock-tab unread counters: per-panel display switches and per-panel/group read watermarks.
    /// Stored in the small portable `tab_badges.json`.
    tab_badges: moon_core::config::TabBadgeSettings,
    /// Whether `tab_badges` has unsaved changes. The read watermark moves from the render path, so
    /// the write is deferred to the same debounce loop that persists the layout instead of putting
    /// an fsync in a frame.
    tab_badges_dirty: bool,
    /// Cache of the default header-ticker source when no choice is saved, as `(core, market)`.
    /// Resolved lazily from exact BTCUSDT or UBTCUSDC matches, then the first broader `BTC` search
    /// result as a fallback, and not persisted.
    header_ticker_default: Option<(CoreId, String)>,
    last_header_ticker_refresh: Option<Instant>,
    /// Dock layouts mapped from group to `DockAreaState`, loaded at startup and saved to
    /// `docks.json` after `DockEvent::LayoutChanged` through the same debounce loop.
    dock_states: HashMap<String, DockAreaState>,
    dock_dirty: bool,
    /// Process-wide topology-only Auto dock authority loaded from `auto_dock.json`.
    auto_dock_topology: Option<DockTopologyByName>,
    /// Whether programmatic Auto seed and repair transitions may persist the current topology.
    /// Invalid or unreadable startup data keeps this false until a user changes the topology.
    auto_dock_automatic_persistence_allowed: bool,
    /// Whether the Auto topology authority changed since the last debounced atomic save.
    auto_dock_dirty: bool,
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
    /// Window group whose Main stack should close its active chart for the built-in Esc binding.
    close_active_chart_group: Option<String>,
    /// Active-chart close-request revision, incremented on Esc.
    /// The addressed group's `ChartTabs` closes its active Main chart when this grows.
    close_active_chart_rev: u64,
    /// Time and SCREEN position of the last chart close a click caused, shared by every chart panel.
    ///
    /// Closing charts one after another walks a fresh chart under the cursor after each ×, and the
    /// presses that keep arriving at that same pixel belong to the closing, not to the chart that
    /// now sits there. A panel's own click counting (`panels/chart/click_series.rs`) rejects the
    /// first of them, but the pair it makes with the next one is genuine as far as any single panel
    /// can see — only a mark none of them owns identifies the whole chain. `None` until a click
    /// closes a chart; closes with no press behind them (Escape, TTL, teardown) leave it alone.
    last_chart_close: Option<(f64, (f32, f32))>,
    /// Toolbar live-follow state: `true` tracks the present; `false` pauses the view.
    follow: bool,
    /// Broad toolbar and order-controls revision used to trigger notification and redraw.
    /// It increments for hotkeys, size edits and wheel changes, fixed-sell slot changes, and
    /// fixed-sell percentage edits, as well as direct size selection.
    order_size_rev: u64,
    /// Request to edit a size-button value inline after a toolbar double-click, as
    /// `(group, F1-F6 index)`. Shell consumes it during render and persists the USD value locally.
    order_size_edit_req: Option<(String, usize)>,
    /// Request to edit a fixed-sell preset inline after an S-button double-click, as
    /// `(group, S1-S6 index)`. Shell writes the visible group value on blur or Enter.
    sell_edit_req: Option<(String, usize)>,
    /// Last attempted group-exit generation per core as `(settings, snapshot revision, ready)`.
    ///
    /// Including the coarse connection phase forces one retry after a feed respawn even when the
    /// first new snapshot equals the retained pre-reconnect value and therefore keeps its revision.
    group_exit_sync: HashMap<CoreId, (moon_core::config::GroupExitSettings, u64, bool)>,
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
    /// Shared per-server CPU/memory history for the Core Status detached-window chart. Kept here so
    /// it accumulates continuously and survives a window opening and closing.
    core_chart_hist: crate::backend::server_chart::ServerChartHistory,
    /// Shared per-core process CPU/memory history, overlaid on the Core Status chart as a line pair
    /// per core. Same lifetime rationale as `core_chart_hist`.
    core_line_hist: crate::backend::server_chart::CoreChartHistory,
    /// Shared per-server client↔core round-trip history (ms), for the Core Status chart's core-ping
    /// line and the badge card. Recorded backend-always like the CPU/memory rings.
    server_ping_hist: crate::backend::server_chart::ServerPingHistory,
    /// Shared per-server core→exchange order-latency history (ms), the exchange-ping companion to
    /// `server_ping_hist`.
    server_exch_hist: crate::backend::server_chart::ServerPingHistory,
    /// Backend-always core warning engine: detects sustained CPU / memory growth and produces
    /// warning episodes. The Core Status panel reads its current state instead of tracking locally.
    warn: crate::backend::core_warn::CoreWarnEngine,
    /// Forever-persistence for closed warning episodes, or `None` if the database could not open.
    warn_store: Option<crate::backend::core_warn::store::WarnStore>,
    /// Closed episodes awaiting the COMPLETE ±1 min history slice, re-captured once the forward tail
    /// has accumulated (~60 s after the episode start). A durable partial is already written at close,
    /// so this queue being in-memory only means a restart drops the forward-tail completion, never the
    /// whole graph.
    warn_pending_slices: Vec<crate::backend::PendingWarnSlice>,
    /// Unix ms of the last retention prune of old warning-episode slices (`0` = not yet this session).
    /// Re-pruned once per `WARN_PRUNE_INTERVAL_MS` so a session outliving the retention window keeps
    /// bounding the file instead of pruning only once at startup.
    warn_last_prune_ms: i64,
    /// Core reconnect requests from the Connections button, drained into `session.reconnect`.
    /// Ported from egui's `SettingsActions.reconnect`.
    reconnect_request: Vec<CoreId>,
    /// Requests from the eye button to show a group window, drained by opening or focusing it.
    /// Ported from egui's `SettingsActions.show_group`.
    show_group_request: Vec<String>,
    /// Open group windows mapped from group to handle, used for eye-button focus and deduplication.
    group_windows: HashMap<String, WindowHandle<Root>>,
    /// Groups whose `open_window` call is constructing a Shell before its handle can be registered.
    /// Availability accepts this narrow bootstrap state but no other missing-window condition.
    opening_group_windows: HashSet<String>,
    /// Floating Settings tool window handle for deduplication and focus.
    settings_window: Option<WindowHandle<Root>>,
    /// Application-wide Strategies OS window handle for deduplication and focus.
    strategies_window: Option<WindowHandle<Root>>,
    /// Atomic Strategies reveal request carrying its core, target, and immutable producer group.
    /// Set from a chart order-line context menu or Orders-table strategy cell, then revalidated by
    /// `StrategiesView` before it disables filters, expands folders, and selects the row.
    strategies_goto: Option<strategies::StrategyRevealRequest>,
    /// Global singleton Assets window for all cores, retained for deduplication and focus.
    assets_window: Option<WindowHandle<Root>>,
    /// Singleton Screener window covering all exchanges with provider deduplication.
    screener_window: Option<WindowHandle<Root>>,
    /// Singleton Analytics window containing report analyzers, retained for deduplication and focus.
    analytics_window: Option<WindowHandle<Root>>,
    /// Independent singleton Profit Monitor desktop window.
    profit_monitor_window: Option<WindowHandle<Root>>,
    /// Single-flight Profit Monitor create request, including monotonic foreground intent.
    profit_monitor_open_pending: Option<analytics::profit_monitor::ProfitMonitorOpenRequest>,
    /// Singleton Report window opened from an Analytics strategy row.
    report_window: Option<WindowHandle<Root>>,
    /// Live scoped Report panel retained weakly so repeated double-clicks can replace its filter.
    report_window_view: Option<WeakEntity<crate::panels::ReportPanel>>,
    /// Built-in debug scenario runner (`--debug-script chart-smoke`). None in normal app runs.
    firetest: Option<firetest::Runtime>,
    /// Whether this process may flush the debounced workspace state — layout, docks, detached
    /// geometry, chart specs, badges, figures and config — to disk.
    ///
    /// False for the whole life of a `--debug-script` run. FireTest drives the real app: it opens
    /// tool windows, switches the locale, changes the price scale, and will detach and repin
    /// panels. Every one of those marks state dirty, and the 100 ms tick duly wrote it, so the
    /// diagnostic silently rewrote the workspace it was meant to be observing.
    ///
    /// Scope, stated precisely because it is narrower than "a run writes nothing": this gates the
    /// two DEBOUNCED flush sites in `startup.rs` — the coordinator tick and the quit hook. It does
    /// NOT gate anything that bypasses the dirty-flag mechanism: the report DB writer, `strat_db`,
    /// `AppConfig::load`'s own uid save and schema-upgrade backup, `applog::purge_old`, the
    /// one-shot chart-id remap that runs before this struct exists, the direct `config.save()` in
    /// `shell/init.rs`, or panels that write straight through (`detects_view`,
    /// `news_tag_settings`, the Settings window's snapshot save). `order-cancel-lag` in particular
    /// places a real order that lands permanently in the developer's `reports.sqlite`. Those are
    /// separate holes; closing them needs a different mechanism.
    persist_allowed: bool,
    /// Detached dock panels, recording panel identity, source group, and window geometry.
    /// Loaded at startup and saved after changes; ported from egui's `WindowLayout.detached`.
    detached: Vec<detached::DetachedSpec>,
    detached_dirty: bool,
    /// Requests to return a panel to its dock after its detached window closes, as
    /// `(group, panel_name)`. The group's Shell consumes each request, adds the panel back to its
    /// `DockArea`, and removes the detached specification.
    repin_request: Vec<(String, String)>,
    /// Requests to detach a docked panel into its own window, as `(group, panel_name)`.
    ///
    /// The mirror of `repin_request`, and it exists for the same reason: detaching is otherwise
    /// reachable only as a `DockEvent` a human raises by double-clicking a tab, so nothing that
    /// holds only a `Backend` — FireTest's panel round-trip stage — can drive it. The group's
    /// Shell drains this beside the repins and routes each through the same `defer_detach_panel`
    /// the UI uses, so the tested path is the real one.
    ///
    /// Pushing alone does NOT wake anything: the drain runs from Shell's `cx.observe(&backend)`,
    /// which fires only on a Backend notify gated behind `backend_dirty_since_notify` and 250 ms.
    /// A caller must also `mark_backend_dirty`, or the request waits for incidental feed traffic.
    ///
    /// The drain consumes a request whether or not the detach happens — `defer_detach_panel`
    /// declines an unsupported panel, an already-detached one, and a failed spawn. There is no
    /// completion or failure signal; a caller watches `dock_states` and gives up on a deadline.
    panel_detach_request: Vec<(String, String)>,
    /// Live detached panel windows, keyed by `(group, panel_name)`.
    ///
    /// `detached::spawn` returned the handle and every caller dropped it, which left no way to
    /// close a panel window from code — a round-trip check has to close what it opened. The
    /// insertion lives inside `spawn` itself, so every route fills it: the startup restore, the
    /// dock's detach action, the panel toolbar button, and the settings-driven reopen after a
    /// group-window rebuild.
    ///
    /// This map is also the AUTHORITY for "this window may repin". A release repins only while the
    /// entry still names that same window, so code that tears a window down on purpose removes the
    /// entry first and the release stays silent — otherwise a window rebuild would return every
    /// detached panel to its dock and delete its `DetachedSpec`. Entries therefore disappear on two
    /// edges, not one: a user-driven release, and a deliberate teardown. Application exit is such a
    /// teardown: the close of the LAST group window unregisters every entry before requesting the
    /// quit, so panels that were detached stay detached across a restart.
    ///
    /// Keyed by identity of the panel, not of the window: two live windows for one `(group,
    /// panel)` leave the map describing only the second, and the first is then inert — it can no
    /// longer repin. Both detach routes decline an already-detached panel, so a reader should treat
    /// a missing entry as "no window to close", never as "no window exists".
    detached_panel_windows: HashMap<(String, String), WindowHandle<Root>>,
    /// Requests to return a chart tab to its strip after its detached window closes, as
    /// `(group, number, bucket)`. The group's `ChartTabs` consumes each request and reattaches it.
    chart_repin_request: Vec<(String, u32, moon_core::config::ChartBucket)>,
    /// ⧉ "apply to all" requests from detached chart windows, which cannot access group stacks.
    /// The group's `ChartTabs` consumes them. ONE queue for every popup that has the button, because
    /// each press is fully described by its value set; requests originating within `ChartTabs` are
    /// applied directly without this queue.
    chart_apply_all: Vec<chart_tabs::apply_all::ApplyAllRequest>,
    /// The "set this as the default" presses, for EVERY group window to drop the overrides they
    /// cleared from its own live stacks. See [`chart_tabs::apply_all::ClearDefaults`].
    ///
    /// A queue, not one slot: a detached window's presses are drained in a loop, so two of them can
    /// land between one window's observations, and the first must not be lost. Each entry carries
    /// the revision it was made at; a window applies everything newer than what it has seen.
    chart_defaults_clear: std::collections::VecDeque<(u64, chart_tabs::apply_all::ClearDefaults)>,
    /// Advances with each such press, including one that stored a value already in the file: a
    /// stack in another window can still be holding an override the press is meant to drop.
    chart_defaults_rev: u64,
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
    /// Trade-detail windows, keyed by the trade they show as `((core_uid, record_id), handle)`.
    ///
    /// Its own list rather than a shared registry, exactly like `detached_chart_windows` above:
    /// each window class answers different questions about its members. Re-clicking a trade
    /// already in this list focuses that window instead of opening a second one, and the list is
    /// capped so a walk down the report cannot accumulate chart engines.
    trade_windows: Vec<((u64, i64), WindowHandle<Root>)>,
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
    /// Whether a drawing tool is ARMED, shown by the toolbar's tool picker and armed by default.
    /// With one armed, the platform secondary modifier plus left-click — Ctrl on Windows and Linux,
    /// Cmd on macOS — places nodes for `fig_tool`.
    ///
    /// It gates NOTHING else: figures stay visible, hoverable, selectable, draggable and
    /// right-clickable with no tool armed, which is what the picker's Cursor entry leaves behind.
    fig_draw_mode: bool,
    /// Selected drawing tool used by Ctrl+left-click and chosen from the settings panel.
    fig_tool: moon_core::figures::FigureTool,
    /// Armed "sells to zone" drawing mode — Moonbot's Sells-to-rectangle — holding the
    /// `(fig_tool, fig_draw_mode)` to put back when it ends.
    ///
    /// While it is `Some` the Zone tool is armed like any other, so the whole drawing path is
    /// reused unchanged; the ONE difference is what the finishing click does with the result: the
    /// band goes to the core as a price zone and the figure is never stored, exactly as Moonbot's
    /// own `CO_SysRect` is drawn and dropped. Band after band can be drawn that way until the mode
    /// ends; the previous tool then comes back, so a mode entered for one job does not silently
    /// redefine what the next Ctrl+click draws.
    sells_zone_arm: Option<(moon_core::figures::FigureTool, bool)>,
    /// Style for new figures — colour, thickness, dash pattern and fill — PER TOOL, keyed by
    /// `ToolDef::key`. A tool absent from the map has never been styled and draws in
    /// `DrawStyle::default()`; read it through `Backend::fig_style`.
    ///
    /// Per tool and not global because that is what a drawing toolbar means everywhere: a red
    /// segment does not make the next Fibonacci red.
    fig_styles: std::collections::HashMap<&'static str, moon_core::figures::DrawStyle>,
    /// Per-tool switch defaults, keyed the same way: what a Fibonacci drawn next will have switched
    /// off, and so on for any tool that offers switches.
    ///
    /// Sparse — only what was changed away from the tool's own default — and, like `fig_styles`,
    /// held for the session rather than persisted: the two are one setting to a user, and half of
    /// it surviving a restart would be worse than neither.
    fig_tool_settings: std::collections::HashMap<&'static str, moon_core::figures::ToolSettings>,
    /// Application-wide selected figure as `(core, market, id)`, used for chart highlighting and
    /// handles as well as Shell deletion and alert hotkeys.
    fig_selected: Option<(CoreId, String, u64)>,
    /// Application-wide chart panel under the cursor, set and cleared by infrequent `on_hover`
    /// enter and leave events. Cursor-dependent hotkeys such as `new_long` and `new_short` use it
    /// to place an order at the pointer price regardless of focus. The weak handle may expire.
    hovered_chart: Option<WeakEntity<crate::panels::ChartPanel>>,
    /// The last chart the pointer entered in each OS window, retained after `hovered_chart` is
    /// cleared on leave and kept per window rather than once for the application.
    ///
    /// The chart shot needs it because a screenshot hotkey is not a cursor gesture: the user
    /// presses it after moving the pointer to a toolbar, a settings field, or off the chart
    /// entirely, and still means the chart they were just working in. The weak handle dies with
    /// the panel, so a closed chart resolves to `None` without anyone clearing it.
    ///
    /// Keyed BY WINDOW because `hovered_chart` is application-global while a keystroke is not: the
    /// last chart hovered anywhere may sit in a detached window that is now behind the group window
    /// the key actually reached. Capturing that one would grab an occluded rectangle belonging to a
    /// window the user is not looking at, so each dispatcher looks up its OWN window and finds
    /// nothing rather than something wrong.
    last_chart: HashMap<gpui::AnyWindowHandle, WeakEntity<crate::panels::ChartPanel>>,
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
    /// Cached answer to "is quiet mode silencing sounds right now", recomputed once per
    /// coordination tick and on every user action; see `backend::quiet`. The detect-sound path runs
    /// inside the feed drain, so it must not do a time-zone conversion per wake.
    quiet_sleeping: bool,
    /// Minute of day (header-clock zone) observed at the previous quiet tick. It gates the 10 Hz
    /// tick down to once a minute; the transitions themselves are compared against absolute
    /// instants, so nothing depends on this value having seen every minute.
    quiet_last_min: u16,
    /// Configuration changed in memory and awaiting a debounced save.
    /// Frequent edits such as mouse-wheel order-size changes write to disk once per coordination
    /// tick; the drain calls `config.save()` and clears this flag.
    config_dirty: bool,
    /// Whether `on_app_quit` is shutting down the application.
    /// Detached windows must not repin while exiting, or their detached state would be cleared and
    /// not restored on the next start; the repin drain checks this flag.
    quitting: bool,
}

/// Notification-only entity for committed report revisions.
struct ReportRevision;

/// Notification-only entity for retained market-data revisions.
struct MarketDataRevision;

/// Notification-only entity for a changed application-wide display zone.
struct DisplayTimeRevision;

/// Notification-only entity for a changed cross-window core filter.
struct CoreFilterRevision;

/// Dispatch hidden updater modes or run the ordinary GPUI application.
///
/// Returns:
///     Success after the selected process role exits.
fn main() -> anyhow::Result<()> {
    // Before the updater, before the configuration, before a window: the UI-atlas tools that work
    // on a file the crawl already wrote need none of it, and running them through a normal launch
    // would put a six-minute walk between a rule and its result.
    #[cfg(uidoc)]
    if uidoc::run_offline_tools(std::env::args()) {
        return Ok(());
    }
    match update::dispatch_process_mode()? {
        update::ProcessDispatch::Run(receipt) => startup::run(receipt),
        update::ProcessDispatch::Exit => Ok(()),
    }
}
