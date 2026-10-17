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
use crate::station_api::CatchupFailReason;

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

/// Why a job failed: the reason the station counts, and the error's own text for its log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fault {
    pub(crate) reason: CatchupFailReason,
    pub(crate) detail: String,
}

impl Fault {
    fn new(reason: CatchupFailReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

/// Name a REST refusal from its error and the HTTP status its response published (0 when the
/// call never got a response, so a transport failure reads as the network's).
pub(crate) fn rest_fault(error: &rest::FetchError, status: u16) -> Fault {
    match error {
        rest::FetchError::UnknownSymbol => Fault::new(
            CatchupFailReason::Http,
            format!("unknown symbol, HTTP {status}"),
        ),
        rest::FetchError::OutOfRetention => {
            Fault::new(CatchupFailReason::Other, "range out of retention")
        }
        rest::FetchError::Throttled { diagnostic, .. } => {
            Fault::new(CatchupFailReason::Throttled, diagnostic.clone())
        }
        rest::FetchError::Transient(text) => {
            let reason = match status {
                0 => CatchupFailReason::Network,
                200..=299 => CatchupFailReason::Corrupt,
                _ => CatchupFailReason::Http,
            };
            Fault::new(reason, text.clone())
        }
        rest::FetchError::RequestFault(text) => Fault::new(
            CatchupFailReason::RequestFault,
            format!("request fault: {text}"),
        ),
    }
}

/// Name a paginator stop; a page error caught on the way explains a transient or unknown stop.
fn stop_fault(stop: Option<TickAbandon>, last: Option<Fault>) -> Fault {
    let other = |detail: &str| Fault::new(CatchupFailReason::Other, detail);
    match stop {
        Some(TickAbandon::Transient) => {
            last.unwrap_or_else(|| Fault::new(CatchupFailReason::Network, "transient failure"))
        }
        Some(TickAbandon::UnknownSymbol) => {
            last.unwrap_or_else(|| Fault::new(CatchupFailReason::Http, "unknown symbol"))
        }
        Some(TickAbandon::RequestFault) => {
            last.unwrap_or_else(|| Fault::new(CatchupFailReason::RequestFault, "request fault"))
        }
        Some(TickAbandon::RateLimited) => Fault::new(
            CatchupFailReason::Throttled,
            "host gate closed before the request",
        ),
        Some(TickAbandon::Deadline) => other("walk deadline passed"),
        Some(TickAbandon::OverPageBudget) => other("page budget spent"),
        Some(TickAbandon::OverTickBudget) => other("tick budget spent"),
        Some(TickAbandon::OutOfRetention) => other("range out of retention"),
        Some(TickAbandon::Cancelled) => other("walk cancelled"),
        Some(TickAbandon::Empty) | None => other("empty slice at the retention edge unconfirmed"),
    }
}

/// Name an archive refusal; a day missing past its publish delay is what `UnknownSymbol` means.
pub(crate) fn archive_fault(error: &archive::ArchiveError) -> Fault {
    use archive::ArchiveError as E;
    match error {
        E::NotPublished => Fault::new(CatchupFailReason::Other, "archive day not published yet"),
        E::UnknownSymbol => Fault::new(
            CatchupFailReason::ArchiveMissing,
            "archive day missing past its publish delay",
        ),
        E::Throttled { .. } => Fault::new(CatchupFailReason::Throttled, "archive host throttled"),
        E::NoRoom => Fault::new(
            CatchupFailReason::NoRoom,
            "disk reserve refused the archive",
        ),
        E::TooLarge => Fault::new(
            CatchupFailReason::Other,
            "archive day over the size ceiling",
        ),
        E::BudgetDeferred => Fault::new(CatchupFailReason::Other, "pass archive budget spent"),
        E::Http(status) => Fault::new(CatchupFailReason::Http, format!("archive HTTP {status}")),
        E::Transient(text) => Fault::new(CatchupFailReason::Network, text.clone()),
        E::Corrupt(text) => Fault::new(CatchupFailReason::Corrupt, text.clone()),
    }
}

/// Paid-for coverage and ticks survive a partial walk or an archive refusal.
#[derive(Clone, Debug, Default)]
pub(crate) struct JobOutcome {
    pub(crate) file: Coverage,
    pub(crate) ticks: Vec<Tick>,
    pub(crate) failure: Option<Failure>,
    /// Why `failure` happened; set whenever it is.
    pub(crate) fault: Option<Fault>,
    /// Bytes returned by this archive fetch, including failed or partially completed bodies.
    pub(crate) archive_bytes: u64,
    /// An expired REST range can move to the archive without imposing host backoff.
    out_of_retention: bool,
    /// A per-day unknown result becomes permanent only after every requested day agrees.
    archive_unknown: bool,
    /// Unpublished days and budget admission waits are not failed download attempts.
    waiting: bool,
    /// A day file no longer fit the pass's remaining archive budget.
    budget_spent: bool,
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
    let mut last = None;
    let verdict = paginate_ticks(
        route,
        &plan,
        TICK_BUDGET,
        60,
        || false,
        |trade| asked_at.elapsed() >= Duration::from_secs(if trade { 180 } else { 45 }),
        &mut observer,
        |from, to, cursor| {
            let page = gate::absorb_page(
                gate,
                route.host(),
                fetch(route, &job.trade.market, from, to, cursor),
            );
            if let Err(error) = &page {
                last = Some(rest_fault(error, rest::exchange_notice().status));
            }
            page
        },
    );
    let throttled = gate::settle_verdict(gate, route.host(), &verdict, asked_at);
    let stop = match &verdict {
        TickVerdict::Ready(harvest) => harvest.stop,
        TickVerdict::Abandoned(reason) => Some(*reason),
    };
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
        outcome.fault = Some(
            last.filter(|fault| fault.reason == CatchupFailReason::Throttled)
                .unwrap_or_else(|| {
                    Fault::new(CatchupFailReason::Throttled, "host refused further pages")
                }),
        );
    } else if outcome.failure.is_some() {
        outcome.fault = Some(stop_fault(stop, last));
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
        TickAbandon::UnknownSymbol | TickAbandon::RequestFault => Some(Failure::Permanent),
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
    let budget_spent = matches!(
        &result,
        Err(archive::ArchiveError::BudgetDeferred)
            | Ok(archive::ArchiveFill {
                deferred: Some(archive::ArchiveError::BudgetDeferred),
                ..
            })
    );
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
            fault: fill.deferred.as_ref().map(archive_fault),
            failure: fill.deferred.map(archive_failure),
            archive_bytes: bytes,
            budget_spent,
            ..JobOutcome::default()
        },
        Err(error) => JobOutcome {
            waiting: matches!(
                error,
                archive::ArchiveError::BudgetDeferred | archive::ArchiveError::NotPublished
            ),
            archive_unknown: error == archive::ArchiveError::UnknownSymbol,
            fault: Some(archive_fault(&error)),
            failure: Some(archive_failure(error)),
            archive_bytes: bytes,
            budget_spent,
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
        | archive::ArchiveError::Http(_)
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

/// Whether part of the job's missing tape can only come from a daily archive at `now_ms`.
pub(super) fn needs_archive(job: &CatchupJob, now_ms: i64) -> bool {
    let (_, _, archive, old) = portions(job, now_ms);
    archive.is_some() && !old.is_empty()
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
    /// Why `failure` happened, for the pass summary and its log.
    pub(crate) fault: Option<Fault>,
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
                outcome.fault = Some(Fault::new(
                    CatchupFailReason::Throttled,
                    "host stopped earlier this pass",
                ));
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
                                fault: Some(if stopped_hosts.contains(route.host()) {
                                    Fault::new(
                                        CatchupFailReason::Throttled,
                                        "archive host stopped earlier this pass",
                                    )
                                } else {
                                    Fault::new(
                                        CatchupFailReason::Other,
                                        "pass archive budget spent",
                                    )
                                }),
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
                            // A day that no longer fits ends archive work until the hour rolls;
                            // smaller days would only open venue requests to be deferred again.
                            if answer.budget_spent {
                                downloaded = downloaded.max(ARCHIVE_BYTES_PER_PASS);
                            }
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
                    outcome.fault = Some(archive_fault(&archive::ArchiveError::UnknownSymbol));
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
            fault: outcome.fault,
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
    // The reason follows the failure that wins below; the first one named keeps a tie.
    if part.fault.is_some() && (into.fault.is_none() || rank(part.failure) > rank(into.failure)) {
        into.fault = part.fault;
    }
    into.failure = match (into.failure, part.failure) {
        (Some(Failure::Throttled), _) | (_, Some(Failure::Throttled)) => Some(Failure::Throttled),
        (Some(Failure::Permanent), _) | (_, Some(Failure::Permanent)) => Some(Failure::Permanent),
        (Some(Failure::Retry), _) | (_, Some(Failure::Retry)) => Some(Failure::Retry),
        (None, None) => None,
    };
}

/// Precedence of a failure when two source portions disagree, matching [`merge`].
fn rank(failure: Option<Failure>) -> u8 {
    match failure {
        None => 0,
        Some(Failure::Retry) => 1,
        Some(Failure::Permanent) => 2,
        Some(Failure::Throttled) => 3,
    }
}

/// Use the replay client's REST protocol without starting any replay worker or cache.
pub(super) fn fetch_rest(
    route: TradeRoute,
    market: &str,
    from: i64,
    to: i64,
    cursor: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    super::pacing::wait();
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
    archive::with_pacing(super::pacing::wait, || {
        archive::fill_counted(route, market, spans, gate::shared(), budget)
    })
}
