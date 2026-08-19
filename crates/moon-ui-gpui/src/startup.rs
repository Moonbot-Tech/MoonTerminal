//! Application startup: logger and panic/SEH hooks, config loading, GPUI `App` startup,
//! shared [`Backend`] creation, background loops (feed wakes and coordination), and group windows.
//! Extracted verbatim from `main.rs` (the former `main()` body is now [`run`]).

use std::borrow::Cow;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use gpui::*;

use moon_ui::{MoonTheme, MoonThemeConfig, ThemeMode, init as init_moon_ui};

use moon_core::config::{AppConfig, UiThemeMode, WindowLayout};

use crate::diagnostics::crash;
use crate::persistence::chart_persist;
use crate::persistence::coordinator::{
    PersistenceAck, PersistenceCoordinator, PersistenceSnapshot,
};
use crate::{Backend, diag, firetest};

/// Minimum spacing between background report-data UI revision publications.
const BACKGROUND_REVISION_INTERVAL: Duration = Duration::from_secs(60);

/// Restore dirty state for persistence classes whose background write failed.
///
/// Args:
///     backend: Shared state whose current authority must be retried after failure.
///     acknowledgement: Per-class result returned by the serial worker.
///
/// Returns:
///     Nothing; failures mark the complete affected authority dirty for a later full snapshot.
fn apply_persistence_ack(backend: &mut Backend, acknowledgement: PersistenceAck) {
    let failed = acknowledgement.failed();
    if failed.layout {
        backend.layout_dirty = true;
    }
    if failed.classic {
        backend.dock_dirty = true;
        backend.detached_dirty = true;
    }
    if failed.auto {
        backend.auto_dock_dirty = true;
    }
}

/// Poll the serial worker and enqueue at most one immutable live persistence snapshot.
///
/// Dirty flags are cleared when their complete authority is accepted, not when an older write
/// later succeeds. A mutation arriving while that request is in flight therefore remains dirty.
/// Failed acknowledgements restore the affected class for retry. This function only communicates
/// over channels and never performs file I/O.
///
/// Args:
///     backend: Shared state containing complete authorities and their dirty flags.
///     coordinator: Application-thread dispatch side of the serial persistence worker.
///
/// Returns:
///     Nothing; accepted work completes asynchronously and is polled on a later tick.
fn dispatch_live_persistence(backend: &mut Backend, coordinator: &mut PersistenceCoordinator) {
    if let Some(acknowledgement) = coordinator.poll() {
        apply_persistence_ack(backend, acknowledgement);
    }
    if coordinator.is_in_flight() {
        return;
    }

    let save_layout = backend.layout_dirty;
    let save_classic = backend.dock_dirty || backend.detached_dirty;
    let auto_topology = (backend.auto_dock_dirty
        && backend.auto_dock_automatic_persistence_allowed)
        .then(|| backend.auto_dock_topology.clone())
        .flatten();
    let save_auto = auto_topology.is_some();
    let mut snapshot = PersistenceSnapshot::empty();
    if save_layout {
        snapshot = snapshot.with_layout(backend.layout.clone());
    }
    if save_classic {
        snapshot = snapshot.with_classic(backend.dock_states.clone(), backend.detached.clone());
    }
    if let Some(topology) = auto_topology {
        snapshot = snapshot.with_auto(topology);
    }
    if snapshot.is_empty() {
        return;
    }

    if save_layout {
        backend.layout_dirty = false;
    }
    if save_classic {
        backend.dock_dirty = false;
        backend.detached_dirty = false;
    }
    if save_auto {
        backend.auto_dock_dirty = false;
    }
    if !coordinator.dispatch(snapshot) {
        if save_layout {
            backend.layout_dirty = true;
        }
        if save_classic {
            backend.dock_dirty = true;
            backend.detached_dirty = true;
        }
        if save_auto {
            backend.auto_dock_dirty = true;
        }
    }
}

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
///
/// Args:
///     startup_update: Validated resume or recovery receipt dispatched before GPUI startup.
pub(crate) fn run(startup_update: Option<crate::update::StartupUpdate>) -> anyhow::Result<()> {
    // The fixture bench BEFORE everything else, including diagnostics: it replaces the data root,
    // and the very next line resolves a path under it. Preparing it any later would open — and,
    // through the report writer, WRITE TO — the developer's real databases first.
    // `args_os`: a non-UTF-8 argument makes `std::env::args` panic, and this call sits before the
    // panic hook. It is not the only such call — `update::dispatch_process_mode` collects `args()`
    // in `main` before startup is entered at all — but there is no reason to add a second one.
    let fixture =
        fixture::bootstrap(std::env::args_os().map(|arg| arg.to_string_lossy().into_owned()))?;

    // Diagnostics BEFORE the logger: the `[log]` areas in `cfg/diagnostics.toml` decide the
    // logger's filter, so they have to be known before it is built. Reading that file this early is
    // safe precisely because it neither creates directories nor logs — a missing file yields
    // all-off defaults, which is the right state for a first launch. The file itself is created
    // further down, after the config directory has settled.
    let (diag_cfg, diag_err) = moon_core::diagnostics::init();
    // The wrapper duplicates emitted records into the in-memory ring shown by the Log tab (ported
    // from egui main) and owns the filter, which is what makes the areas switchable while running.
    if let Err(e) = moon_core::applog::install(&moon_core::diagnostics::filter_string(&diag_cfg)) {
        eprintln!("не удалось установить логгер: {e}");
    }
    if let Some(e) = diag_err {
        log::warn!("{e}");
    }
    log::info!(
        "build: moonterminal={} release_base={} moonui={}",
        option_env!("MOONTERMINAL_GIT_REV").unwrap_or("unknown"),
        option_env!("MOONTERMINAL_RELEASE_BASE").unwrap_or("unknown"),
        option_env!("MOONUI_GIT_REV").unwrap_or("unknown")
    );
    // A resumed update becomes accepted before any portable storage migration or open. Rolling
    // the executable back after a newer schema touched cfg/data would be unsafe for the old build.
    crate::update::acknowledge_healthy(startup_update.as_ref())?;
    let update_recovered = startup_update
        .as_ref()
        .is_some_and(crate::update::StartupUpdate::recovered);
    if let Some(fixture) = fixture {
        // Announced HERE, not in `prepare`: that runs before `applog::install`, so its line would
        // go nowhere.
        fixture.announce();
    }
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
    // After the logger, so a failed write can be reported, and after the migrations purely so the
    // `cfg/` directory is settled before anything is added to it. `diagnostics.toml` is in neither
    // migration's file list, so no move could reach it either way.
    moon_core::diagnostics::ensure_file();
    // Announced only now, and from the ACTIVE state rather than the one `init` returned:
    // `ensure_file` may have found a file that appeared in between and re-applied it, and a
    // warning naming the switches has to name the ones actually in force.
    moon_core::diagnostics::announce(&moon_core::diagnostics::active());

    // Read the core-keyed stores BEFORE the config: `AppConfig::load` assigns uids to entries
    // that carry none and persists them, so the floor has to be known by then. These loads are
    // config-independent and their values are reused below rather than read twice.
    let layout = WindowLayout::load();
    let saved_chart_specs = chart_persist::load_all();
    let figures = moon_core::figures::FigureStore::load();
    let uid_floor = observed_uid_floor(&layout, &saved_chart_specs, &figures);

    // Preserve the old reports replica only after its core uids have contributed to `uid_floor`,
    // but before any writer exists. The private permit also proves this process owns the
    // interprocess lease; `spawn_writer` cannot be called without it.
    let report_write_permit = moon_core::db::report_recovery::prepare();

    // Use one time origin for sessions and chart views, equivalent to epoch_ms in egui.
    let epoch = moon_chart::paint::now_unix_ms();

    // Register MoonUI's embedded SVG icons as the AssetSource; otherwise `IconName::*` values
    // (such as the `cleanable` clear icon, CircleX) cannot find their SVGs and render empty.
    let app = gpui_platform::application().with_assets(moon_ui::MoonAssets);
    app.run(move |cx| {
        init_moon_ui(cx);
        // Say it out loud rather than relying on the default: on macOS this switch decides whether
        // a Control+left press is delivered as a right click with Control erased. Moonbot's
        // default move gesture for a sell line IS Ctrl+Left, so the terminal needs the raw press;
        // Control-click stops opening context menus on a Mac in exchange, which is the trade the
        // reference makes too.
        gpui::set_macos_control_click_as_secondary(false);
        cx.text_system()
            .add_fonts(embedded_fonts())
            .expect("failed to add embedded Moonbot fonts");
        // The configuration is loaded HERE rather than before the event loop, because opening it
        // may require asking the user for a password, and asking requires a window. `unlock` runs
        // whatever prompts are due and calls `boot` once the terminal is actually unlocked.
        unlock::start(
            uid_floor,
            boot::BootInput {
                layout,
                chart_specs: saved_chart_specs,
                figures,
                epoch,
                firetest: firetest_config,
                report_write_permit,
                update_recovered,
            },
            cx,
        );
    });
    Ok(())
}

mod boot;
mod fixture;
mod unlock;

#[cfg(test)]
mod tests;
