//! The order traces a terminal pulls from its station, once per run (STATION.md §4.4, §4.9).
//!
//! The station, connected all the time, archives every trade's traces at its close; the terminal
//! asks its cores only while it runs, and a core's archive is bounded in time. So, a while after
//! start — the replica caught up and the cores' own backfill under way — the terminal lists its
//! recent closed trades without lines ([`order_traces::lacking_lines`]), asks the station for
//! them, files what it holds (an empty answer from a core never wipes them, `store_answer`) and
//! shows them on the rows already on screen.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::Context;
use moon_core::db::order_traces::{self, TraceDbMsg};
use moon_core::feed::ArchivedOrderTrace;
use moon_core::session::CoreId;

use crate::Backend;

/// After start: the replica's catch-up and the cores' own trace backfill come first.
const DELAY: Duration = Duration::from_secs(60);
/// Between attempts after a failure: the replica not ready yet, the station out of reach.
const RETRY: Duration = Duration::from_secs(120);
/// Attempts per run.
const ATTEMPTS: u32 = 5;

/// One trade's lines the pull filed.
type Pulled = (CoreId, i64, Arc<[ArchivedOrderTrace]>);

#[derive(Default)]
enum Phase {
    /// Not seen yet: the clock starts at the first tick.
    #[default]
    Unstarted,
    /// Attempt `n` (from 1) is due at this instant.
    Waiting(Instant, u32),
    /// Attempt `n` is on its thread; the result lands in [`State::result`].
    Running(u32),
    /// Done for this run.
    Done,
}

#[derive(Default)]
struct State {
    phase: Phase,
    /// What an attempt filed, and whether it went through to the end.
    result: Option<(Vec<Pulled>, bool)>,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn lock() -> std::sync::MutexGuard<'static, State> {
    STATE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The coordination tick's call: a clock compare while waiting, an attempt once due, its lines
/// onto the rows on screen once done — and, after a failed attempt, another later.
pub(crate) fn tick(backend: &mut Backend, cx: &mut Context<Backend>) {
    let mut st = lock();
    match std::mem::take(&mut st.phase) {
        Phase::Unstarted => st.phase = Phase::Waiting(Instant::now() + DELAY, 1),
        Phase::Waiting(due, n) if Instant::now() < due => st.phase = Phase::Waiting(due, n),
        Phase::Waiting(_, n) => {
            // Read when due: a server set up meanwhile counts.
            let Some(target) = super::known_target() else {
                st.phase = Phase::Done;
                return;
            };
            let here = super::cores_sync::trace_cores(&backend.config);
            let listing = backend.station.cores_seen.clone();
            st.phase = Phase::Running(n);
            let spawned = std::thread::Builder::new()
                .name("station-traces".into())
                .spawn(move || {
                    let (filed, outcome) = pull(&target, &here, listing);
                    if let Err(e) = &outcome {
                        log::warn!("station: order traces pull, attempt {n} of {ATTEMPTS}: {e:#}");
                    }
                    lock().result = Some((filed, outcome.is_ok()));
                });
            if let Err(e) = spawned {
                log::warn!("station: traces pull thread did not start: {e}");
                st.phase = Phase::Done;
            }
        }
        Phase::Running(n) => match st.result.take() {
            None => st.phase = Phase::Running(n),
            Some((filed, done)) => {
                st.phase = if done || n >= ATTEMPTS {
                    Phase::Done
                } else {
                    Phase::Waiting(Instant::now() + RETRY, n + 1)
                };
                drop(st);
                backend.traces_pulled(filed, cx);
            }
        },
        Phase::Done => st.phase = Phase::Done,
    }
}

/// Ask the station for every recent closed trade without lines here and file what it holds.
///
/// Returns:
///     What was filed — kept when a later core's request fails, the writer has it already — and
///     whether the pull went through to the end.
fn pull(
    target: &moon_remote::ssh::Target,
    here: &[super::cores_sync::LocalCore],
    seen: Option<Option<Vec<moon_core::station_api::ListedCore>>>,
) -> (Vec<Pulled>, anyhow::Result<()>) {
    let mut filed = Vec::new();
    let outcome = trace_listing(seen, || moon_remote::station::bot::bot_state(target))
        .and_then(|listing| pull_into(target, &mut filed, here, listing.as_deref()));
    (filed, outcome)
}

/// Fetch unread identities off the UI thread; a missing status retries instead of guessing uids.
fn trace_listing(
    seen: Option<Option<Vec<moon_core::station_api::ListedCore>>>,
    fetch: impl FnOnce() -> anyhow::Result<moon_remote::station::bot::BotState>,
) -> anyhow::Result<Option<Vec<moon_core::station_api::ListedCore>>> {
    match seen {
        Some(listing) => Ok(listing),
        None => fetch()?
            .station
            .map(|status| status.cores)
            .ok_or_else(|| anyhow::anyhow!("the station is not answering yet")),
    }
}

/// [`pull`], filing into `filed` as it goes.
fn pull_into(
    target: &moon_remote::ssh::Target,
    filed: &mut Vec<Pulled>,
    here: &[super::cores_sync::LocalCore],
    listing: Option<&[moon_core::station_api::ListedCore]>,
) -> anyhow::Result<()> {
    let now_ms = moon_core::util::time::now_unix_ms_i64();
    let wanted = order_traces::lacking_lines(now_ms)
        .map_err(|e| anyhow::anyhow!("list the trades without traces: {e:?}"))?;
    let asked: usize = wanted.values().map(Vec::len).sum();
    if asked == 0 {
        return Ok(());
    }
    let sink = order_traces::sink()
        .ok_or_else(|| anyhow::anyhow!("the local trace archive is not open"))?;
    let pull = moon_remote::station::pull::Pull::open(target)?;
    for (core, uids) in wanted {
        let Some(station_uid) = super::cores_sync::trace_uid(core, here, Some(listing)) else {
            log::debug!(
                "station: skipping trace pull for local core {core}: no unique station address"
            );
            continue;
        };
        let mut writer_gone = false;
        pull.traces(station_uid, &uids, |trade| {
            let lines: Arc<[ArchivedOrderTrace]> =
                trade.lines.into_iter().map(Into::into).collect();
            if lines.is_empty() || writer_gone {
                return;
            }
            writer_gone = !sink.send_waiting(TraceDbMsg::Answer {
                core_uid: core,
                report_uid: trade.report_uid,
                lines: lines.clone(),
            });
            if !writer_gone {
                filed.push((core, trade.report_uid, lines));
            }
        })?;
        anyhow::ensure!(!writer_gone, "the local trace archive's writer is gone");
    }
    log::info!(
        "station: order traces pulled for {} of {asked} trade(s) without lines",
        filed.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests;
