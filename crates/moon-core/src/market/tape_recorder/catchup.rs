//! Plan and execute closed-trade tape recovery without opening the terminal's tape store.

mod executor;
mod pacing;
mod state;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use super::{Cmd, TradeId};
use crate::market::trade_replay::venue_caps::{self, ArchiveRoute, Fetchable, TradeRoute};
use crate::market::trade_replay::{Coverage, ReplayWindow};
use crate::station_api::TapeCatchup;
use crate::venue::Venue;

pub(crate) use crate::market::source::ARCHIVE_WAIT;
pub(crate) use executor::CoreSpanFetch;
pub(super) use executor::PassResult;
pub(super) use state::RecorderCatchup;

/// Wake CPU pacing promptly when the recorder host is shutting down.
pub(super) fn stop_worker() {
    pacing::stop();
}

/// One resolved closed trade supplied by the session, with true-UTC entry and exit stamps.
#[derive(Clone)]
pub struct CatchupTrade {
    pub id: TradeId,
    pub exchange: String,
    pub market: String,
    pub venue: Option<Venue>,
    /// Captured exchange identity for the core archive fallback.
    pub core: Option<crate::market::source::ReplayAddress>,
    pub open_ms: i64,
    pub close_ms: i64,
    /// Whether this listed trade was emulated.
    pub emulator: bool,
    /// Whether the Entry/Exit tuner can replay this trade: its report row carries millisecond
    /// stamps ([`crate::db::tuner::ticks::deals::has_ms_stamps`]). Live closes always can.
    pub tunable: bool,
}

impl std::fmt::Debug for CatchupTrade {
    /// Describe the trade without formatting the address's worker handles.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatchupTrade")
            .field("id", &self.id)
            .field("exchange", &self.exchange)
            .field("market", &self.market)
            .field("venue", &self.venue)
            .field(
                "core",
                &self.core.as_ref().map(|address| &address.exchange_key),
            )
            .field("open_ms", &self.open_ms)
            .field("close_ms", &self.close_ms)
            .field("emulator", &self.emulator)
            .field("tunable", &self.tunable)
            .finish()
    }
}

/// Sources available for the job's missing focus stretches.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Source {
    Core,
    Rest(TradeRoute),
    Archive(ArchiveRoute),
    Both(TradeRoute, ArchiveRoute),
}

/// One bounded recovery request, ordered by the first missing REST stretch's expiry.
#[derive(Clone, Debug)]
pub(crate) struct CatchupJob {
    pub(crate) trade: CatchupTrade,
    pub(crate) missing: Coverage,
    pub(crate) expires_at_ms: i64,
    pub(crate) source: Source,
    /// Captured boundary distinguishes a missing tuner tail from an ordinary focus gap.
    pub(crate) tail_start_ms: i64,
}

/// One immutable window shape shared by live catch-up planning and retention.
#[derive(Clone, Copy, Debug)]
pub(super) struct Shape {
    pub margin_ms: i64,
    pub long_ms: i64,
    pub tail_ms: i64,
}

impl Shape {
    /// Capture one consistent set of settings for recording and retention.
    pub(super) fn current() -> Self {
        let margin_ms = crate::market::trade_replay::margin_ms();
        Self {
            margin_ms,
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::db::tuner::ticks::tail::record_tail_ms(
                crate::market::trade_replay::min_tail_s(),
                margin_ms,
            ),
        }
    }
}

/// A pure planning pass and the reasons trades did not require a request.
#[derive(Default)]
pub(crate) struct CatchupPlan {
    pub(crate) jobs: Vec<CatchupJob>,
    pub(crate) complete: u32,
    pub(crate) no_route: u32,
    pub(crate) out_of_reach: u32,
    pub(crate) too_fresh: u32,
    /// IDs let the recorder settle a whole planning batch without planning each row again.
    pub(crate) complete_ids: Vec<TradeId>,
    pub(crate) permanent_ids: Vec<TradeId>,
    pub(crate) tail_final_ids: Vec<TradeId>,
}

/// Terminal refusals stay closed; recoverable failures wait for the next hourly pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Latch {
    Permanent,
    RetryAfter(i64),
}

/// Exercise the production fetch pipeline through page and archive function seams.
pub(crate) fn run_job(
    job: &CatchupJob,
    fetch_rest: executor::FetchRest,
    fetch_archive: executor::FetchArchive,
) -> executor::JobOutcome {
    executor::run_job_at(
        job,
        crate::util::now_unix_ms_i64(),
        crate::market::trade_replay::gate::shared(),
        fetch_rest,
        fetch_archive,
    )
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static AUTO: AtomicBool = AtomicBool::new(true);
static ARCHIVES: AtomicBool = AtomicBool::new(true);
static MAX_AGE_DAYS: AtomicU16 = AtomicU16::new(crate::station_api::DEFAULT_MAX_AGE_DAYS);
static RECORD_EMU: AtomicBool = AtomicBool::new(true);
/// Keep tape only for trades the tuner can replay; off keeps every trade, as before the switch.
static TUNER_ONLY: AtomicBool = AtomicBool::new(false);
static PURGE_WANTED: AtomicBool = AtomicBool::new(false);
static RELIST_GEN: AtomicU64 = AtomicU64::new(0);
/// Spawned and successfully completed generations, keyed by durable core identity.
static LISTINGS: Mutex<std::collections::BTreeMap<u64, (u64, Option<u64>)>> =
    Mutex::new(std::collections::BTreeMap::new());
static SWITCH_GEN: AtomicU64 = AtomicU64::new(0);
static TAIL_FINAL: AtomicU32 = AtomicU32::new(0);
static CORE_FETCH: RwLock<Option<CoreSpanFetch>> = RwLock::new(None);

/// Install the session's archive reader; the catch-up worker invokes it outside this lock.
pub(crate) fn set_core_fetch(fetch: CoreSpanFetch) {
    *CORE_FETCH
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(fetch);
}

/// Final incomplete tails in the recorder's current settings generation, for station status.
pub fn tail_final() -> u32 {
    TAIL_FINAL.load(Ordering::Relaxed)
}
/// Include offline sessions in the protection barrier, not only successful listing spawns.
static REQUIRED_CORES: Mutex<Option<Vec<u64>>> = Mutex::new(None);

/// Refresh the sessions whose reports can protect shared tape.
pub(crate) fn require_listings(cores: Vec<u64>) {
    *REQUIRED_CORES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cores);
}

/// Conservative global barrier: unresolved reports can protect any shared market.
pub(super) fn listings_complete(generation: u64) -> bool {
    let cores = REQUIRED_CORES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    cores.is_some_and(|cores| all_listed(&cores, generation))
}

/// Required cores whose listing has not completed for `generation` — what the station log names
/// when retention gives up waiting, since that barrier decides whether a pass deletes anything.
/// `None` while the session has not published its cores yet, which holds the barrier as well.
pub(super) fn unlisted(generation: u64) -> Option<Vec<u64>> {
    let cores = REQUIRED_CORES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()?;
    let listings = LISTINGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Some(
        cores
            .into_iter()
            .filter(|core| {
                listings
                    .get(core)
                    .is_none_or(|entry| entry.1 != Some(generation))
            })
            .collect(),
    )
}
static STATUS: Mutex<TapeCatchup> = Mutex::new(TapeCatchup {
    running: false,
    auto: None,
    archives: None,
    max_age_days: None,
    record_emulator: None,
    tuner_only: None,
    min_tail_s: None,
    tail_final: None,
    db_bytes: None,
    tape_trades: None,
    tape_ages: Vec::new(),
    catchup_rate_per_hour: None,
    purge: None,
    pending: 0,
    filled: 0,
    no_route: 0,
    out_of_reach: 0,
    failed: 0,
    last_pass_ms: None,
    archive_bytes: 0,
    failed_by: Vec::new(),
});

/// Enable recovery for the station without changing the terminal's recording settings.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Whether this host opted into closed-trade recovery.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Apply reloadable switches atomically with respect to status readers and recorder snapshots.
pub fn set_switches(auto: bool, archives: bool) {
    let _status = STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let changed = AUTO.swap(auto, Ordering::Relaxed) != auto;
    let changed = (ARCHIVES.swap(archives, Ordering::Relaxed) != archives) || changed;
    if changed {
        SWITCH_GEN.fetch_add(1, Ordering::Relaxed);
    }
}

/// Apply retention choices coherently and request a purge only when retention narrows.
pub fn set_keep(max_age_days: u16, record_emulator: bool) {
    let _status = STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let max_age_days = crate::station_api::clamp_max_age_days(max_age_days);
    let old_days = MAX_AGE_DAYS.swap(max_age_days, Ordering::Relaxed);
    let old_emulator = RECORD_EMU.swap(record_emulator, Ordering::Relaxed);
    if old_days != max_age_days || old_emulator != record_emulator {
        SWITCH_GEN.fetch_add(1, Ordering::Relaxed);
        RELIST_GEN.fetch_add(1, Ordering::Relaxed);
    }
    if (crate::station_api::TapeKeep {
        max_age_days: old_days,
        record_emulator: old_emulator,
        tuner_only: None,
        min_tail_s: None,
    })
    .removes_tape(crate::station_api::TapeKeep {
        max_age_days,
        record_emulator,
        tuner_only: None,
        min_tail_s: None,
    }) {
        PURGE_WANTED.store(true, Ordering::Relaxed);
    }
}

/// Whether emulator trades should contribute recorded tape.
pub fn record_emulator() -> bool {
    RECORD_EMU.load(Ordering::Relaxed)
}

/// Apply the "keep tape only for trades the tuner can use" choice like a retention change:
/// switching it on re-lists the cores and requests the same purge a narrowing retention write
/// does; switching it off deletes nothing and returns the skipped trades to the queue.
///
/// Args:
///     on: Whether catch-up and retention keep only tuner-usable trades.
pub fn set_tuner_only(on: bool) {
    let _status = STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let old = TUNER_ONLY.swap(on, Ordering::Relaxed);
    if old != on {
        SWITCH_GEN.fetch_add(1, Ordering::Relaxed);
        RELIST_GEN.fetch_add(1, Ordering::Relaxed);
    }
    if !old && on {
        PURGE_WANTED.store(true, Ordering::Relaxed);
    }
}

/// Whether only trades the tuner can replay keep and fetch tape.
pub fn tuner_only() -> bool {
    TUNER_ONLY.load(Ordering::Relaxed)
}

/// Whether the current choices leave `trade` out of catch-up: an emulator trade while emulator
/// recording is off, or a trade the tuner cannot replay while only tuner-usable trades are kept.
///
/// Args:
///     trade: A listed closed trade.
///
/// Returns:
///     `true` when the trade must not enter the fetch queue.
pub(crate) fn skipped(trade: &CatchupTrade) -> bool {
    (trade.emulator && !record_emulator()) || (!trade.tunable && tuner_only())
}

/// Effective retention limit in days.
pub fn max_age_days() -> u16 {
    MAX_AGE_DAYS.load(Ordering::Relaxed)
}

/// Generation of trade listings requested by retention changes.
pub fn relist_gen() -> u64 {
    RELIST_GEN.load(Ordering::Relaxed)
}

/// Remember the generation captured by a successfully spawned listing, including stale ones.
pub(crate) fn listing_started(core: u64, generation: u64) {
    LISTINGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(core)
        .and_modify(|entry| entry.0 = generation)
        .or_insert((generation, None));
}

/// Last spawned generation; a mismatch retries after any older listing exits.
pub(crate) fn listing_generation(core: u64) -> Option<u64> {
    LISTINGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&core)
        .map(|entry| entry.0)
}

/// Queue completion after every submitted page so purge planning cannot outrun known trades.
pub fn listed(core: u64, generation: u64) {
    super::send(Cmd::Listed { core, generation });
}

/// Apply a successful completion only when its retention generation is still current.
pub(super) fn listing_applied(core: u64, generation: u64) {
    let _status = STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if generation == relist_gen() {
        LISTINGS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(core)
            .and_modify(|entry| entry.1 = Some(generation))
            .or_insert((generation, Some(generation)));
    }
}

/// Whether every required core completed a listing for the still-current generation.
pub fn all_listed(cores: &[u64], generation: u64) -> bool {
    let _status = STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let listings = LISTINGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    generation == relist_gen()
        && cores.iter().all(|core| {
            listings
                .get(core)
                .is_some_and(|entry| entry.1 == Some(generation))
        })
}

/// Consume the request to purge tape after a narrower retention choice.
pub fn take_purge_wanted() -> bool {
    PURGE_WANTED.swap(false, Ordering::Relaxed)
}

/// Earliest catch-up timestamp allowed by retention; forever leaves venue availability to fetching.
pub fn edge_ms(now_ms: i64) -> i64 {
    retention_edge_ms(now_ms).unwrap_or(i64::MIN)
}

/// Earliest retained timestamp for deletion, with no age deletion under keep forever.
pub fn retention_edge_ms(now_ms: i64) -> Option<i64> {
    let days = max_age_days();
    (days != 0).then(|| now_ms.saturating_sub(i64::from(days) * 86_400_000))
}

/// Invalidate settled source decisions when the effective tape window changes.
pub(crate) fn window_changed() {
    let _status = STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    SWITCH_GEN.fetch_add(1, Ordering::Relaxed);
}

/// Read one coherent generation and its choices under the shared status mutex.
fn switches() -> (u64, bool, bool) {
    let _status = STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (
        SWITCH_GEN.load(Ordering::Relaxed),
        AUTO.load(Ordering::Relaxed),
        ARCHIVES.load(Ordering::Relaxed),
    )
}

/// Queue resolved rows on the recorder, which owns all reads and writes of its tape cache.
pub fn submit(trades: Vec<CatchupTrade>) {
    if enabled() && !trades.is_empty() {
        super::send(Cmd::Catchup(trades));
    }
}

/// Retain resolved real windows without scheduling historical requests outside archive reach.
pub fn protect(trades: Vec<CatchupTrade>) {
    if enabled() {
        super::notify(Cmd::Protect(trades));
    }
}

/// Read the latest progress snapshot without touching either tape database.
pub fn status() -> TapeCatchup {
    let snapshot = STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut status = snapshot.clone();
    status.auto = Some(AUTO.load(Ordering::Relaxed));
    status.archives = Some(ARCHIVES.load(Ordering::Relaxed));
    status.max_age_days = Some(max_age_days());
    status.record_emulator = Some(record_emulator());
    status.tuner_only = Some(tuner_only());
    status.min_tail_s = Some(crate::market::trade_replay::min_tail_s());
    status.tail_final = Some(tail_final());
    status
}

/// Count a session-latched unknown coin without creating an unresolved recorder job.
pub(crate) fn note_no_route() {
    let mut status = STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    status.no_route = status.no_route.saturating_add(1);
}

/// Shape exactly the same focus as live recording, using the settings of this planning pass.
pub(super) fn needed(trade: &CatchupTrade, shape: &Shape) -> Coverage {
    let margin = shape.margin_ms.max(0);
    let window = ReplayWindow {
        from_ms: trade.open_ms.saturating_sub(margin),
        to_ms: trade.close_ms.saturating_add(margin),
        open_ms: trade.open_ms,
        close_ms: trade.close_ms,
        margin_ms: margin,
        long_position_ms: shape.long_ms,
        over_budget: false,
    };
    crate::db::tuner::ticks::tail::focus_with_tail(&window, shape.tail_ms)
}

/// Prefer expiring REST-only tape, omit held or latched rows, and leave live tails alone.
pub(super) fn plan(
    trades: &[CatchupTrade],
    held: impl Fn(&CatchupTrade) -> Coverage,
    latched: &HashMap<TradeId, Latch>,
    now_ms: i64,
    shape: &Shape,
) -> CatchupPlan {
    plan_with(trades, held, latched, now_ms, shape, true)
}

/// Plan against the archive choice captured for this recorder pass, leaving REST unchanged.
pub(super) fn plan_with(
    trades: &[CatchupTrade],
    held: impl Fn(&CatchupTrade) -> Coverage,
    latched: &HashMap<TradeId, Latch>,
    now_ms: i64,
    shape: &Shape,
    archives: bool,
) -> CatchupPlan {
    let mut result = CatchupPlan::default();
    let edge = edge_ms(now_ms);
    for trade in trades {
        if skipped(trade) {
            continue;
        }
        if latched.get(&trade.id).is_some_and(|latch| match latch {
            Latch::Permanent => true,
            Latch::RetryAfter(at) => now_ms < *at,
        }) {
            continue;
        }
        if trade.close_ms < edge {
            result.out_of_reach += 1;
            result.permanent_ids.push(trade.id);
            continue;
        }
        let focus = needed(trade, shape).clip(&Coverage::one((edge, i64::MAX)));
        let mut missing = focus.minus(&held(trade));
        if missing.is_empty() {
            result.complete += 1;
            result.complete_ids.push(trade.id);
            continue;
        }
        let tail_start_ms = trade.close_ms.saturating_add(shape.margin_ms.max(0));
        let tail_only = state::tail_only(&missing, tail_start_ms);
        if trade
            .close_ms
            .saturating_add(shape.margin_ms.max(shape.tail_ms).max(0))
            .saturating_add(super::plan::GIVE_UP_MS)
            > now_ms
        {
            result.too_fresh += 1;
            continue;
        }
        let Some((from, to)) = focus.hull() else {
            continue;
        };
        // Unlisted exchanges recover every retained gap; known venues supplement tails only.
        let core_tail = (trade.venue.is_none() || tail_only)
            && trade.core.is_some()
            && if trade.venue.is_none() {
                missing.spans().iter().any(|&(_, to)| {
                    to >= now_ms.saturating_sub(crate::market::source::CORE_ARCHIVE_RETENTION_MS)
                })
            } else {
                missing.spans().iter().all(|&(from, _)| {
                    from >= now_ms.saturating_sub(crate::market::source::CORE_ARCHIVE_RETENTION_MS)
                })
            };
        if core_tail
            && matches!(
                venue_caps::fetchable_with(
                    trade.venue,
                    &trade.market,
                    missing.hull().expect("nonempty missing tail").0,
                    now_ms,
                    archives,
                ),
                Fetchable::NoRoute | Fetchable::OutOfRetention
            )
        {
            result.jobs.push(CatchupJob {
                trade: trade.clone(),
                missing,
                expires_at_ms: trade
                    .close_ms
                    .saturating_add(crate::market::source::CORE_ARCHIVE_RETENTION_MS),
                source: Source::Core,
                tail_start_ms,
            });
            continue;
        }
        let Some(venue) = trade.venue else {
            // No retained core coverage remains; do not retry expired gaps.
            result.permanent_ids.push(trade.id);
            if tail_only {
                result.no_route += 1;
                result.tail_final_ids.push(trade.id);
            }
            continue;
        };
        let rest = venue_caps::trade_route(venue);
        let archive = archives
            .then(|| venue_caps::archive_route(venue, &trade.market))
            .flatten();
        match venue_caps::fetchable_with(Some(venue), &trade.market, from, now_ms, archives) {
            Fetchable::NoRoute => {
                result.no_route += 1;
                result.permanent_ids.push(trade.id);
                if tail_only {
                    result.tail_final_ids.push(trade.id);
                }
                continue;
            }
            Fetchable::OutOfRetention => {
                // A REST-only window crossing the edge may still recover its reachable exit.
                if venue_caps::fetchable_with(Some(venue), &trade.market, to, now_ms, archives)
                    != Fetchable::Rest
                {
                    result.out_of_reach += 1;
                    result.permanent_ids.push(trade.id);
                    if tail_only {
                        result.tail_final_ids.push(trade.id);
                    }
                    continue;
                }
                let earliest = rest
                    .and_then(|route| route.retention_ms())
                    .map(|retention| {
                        now_ms
                            .saturating_sub(retention)
                            .saturating_add(venue_caps::RETENTION_SAFETY_MS)
                    });
                if let Some(earliest) = earliest {
                    missing = missing.clip(&Coverage::one((earliest, i64::MAX)));
                }
                if missing.is_empty() {
                    result.out_of_reach += 1;
                    result.permanent_ids.push(trade.id);
                    continue;
                }
            }
            Fetchable::Rest | Fetchable::Archive => {}
        }
        let source = match (rest, archive) {
            (Some(rest), Some(archive)) => Source::Both(rest, archive),
            (Some(rest), None) => Source::Rest(rest),
            (None, Some(archive)) => Source::Archive(archive),
            (None, None) => unreachable!("fetchable source has a route"),
        };
        let expires_at_ms = match source {
            Source::Rest(route) => route.retention_ms().map_or(i64::MAX, |retention| {
                missing
                    .hull()
                    .expect("nonempty missing tape")
                    .0
                    .saturating_add(retention)
            }),
            Source::Both(route, _) => route.retention_ms().map_or(i64::MAX, |retention| {
                missing
                    .clip(&Coverage::one((
                        now_ms
                            .saturating_sub(retention)
                            .saturating_add(venue_caps::RETENTION_SAFETY_MS),
                        i64::MAX,
                    )))
                    .hull()
                    .map_or(i64::MAX, |(from, _)| from.saturating_add(retention))
            }),
            Source::Archive(_) | Source::Core => i64::MAX,
        };
        result.jobs.push(CatchupJob {
            trade: trade.clone(),
            missing,
            expires_at_ms,
            source,
            tail_start_ms,
        });
    }
    result.jobs.sort_by(|a, b| {
        a.expires_at_ms
            .cmp(&b.expires_at_ms)
            .then_with(|| b.trade.close_ms.cmp(&a.trade.close_ms))
    });
    result
}

#[cfg(test)]
pub(crate) mod tests;
