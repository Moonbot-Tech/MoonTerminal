//! moon-station: the terminal's server-side half, headless.
//!
//! A PROBE first (`docs-internal/STATION.md`, phases 1–6): it runs the terminal's own `moon-core`
//! as it is, without moving a line, so what a server build actually needs — and what stands in
//! its way — is learned from a running binary rather than guessed. What it does:
//!
//! - replicates every core's reports into `reports.sqlite` — the same writer, the same checkpoints;
//! - backfills and keeps each closed trade's order traces (`order_traces.sqlite`);
//! - records the tape around every trade by archive requests alone
//!   (`market::tape_recorder`, always on here) into `tape_recorder.sqlite`.
//!
//! It never elects a market provider — the terminal does that from its open charts — so no core
//! is asked for its exchange's live trade stream.
//!
//! Usage: `moon-station --data <dir>`. The directory is the station's whole state: `station.toml`
//! (the cores), the databases under `data/`, the logs under `logs/`, `cfg/diagnostics.toml`.

mod cores;

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often the feeds' channels are drained — the terminal's own coordination cadence.
const DRAIN_EVERY: Duration = Duration::from_millis(100);
/// How often the diagnostics file is re-read.
const DIAG_POLL_EVERY: Duration = Duration::from_secs(1);
/// How often the connection summary is logged.
const STATUS_EVERY: Duration = Duration::from_secs(60);

fn main() -> anyhow::Result<()> {
    let data_root = data_root()?;
    anyhow::ensure!(
        moon_core::config::paths::set_data_dir_override(data_root.clone()),
        "data root already set"
    );
    let (diag_cfg, diag_err) = moon_core::diagnostics::init();
    if let Err(e) = moon_core::applog::install(&moon_core::diagnostics::filter_string(&diag_cfg)) {
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

    let cfg = cores::load(&data_root)?;
    log::info!(
        "cores: {} configured, {} active",
        cfg.servers.len(),
        cfg.servers.iter().filter(|s| s.active).count()
    );

    moon_core::market::tape_recorder::set_always_on();
    // The interprocess lease on the replica: a second station, or a terminal, on the same data
    // root is refused here rather than corrupting the file.
    let Some(permit) = moon_core::db::report_recovery::prepare() else {
        anyhow::bail!("the report replica is held by another process on this data root");
    };
    let reports = moon_core::db::spawn_writer(permit)
        .ok_or_else(|| anyhow::anyhow!("report writer did not start"))?;
    let epoch = moon_core::util::now_unix_ms_i64() as f64;
    let mut session =
        moon_core::session::SessionManager::start(&cfg, epoch, Some(&reports.tx), None);
    // What the terminal gets from electing providers for its open charts — a catalog to resolve
    // a trade's market in and the core's exchange to address it by — the station gets without
    // the election, and so without any core's exchange-wide trade stream. Once now, before the
    // feeds deliver anything, and again whenever a core names its exchange.
    session.map_cores_to_themselves();

    let groups: Vec<String> = {
        let mut groups: Vec<String> = cfg.servers.iter().map(|s| s.group.clone()).collect();
        groups.sort();
        groups.dedup();
        groups
    };
    let mut last_diag = Instant::now();
    let mut last_status = Instant::now();
    loop {
        if session.drain().identity {
            session.map_cores_to_themselves();
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

/// `--data <dir>`, required: the station never guesses where its state lives.
fn data_root() -> anyhow::Result<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--data" {
            let dir = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--data needs a directory"))?;
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir)?;
            return Ok(dir);
        }
    }
    anyhow::bail!("usage: moon-station --data <dir>")
}
