//! moon-station: the terminal's server-side half, headless.
//!
//! A PROBE first (`docs-internal/STATION.md`, phases 1–6): it runs the terminal's own `moon-core`
//! as it is, without moving a line, so what a server build actually needs — and what stands in
//! its way — is learned from a running binary rather than guessed. What it does:
//!
//! - replicates every core's reports into `reports.sqlite` — the same writer, the same checkpoints;
//! - backfills and keeps each closed trade's order traces (`order_traces.sqlite`);
//! - records the tape around every trade (`market::tape_recorder`, always on here) into
//!   `tape_recorder.sqlite`: the pair subscribed for the trade's lifetime, seeded once with the
//!   core's chart archive.
//!
//! With `[telegram]` in `station.toml` it also runs the bot (`tg.rs`, over `moon-tg`), and with
//! the Mini App on, the account the Mini App shows: orders, balances, strategies, the cores'
//! health, and the USDT valuation of its reports (`feed::station::Profile::Account`).
//!
//! It never elects a market provider — the terminal does that from its open charts — so no core
//! is asked to keep every market's trades; only the pairs of trades in progress are selected.
//!
//! Usage: `moon-station --data <dir> [--config <station.toml>]`. The directory is the station's
//! whole state: the databases under `data/`, the logs under `logs/`, `cfg/diagnostics.toml`, and
//! `station.toml` (the cores) unless `--config` names it elsewhere — the service keeps it in
//! `/etc/moon-station`, read-only to the station. Core keys come as systemd credentials
//! (`cores.rs`).
//!
//! Signals: SIGTERM (and SIGINT) stop it cleanly — the tape recorder files what it drained before
//! the process exits; the report replica and the order traces need no such step, the replica
//! resuming from its last committed checkpoint and the traces backfilled at the next start.
//! SIGHUP re-reads `station.toml`: cores removed or switched off disconnect, the tape window moves.
//! A core ADDED needs its credential, which systemd hands over only at a start, so that one takes
//! a restart.
//!
//! The allocator is musl's own. `mimalloc` was measured in its place (2026-09-28): the same CPU at
//! idle, and resident memory at twice the size and growing — not worth it on a 1 GB server.

mod cores;
mod signals;
mod tg;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How often the feeds' channels are drained — the terminal's own coordination cadence.
const DRAIN_EVERY: Duration = Duration::from_millis(100);
/// How often the diagnostics file is re-read.
const DIAG_POLL_EVERY: Duration = Duration::from_secs(1);
/// How often the connection summary is logged.
const STATUS_EVERY: Duration = Duration::from_secs(60);
/// How long a stopping station waits for the tape recorder's last writes. systemd waits 90 s
/// before it kills.
const STOP_WAIT: Duration = Duration::from_secs(15);

fn main() -> anyhow::Result<()> {
    // The bot's dictionary, built at the base of the stack before anything can reach a `t!`.
    moon_tg::warm_locales();
    let (data_root, config) = args()?;
    anyhow::ensure!(
        moon_core::config::paths::set_data_dir_override(data_root.clone()),
        "data root already set"
    );
    let (diag_cfg, diag_err) = moon_core::diagnostics::init();
    // The terminal's base filter raises only its own crates to `info`; the station's startup,
    // tape-window and minute status lines come from this one.
    let filter = format!(
        "{},moon_station=info",
        moon_core::diagnostics::filter_string(&diag_cfg)
    );
    if let Err(e) = moon_core::applog::install(&filter) {
        eprintln!("logger not installed: {e}");
    }
    moon_core::applog::set_file_logging(true, 14);
    if let Some(e) = diag_err {
        log::warn!("{e}");
    }
    moon_core::diagnostics::ensure_file();
    log::info!(
        "moon-station {} starting, data root {}",
        env!("CARGO_PKG_VERSION"),
        data_root.display()
    );

    let signals = signals::Signals::install()?;
    let config_path = config.unwrap_or_else(|| data_root.join("station.toml"));
    let station = cores::load(&config_path)?;
    // The terminal's window around a trade, before the recorder builds its first one.
    apply_tape(&station.tape);
    let profile = station.profile();
    let telegram = station.telegram;
    let mut cfg = station.config;
    log_cores(&cfg);
    log::info!("profile: {profile:?}");

    // Before any core is spawned: every feed reads it when its client is built.
    moon_core::feed::station::enable(profile);
    moon_core::market::tape_recorder::set_always_on();
    // The interprocess lease on the replica: a second station, or a terminal, on the same data
    // root is refused here rather than corrupting the file.
    let Some(permit) = moon_core::db::report_recovery::prepare() else {
        anyhow::bail!("the report replica is held by another process on this data root");
    };
    let reports = moon_core::db::spawn_writer(permit)
        .ok_or_else(|| anyhow::anyhow!("report writer did not start"))?;
    // The USDT valuation of reports whose quote is not USDT, for the Mini App's report. The light
    // station stages no outbox for it, so it runs none.
    let valuation = profile
        .runs_account()
        .then(|| moon_core::db::valuation::spawn_worker(reports.tx.clone()))
        .flatten();
    let epoch = moon_core::util::now_unix_ms_i64() as f64;
    let mut session =
        moon_core::session::SessionManager::start(&cfg, epoch, Some(&reports.tx), None);
    // What the terminal gets from electing providers for its open charts — a catalog to resolve
    // a trade's market in and the core's exchange to address it by — the station gets without
    // the election, and so without any core's exchange-wide trade stream. Once now, before the
    // feeds deliver anything, and again whenever a core names its exchange.
    session.map_cores_to_themselves();
    // A saved pairing that cannot be read keeps the bot off, not the station.
    let mut bot = telegram.as_ref().and_then(|telegram| {
        match tg::StationTg::start(&mut cfg, telegram, &data_root) {
            Ok(bot) => Some(bot),
            Err(e) => {
                log::error!("telegram: bot not started: {e:#}");
                None
            }
        }
    });

    let mut groups = groups_of(&cfg);
    let mut last_diag = Instant::now();
    let mut last_status = Instant::now();
    loop {
        if signals.stop_requested() {
            log::info!("stopping");
            // The bot's long poll winds down while the tape recorder files its last writes; the
            // join comes after, so neither waits on the other.
            if let Some(bot) = bot.as_mut() {
                bot.request_stop();
            }
            if !moon_core::market::tape_recorder::shutdown(STOP_WAIT) {
                log::warn!("the tape recorder did not finish its last writes in time");
            }
            if let Some(bot) = bot.as_mut() {
                bot.stop();
            }
            log::info!("stopped");
            return Ok(());
        }
        if signals.take_reload() {
            match reload(&config_path) {
                Ok(reloaded) => {
                    if !same_bot(reloaded.telegram.as_ref(), telegram.as_ref()) {
                        log::warn!(
                            "[telegram] changed: it takes effect on the next start, not a reload"
                        );
                    }
                    let mut reloaded = reloaded.config;
                    // The process runs the profile it started with; the bot keeps its pairing.
                    cores::set_feed(&mut reloaded, profile);
                    reloaded.telegram = cfg.telegram.clone();
                    session.reconcile(&reloaded, Some(&reports.tx));
                    session.map_cores_to_themselves();
                    groups = groups_of(&reloaded);
                    // Kept for the identity respawns below, which rebuild a core from it.
                    cfg = reloaded;
                }
                // The running set stays as it was: a half-written file must not drop every core.
                Err(e) => log::error!("reload of {} failed: {e:#}", config_path.display()),
            }
        }
        if session.drain().identity {
            session.map_cores_to_themselves();
        }
        // A core whose exchange identity went stale (restart or hot exchange switch) is rebuilt on
        // a fresh client, debounced per core by the session; its new venue then re-maps above.
        for id in session.take_identity_respawn_requests(Instant::now()) {
            session.reconnect(id, &cfg, Some(&reports.tx));
        }
        if let Some(bot) = bot.as_mut() {
            // The Mini App's Reconnect: the same rebuild on a fresh client.
            for id in bot.tick(&mut cfg, &mut session) {
                session.reconnect(id, &cfg, Some(&reports.tx));
            }
        }
        // A committed report page wakes the valuation, as the terminal's coordination tick does.
        let committed = reports.immediate_commit_dirty.swap(false, Ordering::AcqRel)
            | reports
                .background_commit_dirty
                .swap(false, Ordering::AcqRel);
        if committed {
            if let Some(valuation) = &valuation {
                valuation.wake();
            }
        }
        let now = Instant::now();
        if now.duration_since(last_diag) >= DIAG_POLL_EVERY {
            last_diag = now;
            if let Some(changed) = moon_core::diagnostics::poll() {
                moon_core::diagnostics::announce(&changed);
            }
        }
        if now.duration_since(last_status) >= STATUS_EVERY {
            last_status = now;
            let (mut ready, mut total, mut down) = (0, 0, Vec::new());
            for summary in groups.iter().map(|g| session.conn_summary_group(g)) {
                ready += summary.ready;
                total += summary.total;
                down.extend(summary.down.into_iter().map(|d| match d.fault {
                    Some(fault) => format!("{} {:?} ({fault:?})", d.name, d.status),
                    None => format!("{} {:?}", d.name, d.status),
                }));
            }
            match down.is_empty() {
                true => log::info!("status: {ready}/{total} cores ready"),
                false => log::info!(
                    "status: {ready}/{total} cores ready; not ready: {}",
                    down.join("; ")
                ),
            }
        }
        std::thread::sleep(DRAIN_EVERY);
    }
}

/// Re-read `station.toml` and apply its tape window; the cores are for the caller to reconcile.
fn reload(path: &Path) -> anyhow::Result<cores::Station> {
    let station = cores::load(path)?;
    log::info!("reloaded {}", path.display());
    apply_tape(&station.tape);
    log_cores(&station.config);
    Ok(station)
}

/// Whether two `[telegram]` sections run the same bot the same way.
fn same_bot(a: Option<&cores::Telegram>, b: Option<&cores::Telegram>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_as(b),
        _ => false,
    }
}

/// The terminal's window around a trade; absent fields keep what is in force.
fn apply_tape(tape: &cores::Tape) {
    if let Some(secs) = tape.margin_s {
        moon_core::market::trade_replay::set_margin_s(secs);
    }
    if let Some(minutes) = tape.long_position_min {
        moon_core::market::trade_replay::set_long_position_min(minutes);
    }
    log::info!(
        "tape window: margin {} s, long position from {} min",
        moon_core::market::trade_replay::margin_ms() / 1_000,
        moon_core::market::trade_replay::long_position_ms() / 60_000
    );
}

fn log_cores(cfg: &moon_core::config::AppConfig) {
    log::info!(
        "cores: {} configured, {} active",
        cfg.servers.len(),
        cfg.servers.iter().filter(|s| s.active).count()
    );
}

/// The core groups the status line sums over.
fn groups_of(cfg: &moon_core::config::AppConfig) -> Vec<String> {
    let mut groups: Vec<String> = cfg.servers.iter().map(|s| s.group.clone()).collect();
    groups.sort();
    groups.dedup();
    groups
}

/// `--data <dir>`, required: the station never guesses where its state lives. `--config <file>`,
/// optional: `station.toml` somewhere else.
fn args() -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
    const USAGE: &str = "usage: moon-station --data <dir> [--config <station.toml>]";
    let (mut data, mut config) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let slot = match arg.as_str() {
            "--data" => &mut data,
            "--config" => &mut config,
            other => anyhow::bail!("unknown argument {other:?}; {USAGE}"),
        };
        let value = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("{arg} needs a path; {USAGE}"))?;
        *slot = Some(PathBuf::from(value));
    }
    let data = data.ok_or_else(|| anyhow::anyhow!(USAGE))?;
    std::fs::create_dir_all(&data)?;
    Ok((data, config))
}
