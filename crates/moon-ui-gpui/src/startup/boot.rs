//! Everything the terminal builds once the configuration is available.
//!
//! Split out of `startup::run` when the login window arrived. That window can only exist inside
//! `App::run`, and there is exactly one of those per process, so the work that used to follow
//! `AppConfig::load` had to become something callable LATER — either straight away, or at the
//! moment the user finishes typing a password. That is this function.
//!
//! Nothing here changed in the move except its indentation and where its inputs come from.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::Root;

use super::{
    ReportRevisionGate, TickEdges, apply_persistence_ack, consume_report_commit,
    dispatch_live_persistence, install_moon_theme_for_config,
};
use crate::persistence::coordinator::{PersistenceCoordinator, PersistenceSnapshot};
use crate::persistence::{chart_persist, dock_persist};
use crate::window::detached;
use crate::{Backend, UiSessionState, firetest};
use moon_core::config::{AppConfig, WindowLayout};
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::session::{CoreId, SessionManager};

/// Startup state gathered before the configuration was available.
pub(super) struct BootInput {
    /// Window layout, read before the config so its core uids could raise the uid floor.
    pub layout: WindowLayout,
    /// Chart-tab state read alongside the layout.
    pub chart_specs: Vec<chart_persist::ChartTabSpec>,
    /// User chart figures read alongside the layout.
    pub figures: moon_core::figures::FigureStore,
    /// Shared time origin for sessions and chart views.
    pub epoch: f64,
    /// FireTest scenario, when the process was launched as a diagnostic run.
    pub firetest: Option<firetest::Config>,
    /// Permit proving this process owns the reports lease and preserved any damaged replica.
    ///
    /// Taken before the event loop, where it belongs: the damaged main/WAL/SHM set must be
    /// preserved after the uid floor has been read from it and before any writer exists, and none
    /// of that depends on the configuration being open.
    pub report_write_permit: Option<moon_core::db::report_recovery::ReportWritePermit>,
}

/// Build sessions, windows and the coordination loop for a configuration that is now open.
pub(super) fn boot(cfg: AppConfig, input: BootInput, cx: &mut App) {
    let BootInput {
        layout,
        chart_specs: saved_chart_specs,
        figures,
        epoch,
        firetest: firetest_config,
        report_write_permit,
    } = input;

    // FireTest drives production surfaces but must not create durable backups. Normal startup
    // starts the shared UTC-noon scheduler before sessions can initialize a fresh strategy database.
    if firetest_config.is_none() {
        moon_core::backups::start_daily(&cfg);
    }
    // Apply the configured UI language to the global rust-i18n locale used by t! here and in MoonUI.
    rust_i18n::set_locale(cfg.language.code());
    // Configure file logging from the config and purge old log files once at startup.
    moon_core::applog::set_file_logging(cfg.log_to_file, cfg.log_retention_days);
    moon_core::applog::purge_old();
    let group_list = crate::window::group_window::groups(&cfg);
    log::info!("groups: {group_list:?} (servers: {})", cfg.servers.len());

    install_moon_theme_for_config(&cfg, cx);
    let persistence = Rc::new(RefCell::new(PersistenceCoordinator::start()));

    let (dock_states, detached) = crate::persistence::window_state_persist::load_all();
    let auto_dock_startup = crate::persistence::auto_dock_persist::load().into_startup_state();

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
    let market_data_revision = cx.new(|_| crate::MarketDataRevision);
    let display_time_revision = cx.new(|_| crate::DisplayTimeRevision);
    let workspace_revision = cx.new(|_| crate::workspace::WorkspaceRevision::default());
    let auto_workspace_layout_revision =
        cx.new(|_| crate::workspace::AutoWorkspaceLayoutRevision::default());
    let core_filter_revision = cx.new(|_| crate::CoreFilterRevision);
    crate::chartdx::axes::set_display_zone(crate::chrome::clock::resolved_header_clock_zone(
        layout.header_clock_zone.as_deref(),
    ));
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
        market_data_revision: market_data_revision.clone(),
        display_time_revision: display_time_revision.clone(),
        workspace_revision: workspace_revision.clone(),
        auto_workspace_layout_revision: auto_workspace_layout_revision.clone(),
        core_filter: HashSet::new(),
        core_filter_revision,
        workspace_focus: None,
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
        open_main_request: crate::backend::OpenMainRequest::default(),
        open_compare_request: None,
        open_compare_request_rev: 0,
        diag_open_first_market: std::env::var_os("MOON_RENDER_DIAG_OPEN_FIRST_MARKET").is_some(),
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
        auto_dock_topology: auto_dock_startup.topology,
        auto_dock_automatic_persistence_allowed: auto_dock_startup.automatic_persistence_allowed,
        auto_dock_dirty: false,
        price_scale: None,
        price_scale_group: None,
        price_scale_rev: 0,
        switch_charts_group: None,
        switch_charts_rev: 0,
        close_all_charts_rev: 0,
        close_active_chart_group: None,
        close_active_chart_rev: 0,
        last_chart_close: None,
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
        opening_group_windows: HashSet::new(),
        settings_window: None,
        strategies_window: None,
        strategies_goto: None,
        assets_window: None,
        screener_window: None,
        analytics_window: None,
        profit_monitor_window: None,
        profit_monitor_open_pending: None,
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
        fig_styles: std::collections::HashMap::new(),
        fig_tool_settings: std::collections::HashMap::new(),
        fig_selected: None,
        last_chart_alerts_activity: 0,
        last_detect_seq: std::collections::HashMap::new(),
        last_detect_rev: std::collections::HashMap::new(),
        default_alert_sound: "ding1".to_string(),
        // Seeded right after construction from the persisted schedule, once the clock zone is
        // settled; see `refresh_quiet_state` below.
        quiet_sleeping: false,
        quiet_last_min: 0,
        config_dirty: false,
        quitting: false,
    });
    backend.update(cx, |b, _| b.refresh_header_ticker_default(true));
    // Settle the header clock fields once before any window reads them: detect an untouched
    // profile's OS zone, migrate an old nonzero offset, or refresh a saved zone's offset mirror.
    crate::chrome::clock::reconcile_clock_zone(&backend, cx);
    // Quiet mode reads the wall clock in that zone, so it is seeded only after the zone is settled:
    // a terminal started at 3 a.m. inside a sleep window must come up already silent, before the
    // first detect arrives.
    backend.update(cx, |b, _| b.refresh_quiet_state());

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
        let (to_close, quit) = quit_backend.update(app, |b, bcx| {
            if let Some((group, last_group_window)) = b.close_group_window(closed_id, bcx) {
                if last_group_window {
                    // The last group window triggers a full exit; quit closes everything detached
                    // too. Unregister the detached panel windows FIRST, for the same reason the
                    // multi-window branch below does: they die with the application, and a release
                    // that still finds itself in the map queues a repin — which docks the panel and
                    // DELETES its `DetachedSpec`, so the final save would persist every panel docked
                    // and the next launch would lose the detachment. `quitting` covers the exits
                    // that close no group window at all (the macOS menu, Cmd+Q).
                    b.quitting = true;
                    let doomed = detached::take_windows(b, |_| true);
                    detached::prune_requests(b, |_| true);
                    return (doomed, true);
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
    let app_quit_persistence = persistence.clone();
    cx.on_app_quit(move |cx| {
        moon_core::detect_diag::line("[quit] on_app_quit → сохраняю charts.json");
        let final_persistence = app_quit_backend.update(cx, |b, _| {
            b.quitting = true;
            // One of the two DEBOUNCED flush sites; the other is the coordinator tick below.
            // Not reached by FireTest at all, which exits through `std::process::exit` — kept
            // gated so the rule holds however the run ends.
            if !b.persist_allowed {
                return PersistenceSnapshot::empty();
            }
            if b.config_dirty {
                if let Err(e) = b.config.save() {
                    log::warn!("config save (quit) failed: {e}");
                } else {
                    b.config_dirty = false;
                }
            }
            chart_persist::save_all(&b.chart_specs);
            if b.tab_badges_dirty {
                b.tab_badges.save();
                b.tab_badges_dirty = false;
            }
            if b.figures.borrow().dirty {
                b.figures.borrow_mut().save();
            }
            // The full final authorities supersede any debounced snapshot already in flight.
            // The serial worker receives this request behind prior work, so no second writer
            // can race the same atomic temp paths during shutdown.
            let mut snapshot = PersistenceSnapshot::empty()
                .with_layout(b.layout.clone())
                .with_classic(b.dock_states.clone(), b.detached.clone());
            if b.auto_dock_automatic_persistence_allowed
                && let Some(topology) = b.auto_dock_topology.clone()
            {
                snapshot = snapshot.with_auto(topology);
            }
            snapshot
        });
        let final_acknowledgement = app_quit_persistence
            .borrow_mut()
            .shutdown(final_persistence);
        app_quit_backend.update(cx, |b, _| {
            apply_persistence_ack(b, final_acknowledgement);
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
                    if drain.market_data {
                        b.market_data_revision.update(cx, |_, cx| cx.notify());
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
    let coord_persistence = persistence.clone();
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
                    // Before the warning engine: a schedule boundary crossed on this very tick must
                    // already be in force for the alerts this tick opens.
                    b.tick_quiet(cx);
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
                        dispatch_live_persistence(b, &mut coord_persistence.borrow_mut());
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

    // Reopen the Profit Monitor when the last session left it open. It is an independent
    // desktop window with NO taskbar button of its own, so a launch that silently drops it
    // leaves the user nothing to notice — the window is simply gone until they remember the
    // toolbar. Deferred so the group windows exist first: `restore` takes one of them as the
    // display fallback, and it deliberately does not activate, which would steal the
    // foreground from Main on every launch.
    //
    // Gated on `persist_allowed` for the same reason that flag exists: a FireTest run drives
    // the real application, and resurrecting the developer's windows would put an unmeasured
    // one in front of its own scene.
    let restore_monitor = {
        let backend = backend.read(cx);
        backend.persist_allowed && backend.layout.profit_monitor_open
    };
    if restore_monitor {
        let backend = backend.clone();
        cx.defer(move |cx| {
            let owner = cx.active_window();
            crate::analytics::profit_monitor::restore(backend, owner, cx);
        });
    }
}

#[cfg(test)]
mod tests;
