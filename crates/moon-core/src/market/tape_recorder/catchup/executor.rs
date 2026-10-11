//! Fetch bounded catch-up jobs and group daily archives; the recorder alone files the results.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use super::{CatchupJob, Source};
use crate::feed::Tick;
use crate::market::trade_replay::venue_caps::{ArchiveRoute, RETENTION_SAFETY_MS, TradeRoute};
use crate::market::trade_replay::worker::{
    TICK_BUDGET, TickAbandon, TickObserver, TickVerdict, paginate_ticks,
};
use crate::market::trade_replay::{
    Coverage, ReplayIntent, ReplayWindow, TickPlan, archive, gate, rest, tick_plan,
};

/// Compressed archive traffic admitted during one hourly pass.
pub(crate) const ARCHIVE_BYTES_PER_PASS: u64 = 2 * 1024 * 1024 * 1024;
const DAY_MS: i64 = 86_400_000;

/// Page transport seam, identical to the public REST fetcher's route and slice contract.
pub(crate) type FetchRest = fn(
    TradeRoute,
    &str,
    i64,
    i64,
    Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError>;
/// Archive transport seam returns only coverage actually completed by published days.
pub(crate) type FetchArchive = fn(
    ArchiveRoute,
    &str,
    &Coverage,
    u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64);

/// Refusal classification, independent of the recorder's wall-clock retry deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Retry,
    Permanent,
    Throttled,
}

/// Paid-for coverage and ticks survive a partial walk or an archive refusal.
#[derive(Clone, Debug, Default)]
pub(crate) struct JobOutcome {
    pub(crate) file: Coverage,
    pub(crate) ticks: Vec<Tick>,
    pub(crate) failure: Option<Failure>,
    /// Bytes returned by this archive fetch, including failed or partially completed bodies.
    pub(crate) archive_bytes: u64,
    /// An expired REST range can move to the archive without imposing host backoff.
    out_of_retention: bool,
    /// A per-day unknown result becomes permanent only after every requested day agrees.
    archive_unknown: bool,
    /// Unpublished days and budget admission waits are not failed download attempts.
    waiting: bool,
}

/// Pace and stop catch-up pages against the same process-wide host budgets as terminal replay.
struct CatchupObserver<'a> {
    gate: &'a gate::ReplayGate,
    route: TradeRoute,
}
impl TickObserver for CatchupObserver<'_> {
    /// Claim the route's stable host key before the first page.
    fn claim(&mut self, _: &str) -> Result<(), u32> {
        self.gate.claim(self.route.host(), Instant::now())
    }
    /// Spend the same request weight and minimum interval as the worker.
    fn pace(&mut self, _: &str) {
        self.gate.pace_weighted(
            self.route.host(),
            self.route.page_interval(),
            self.route.request_weight(),
        );
    }
    /// A response refusal stops further pages before their request is sent.
    fn host_closed(&mut self, _: &str) -> bool {
        self.gate.claim(self.route.host(), Instant::now()).is_err()
    }
}

/// Build an exact missing-only REST plan, preserving the paginator's slice priority and direction.
fn rest_plan(job: &CatchupJob, route: TradeRoute, now_ms: i64) -> TickPlan {
    let Some((from, to)) = job.missing.hull() else {
        return TickPlan {
            slices: Vec::new(),
            trade_len: 0,
            focus_len: 0,
        };
    };
    // The planner already shaped the focus. This window admits every missing span before the
    // intersection below removes held islands, without re-reading mutable recording settings.
    let window = ReplayWindow {
        from_ms: from,
        to_ms: to,
        open_ms: from,
        close_ms: to,
        margin_ms: 0,
        long_position_ms: i64::MAX,
        over_budget: false,
    };
    let earliest = route.retention_ms().map(|retention| {
        now_ms
            .saturating_sub(retention)
            .saturating_add(RETENTION_SAFETY_MS)
    });
    let planned = tick_plan(window, route, earliest, ReplayIntent::Model);
    let backward = matches!(
        route,
        TradeRoute::OkxHistoryTrades | TradeRoute::BitgetSpotFills | TradeRoute::BitgetMixFills
    );
    let mut result = TickPlan {
        slices: Vec::new(),
        trade_len: 0,
        focus_len: 0,
    };
    for (index, &span) in planned.slices.iter().enumerate() {
        let clipped = Coverage::one(span).clip(&job.missing);
        let mut slices = clipped.spans().to_vec();
        if backward {
            slices.reverse();
        }
        if index < planned.trade_len {
            result.trade_len += slices.len();
        }
        if index < planned.focus_len {
            result.focus_len += slices.len();
        }
        result.slices.extend(slices);
    }
    result
}

/// Run the real paginator through an offline page seam and settle its shared-gate verdict.
fn run_rest(
    job: &CatchupJob,
    route: TradeRoute,
    now_ms: i64,
    gate: &gate::ReplayGate,
    fetch: FetchRest,
) -> JobOutcome {
    let plan = rest_plan(job, route, now_ms);
    let mut observer = CatchupObserver { gate, route };
    let asked_at = Instant::now();
    let verdict = paginate_ticks(
        route,
        &plan,
        TICK_BUDGET,
        60,
        || false,
        |trade| asked_at.elapsed() >= Duration::from_secs(if trade { 180 } else { 45 }),
        &mut observer,
        |from, to, cursor| {
            gate::absorb_page(
                gate,
                route.host(),
                fetch(route, &job.trade.market, from, to, cursor),
            )
        },
    );
    let throttled = gate::settle_verdict(gate, route.host(), &verdict, asked_at);
    let mut outcome = match verdict {
        TickVerdict::Ready(harvest) => {
            let uncertain = edge_empty_slices(route, now_ms, &plan.slices, &harvest.ticks);
            JobOutcome {
                file: harvest.covered.minus(&uncertain),
                ticks: harvest.ticks,
                failure: harvest
                    .stop
                    .and_then(failure_from_stop)
                    .or_else(|| (!uncertain.is_empty()).then_some(Failure::Retry)),
                out_of_retention: harvest.stop == Some(TickAbandon::OutOfRetention),
                ..JobOutcome::default()
            }
        }
        TickVerdict::Abandoned(TickAbandon::Empty) => {
            let uncertain = edge_empty_slices(route, now_ms, &plan.slices, &[]);
            JobOutcome {
                file: Coverage::from_spans(plan.slices).minus(&uncertain),
                failure: (!uncertain.is_empty()).then_some(Failure::Retry),
                ..JobOutcome::default()
            }
        }
        TickVerdict::Abandoned(reason) => JobOutcome {
            failure: failure_from_stop(reason),
            out_of_retention: reason == TickAbandon::OutOfRetention,
            ..JobOutcome::default()
        },
    };
    if throttled {
        outcome.failure = Some(Failure::Throttled);
    }
    clip(&mut outcome, &job.missing);
    outcome
}

/// Empty slices near a rolling retention edge cannot prove quiet tape even when another slice
/// returned prints; exclude each uncertain slice independently.
fn edge_empty_slices(
    route: TradeRoute,
    now_ms: i64,
    slices: &[(i64, i64)],
    ticks: &[Tick],
) -> Coverage {
    let Some(retention) = route.retention_ms() else {
        return Coverage::none();
    };
    let edge_day = now_ms.saturating_sub(retention).saturating_add(DAY_MS);
    Coverage::from_spans(slices.iter().copied().filter(|&(from, to)| {
        from < edge_day
            && !ticks.iter().any(|tick| {
                tick.time_ms.is_finite() && tick.time_ms >= from as f64 && tick.time_ms <= to as f64
            })
    }))
}

/// Convert paginator stops into permanent market refusals, hourly retries or pass stops.
fn failure_from_stop(reason: TickAbandon) -> Option<Failure> {
    match reason {
        TickAbandon::UnknownSymbol => Some(Failure::Permanent),
        TickAbandon::RateLimited => Some(Failure::Throttled),
        TickAbandon::Empty => None,
        TickAbandon::Cancelled
        | TickAbandon::OutOfRetention
        | TickAbandon::Deadline
        | TickAbandon::Transient
        | TickAbandon::OverPageBudget
        | TickAbandon::OverTickBudget => Some(Failure::Retry),
    }
}

/// Keep only exhaustive missing stretches and their finite prints, never the whole returned hull.
fn clip(outcome: &mut JobOutcome, missing: &Coverage) {
    outcome.file = outcome.file.clip(missing);
    outcome
        .ticks
        .retain(|tick| tick.time_ms.is_finite() && outcome.file.contains_ms(tick.time_ms as i64));
    outcome
        .ticks
        .sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
}

/// Fetch one archive portion and classify content refusals separately from recoverable outages.
fn run_archive(
    job: &CatchupJob,
    route: ArchiveRoute,
    fetch: FetchArchive,
    budget: u64,
) -> JobOutcome {
    let (result, bytes) = fetch(route, &job.trade.market, &job.missing, budget);
    let mut outcome = match result {
        Ok(fill) => JobOutcome {
            waiting: (fill.covered.is_empty() && fill.deferred.is_none())
                || matches!(
                    fill.deferred,
                    Some(
                        archive::ArchiveError::BudgetDeferred | archive::ArchiveError::NotPublished
                    )
                ),
            file: fill.covered,
            ticks: fill.ticks,
            failure: fill.deferred.map(archive_failure),
            archive_bytes: bytes,
            ..JobOutcome::default()
        },
        Err(error) => JobOutcome {
            waiting: matches!(
                error,
                archive::ArchiveError::BudgetDeferred | archive::ArchiveError::NotPublished
            ),
            archive_unknown: error == archive::ArchiveError::UnknownSymbol,
            failure: Some(archive_failure(error)),
            archive_bytes: bytes,
            ..JobOutcome::default()
        },
    };
    clip(&mut outcome, &job.missing);
    outcome
}

/// Classify a deferred archive refusal after retaining all earlier completed days.
fn archive_failure(error: archive::ArchiveError) -> Failure {
    match error {
        archive::ArchiveError::UnknownSymbol => Failure::Permanent,
        archive::ArchiveError::Throttled { .. } => Failure::Throttled,
        archive::ArchiveError::NotPublished
        | archive::ArchiveError::Corrupt(_)
        | archive::ArchiveError::TooLarge
        | archive::ArchiveError::BudgetDeferred
        | archive::ArchiveError::NoRoom
        | archive::ArchiveError::Transient(_) => Failure::Retry,
    }
}

/// Partition missing tape at the REST boundary; archives receive only the older portion.
fn portions(
    job: &CatchupJob,
    now_ms: i64,
) -> (Option<TradeRoute>, Coverage, Option<ArchiveRoute>, Coverage) {
    match job.source {
        Source::Rest(route) => (Some(route), job.missing.clone(), None, Coverage::none()),
        Source::Archive(route) => (None, Coverage::none(), Some(route), job.missing.clone()),
        Source::Both(rest, archive) => {
            let recent = rest.retention_ms().map_or_else(
                || job.missing.clone(),
                |retention| {
                    job.missing.clip(&Coverage::one((
                        now_ms
                            .saturating_sub(retention)
                            .saturating_add(RETENTION_SAFETY_MS),
                        i64::MAX,
                    )))
                },
            );
            let old = job.missing.minus(&recent);
            (Some(rest), recent, Some(archive), old)
        }
    }
}

/// Execute one job at an injected clock and gate, preserving paid-for REST tape on archive errors.
pub(super) fn run_job_at(
    job: &CatchupJob,
    now_ms: i64,
    gate: &gate::ReplayGate,
    fetch_rest: FetchRest,
    fetch_archive: FetchArchive,
) -> JobOutcome {
    match job.source {
        Source::Rest(route) => run_rest(job, route, now_ms, gate, fetch_rest),
        Source::Archive(route) => run_archive(job, route, fetch_archive, u64::MAX),
        Source::Both(_, _) => {
            let mut outcome = JobOutcome::default();
            run_pass_with(
                vec![job.clone()],
                || now_ms,
                gate,
                (fetch_rest, fetch_archive),
                0,
                run_job_at,
                |_, result| outcome = result.clone(),
            );
            outcome
        }
    }
}

/// Lightweight settlement metadata sent after all backfilled commands on the same queue.
pub(crate) struct FinishedJob {
    pub(crate) job: CatchupJob,
    pub(crate) failure: Option<Failure>,
    /// Incomplete work may count as failed unless only a host or budget deferred it.
    pub(crate) attempted: bool,
}

/// Pass accounting includes deferred jobs so they remain pending until the next hourly retry.
pub(crate) struct PassResult {
    pub(crate) finished: Vec<FinishedJob>,
    pub(crate) archive_bytes: u64,
}

/// A route and native symbol identify the same public file across all account exchange keys.
type DayKey = (ArchiveRoute, String, i64);

/// Split exact missing stretches at UTC midnight, preserving holes within each day.
fn day_parts(route: ArchiveRoute, market: &str, spans: &Coverage) -> Vec<(DayKey, Coverage)> {
    let mut parts = std::collections::BTreeMap::<i64, Coverage>::new();
    for &(from, to) in spans.spans() {
        let mut day = from.div_euclid(DAY_MS).saturating_mul(DAY_MS);
        while day <= to {
            let end = day.saturating_add(DAY_MS - 1);
            parts
                .entry(day)
                .or_default()
                .add((from.max(day), to.min(end)));
            let next = day.saturating_add(DAY_MS);
            if next <= day {
                break;
            }
            day = next;
        }
    }
    parts
        .into_iter()
        .map(|(day, spans)| ((route, market.to_owned(), day), spans))
        .collect()
}

/// Walk all REST portions before grouped archive downloads, refreshing the clock for each job.
pub(super) fn run_pass_with(
    jobs: Vec<CatchupJob>,
    now: impl Fn() -> i64,
    gate: &gate::ReplayGate,
    transports: (FetchRest, FetchArchive),
    already_downloaded: u64,
    run: impl Fn(&CatchupJob, i64, &gate::ReplayGate, FetchRest, FetchArchive) -> JobOutcome,
    mut file: impl FnMut(&CatchupJob, &JobOutcome),
) -> PassResult {
    let (fetch_rest, fetch_archive) = transports;
    let mut downloaded = already_downloaded;
    let mut groups = HashMap::<DayKey, Coverage>::new();
    let mut fetched = HashMap::<DayKey, (JobOutcome, bool)>::new();
    let mut stopped_hosts = HashSet::new();
    let mut finished = Vec::new();
    let mut prepared = Vec::new();
    for job in jobs {
        let now_ms = now();
        let (rest, recent, archive, mut old) = portions(&job, now_ms);
        let mut outcome = JobOutcome::default();
        let mut attempted = false;
        let mut deferred = false;
        let mut failed_fetch = false;
        if let Some(route) = rest.filter(|_| !recent.is_empty()) {
            let part = CatchupJob {
                missing: recent.clone(),
                source: Source::Rest(route),
                ..job.clone()
            };
            if stopped_hosts.contains(route.host()) {
                outcome.failure = Some(Failure::Retry);
                deferred = true;
            } else {
                let answer = run(&part, now_ms, gate, fetch_rest, fetch_archive);
                attempted = true;
                failed_fetch |= answer.failure.is_some();
                if answer.failure == Some(Failure::Throttled) {
                    stopped_hosts.insert(route.host());
                }
                if answer.out_of_retention && archive.is_some() {
                    for &span in recent.minus(&answer.file).spans() {
                        old.add(span);
                    }
                }
                merge(&mut outcome, answer);
            }
        }
        if let Some(route) = archive {
            for (key, spans) in day_parts(route, &job.trade.market, &old) {
                for &span in spans.spans() {
                    groups.entry(key.clone()).or_default().add(span);
                }
            }
        }
        // File paid-for REST tape immediately so an archive wait cannot delay its preservation.
        if !outcome.file.is_empty() {
            file(&job, &outcome);
        }
        prepared.push((
            job,
            archive,
            old,
            outcome,
            attempted,
            deferred,
            failed_fetch,
        ));
    }
    for (job, archive, old, mut outcome, mut attempted, mut deferred, mut failed_fetch) in prepared
    {
        let mut all_unknown = !old.is_empty();
        if !matches!(outcome.failure, Some(Failure::Permanent)) {
            if let Some(route) = archive {
                for (key, spans) in day_parts(route, &job.trade.market, &old) {
                    if !fetched.contains_key(&key) {
                        let skipped = downloaded >= ARCHIVE_BYTES_PER_PASS
                            || stopped_hosts.contains(route.host());
                        let answer = if skipped {
                            JobOutcome {
                                failure: Some(Failure::Retry),
                                ..JobOutcome::default()
                            }
                        } else {
                            let group = CatchupJob {
                                missing: groups[&key].clone(),
                                source: Source::Archive(route),
                                ..job.clone()
                            };
                            let answer = run_archive(
                                &group,
                                route,
                                fetch_archive,
                                ARCHIVE_BYTES_PER_PASS.saturating_sub(downloaded),
                            );
                            attempted |= !answer.waiting;
                            downloaded = downloaded.saturating_add(answer.archive_bytes);
                            if answer.failure == Some(Failure::Throttled) {
                                stopped_hosts.insert(route.host());
                            }
                            answer
                        };
                        fetched.insert(key.clone(), (answer, skipped));
                    }
                    let (mut answer, skipped) = fetched[&key].clone();
                    all_unknown &= answer.archive_unknown && !skipped;
                    // One missing daily file does not refuse the trade's other requested days.
                    if answer.archive_unknown {
                        answer.failure = Some(Failure::Retry);
                    }
                    deferred |= skipped || answer.waiting;
                    failed_fetch |= !skipped && !answer.waiting && answer.failure.is_some();
                    clip(&mut answer, &spans);
                    merge(&mut outcome, answer);
                    if stopped_hosts.contains(route.host()) {
                        break;
                    }
                }
                if all_unknown && outcome.file.is_empty() {
                    outcome.failure = Some(Failure::Permanent);
                }
            }
        }
        clip(&mut outcome, &job.missing);
        if !old.is_empty() || outcome.file.is_empty() {
            file(&job, &outcome);
        }
        finished.push(FinishedJob {
            job,
            failure: outcome.failure,
            attempted: attempted && (!deferred || failed_fetch),
        });
    }
    PassResult {
        finished,
        archive_bytes: downloaded,
    }
}

/// Merge disjoint source portions without widening coverage across unwalked ground.
fn merge(into: &mut JobOutcome, mut part: JobOutcome) {
    into.archive_bytes = into.archive_bytes.saturating_add(part.archive_bytes);
    for &span in part.file.spans() {
        into.file.add(span);
    }
    into.ticks.append(&mut part.ticks);
    into.failure = match (into.failure, part.failure) {
        (Some(Failure::Throttled), _) | (_, Some(Failure::Throttled)) => Some(Failure::Throttled),
        (Some(Failure::Permanent), _) | (_, Some(Failure::Permanent)) => Some(Failure::Permanent),
        (Some(Failure::Retry), _) | (_, Some(Failure::Retry)) => Some(Failure::Retry),
        (None, None) => None,
    };
}

/// Use the replay client's REST protocol without starting any replay worker or cache.
pub(super) fn fetch_rest(
    route: TradeRoute,
    market: &str,
    from: i64,
    to: i64,
    cursor: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    rest::fetch_trades(
        AGENT.get_or_init(rest::agent),
        route,
        market,
        from,
        to,
        cursor,
    )
}

/// Read daily tape through the shared process-wide gate and the installed station disk guard.
pub(super) fn fetch_archive(
    route: ArchiveRoute,
    market: &str,
    spans: &Coverage,
    budget: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    archive::fill_counted(route, market, spans, gate::shared(), budget)
}
