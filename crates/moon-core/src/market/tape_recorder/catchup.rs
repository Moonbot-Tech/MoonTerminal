//! Plan and execute closed-trade tape recovery without opening the terminal's tape store.

mod executor;
mod state;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{Cmd, TradeId};
use crate::market::trade_replay::venue_caps::{self, ArchiveRoute, Fetchable, TradeRoute};
use crate::market::trade_replay::{Coverage, ReplayWindow};
use crate::station_api::TapeCatchup;
use crate::venue::Venue;

pub(super) use executor::PassResult;
pub(super) use state::RecorderCatchup;

/// One resolved closed trade supplied by the session, with true-UTC entry and exit stamps.
#[derive(Clone, Debug)]
pub struct CatchupTrade {
    pub id: TradeId,
    pub exchange: String,
    pub market: String,
    pub venue: Venue,
    pub open_ms: i64,
    pub close_ms: i64,
}

/// Sources available for the job's missing focus stretches.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Source {
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
static SWITCH_GEN: AtomicU64 = AtomicU64::new(0);
static STATUS: Mutex<TapeCatchup> = Mutex::new(TapeCatchup {
    running: false,
    auto: None,
    archives: None,
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

/// Read the latest progress snapshot without touching either tape database.
pub fn status() -> TapeCatchup {
    let snapshot = STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut status = snapshot.clone();
    status.auto = Some(AUTO.load(Ordering::Relaxed));
    status.archives = Some(ARCHIVES.load(Ordering::Relaxed));
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
fn needed(trade: &CatchupTrade, margin_ms: i64, long_ms: i64) -> Coverage {
    let margin = margin_ms.max(0);
    ReplayWindow {
        from_ms: trade.open_ms.saturating_sub(margin),
        to_ms: trade.close_ms.saturating_add(margin),
        open_ms: trade.open_ms,
        close_ms: trade.close_ms,
        margin_ms: margin,
        long_position_ms: long_ms,
        over_budget: false,
    }
    .focus_spans()
}

/// Prefer expiring REST-only tape, omit held or latched rows, and leave live tails alone.
pub(crate) fn plan(
    trades: &[CatchupTrade],
    held: impl Fn(&CatchupTrade) -> Coverage,
    latched: &HashMap<TradeId, Latch>,
    now_ms: i64,
    margin_ms: i64,
    long_ms: i64,
) -> CatchupPlan {
    plan_with(trades, held, latched, now_ms, margin_ms, long_ms, true)
}

/// Plan against the archive choice captured for this recorder pass, leaving REST unchanged.
pub(crate) fn plan_with(
    trades: &[CatchupTrade],
    held: impl Fn(&CatchupTrade) -> Coverage,
    latched: &HashMap<TradeId, Latch>,
    now_ms: i64,
    margin_ms: i64,
    long_ms: i64,
    archives: bool,
) -> CatchupPlan {
    let mut result = CatchupPlan::default();
    for trade in trades {
        if latched.get(&trade.id).is_some_and(|latch| match latch {
            Latch::Permanent => true,
            Latch::RetryAfter(at) => now_ms < *at,
        }) {
            continue;
        }
        if trade.close_ms < now_ms.saturating_sub(venue_caps::ARCHIVE_DEPTH_MS) {
            result.out_of_reach += 1;
            result.permanent_ids.push(trade.id);
            continue;
        }
        let focus = needed(trade, margin_ms, long_ms);
        let mut missing = focus.minus(&held(trade));
        if missing.is_empty() {
            result.complete += 1;
            result.complete_ids.push(trade.id);
            continue;
        }
        if trade
            .close_ms
            .saturating_add(margin_ms.max(0))
            .saturating_add(super::plan::GIVE_UP_MS)
            > now_ms
        {
            result.too_fresh += 1;
            continue;
        }
        let Some((from, to)) = focus.hull() else {
            continue;
        };
        let rest = venue_caps::trade_route(trade.venue);
        let archive = archives
            .then(|| venue_caps::archive_route(trade.venue, &trade.market))
            .flatten();
        match venue_caps::fetchable_with(trade.venue, &trade.market, from, now_ms, archives) {
            Fetchable::NoRoute => {
                result.no_route += 1;
                result.permanent_ids.push(trade.id);
                continue;
            }
            Fetchable::OutOfRetention => {
                // A REST-only window crossing the edge may still recover its reachable exit.
                if venue_caps::fetchable_with(trade.venue, &trade.market, to, now_ms, archives)
                    != Fetchable::Rest
                {
                    result.out_of_reach += 1;
                    result.permanent_ids.push(trade.id);
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
            Source::Archive(_) => i64::MAX,
        };
        result.jobs.push(CatchupJob {
            trade: trade.clone(),
            missing,
            expires_at_ms,
            source,
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
mod tests;
