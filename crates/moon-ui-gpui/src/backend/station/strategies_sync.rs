//! The strategy version history a terminal shares with its station, both ways.
//!
//! A core keeps only a strategy's current state, so the history exists only with those who watched
//! it. The station watches all the time: a terminal set up fresh on another machine fills from it,
//! and one that was off catches up on the edits it missed. A terminal also holds what a station
//! missed while it was down, and everything from before the station existed. Live edits need
//! nothing from here, the core tells both sides itself; this pass closes what either side missed.
//!
//! No setting and no button: a station with the account profile says so in its status, and the
//! pass runs on its own. The merge itself is `moon_core::strat_db::merge`, on each side's writer.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use moon_core::station_api::{StrategySummary, TAPE_REPLY_BUDGET};
use moon_core::strat_db::{merge::IncomingChain, sync};

use crate::Backend;

/// After start: the replica's catch-up and the other pulls come first.
const DELAY: Duration = Duration::from_secs(90);
/// Between attempts after a failure: the station out of reach, a broken answer.
const RETRY: Duration = Duration::from_secs(120);
/// Attempts per pass.
const ATTEMPTS: u32 = 5;
/// Between passes. A running terminal hears its connected cores itself, so a pass only catches
/// what one side missed; the listing that decides that is one small request.
const PERIOD: Duration = Duration::from_secs(6 * 60 * 60);
/// After the terminal's core list changed: a core added on a new machine has its history to fetch,
/// and adding several in a row should not start a pass each.
const LIST_CHANGE_DELAY: Duration = Duration::from_secs(5);
/// How long a pass waits for this machine's writer on one chain.
const MERGE_WAIT: Duration = Duration::from_secs(120);
/// Strategies failing in a row after which a pass stops: the station is gone, and asking it for
/// each of a thousand more only takes longer to say so.
const STOP_AFTER_FAILURES: u32 = 8;

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
}

#[derive(Default)]
struct State {
    phase: Phase,
    /// The core list last seen, to notice a change.
    cores: Option<Vec<u64>>,
    /// The list changed and no wait has been brought forward for it yet: a change during a pass
    /// was not covered by that pass, so it must outlive it.
    list_changed: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The phase after `phase` at `now`; `start(n)` launches attempt `n` and says whether it did.
fn next(phase: Phase, now: Instant, start: impl FnOnce(u32) -> bool) -> Phase {
    match phase {
        Phase::Unstarted => Phase::Waiting(now + DELAY, 1),
        Phase::Waiting(due, n) if now < due => Phase::Waiting(due, n),
        Phase::Waiting(_, n) => {
            if start(n) {
                Phase::Running(n)
            } else {
                // No station yet: one set up later is found on the next look.
                Phase::Waiting(now + PERIOD, 1)
            }
        }
        Phase::Ended(n, false) if n < ATTEMPTS => Phase::Waiting(now + RETRY, n + 1),
        Phase::Ended(..) => Phase::Waiting(now + PERIOD, 1),
        Phase::Running(n) => Phase::Running(n),
    }
}

/// The phase once the core list changed: a wait for later comes forward, and anything else stands.
fn after_list_change(phase: Phase, now: Instant) -> Phase {
    match phase {
        Phase::Waiting(due, _) if due > now + LIST_CHANGE_DELAY => {
            Phase::Waiting(now + LIST_CHANGE_DELAY, 1)
        }
        other => other,
    }
}

/// The coordination tick's call: a clock compare while waiting, a pass on its own thread once due.
pub(crate) fn tick(backend: &Backend) {
    // A FireTest run must not write a developer's history, and this pass can.
    if !backend.persist_allowed {
        return;
    }
    let now = Instant::now();
    let cores: Vec<u64> = backend.config.servers.iter().map(|s| s.uid).collect();
    let mut guard = STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = guard.get_or_insert_with(State::default);
    if state.cores.as_ref().is_some_and(|seen| *seen != cores) {
        state.list_changed = true;
    }
    if state.cores.as_ref() != Some(&cores) {
        state.cores = Some(cores);
    }
    state.phase = next(state.phase, now, |n| spawn(backend, n));
    if state.list_changed && matches!(state.phase, Phase::Waiting(..)) {
        state.phase = after_list_change(state.phase, now);
        state.list_changed = false;
    }
}

/// Launch attempt `n`: `false` when there is no station, or no thread to run on.
fn spawn(backend: &Backend, n: u32) -> bool {
    // Read when due: a server set up meanwhile counts.
    let Some(target) = super::known_target() else {
        return false;
    };
    let here = super::cores_sync::trace_cores(&backend.config);
    let spawned = std::thread::Builder::new()
        .name("station-strategies".into())
        .spawn(move || {
            // A panic must not leave the phase on Running: the history would stop until a restart.
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pass(&target, &here)))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("the pass panicked")));
            if let Err(e) = &outcome {
                log::warn!("station: strategy history, attempt {n} of {ATTEMPTS}: {e:#}");
            }
            let mut guard = STATE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(state) = guard.as_mut() {
                state.phase = Phase::Ended(n, outcome.is_ok());
            }
        });
    match spawned {
        Ok(_) => true,
        Err(e) => {
            log::warn!("station: strategy history thread did not start: {e}");
            false
        }
    }
}

/// What a pass did, for its log line.
#[derive(Debug, Default, PartialEq, Eq)]
struct Count {
    /// Strategies whose summaries differed.
    differed: usize,
    /// Of them, those this machine's archive took something from.
    taken: usize,
    /// Those whose merged chain went to the station.
    sent: usize,
    /// Cores on this machine with no unique match on the station, left for a later pass.
    unmatched: usize,
}

/// Whether two summaries describe chains that have settled.
fn settled(a: &StrategySummary, b: &StrategySummary) -> bool {
    a.versions == b.versions && a.first_from == b.first_from && a.last_from == b.last_from
}

/// What to do about one strategy, from what each side's listing says of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// The two chains have settled.
    Nothing,
    /// Only this machine holds it.
    Send,
    /// The station holds it: merge its chain here, then send if the station lacks something.
    Exchange,
}

/// Decide one strategy's step.
///
/// Args:
///     station: The station's summary, when it holds the strategy.
///     here: This machine's summary, when it holds it.
///     forgotten: Whether this machine forgot everything the station holds of it.
fn step(
    station: Option<&StrategySummary>,
    here: Option<&StrategySummary>,
    forgotten: bool,
) -> Step {
    match (station, here) {
        (None, None) => Step::Nothing,
        (None, Some(_)) => Step::Send,
        (Some(_), None) if forgotten => Step::Nothing,
        (Some(s), Some(h)) if settled(s, h) => Step::Nothing,
        (Some(_), _) => Step::Exchange,
    }
}

/// Failures of one pass, which go on past one strategy: a chain that fails every time must not
/// starve every strategy after it, attempt after attempt.
#[derive(Debug, Default)]
struct Failures {
    total: u32,
    in_a_row: u32,
}

impl Failures {
    /// Count one strategy's result.
    ///
    /// Returns:
    ///     An error when so many failed in a row that the pass should stop here.
    fn note(&mut self, strategy_id: i64, result: anyhow::Result<()>) -> anyhow::Result<()> {
        match result {
            Ok(()) => self.in_a_row = 0,
            Err(e) => {
                self.total += 1;
                self.in_a_row += 1;
                log::warn!("station: strategy history, strategy {strategy_id}: {e:#}");
                anyhow::ensure!(
                    self.in_a_row < STOP_AFTER_FAILURES,
                    "{} strategies failed in a row",
                    self.in_a_row
                );
            }
        }
        Ok(())
    }
}

/// One pass over every core both sides know.
fn pass(
    target: &moon_remote::ssh::Target,
    here: &[super::cores_sync::LocalCore],
) -> anyhow::Result<()> {
    let pull = moon_remote::station::pull::Pull::open(target)?;
    let status = pull.status()?;
    if !status.strategies_sync {
        log::info!("station: strategy history skipped, this station keeps no archive");
        return Ok(());
    }
    let listing = status.cores.unwrap_or_default();
    let mut station_held: BTreeMap<u64, BTreeMap<i64, StrategySummary>> = BTreeMap::new();
    for summary in pull.strategy_index()? {
        station_held
            .entry(summary.core_uid)
            .or_default()
            .insert(summary.strategy_id, summary);
    }
    let mut local_held: BTreeMap<u64, BTreeMap<i64, StrategySummary>> = BTreeMap::new();
    for summary in sync::local_summaries()? {
        local_held
            .entry(summary.core_uid)
            .or_default()
            .insert(summary.strategy_id, summary);
    }
    let sink = moon_core::strat_db::sink()
        .ok_or_else(|| anyhow::anyhow!("the local strategy archive is not open"))?;
    let empty = BTreeMap::new();
    let mut count = Count::default();
    let mut failures = Failures::default();
    let mut snapshot_taken = false;
    for core in here {
        let Some(station_uid) = super::cores_sync::station_uid_for(core.uid, here, &listing) else {
            count.unmatched += 1;
            continue;
        };
        let there = station_held.get(&station_uid).unwrap_or(&empty);
        let mine = local_held.get(&core.uid).unwrap_or(&empty);
        let ids: BTreeSet<i64> = there.keys().chain(mine.keys()).copied().collect();
        for id in ids {
            let (s, h) = (there.get(&id), mine.get(&id));
            // Only a chain this machine holds nothing of can have been forgotten whole.
            let forgotten =
                h.is_none() && s.is_some_and(|s| sync::forgotten_covers(core.uid, id, s.last_from));
            let work = step(s, h, forgotten);
            // A settled strategy asks the station nothing, so it says nothing about the station:
            // it must not reset the count of failures in a row.
            if work == Step::Nothing {
                continue;
            }
            if work == Step::Exchange && !snapshot_taken {
                // This machine's file is its only copy: no merge into it without one. Outside the
                // per-strategy handling on purpose, a failure here stops the whole pass.
                sync::snapshot_before_first_merge()?;
                snapshot_taken = true;
            }
            let mut one = || -> anyhow::Result<()> {
                let send = match work {
                    Step::Nothing => false,
                    Step::Send => true,
                    Step::Exchange => {
                        count.differed += 1;
                        let (head, versions) = pull.strategy_chain(station_uid, id)?;
                        let outcome = sink
                            .merge_chain(IncomingChain {
                                core_uid: core.uid,
                                core_name: core.name.clone(),
                                strategy_id: id,
                                head,
                                versions,
                                from_station: true,
                            })?
                            .recv_timeout(MERGE_WAIT)
                            .map_err(|_| {
                                anyhow::anyhow!("the local archive did not answer in time")
                            })?
                            .map_err(anyhow::Error::msg)?;
                        count.taken += usize::from(outcome.changed);
                        outcome.other_lacks
                    }
                };
                if send {
                    send_chain(&pull, core.uid, station_uid, id)?;
                    count.sent += 1;
                }
                Ok(())
            };
            let result = one();
            failures.note(id, result)?;
        }
    }
    log::info!(
        "station: strategy history, {} differed, {} taken here, {} sent, {} failed, {} core(s) unmatched",
        count.differed,
        count.taken,
        count.sent,
        failures.total,
        count.unmatched
    );
    anyhow::ensure!(
        failures.total == 0,
        "{} strategies failed, the rest went through",
        failures.total
    );
    Ok(())
}

/// Hand the station this machine's whole chain of one strategy, page by page.
fn send_chain(
    pull: &moon_remote::station::pull::Pull,
    local_uid: u64,
    station_uid: u64,
    strategy_id: i64,
) -> anyhow::Result<()> {
    let mut after = None;
    loop {
        let page = sync::chain_page(local_uid, strategy_id, after, TAPE_REPLY_BUDGET)?;
        let resume = page.resume;
        pull.strategy_put(station_uid, strategy_id, page)?;
        match resume {
            Some(resume) => after = Some(resume),
            None => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
