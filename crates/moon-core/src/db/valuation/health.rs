//! Valuation worker health: what the worker is doing, and why it is not making progress.
//!
//! The report and Analytics surfaces can already count valued rows, but a row count cannot tell a
//! slow backfill apart from a worker retrying one unreachable provider forever. This module owns
//! the worker-side facts those surfaces are missing — which stage failed, how many times in a row,
//! and for how long — plus the ONE definition of "stalled" that the log line, the UI chip and the
//! tests all read, so none of them can drift into their own threshold.
//!
//! It deliberately depends on nothing in `worker`: the worker records into these types, the UI
//! reads them, and neither side owns the vocabulary.

use std::time::Duration;

use crate::util::fnv1a64;

#[cfg(test)]
mod tests;

/// Consecutive failures a stage must reach before it can count as stalled.
const STALL_FAILURES: u32 = 3;

/// Milliseconds a stage must have been failing before it can count as stalled.
const STALL_MILLIS: i64 = 180_000;

/// One independently-retried unit of valuation work.
///
/// Health is tracked per stage because the stages fail for unrelated reasons: the derived cache can
/// be unwritable while the provider is fine. A single shared counter would let one healthy stage
/// clear a run that another stage is permanently stuck in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValuationStage {
    /// Reopening the derived cache after it was found unusable.
    CacheRecovery,
    /// Walking persisted report rows that have no prepared value yet.
    Reconcile,
    /// Applying the durable report-change outbox.
    Outbox,
    /// Retrying rows whose closing minute had not closed when they arrived.
    DeferredMinute,
}

impl ValuationStage {
    /// Stable machine identifier, kept out of the translated sentence.
    ///
    /// This is a DIAGNOSTIC token: it goes in the log line and in the tooltip a user can quote back.
    /// The sentence itself resolves a localized label through `valuation.stage.<code>`, so renaming
    /// a code is a locale change too.
    ///
    /// Returns:
    ///     Lowercase code used in logs and tooltips.
    pub const fn code(self) -> &'static str {
        match self {
            Self::CacheRecovery => "cache_recovery",
            Self::Reconcile => "reconcile",
            Self::Outbox => "outbox",
            Self::DeferredMinute => "deferred_minute",
        }
    }

    /// Dense slot used to index per-stage health.
    ///
    /// Returns:
    ///     Position of this stage in `ValuationStatus::stages`.
    const fn index(self) -> usize {
        match self {
            Self::CacheRecovery => 0,
            Self::Reconcile => 1,
            Self::Outbox => 2,
            Self::DeferredMinute => 3,
        }
    }
}

/// What went wrong, at the granularity a user can act on.
///
/// The classes are coarse on purpose. Every provider failure — transport, HTTP status, rate limit,
/// malformed body — leads to the same response (wait; the worker retries with backoff), and the
/// free-text detail already carries the specific `binance HTTP 429` or `bybit retCode 10001`.
/// Splitting the provider case further would add variants that no caller branches on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    /// A public spot-rate route failed and may recover.
    Provider,
    /// Reading the report replica failed.
    ReportRead,
    /// Writing the derived valuation cache failed.
    CacheWrite,
    /// The derived cache is proven damaged; writes are refused until it is rebuilt.
    CacheUnhealthy,
}

impl FailureKind {
    /// Stable machine identifier, kept out of the translated sentence.
    ///
    /// Diagnostic only, like [`ValuationStage::code`]; the localized label lives at
    /// `valuation.kind.<code>`.
    ///
    /// Returns:
    ///     Lowercase code used in logs and tooltips.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::ReportRead => "report_read",
            Self::CacheWrite => "cache_write",
            Self::CacheUnhealthy => "cache_unhealthy",
        }
    }

    /// Dense slot used when packing a state signature.
    ///
    /// Returns:
    ///     Position of this kind among the four classes.
    const fn index(self) -> u64 {
        match self {
            Self::Provider => 0,
            Self::ReportRead => 1,
            Self::CacheWrite => 2,
            Self::CacheUnhealthy => 3,
        }
    }
}

/// A classified failure that does not yet know which stage hit it.
///
/// The helpers that fail — preparing a row, prefetching rates, deleting a partition — are shared by
/// three stages, so they cannot name one. The worker loop stamps the stage at the call site, where
/// it is unambiguous, instead of every helper carrying a stage argument it only forwards.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultCause {
    /// Coarse class the UI renders.
    pub kind: FailureKind,
    /// Free-text description retained for the log line and the tooltip.
    pub detail: String,
}

impl FaultCause {
    /// Classify one failure.
    ///
    /// Args:
    ///     kind: Coarse failure class.
    ///     detail: Free-text description.
    ///
    /// Returns:
    ///     Stage-less classified cause.
    pub fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    /// Attach the stage that hit this cause.
    ///
    /// Args:
    ///     stage: Worker stage reporting the failure.
    ///
    /// Returns:
    ///     Complete publishable fault.
    pub fn at(self, stage: ValuationStage) -> ValuationFault {
        ValuationFault {
            stage,
            kind: self.kind,
            detail: self.detail,
        }
    }
}

/// One complete published failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValuationFault {
    /// Stage that failed.
    pub stage: ValuationStage,
    /// Coarse class the UI renders.
    pub kind: FailureKind,
    /// Free-text description retained for the log line and the tooltip.
    pub detail: String,
}

/// Consecutive-failure run for one stage.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StageHealth {
    /// Failures since this stage last made progress.
    pub consecutive_failures: u32,
    /// Start of the current failing run.
    pub first_failure_ms: Option<i64>,
    /// Earliest time this stage may be attempted again.
    pub retry_at_ms: Option<i64>,
    /// Latest failure, retained while the run lasts.
    pub fault: Option<ValuationFault>,
}

impl StageHealth {
    /// Whether this stage has been failing long enough and often enough to report as stuck.
    ///
    /// BOTH conditions are required. A count alone fires during an ordinary rate-limit backoff,
    /// where three refusals inside a minute are routine; an elapsed time alone fires on one slow
    /// failure that the next attempt clears.
    ///
    /// Args:
    ///     now_ms: Current wall-clock time in Unix milliseconds.
    ///
    /// Returns:
    ///     `true` once the run is both long enough and old enough.
    pub fn is_stalled(&self, now_ms: i64) -> bool {
        self.consecutive_failures >= STALL_FAILURES
            && self
                .first_failure_ms
                .is_some_and(|first| now_ms.saturating_sub(first) >= STALL_MILLIS)
    }

    /// Milliseconds this stage has been failing without interruption.
    ///
    /// Args:
    ///     now_ms: Current wall-clock time in Unix milliseconds.
    ///
    /// Returns:
    ///     Elapsed failing time, or zero when the stage is healthy.
    pub fn failing_for_ms(&self, now_ms: i64) -> i64 {
        self.first_failure_ms
            .map_or(0, |first| now_ms.saturating_sub(first).max(0))
    }

    /// Whole minutes this stage has been failing.
    ///
    /// Every surface states the age of a stall in minutes; rounding it here keeps two surfaces from
    /// disagreeing by one about the same run.
    ///
    /// Args:
    ///     now_ms: Current wall-clock time in Unix milliseconds.
    ///
    /// Returns:
    ///     Truncated elapsed minutes.
    pub fn failing_for_minutes(&self, now_ms: i64) -> i64 {
        self.failing_for_ms(now_ms) / 60_000
    }
}

/// Worker health published to the UI.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ValuationStatus {
    /// Per-stage failing runs, indexed by [`ValuationStage::index`].
    stages: [StageHealth; 4],
}

impl ValuationStatus {
    /// Read one stage's failing run.
    ///
    /// Args:
    ///     stage: Stage to inspect.
    ///
    /// Returns:
    ///     That stage's health.
    pub fn stage(&self, stage: ValuationStage) -> &StageHealth {
        &self.stages[stage.index()]
    }

    /// Clear one stage's failing run, because it completed a turn or has no work left due.
    ///
    /// A turn that changed nothing still counts: a reconciliation sweep over already-valued rows
    /// legitimately commits nothing for minutes, and treating that as absent progress would report
    /// a healthy backfill as stuck. A stage with nothing due counts for the same reason — its
    /// failed workload can be removed by an unrelated report event, and a run left open for work
    /// that is absent would report a stall nothing can ever clear.
    ///
    /// Args:
    ///     stage: Stage that completed a turn or has no work due.
    ///
    /// Returns:
    ///     Whether a failing run was actually cleared. A clean stage reports `false`, so the caller
    ///     can skip republishing on the many turns where nothing was wrong.
    pub fn record_progress(&mut self, stage: ValuationStage) -> bool {
        let health = &mut self.stages[stage.index()];
        if *health == StageHealth::default() {
            return false;
        }
        *health = StageHealth::default();
        true
    }

    /// Clear the failing runs a replaced cache actually explains.
    ///
    /// A rebuilt cache invalidates its own failures — a write that could not land, a file proven
    /// damaged. It says nothing about a provider still refusing to answer or a report replica still
    /// unreadable, so those runs survive. A stage with no fault is normalized to its healthy
    /// default; under the status invariants it is already in that state. Clearing actual failures
    /// wholesale would restart a continuing outage at failure one and retract a stall that had
    /// already been reported for minutes whenever an unrelated cache rebuild succeeded.
    pub fn record_recovery(&mut self) {
        for health in &mut self.stages {
            let explained_by_the_cache = health.fault.as_ref().is_none_or(|fault| {
                matches!(
                    fault.kind,
                    FailureKind::CacheWrite | FailureKind::CacheUnhealthy
                )
            });
            if explained_by_the_cache {
                *health = StageHealth::default();
            }
        }
    }

    /// Extend one stage's failing run and schedule its next attempt.
    ///
    /// Args:
    ///     fault: Classified failure carrying its stage.
    ///     now_ms: Current wall-clock time in Unix milliseconds.
    ///
    /// Returns:
    ///     Delay before this stage may be retried.
    pub fn record_failure(&mut self, fault: ValuationFault, now_ms: i64) -> Duration {
        let health = &mut self.stages[fault.stage.index()];
        health.consecutive_failures = health.consecutive_failures.saturating_add(1);
        health.first_failure_ms.get_or_insert(now_ms);
        health.fault = Some(fault);
        let delay = retry_delay(health.consecutive_failures);
        health.retry_at_ms =
            Some(now_ms.saturating_add(delay.as_millis().min(i64::MAX as u128) as i64));
        delay
    }

    /// Remaining backoff before a stage may be attempted again.
    ///
    /// A committed report change unparks the worker, so a stage that did not consult this would be
    /// retried on every commit and its backoff would never take effect. Zero means eligible now.
    ///
    /// Args:
    ///     stage: Stage about to be attempted.
    ///     now_ms: Current wall-clock time in Unix milliseconds.
    ///
    /// Returns:
    ///     Outstanding wait, or zero when the stage is eligible now.
    pub fn wait_for(&self, stage: ValuationStage, now_ms: i64) -> Duration {
        self.stages[stage.index()]
            .retry_at_ms
            .map_or_else(Duration::default, |at| {
                Duration::from_millis(at.saturating_sub(now_ms).max(0) as u64)
            })
    }

    /// Earliest time an open failing run will cross the stall threshold.
    ///
    /// The worker parks between retries, and with the capped backoff that park can outlast the
    /// pending stall deadline — so without this the transition to stalled would wait for the next
    /// failure rather than happening when it becomes true.
    ///
    /// Args:
    ///     now_ms: Current wall-clock time in Unix milliseconds.
    ///
    /// Returns:
    ///     Deadline of the run closest to stalling, or `None` when none can reach it.
    pub fn next_stall_ms(&self, now_ms: i64) -> Option<i64> {
        self.stages
            .iter()
            .filter(|health| {
                health.consecutive_failures >= STALL_FAILURES && !health.is_stalled(now_ms)
            })
            .filter_map(|health| {
                health
                    .first_failure_ms
                    .map(|first| first.saturating_add(STALL_MILLIS))
            })
            .min()
    }

    /// Whether any stage is failing at all, stalled or not.
    ///
    /// Returns:
    ///     `true` while at least one run is open.
    pub fn is_retrying(&self) -> bool {
        self.stages
            .iter()
            .any(|health| health.consecutive_failures > 0)
    }

    /// The stalled stage that has been failing longest, if any.
    ///
    /// Args:
    ///     now_ms: Current wall-clock time in Unix milliseconds.
    ///
    /// Returns:
    ///     Health of the worst stalled stage, or `None` while none qualifies.
    pub fn stalled(&self, now_ms: i64) -> Option<&StageHealth> {
        self.stages
            .iter()
            .filter(|health| health.is_stalled(now_ms))
            .max_by_key(|health| health.failing_for_ms(now_ms))
    }

    /// Pack the facts the UI renders into one comparable value.
    ///
    /// Attempt counts and raw timestamps are deliberately excluded: the worker publishes a revision
    /// only when this value changes, and including a per-turn counter would wake the UI on every
    /// retry instead of on the transitions the surfaces actually render. The derived stall bit
    /// still changes when the first-failure timestamp crosses the threshold. The fault DETAIL is
    /// included, because the tooltip renders it — leaving it out would let a surface keep quoting
    /// the first failure's text long after the real cause had moved on. Retries repeating the same
    /// text still publish nothing, so this costs a wake only when the cause genuinely changed.
    ///
    /// Args:
    ///     now_ms: Current wall-clock time in Unix milliseconds.
    ///
    /// Returns:
    ///     Signature covering healthy/failing, failure class, detail text, and the stall threshold.
    pub fn signature(&self, now_ms: i64) -> u64 {
        let mut packed = 0u64;
        for health in &self.stages {
            let slot = match &health.fault {
                None => 0,
                Some(fault) => {
                    let state = 1 + fault.kind.index() * 2 + u64::from(health.is_stalled(now_ms));
                    state ^ fnv1a64(fault.detail.as_bytes())
                }
            };
            packed = packed.rotate_left(16) ^ slot;
        }
        packed
    }
}

/// Delay before one stage's next attempt.
///
/// Non-decreasing and capped: an unbounded curve would make the worker take an hour to notice that
/// the network came back, and a flat 30 s keeps hammering a provider that is refusing on purpose.
///
/// Args:
///     consecutive_failures: Length of the current failing run, counting this failure. Production
///         callers pass 1 for the first failure, so the curve they see is 30, 30, 60, 120, 300.
///
/// Returns:
///     Wait before the stage may be attempted again.
fn retry_delay(consecutive_failures: u32) -> Duration {
    let secs = match consecutive_failures {
        0..=2 => 30,
        3 => 60,
        4 => 120,
        _ => 300,
    };
    Duration::from_secs(secs)
}

/// Whether this attempt of a failing run is worth a log line.
///
/// A permanently unreachable provider retries forever. Logging every attempt fills `logs/` with one
/// line per backoff for as long as the terminal runs, which buries the failures that are new.
///
/// Args:
///     attempt: Length of the current failing run, counting this failure.
///
/// Returns:
///     `true` for the first few attempts and then periodically.
const fn should_log(attempt: u32) -> bool {
    matches!(attempt, 1 | 2 | 3 | 5 | 10) || (attempt > 10 && attempt % 20 == 0)
}

/// Emit one failing-run line in the shape every stage shares.
///
/// Args:
///     fault: Classified failure carrying its stage.
///     attempt: Length of the current failing run, counting this failure.
///     failing_for_ms: Elapsed time since the run started.
pub(crate) fn log_fault(fault: &ValuationFault, attempt: u32, failing_for_ms: i64) {
    let stage = fault.stage.code();
    let kind = fault.kind.code();
    let failing_for = failing_for_ms / 1_000;
    let detail = &fault.detail;
    if should_log(attempt) {
        log::warn!(
            "valuation: stage={stage} kind={kind} attempt={attempt} failing_for={failing_for}s detail={detail}"
        );
    } else {
        log::debug!(
            "valuation: stage={stage} kind={kind} attempt={attempt} failing_for={failing_for}s detail={detail}"
        );
    }
}
