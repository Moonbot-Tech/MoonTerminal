//! The market-list prices a terminal pulls from its station, once per run (STATION.md §4.9).
//!
//! The station records prices around every trade all the time; the terminal only while it runs.
//! So, a while after start, the terminal asks the station for everything it holds — as far back
//! as the station's tape settings keep it — works out what its own file lacks, and files the
//! station's answer beside its own (`moon_core::market::price_recorder::remote`). A terminal set up
//! fresh on another machine so takes the station's whole history. The trade window reads the file
//! when it opens a trade, and a pull that filed something wakes the windows already open
//! ([`PriceImports`]) to read it again.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::Entity;

use moon_core::diagnostics::PRICE_PULL_TARGET as LOG;
use moon_core::market::price_recorder::remote;

/// After start: the replica's catch-up and the traces pull come first.
const DELAY: Duration = Duration::from_secs(90);
/// Between attempts after a failure: the station out of reach, a broken answer.
const RETRY: Duration = Duration::from_secs(120);
/// Attempts per run.
const ATTEMPTS: u32 = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    /// Not seen yet: the clock starts at the first tick.
    #[default]
    Unstarted,
    /// Attempt `n` (from 1) is due at this instant.
    Waiting(Instant, u32),
    /// Attempt `n` is on its thread.
    Running(u32),
    /// Attempt `n` ended; `true` when it went through to the end.
    Ended(u32, bool),
    /// Done for this run.
    Done,
}

static PHASE: Mutex<Phase> = Mutex::new(Phase::Unstarted);
/// Set by an attempt that filed prices, taken by the next [`tick`].
static FILED: AtomicBool = AtomicBool::new(false);

/// Wake channel of the pull: notified once a pull filed prices. Only the trade windows observe
/// it — they read the file when they open a trade and re-read it on this — so a pull wakes nothing
/// else.
pub(crate) struct PriceImports;

impl crate::Backend {
    /// The pull's wake channel, for a trade window to observe.
    pub(crate) fn price_imports(&self) -> Entity<PriceImports> {
        self.price_imports.clone()
    }
}

fn lock() -> std::sync::MutexGuard<'static, Phase> {
    PHASE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The coordination tick's call: a clock compare while waiting, an attempt on its own thread once
/// due, and after a failed one another later.
///
/// Returns:
///     Whether an attempt filed prices since the last call — the caller wakes [`PriceImports`].
pub(crate) fn tick() -> bool {
    let mut phase = lock();
    *phase = next(*phase, Instant::now(), spawn);
    FILED.swap(false, Ordering::Relaxed)
}

/// The phase after `phase` at `now`; `start(n)` launches attempt `n` and says whether it did.
fn next(phase: Phase, now: Instant, start: impl FnOnce(u32) -> bool) -> Phase {
    match phase {
        Phase::Unstarted => Phase::Waiting(now + DELAY, 1),
        Phase::Waiting(due, n) if now < due => Phase::Waiting(due, n),
        Phase::Waiting(_, n) => {
            if start(n) {
                Phase::Running(n)
            } else {
                Phase::Done
            }
        }
        Phase::Ended(n, done) if !done && n < ATTEMPTS => Phase::Waiting(now + RETRY, n + 1),
        Phase::Ended(..) => Phase::Done,
        other => other,
    }
}

/// Launch attempt `n`: `false` when there is no station, or no thread to run on.
fn spawn(n: u32) -> bool {
    // Read when due: a server set up meanwhile counts.
    let Some(target) = super::known_target() else {
        return false;
    };
    let spawned = std::thread::Builder::new()
        .name("station-prices".into())
        .spawn(move || {
            let outcome = pull(&target);
            if let Err(e) = &outcome {
                log::warn!(target: LOG, "station: price pull, attempt {n} of {ATTEMPTS}: {e:#}");
            }
            *lock() = Phase::Ended(n, outcome.is_ok());
        });
    match spawned {
        Ok(_) => true,
        Err(e) => {
            log::warn!(target: LOG, "station: price pull thread did not start: {e}");
            false
        }
    }
}

/// One pull: the station's listing, what this file lacks of it, the station's answer filed.
fn pull(target: &moon_remote::ssh::Target) -> anyhow::Result<()> {
    let pull = moon_remote::station::pull::Pull::open(target)?;
    if !pull.prices_supported()? {
        log::info!(target: LOG, "station: price pull skipped, the station is older than it");
        return Ok(());
    }
    let now_ms = moon_core::util::time::now_unix_ms_i64();
    // Everything the station holds: its retention, set with its tape's, decides how far back.
    let held = pull.price_index(i64::MIN, || false)?;
    let listed = held.len();
    let wants = remote::lacking(held, now_ms)
        .map_err(|e| anyhow::anyhow!("read the local price file: {e}"))?;
    if wants.is_empty() {
        log::info!(target: LOG, "station: price pull, nothing lacking ({listed} market listing(s))");
        return Ok(());
    }
    let asked = wants.len();
    let mut importer =
        remote::Importer::open().map_err(|e| anyhow::anyhow!("open the local price file: {e}"))?;
    let pulled = pull.prices(
        wants,
        || false,
        |want, piece| {
            importer
                .add(want, piece)
                .map_err(|e| anyhow::anyhow!("file the station's prices: {e}"))
        },
    );
    // What came before a broken answer is filed too.
    let finished = importer
        .finish()
        .map_err(|e| anyhow::anyhow!("file the station's prices: {e}"));
    if importer.filed > 0 {
        FILED.store(true, Ordering::Relaxed);
    }
    let count = pulled?;
    finished?;
    log::info!(
        target: LOG,
        "station: price pull, {} piece(s) for {asked} market(s) in {} request(s): {} filed, {} refused, {} already held",
        count.pieces,
        count.requests,
        importer.filed,
        importer.refused,
        importer.overlapped
    );
    Ok(())
}

#[cfg(test)]
mod tests;
