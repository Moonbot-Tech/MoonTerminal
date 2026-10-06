use super::*;

/// What one gated stage attempt did.
pub(in crate::db::valuation::worker) enum Attempt {
    /// The stage is inside its backoff and was not run.
    Resting,
    /// The stage was attempted and reported its own outcome.
    Done(StageTurn),
    /// The stage failed; its run is recorded and its backoff selected.
    Failed,
    /// The failure proved the derived cache damaged, so recovery owns it and this stage does not.
    CacheLost,
}

/// Run one stage under its own backoff, recording its health and selecting the next wait.
///
/// The four gated work stages share this scaffold so the backoff gate, unavailable-replica handling,
/// progress/failure publication, cache-health check before fault recording, and selection of the
/// shortest wait are written once. Stage-specific work and reactions to completed outcomes stay at
/// the call sites.
///
/// Args:
///     status: Worker health being tracked.
///     sink: Channel publishing health to the UI.
///     stage: Stage to attempt.
///     now_ms: Current wall-clock time in Unix milliseconds.
///     retry_after: Earliest wait selected so far this turn, narrowed in place.
///     run: The stage's own work.
///
/// Returns:
///     Whether the stage rested, completed with its own outcome, failed, or lost the cache.
pub(in crate::db::valuation::worker) fn attempt(
    status: &mut ValuationStatus,
    sink: &mut StatusSink,
    stage: ValuationStage,
    now_ms: i64,
    retry_after: &mut Option<Duration>,
    run: impl FnOnce() -> Result<StageTurn, FaultCause>,
) -> Attempt {
    let wait = status.wait_for(stage, now_ms);
    if !wait.is_zero() {
        *retry_after = shorter(*retry_after, wait);
        return Attempt::Resting;
    }
    match run() {
        Ok(turn) => {
            // A turn the stage could not act on is not progress: clearing an unresolved provider
            // outage because the report replica went missing would retract a reported stall.
            if turn.is_progress() {
                record_progress(status, sink, stage, now_ms);
            } else {
                *retry_after = shorter(*retry_after, REPLICA_POLL);
            }
            Attempt::Done(turn)
        }
        Err(cause) => {
            // Order matters: a read against the attached cache can PROVE the cache damaged, and
            // that failure belongs to the recovery stage. Recording it here first would open a run
            // whose kind names the wrong subsystem and which then outlives the repair as the
            // oldest stall.
            if !crate::db::valuation::cache_is_healthy() {
                return Attempt::CacheLost;
            }
            let delay = note_failure(status, sink, cause.at(stage), now_ms);
            *retry_after = shorter(*retry_after, delay);
            Attempt::Failed
        }
    }
}

/// Cap one park by the current-rate freshness deadline.
///
/// Expiry is evaluated when the loop turns, so a park that outlives the cutoff leaves an expired
/// figure on screen for the difference — and both parks can be minutes long: the recovery arm's
/// cache backoff and the stage's own provider backoff both reach five minutes. Shared by BOTH so a
/// third park cannot be added without the cap.
///
/// Args:
///     current: Pass state holding the gathered rates.
///     wanted: Whether the application-wide current-rate mode is enabled.
///     delay: Park the caller selected.
///     now_ms: Current wall clock in Unix milliseconds.
///
/// Returns:
///     The shorter of the caller's park and the wait until the earliest expiry.
fn until_expiry(
    current: &CurrentRateState,
    wanted: bool,
    delay: Duration,
    now_ms: i64,
) -> Duration {
    cap_at_deadline(delay, current.next_expiry_ms().filter(|_| wanted), now_ms)
}

/// Shorten a park so it ends no later than an outstanding deadline.
///
/// Args:
///     delay: Park the caller selected.
///     deadline: Instant the park must not outlive, in Unix milliseconds.
///     now_ms: Current wall clock in Unix milliseconds.
///
/// Returns:
///     The shorter of the park and the wait to the deadline, never zero.
fn cap_at_deadline(delay: Duration, deadline: Option<i64>, now_ms: i64) -> Duration {
    match deadline {
        Some(deadline) => delay.min(Duration::from_millis(
            deadline.saturating_sub(now_ms).max(1) as u64,
        )),
        None => delay,
    }
}

/// Cap one park by the next UTC minute deadline while a deferred row could become eligible there.
///
/// A deferred row's candle can close at the next minute boundary independently of any stage's own
/// backoff, so a park selected only from stage waits can oversleep it by as long as the longest
/// backoff, up to five minutes.
///
/// Args:
///     deferred_due: Whether at least one deferred row could become eligible at the next minute
///         deadline.
///     delay: Park the caller selected.
///     now_ms: Current wall clock in Unix milliseconds.
///
/// Returns:
///     The shorter of the caller's park and the wait until the next minute deadline, or the
///     caller's park unchanged while no deferred row is waiting on that deadline.
pub(in crate::db::valuation::worker) fn until_next_minute(
    deferred_due: bool,
    delay: Duration,
    now_ms: i64,
) -> Duration {
    cap_at_deadline(
        delay,
        deferred_due.then(|| next_minute_deadline_ms(now_ms)),
        now_ms,
    )
}

/// Apply every outstanding deadline to one selected park.
///
/// Split out of [`park_worker`] so all three caps are applied together in a pure function.
///
/// Args:
///     status: Current worker health, carrying the stall deadline.
///     current: Pass state, carrying the freshness deadline.
///     wanted: Whether the application-wide current-rate mode is enabled.
///     deferred_due: Whether at least one deferred row could become eligible at the next minute
///         deadline.
///     delay: Park the loop selected on its own.
///     now_ms: Current wall clock in Unix milliseconds.
///
/// Returns:
///     The park shortened by the freshness, stall, and next-minute deadlines, never zero.
pub(in crate::db::valuation::worker) fn park_delay(
    status: &ValuationStatus,
    current: &CurrentRateState,
    wanted: bool,
    deferred_due: bool,
    delay: Duration,
    now_ms: i64,
) -> Duration {
    let delay = until_expiry(current, wanted, delay, now_ms);
    let delay = until_stall(status, delay, now_ms);
    until_next_minute(deferred_due, delay, now_ms)
}

/// Park the worker for `delay`, shortened by every deadline it must not oversleep.
///
/// The ONE park in this loop. Applying all three caps here keeps the invariant "no park outlives a
/// deadline" true for every caller, including any future park arm.
///
/// Args:
///     status: Current worker health, carrying the stall deadline.
///     current: Pass state, carrying the freshness deadline.
///     current_wanted: Whether the application-wide current-rate mode is enabled.
///     deferred_due: Whether at least one deferred row could become eligible at the next minute
///         boundary.
///     delay: Park the loop selected on its own.
///     now_ms: Current wall clock in Unix milliseconds.
pub(in crate::db::valuation::worker) fn park_worker(
    status: &ValuationStatus,
    current: &CurrentRateState,
    current_wanted: &AtomicBool,
    deferred_due: bool,
    delay: Duration,
    now_ms: i64,
) {
    let wanted = current_wanted.load(Ordering::Acquire);
    let delay = park_delay(status, current, wanted, deferred_due, delay, now_ms);
    std::thread::park_timeout(delay);
}

/// Shorten a park so a run that is only waiting out the clock still publishes when it stalls.
///
/// A backoff can extend past the pending stall deadline, so without this the worker would sleep
/// straight through the moment its own definition of "stalled" became true and the surfaces would
/// keep saying "retrying" after that stopped being the honest word.
///
/// Args:
///     status: Current worker health.
///     delay: Park the loop selected on its own.
///     now_ms: Current wall-clock time in Unix milliseconds.
///
/// Returns:
///     The shorter of the requested park and the wait to the next stall deadline.
fn until_stall(status: &ValuationStatus, delay: Duration, now_ms: i64) -> Duration {
    cap_at_deadline(delay, status.next_stall_ms(now_ms), now_ms)
}

/// Keep the earliest of two outstanding waits so no eligible stage oversleeps.
///
/// Args:
///     current: Wait already selected this turn, if any.
///     candidate: Wait requested by another stage.
///
/// Returns:
///     The shorter of the two.
pub(in crate::db::valuation::worker) fn shorter(
    current: Option<Duration>,
    candidate: Duration,
) -> Option<Duration> {
    Some(current.map_or(candidate, |current| current.min(candidate)))
}

/// Record that one stage completed a turn, publishing only if that cleared a failing run.
///
/// A turn that changed nothing still counts as progress: reconciliation can find that every row is
/// already valued, and treating that quiet result as absent progress would report a healthy worker
/// as stuck.
///
/// Args:
///     status: Worker health being tracked.
///     sink: Channel publishing health to the UI.
///     stage: Stage that completed a turn or has no work due.
///     now_ms: Current wall-clock time in Unix milliseconds.
pub(in crate::db::valuation::worker) fn record_progress(
    status: &mut ValuationStatus,
    sink: &mut StatusSink,
    stage: ValuationStage,
    now_ms: i64,
) {
    if status.record_progress(stage) {
        sink.publish(status, now_ms);
    }
}

/// Record one stage failure, log it at a thinning cadence, and publish any transition.
///
/// Args:
///     status: Worker health being tracked.
///     sink: Channel publishing health to the UI.
///     fault: Classified failure carrying its stage.
///     now_ms: Current wall-clock time in Unix milliseconds.
///
/// Returns:
///     Backoff before that stage may be attempted again.
pub(in crate::db::valuation::worker) fn note_failure(
    status: &mut ValuationStatus,
    sink: &mut StatusSink,
    fault: ValuationFault,
    now_ms: i64,
) -> Duration {
    let stage = fault.stage;
    let delay = status.record_failure(fault, now_ms);
    let health = status.stage(stage);
    if let Some(fault) = &health.fault {
        health::log_fault(
            fault,
            health.consecutive_failures,
            health.failing_for_ms(now_ms),
        );
    }
    sink.publish(status, now_ms);
    delay
}
