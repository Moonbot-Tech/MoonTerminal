//! Application startup: logger and panic/SEH hooks, config loading, GPUI `App` startup,
//! shared [`Backend`] creation, background loops (feed wakes and coordination), and group windows.
//! Extracted verbatim from `main.rs` (the former `main()` body is now [`run`]).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use gpui::*;

use moon_ui::{MoonTheme, MoonThemeConfig, Root, ThemeMode, init as init_moon_ui};

use moon_core::config::{AppConfig, UiThemeMode, WindowLayout};
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::session::{CoreId, SessionManager};

use crate::diagnostics::crash;
use crate::persistence::{chart_persist, dock_persist};
use crate::window::detached;
use crate::{Backend, UiSessionState, diag, firetest};

/// Minimum spacing between background report-data UI revision publications.
const BACKGROUND_REVISION_INTERVAL: Duration = Duration::from_secs(60);

/// Side effects selected for one report/valuation coordination tick.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ReportRevisionDecision {
    /// Whether report-data observers must receive a revision notification.
    notify: bool,
    /// Whether any committed report change must wake the valuation worker.
    wake_valuation: bool,
}

/// The report-data, valuation-data, and health edges observed in one coordination tick.
///
/// A struct rather than four positional `bool`s: they share a type but have different publication
/// and wake semantics. Swapping a health edge with a report-commit edge would compile while making
/// a UI-only health transition unpark the valuation worker.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TickEdges {
    /// Live report data committed this tick.
    immediate_report: bool,
    /// Historical catch-up report data committed this tick.
    background_report: bool,
    /// The valuation worker published historical values, coverage, or a current-rate snapshot this
    /// tick.
    valuation: bool,
    /// Published valuation health changed shape this tick.
    valuation_status: bool,
}

/// Coalesce background report data without delaying live report commits or valuation work.
struct ReportRevisionGate {
    /// Whether a catch-up or valuation change still needs a UI revision publication.
    background_pending: bool,
    /// Time of the latest revision publication, including report-driven publications.
    last_published_at: Instant,
}

impl ReportRevisionGate {
    /// Create a clean gate aligned with application startup.
    ///
    /// Args:
    ///     now: Initial publication baseline.
    ///
    /// Returns:
    ///     A gate with no pending background revision.
    fn new(now: Instant) -> Self {
        Self {
            background_pending: false,
            last_published_at: now,
        }
    }

    /// Select the report-revision side effects for one coordination tick.
    ///
    /// An immediate report commit publishes at once and covers every background generation visible
    /// in that notification. Catch-up report pages, valuation work, and valuation health changes
    /// remain pending until the one-minute boundary, while every report commit still wakes
    /// valuation processing at once. A stall is minutes old before it is reportable, so deferring
    /// its publication by up to a minute costs nothing the user can perceive.
    ///
    /// Args:
    ///     edges: Commit and health edges consumed this tick.
    ///     now: Current coordination time.
    ///
    /// Returns:
    ///     The notification and valuation-wake side effects for this tick.
    fn observe(&mut self, edges: TickEdges, now: Instant) -> ReportRevisionDecision {
        let report_committed = edges.immediate_report || edges.background_report;
        // A health change carries no rows, so it never counts as a report commit and never wakes
        // the worker; it only has to reach the next revision publication, which is what makes a
        // stall visible to surfaces that would otherwise poll the data generation forever.
        self.background_pending |=
            edges.background_report || edges.valuation || edges.valuation_status;
        if edges.immediate_report {
            self.background_pending = false;
            self.last_published_at = now;
            return ReportRevisionDecision {
                notify: true,
                wake_valuation: report_committed,
            };
        }

        if self.background_pending
            && now.saturating_duration_since(self.last_published_at) >= BACKGROUND_REVISION_INTERVAL
        {
            self.background_pending = false;
            self.last_published_at = now;
            return ReportRevisionDecision {
                notify: true,
                wake_valuation: report_committed,
            };
        }

        ReportRevisionDecision {
            notify: false,
            wake_valuation: report_committed,
        }
    }
}

/// Consume one coalesced post-commit edge and notify report-data observers.
///
/// Args:
///     dirty: Optional writer edge; absent when report storage failed to initialize.
///     on_commit: Notification emitted exactly once for a set edge.
fn consume_report_commit(dirty: Option<&std::sync::atomic::AtomicBool>, on_commit: impl FnOnce()) {
    if dirty.is_some_and(|dirty| dirty.swap(false, Ordering::AcqRel)) {
        on_commit();
    }
}

fn embedded_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        include_bytes!("../../../assets/fonts/Inter-400.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Inter-500.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Inter-600.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-400.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-500.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-600.ttf")
            .as_slice()
            .into(),
    ]
}

pub(crate) fn moon_theme_config_for(cfg: &AppConfig) -> MoonThemeConfig {
    let mut theme = match cfg.ui_theme_mode {
        UiThemeMode::Dark => MoonThemeConfig::moon_terminal(),
        UiThemeMode::Light => MoonThemeConfig::moon_light(),
    };
    theme.mode = match cfg.ui_theme_mode {
        UiThemeMode::Dark => ThemeMode::Dark,
        UiThemeMode::Light => ThemeMode::Light,
    };
    theme
        .with_font_delta(cfg.ui_font_delta)
        .with_ui_scale(cfg.ui_scale)
}

pub(crate) fn install_moon_theme_for_config(cfg: &AppConfig, cx: &mut App) {
    MoonTheme::install_config(moon_theme_config_for(cfg), cx);
}

/// Fold one store's read into the uid floor, keeping an absent store quiet and a broken one loud.
fn store_floor(label: &str, read: moon_core::db::ReadResult<Option<u64>>) -> Option<u64> {
    match read {
        Ok(max) => max,
        // An absent store is the ordinary fresh-install state, not a failure.
        Err(moon_core::db::ReadFail::NotReady) => None,
        Err(e) => {
            log::warn!("uid floor: {label} не прочитаны ({e}) — счётчик uid не поднят по ним");
            None
        }
    }
}

/// Highest core uid observed across the durable stores loaded during startup.
///
/// The uid counter in `settings.toml` only arrived in `SCHEMA_VERSION` 15 and is seeded from the
/// servers that still exist, but nothing purges a deleted server's rows from the report replica,
/// the strategy history, or the persisted UI state. Feeding this floor into `AppConfig::load`
/// stops a new core from taking a deleted one's identity and inheriting its trades, P&L,
/// strategy versions and figures.
///
/// A store that cannot be read contributes nothing. That is a best-effort repair, not a
/// guarantee: the mark may only ever rise, so a missing contribution leaves the previous
/// behaviour intact rather than making it worse, and the next boot retries. The reports probe
/// distinguishes absence from metadata, open, and query failures. The strategies probe preserves
/// open and query failures after a lossy existence check. The three file loaders collapse absent,
/// unreadable, and empty states into empty values, although parse failures are logged first.
fn observed_uid_floor(
    layout: &WindowLayout,
    chart_specs: &[chart_persist::ChartTabSpec],
    figures: &moon_core::figures::FigureStore,
) -> Option<u64> {
    // NOT `db::open_reader`: that opens read-write, and this probe runs before `spawn_writer`,
    // so it would be the file's only connection — closing it would checkpoint the whole WAL
    // inline, before the first window.
    let reports = store_floor(
        "отчёты",
        moon_core::db::open_readonly().and_then(|conn| moon_core::db::max_core_uid(&conn)),
    );
    let strategies = store_floor("история стратегий", moon_core::strat_db::max_core_uid());
    [
        reports,
        strategies,
        layout.max_core_uid(),
        figures.max_core_uid(),
        chart_persist::max_core_uid(chart_specs),
    ]
    .into_iter()
    .flatten()
    .max()
}

/// Initialize persistence, configuration, application services, and the GPUI event loop.
///
/// Core-keyed durable state is loaded before configuration because config loading may assign and
/// persist missing uids; observing the floor afterwards would be too late to prevent reuse.
///
/// Returns:
///     `Ok(())` after the application event loop exits normally.
///
/// Errors:
///     Returns startup argument or configuration-loading failures before the event loop begins.
pub(crate) fn run() -> anyhow::Result<()> {
    // Build env_logger as a Logger (rather than calling .init()) and wrap it in TeeLogger, which
    // duplicates emitted records into the in-memory ring shown by the Log tab (ported from egui main).
    let env = env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("warn,moon_ui_gpui=info,moon_gpui=info,moon_core=info"),
    )
    .build();
    if let Err(e) = moon_core::applog::install(env) {
        eprintln!("не удалось установить логгер: {e}");
    }
    log::info!(
        "build: moonterminal={} moonui={}",
        option_env!("MOONTERMINAL_GIT_REV").unwrap_or("unknown"),
        option_env!("MOONUI_GIT_REV").unwrap_or("unknown")
    );
    let firetest_config = firetest::Config::from_args(std::env::args())?;
    if firetest_config.is_some() {
        diag::force_enable();
    }

    // Native crashes (an access violation in DirectX/the GPUI fork, such as presenting through a
    // stale window handle during reconnect) bypass Rust's panic hook: the process exits silently
    // and leaves `panic.log` empty. Install a top-level SEH filter so these crashes also reach
    // `panic.log` with their code, address, and backtrace. Do this first, before creating windows.
    crash::install_native_handler();

    // Panic hook: a GUI application has no console, so panic messages written to stderr disappear
    // (and with panic=abort this looks like native crash 0xc0000409 in ucrtbase). Write the panic
    // location and message to `panic.log` (in the cwd) and the shared log BEFORE aborting so the
    // exact source location remains visible.
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let loc = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "?".into());
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("<non-string>");
            // Force a backtrace without RUST_BACKTRACE: clamp panics report a location inside core,
            // while we need the CALLING frame in our code.
            let bt = std::backtrace::Backtrace::force_capture();
            // Both sinks redact on their own: `panic_log` owns the file, `TeeLogger` the log.
            // A panic message can quote foreign text carrying an endpoint.
            moon_core::applog::panic_log(&format!(
                "PANIC at {loc}: {payload}\n--- backtrace ---\n{bt}\n--- end ---"
            ));
            log::error!("PANIC at {loc}: {payload}");
            default_hook(info);
        }));
    }

    // Settle the storage layout FIRST. `AppConfig::load` runs these itself, but `layout.toml`
    // and `charts.json` are among the files `migrate_flat_to_cfg` moves into `cfg/`, and both
    // are read just below through their post-move paths — reading before the move would see
    // nothing on exactly the old installs the uid floor exists to repair. Both migrations are
    // idempotent, so the call inside `AppConfig::load` stays a no-op.
    moon_core::config::paths::migrate_bundle_data();
    moon_core::config::paths::migrate_flat_to_cfg();

    // Read the core-keyed stores BEFORE the config: `AppConfig::load` assigns uids to entries
    // that carry none and persists them, so the floor has to be known by then. These loads are
    // config-independent and their values are reused below rather than read twice.
    let layout = WindowLayout::load();
    let saved_chart_specs = chart_persist::load_all();
    let figures = moon_core::figures::FigureStore::load();
    let uid_floor = observed_uid_floor(&layout, &saved_chart_specs, &figures);

    let cfg = AppConfig::load(uid_floor)?;
    // Apply the configured UI language to the global rust-i18n locale used by t! here and in MoonUI.
    rust_i18n::set_locale(cfg.language.code());
    // Configure file logging from the config and purge old log files once at startup.
    moon_core::applog::set_file_logging(cfg.log_to_file, cfg.log_retention_days);
    moon_core::applog::purge_old();
    // Preserve the old reports replica only after its core uids have contributed to `uid_floor`,
    // but before any writer exists. The private permit also proves this process owns the
    // interprocess lease; `spawn_writer` cannot be called without it.
    let report_write_permit = moon_core::db::report_recovery::prepare();
    let group_list = crate::window::group_window::groups(&cfg);
    log::info!("groups: {group_list:?} (servers: {})", cfg.servers.len());

    // Use one time origin for sessions and chart views, equivalent to epoch_ms in egui.
    let epoch = moon_chart::paint::now_unix_ms();

    // Register MoonUI's embedded SVG icons as the AssetSource; otherwise `IconName::*` values
    // (such as the `cleanable` clear icon, CircleX) cannot find their SVGs and render empty.
    let app = gpui_platform::application().with_assets(moon_ui::MoonAssets);
    app.run(move |cx| {
        init_moon_ui(cx);
        install_moon_theme_for_config(&cfg, cx);
        cx.text_system()
            .add_fonts(embedded_fonts())
            .expect("failed to add embedded Moonbot fonts");

        let dock_states = dock_persist::load_all();
        let detached = detached::load_all();

        // One-time charts.json remap: before schema v11, tabs stored POSITIONAL CoreId values;
        // CoreId is now a stable uid. Rebind while server order still matches the order recorded
        // in the file (the flag is set only when upgrading from an older version).
        let chart_specs = {
            let mut specs = saved_chart_specs;
            if cfg.chart_core_remap_needed {
                chart_persist::remap_core_ids(&mut specs, &cfg.servers);
                chart_persist::save_all(&specs);
            }
            specs
        };

        // Start the report writer. The session receives its `tx` for typed `Event::Report`
        // replication, while Backend retains `generation` for report-derived views. The
        // The two one-bit signals distinguish immediate live data from coalesced catch-up data.
        // Generation remains the source of truth, and already-set bits safely coalesce bursts.
        let reports = report_write_permit.and_then(moon_core::db::spawn_writer);
        let report_immediate_dirty = reports
            .as_ref()
            .map(|reports| reports.immediate_commit_dirty.clone());
        let report_background_dirty = reports
            .as_ref()
            .map(|reports| reports.background_commit_dirty.clone());
        let valuation = reports
            .as_ref()
            .and_then(|reports| moon_core::db::valuation::spawn_worker(reports.tx.clone()));
        let valuation_dirty = valuation
            .as_ref()
            .map(|valuation| valuation.commit_dirty.clone());
        let valuation_status_dirty = valuation
            .as_ref()
            .map(|valuation| valuation.status_dirty.clone());
        // A mode restored from settings.toml has to reach the worker before anything renders, or
        // the first current-rate view would wait out a park for a snapshot nobody had asked for.
        if let Some(valuation) = &valuation {
            valuation.set_current_wanted(
                cfg.report_valuation_mode == moon_core::db::valuation::ValuationMode::Current,
            );
        }
        let report_revision = cx.new(|_| crate::ReportRevision);
        // Check the complete replica once because individual reads only detect
        // damage on pages reached by their query.
        moon_core::db::integrity::spawn_check();
        let (feed_wake_tx, feed_wake_rx) = std::sync::mpsc::channel::<()>();

        let backend = cx.new(|_| Backend {
            session: SessionManager::start(
                &cfg,
                epoch,
                reports.as_ref().map(|h| &h.tx),
                Some(feed_wake_tx.clone()),
            ),
            epoch,
            reports,
            valuation,
            report_revision: report_revision.clone(),
            metrics: Metrics::new(),
            snap: MetricsSnapshot::default(),
            // open = markets of OPEN chart panels, as in App::about_to_wait in egui.
            // Empty at startup; opening a coin populates it (ported with the chart panels).
            // set_open still elects a provider/exchange at startup, which calls
            // subscribe_all_trades (retaining all exchange trades as before so coins open instantly).
            desired: Vec::new(),
            chart_market_refs: HashMap::new(),
            chart_market_refs_epoch: 0,
            chart_orderbook_refs: HashMap::new(),
            desired_orderbook: Vec::new(),
            desired_open_dirty: true,
            last_open_sync: Instant::now() - Duration::from_secs(10),
            main_chart_targets: HashMap::new(),
            main_open_markets: HashMap::new(),
            config: cfg.clone(),
            preview: None,
            open_request: None,
            open_request_rev: 0,
            open_request_activate: false,
            open_compare_request: None,
            open_compare_request_rev: 0,
            diag_open_first_market: std::env::var_os("MOON_RENDER_DIAG_OPEN_FIRST_MARKET")
                .is_some(),
            diag_open_done: false,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            diag_open_10_btc: std::env::var_os("MOON_RENDER_DIAG_OPEN_10_BTC").is_some(),
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            diag_open_10_btc_done: false,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_fill_main_chart_group: None,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_fill_main_chart_rev: 0,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_main_chart_handles: HashMap::new(),
            layout: layout.clone(),
            layout_dirty: false,
            coin_suggest: HashMap::new(),
            ui_session: UiSessionState::default(),
            detects_view: moon_core::config::DetectViewFile::load(),
            news_tag_settings: moon_core::config::NewsTagSettings::load(),
            tab_badges: moon_core::config::TabBadgeSettings::load(),
            tab_badges_dirty: false,
            header_ticker_default: None,
            last_header_ticker_refresh: None,
            dock_states,
            dock_dirty: false,
            price_scale: None,
            price_scale_group: None,
            price_scale_rev: 0,
            switch_charts_group: None,
            switch_charts_rev: 0,
            close_all_charts_rev: 0,
            close_active_chart_group: None,
            close_active_chart_rev: 0,
            follow: true,
            order_size_rev: 0,
            order_size_edit_req: None,
            sell_edit_req: None,
            group_exit_sync: HashMap::new(),
            manual_strat_local: HashMap::new(),
            panic_armed: HashSet::new(),
            backend_dirty_since_notify: false,
            last_backend_notify: None,
            core_chart_hist: Default::default(),
            core_line_hist: Default::default(),
            server_ping_hist: Default::default(),
            server_exch_hist: Default::default(),
            warn: Default::default(),
            warn_store: crate::backend::core_warn::store::WarnStore::open(
                &moon_core::config::paths::core_warnings_db_path(),
            )
            .map_err(|e| log::warn!("core warnings db open failed: {e}"))
            .ok(),
            warn_pending_slices: Vec::new(),
            warn_last_prune_ms: 0,
            reconnect_request: Vec::new(),
            show_group_request: Vec::new(),
            group_windows: HashMap::new(),
            settings_window: None,
            strategies_window: None,
            strategies_goto: None,
            assets_window: None,
            screener_window: None,
            analytics_window: None,
            report_window: None,
            report_window_view: None,
            firetest: firetest_config.clone().map(firetest::Runtime::new),
            // A diagnostic run never persists: it drives the real app, so everything it does would
            // otherwise land in the developer's saved workspace.
            persist_allowed: firetest_config.is_none(),
            hovered_chart: None,
            detached,
            detached_dirty: false,
            repin_request: Vec::new(),
            panel_detach_request: Vec::new(),
            detached_panel_windows: HashMap::new(),
            chart_repin_request: Vec::new(),
            chart_apply_all: Vec::new(),
            chart_candle_apply_all: Vec::new(),
            chart_x_sync: None,
            chart_x_sync_rev: 0,
            detached_chart_windows: Vec::new(),
            last_main_input: std::collections::HashMap::new(),
            exclude_bl_delta: std::collections::HashMap::new(),
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_window: None,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_chart_windows: Vec::new(),
            chart_consumers: Vec::new(),
            chart_specs,
            chart_specs_dirty: false,
            figures: std::rc::Rc::new(std::cell::RefCell::new(figures)),
            fig_draw_mode: true,
            fig_tool: moon_core::figures::FigureTool::HLine,
            fig_style: moon_core::figures::DrawStyle::default(),
            fig_selected: None,
            last_chart_alerts_activity: 0,
            last_detect_seq: std::collections::HashMap::new(),
            last_detect_rev: std::collections::HashMap::new(),
            default_alert_sound: "ding1".to_string(),
            config_dirty: false,
            quitting: false,
        });
        backend.update(cx, |b, _| b.refresh_header_ticker_default(true));
        // Settle the header clock fields ONCE before any window reads them: derive a missing city
        // zone from the compatibility seed and refresh the chosen city's current offset mirror.
        crate::chrome::clock::reconcile_clock_zone(&backend, cx);

        // Register panel factories used to restore dock layouts (PanelRegistry is global).
        dock_persist::register_panels(cx, backend.clone(), epoch);

        // Tab over an order line cancels the order, matching Del. MoonRoot binds the "tab" key to
        // the root::Tab action (focus_next), and GPUI dispatches actions BEFORE on_key_down, ahead
        // of the hotkey resolver (`hotkeys::resolve` -> CancelHoveredOrder). Tab therefore never
        // reached the resolver and merely moved focus across controls. This interceptor runs BEFORE
        // actions: cancel the hovered order and stop the event when one exists; otherwise let it
        // through so Tab remains focus navigation.
        let tab_backend = backend.clone();
        cx.intercept_keystrokes(move |ev, _window, cx| {
            if ev.keystroke.key == "tab"
                && ev.keystroke.modifiers == Modifiers::default()
                && crate::hotkeys::cancel_hovered_order(&tab_backend, cx)
            {
                cx.stop_propagation();
            }
        })
        .detach();

        // Closing a MAIN (group) window triggers a full exit when it removes the last entry from
        // group_windows; quit then closes detached chart windows too. Detached chart windows never
        // request quit themselves because their ids are absent from group_windows.
        let quit_backend = backend.clone();
        cx.on_window_closed(move |app, closed_id| {
            // Return (detached windows to close, whether the app should quit).
            let (to_close, quit) = quit_backend.update(app, |b, _| {
                // Determine whether this is a group window and, if so, which group owns it.
                let group = b
                    .group_windows
                    .iter()
                    .find(|(_, h)| h.window_id() == closed_id)
                    .map(|(g, _)| g.clone());
                if let Some(group) = group {
                    b.group_windows.remove(&group);
                    if b.group_windows.is_empty() {
                        // The last group window triggers a full exit; quit closes everything detached too.
                        return (Vec::new(), true);
                    }
                    // Otherwise close detached charts belonging to this group only.
                    let mut close: Vec<WindowHandle<Root>> = b
                        .detached_chart_windows
                        .iter()
                        .filter(|(g, _)| *g == group)
                        .map(|(_, h)| *h)
                        .collect();
                    b.detached_chart_windows.retain(|(g, _)| *g != group);
                    // Their queued repins go too, for the same reason as the panels' below: no
                    // `ChartTabs` for this group survives to drain them, so they would wait for a
                    // future window of the same name and be replayed against it.
                    b.chart_repin_request.retain(|(g, _, _)| *g != group);
                    // The group's detached PANEL windows are OS-owned by the window that just closed
                    // and die with it. `take_windows` unregisters them first, so their release
                    // cannot queue a repin into a dock that no longer exists — such a request would
                    // sit in the queue until some later window for this group name replays it and
                    // deletes the panel's `DetachedSpec` out of context. The specs stay, so
                    // reopening the group — or the next launch — restores the panels detached.
                    close.extend(detached::take_windows(b, |g| g == group));
                    detached::prune_requests(b, |g| g == group);
                    (close, false)
                } else {
                    // A detached chart window (or another window) closed. Its registry entry is
                    // NOT removed here: that registry is the authority for "this window may repin",
                    // and the host itself clears its entry on release, on the same edge that queues
                    // the repin. Removing it here would make a user-driven close look like a
                    // deliberate teardown and silently drop the tab instead of returning it.
                    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                    {
                        if b.debug_window
                            .as_ref()
                            .is_some_and(|h| h.window_id() == closed_id)
                        {
                            b.debug_window = None;
                        }
                        // A debug chart window is the one exception to the rule above: its host is
                        // `DebugChartHost`, which has no release hook to clear the entry itself, so
                        // this branch must do it or the registry grows every time one is closed.
                        let was_debug_chart = b
                            .debug_chart_windows
                            .iter()
                            .any(|h| h.window_id() == closed_id);
                        b.debug_chart_windows.retain(|h| h.window_id() != closed_id);
                        if was_debug_chart {
                            b.detached_chart_windows
                                .retain(|(_, h)| h.window_id() != closed_id);
                        }
                    }
                    (Vec::new(), false)
                }
            });
            crate::window::windowing::close_all(to_close, app);
            if quit {
                app.quit();
            }
        })
        .detach();

        // On application exit, set quitting and save charts.json IMMEDIATELY. When quit begins, the
        // windows have not been removed yet, so detached=Some; otherwise closing detached windows
        // during exit repins them (detached -> None) and they are not restored. quitting also
        // suppresses repin draining (drain_chart_repin) so it cannot clear detached.
        let app_quit_backend = backend.clone();
        cx.on_app_quit(move |cx| {
            moon_core::detect_diag::line("[quit] on_app_quit → сохраняю charts.json");
            app_quit_backend.update(cx, |b, _| {
                b.quitting = true;
                // One of the two DEBOUNCED flush sites; the other is the coordinator tick below.
                // Not reached by FireTest at all, which exits through `std::process::exit` — kept
                // gated so the rule holds however the run ends.
                if !b.persist_allowed {
                    return;
                }
                if b.config_dirty {
                    if let Err(e) = b.config.save() {
                        log::warn!("config save (quit) failed: {e}");
                    } else {
                        b.config_dirty = false;
                    }
                }
                chart_persist::save_all(&b.chart_specs);
                // Flush debounced persistence because the 100 ms tick may not run after the final
                // change. This matters especially for detached.json: otherwise "repin a panel,
                // then close immediately" leaves a stale record that restores as a separate window.
                if b.layout_dirty {
                    b.layout.save();
                    b.layout_dirty = false;
                }
                if b.dock_dirty {
                    dock_persist::save_all(&b.dock_states);
                    b.dock_dirty = false;
                }
                if b.detached_dirty {
                    detached::save_all(&b.detached);
                    b.detached_dirty = false;
                }
                if b.tab_badges_dirty {
                    b.tab_badges.save();
                    b.tab_badges_dirty = false;
                }
                if b.figures.borrow().dirty {
                    b.figures.borrow_mut().save();
                }
            });
            async move {}
        })
        .detach();

        // Feed event path: feed threads send causal wakes after real MoonProto events.
        // Market-only wakes update MarketDataSource/store; visible charts pull it from
        // gpu_canvas.frame() without dirtying Backend/Shell. Account/order wakes still notify
        // Backend through the slow gate and update only chart order overlays here.
        let data_backend = backend.clone();
        cx.spawn(async move |cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let mut feed_wake_rx = feed_wake_rx;
            loop {
                let (rx, woke) = executor
                    .spawn(async move {
                        let woke = feed_wake_rx.recv().is_ok();
                        (feed_wake_rx, woke)
                    })
                    .await;
                feed_wake_rx = rx;
                if !woke {
                    break;
                }
                while feed_wake_rx.try_recv().is_ok() {}

                cx.update(|cx| {
                    data_backend.update(cx, |b, cx| {
                        let drain = b.session.drain();
                        if !drain.any {
                            return;
                        }
                        // The server-side chart-alert set changed, so decode remote figures again
                        // (alerts created in the core/Moonbot). Gate on activity to avoid decoding
                        // blobs on every ui_state tick.
                        let alerts_activity = b.session.store().chart_alerts_activity();
                        if alerts_activity != b.last_chart_alerts_activity {
                            b.last_chart_alerts_activity = alerts_activity;
                            b.sync_remote_alerts();
                        }
                        // Play core detect/alert sounds for new detects that specify a sound.
                        b.play_detect_sounds();
                        if drain.order_lines_data {
                            let chart_consumers = b.live_chart_consumers();
                            for chart in chart_consumers {
                                chart.sync_orders_if_visible(&b.session, false);
                            }
                        }
                        if drain.ui_state {
                            b.mark_backend_dirty(cx);
                        }
                    });
                });
            }
        })
        .detach();

        // Slow coordination path: provider roles, metrics, reconnects and persistence. This may
        // wake the GPUI tree through Backend notify, but it never stages high-rate chart pixels.
        let coord_backend = backend.clone();
        let coord_cfg = cfg.clone();
        let coord_layout = layout.clone();
        let coord_report_immediate_dirty = report_immediate_dirty;
        let coord_report_background_dirty = report_background_dirty;
        let coord_valuation_dirty = valuation_dirty;
        let coord_valuation_status_dirty = valuation_status_dirty;
        let coord_report_revision = report_revision;
        cx.spawn(async move |cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let mut report_revision_gate = ReportRevisionGate::new(Instant::now());
            let mut last_report = Instant::now();
            // Sum of assets_rev across all cores in the previous sample, used for assets_apply delta.
            let mut last_assets_rev_sum: u64 = 0;
            loop {
                executor.timer(Duration::from_millis(100)).await;
                cx.update(|cx| {
                    let mut edges = TickEdges::default();
                    consume_report_commit(coord_report_immediate_dirty.as_deref(), || {
                        edges.immediate_report = true;
                    });
                    consume_report_commit(coord_report_background_dirty.as_deref(), || {
                        edges.background_report = true;
                    });
                    consume_report_commit(coord_valuation_dirty.as_deref(), || {
                        edges.valuation = true;
                    });
                    consume_report_commit(coord_valuation_status_dirty.as_deref(), || {
                        edges.valuation_status = true;
                    });
                    let revision = report_revision_gate.observe(edges, Instant::now());
                    let (show_reqs, open_debug_10) = coord_backend.update(cx, |b, cx| {
                        if revision.wake_valuation {
                            if let Some(valuation) = &b.valuation {
                                valuation.wake();
                            }
                        }
                        b.maybe_diag_open_first_market(cx);
                        b.refresh_header_ticker_default(false);
                        b.sync_open_markets_if_due();
                        b.sync_group_manual_settings();
                        b.snap = b.metrics.sample(Instant::now());
                        b.tick_core_warnings(moon_chart::paint::now_unix_ms() as i64);
                        crate::firetest::tick_backend(b, cx);

                        let recon: Vec<CoreId> = b.reconnect_request.drain(..).collect();
                        for id in recon {
                            b.session
                                .reconnect(id, &b.config, b.reports.as_ref().map(|h| &h.tx));
                        }
                        // The debounced workspace flush. Guarded as a whole rather than per file so
                        // a newly persisted thing added below cannot forget the rule; the dirty
                        // flags stay set, so nothing is lost, it is simply never written.
                        //
                        // This is the flush a diagnostic run actually reaches. It does NOT make the
                        // run write-free: the report DB writer, `strat_db`, `AppConfig::load`'s own
                        // uid save, log purging and panels that write straight through all bypass
                        // the dirty-flag mechanism entirely. `order-cancel-lag` in particular
                        // places a real order that lands permanently in `reports.sqlite`.
                        if b.persist_allowed {
                            if b.layout_dirty {
                                b.layout.save();
                                b.layout_dirty = false;
                            }
                            if b.dock_dirty {
                                dock_persist::save_all(&b.dock_states);
                                b.dock_dirty = false;
                            }
                            if b.detached_dirty {
                                detached::save_all(&b.detached);
                                b.detached_dirty = false;
                            }
                            if b.chart_specs_dirty {
                                chart_persist::save_all(&b.chart_specs);
                                b.chart_specs_dirty = false;
                            }
                            if b.tab_badges_dirty {
                                b.tab_badges.save();
                                b.tab_badges_dirty = false;
                            }
                            if b.figures.borrow().dirty {
                                b.figures.borrow_mut().save();
                            }
                            if b.config_dirty {
                                // Debounce config saves: mouse-wheel resizing updates memory frequently,
                                // but writes to disk once per drain tick rather than on every wheel tick.
                                if let Err(e) = b.config.save() {
                                    log::warn!("config save (debounced) failed: {e}");
                                }
                                b.config_dirty = false;
                            }
                        }
                        b.flush_backend_notify(cx);
                        let reqs = std::mem::take(&mut b.show_group_request);
                        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                        let open_debug_10 = b.take_diag_open_10_btc();
                        #[cfg(not(any(
                            debug_assertions,
                            moon_profile_debug,
                            feature = "debug-tools"
                        )))]
                        let open_debug_10 = false;
                        (reqs, open_debug_10)
                    });
                    if revision.notify {
                        coord_report_revision.update(cx, |_, cx| cx.notify());
                    }

                    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                    if open_debug_10 {
                        log::info!("diag auto-open: spawning 10 live-market chart windows");
                        crate::diagnostics::debug_window::spawn_debug_chart_windows(
                            cx,
                            coord_backend.clone(),
                        );
                    }
                    for g in show_reqs {
                        crate::window::group_window::spawn_group_window(
                            cx,
                            &coord_backend,
                            &coord_cfg,
                            g,
                            epoch,
                            &coord_layout,
                            0.0,
                        );
                    }
                });
                if last_report.elapsed().as_millis() >= 1000 {
                    let ms = last_report.elapsed().as_secs_f64() * 1000.0;
                    last_report = Instant::now();
                    // Point-in-time context for the diagnostics line: process/system CPU, counts of
                    // windows and open chart panels, plus the assets_rev delta (Assets snapshots
                    // collected by feed threads during the interval, even with no window open).
                    // Calculate it BEFORE take_sample so assets_apply lands in the same sample.
                    let ctx = if crate::diag::is_enabled() {
                        cx.update(|cx| {
                            let windows = cx.windows().len();
                            coord_backend.update(cx, |b, _| {
                                let charts = b.live_chart_consumers().len();
                                let rev_sum: u64 =
                                    b.session.store().cores().map(|(_, d)| d.assets_rev).sum();
                                // A core may have reconnected and reset its revision; in that case
                                // the delta is undefined, so take the new sum without a bump.
                                if rev_sum >= last_assets_rev_sum {
                                    crate::diag::bump_by(
                                        &crate::diag::ASSETS_APPLY,
                                        rev_sum - last_assets_rev_sum,
                                    );
                                }
                                last_assets_rev_sum = rev_sum;
                                format!(
                                    "cpu={:.1} sys={:.1} windows={} charts={}",
                                    b.snap.cpu_process, b.snap.cpu_system, windows, charts
                                )
                            })
                        })
                    } else {
                        String::new()
                    };
                    if let Some(sample) = crate::diag::take_sample(ms) {
                        crate::diag::write_sample(ms, &sample, &ctx);
                        cx.update(|cx| {
                            coord_backend.update(cx, |b, _| {
                                crate::firetest::record_diag_sample(b, &sample);
                            });
                        });
                    }
                }
            }
        })
        .detach();
        // Open one window per group through the same helper used by the "show group" eye button.
        for (i, group) in group_list.into_iter().enumerate() {
            crate::window::group_window::spawn_group_window(
                cx,
                &backend,
                &cfg,
                group,
                epoch,
                &layout,
                i as f32 * 40.0,
            );
        }

        // Detached-panel WINDOWS are not opened here: each group window opened above reclaims its
        // own detached panels through `detached::respawn_all`, the single restore route shared with
        // the Settings paths and the show-group button. A second loop here would race it — its
        // deferred body flushes inside the first `open_window` that follows, before that window has
        // registered itself, and both loops would then open a window for every spec.
        //
        // What startup DOES own is repairing the file: docks.json and detached.json are written
        // independently with debouncing, so a spec can go stale after repinning a panel followed by
        // a quick exit. `respawn_all` declines to open such a panel because the dock will restore
        // it, but only startup knows the record itself should be dropped rather than kept waiting.
        let stale: Vec<(String, String)> = {
            let b = backend.read(cx);
            let docked = dock_persist::docked_panels(&b.dock_states);
            b.detached
                .iter()
                .filter(|s| docked.contains(&s.key_ref()))
                .map(|s| s.key())
                .collect()
        };
        if !stale.is_empty() {
            for (group, panel) in &stale {
                log::warn!(
                    "drop detached record: панель уже в доке (протухшая запись) group={group} panel={panel}"
                );
            }
            backend.update(cx, |b, _| {
                b.detached.retain(|s| !stale.contains(&s.key()));
                b.detached_dirty = true;
            });
        }
        // Observation channel for the Analytics coin table, gated by env exactly like
        // `MOON_RENDER_DIAG` (see `diag.rs`): open the window straight onto that panel so it
        // can be driven and read back without clicking through the UI by hand. Inert in every
        // build unless the variable is set — a normal run never opens this window on startup.
        if crate::analytics::probe_enabled() {
            let backend = backend.clone();
            cx.defer(move |cx| crate::analytics::open(backend, None, None, cx));
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests;
