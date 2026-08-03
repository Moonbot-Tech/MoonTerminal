//! Valuation health state and publication-contract tests.

use super::*;

/// Build one failing run of `count` attempts spread evenly across `span_ms`.
///
/// Args:
///     stage: Stage to fail.
///     count: Number of consecutive failures.
///     span_ms: Wall-clock distance between the first and last failure.
///
/// Returns:
///     Status carrying that run, and the timestamp of its last failure.
fn failing_run(stage: ValuationStage, count: u32, span_ms: i64) -> (ValuationStatus, i64) {
    let mut status = ValuationStatus::default();
    let step = if count > 1 {
        span_ms / i64::from(count - 1)
    } else {
        0
    };
    let mut now = 1_000_000;
    for attempt in 0..count {
        now = 1_000_000 + step * i64::from(attempt);
        status.record_failure(
            FaultCause::new(FailureKind::Provider, "binance HTTP 429").at(stage),
            now,
        );
    }
    (status, now)
}

/// Both the count and the elapsed time must be satisfied before a stage reports as stuck.
///
/// Breakage: simplifying `StageHealth::is_stalled` to `consecutive_failures > 0` (dropping either
/// half of the conjunction). The report footer would then show the red "valuation stalled" chip
/// during every ordinary rate-limit backoff, which is the normal way a healthy backfill paces
/// itself — the one signal that is supposed to mean "this will not recover on its own" would fire
/// constantly and stop being read.
#[test]
fn a_stall_needs_both_enough_failures_and_enough_time() {
    let (fast, now) = failing_run(ValuationStage::Reconcile, 10, 60_000);
    assert!(
        fast.stalled(now).is_none(),
        "ten failures inside a minute is a backoff, not a stall"
    );

    let (slow, now) = failing_run(ValuationStage::Reconcile, 3, 200_000);
    assert!(
        slow.stalled(now).is_some(),
        "three failures across 200s is a stall"
    );

    let (few, now) = failing_run(ValuationStage::Reconcile, 2, 200_000);
    assert!(
        few.stalled(now).is_none(),
        "two failures cannot stall however long they took"
    );
}

/// Progress on one stage must not clear another stage's failing run.
///
/// Breakage: replacing the per-stage `stages` array in `ValuationStatus` with one shared
/// `consecutive_failures` counter, so `record_progress` resets it wholesale, or making its return
/// value a constant `true`. The deferred-minute
/// stage completes a turn every minute by design, so a permanently unwritable cache — which fails
/// only in `Reconcile` — would be reset once a minute and could never reach the stall threshold.
#[test]
fn progress_on_one_stage_leaves_another_stages_run_intact() {
    let (mut status, now) = failing_run(ValuationStage::Reconcile, 3, 200_000);
    assert!(status.stalled(now).is_some());

    assert!(
        !status.record_progress(ValuationStage::DeferredMinute),
        "a stage that was never failing has nothing to clear"
    );

    assert!(
        status.stalled(now).is_some(),
        "a healthy deferred-minute turn must not clear the reconcile stall"
    );
    assert_eq!(
        status.stage(ValuationStage::Reconcile).consecutive_failures,
        3
    );
    assert_eq!(
        status
            .stage(ValuationStage::DeferredMinute)
            .consecutive_failures,
        0
    );

    assert!(
        status.record_progress(ValuationStage::Reconcile),
        "clearing a real failing run reports that it cleared one"
    );
    assert!(
        status.stalled(now).is_none(),
        "its own progress does clear it"
    );
    assert!(!status.is_retrying());
}

/// A rebuilt cache must clear only the runs the cache itself explains.
///
/// Breakage: changing `ValuationStatus::record_recovery` to
/// `self.stages = Default::default()`.
/// The stages fail for unrelated reasons, so a cache rebuild triggered by one of them would retract
/// a provider outage that is still ongoing: its run restarts at failure one, the footer drops back
/// from "stalled" to "retrying", and the user is told the problem went away when nothing about the
/// exchange changed.
#[test]
fn a_cache_rebuild_clears_cache_failures_and_leaves_the_rest() {
    let mut status = ValuationStatus::default();
    let start = 1_000_000;
    for attempt in 0..3 {
        status.record_failure(
            FaultCause::new(FailureKind::Provider, "binance HTTP 503")
                .at(ValuationStage::Reconcile),
            start + attempt * 100_000,
        );
    }
    for attempt in 0..3 {
        status.record_failure(
            FaultCause::new(FailureKind::CacheWrite, "disk is full").at(ValuationStage::Outbox),
            start + attempt * 100_000,
        );
    }
    let now = start + 300_000;
    assert!(status.stalled(now).is_some());

    status.record_recovery();

    let provider = status.stage(ValuationStage::Reconcile);
    assert_eq!(
        provider.consecutive_failures, 3,
        "the exchange is still down; rebuilding a local file proves nothing about it"
    );
    assert!(
        provider.is_stalled(now),
        "so its stall must survive the rebuild"
    );
    assert_eq!(
        status.stage(ValuationStage::Outbox).consecutive_failures,
        0,
        "the cache write failure described the file that was just replaced"
    );
}

/// The backoff must grow and then stop growing.
///
/// Breakage: making `retry_delay` unbounded (for example `30 << attempt`). After a few hours of a
/// dead network the delay would exceed an hour, so the worker would keep the report unvalued long
/// after connectivity returned. The cap is what bounds recovery latency.
#[test]
fn the_retry_delay_is_non_decreasing_and_capped() {
    let mut previous = Duration::ZERO;
    for attempt in 1..=200u32 {
        let delay = retry_delay(attempt);
        assert!(
            delay >= previous,
            "attempt {attempt} shortened the backoff to {delay:?}"
        );
        assert!(
            delay <= Duration::from_secs(300),
            "attempt {attempt} exceeded the cap at {delay:?}"
        );
        previous = delay;
    }
    assert_eq!(retry_delay(1), Duration::from_secs(30));
    assert_eq!(retry_delay(200), Duration::from_secs(300));
}

/// A stage inside its backoff must refuse an attempt even when the worker is woken early.
///
/// Breakage: dropping the `retry_at_ms` check from `wait_for` (or never setting it in
/// `record_failure`). Every committed report change unparks the worker, so on a busy terminal a
/// permanently failing provider route would be re-requested several times a second instead of
/// every 30-300 seconds.
#[test]
fn a_stage_inside_its_backoff_refuses_an_early_attempt() {
    let mut status = ValuationStatus::default();
    let now = 1_000_000;
    let delay = status.record_failure(
        FaultCause::new(FailureKind::Provider, "binance HTTP 500").at(ValuationStage::Outbox),
        now,
    );

    let eligible_at = now + delay.as_millis() as i64;
    assert!(!status.wait_for(ValuationStage::Outbox, now + 1).is_zero());
    assert!(!status
        .wait_for(ValuationStage::Outbox, eligible_at - 1)
        .is_zero());
    assert!(status
        .wait_for(ValuationStage::Outbox, eligible_at)
        .is_zero());
    assert!(
        status
            .wait_for(ValuationStage::Reconcile, now + 1)
            .is_zero(),
        "the backoff is per stage"
    );
}

/// The published signature must ignore attempt counts and move only on rendered transitions.
///
/// Breakage: folding `consecutive_failures` into `ValuationStatus::signature`, or dropping
/// removing `fnv1a64(fault.detail.as_bytes())` from it.
/// The worker bumps `status_revision` whenever the signature changes, so the report panel and the
/// Analytics window would take a fresh status snapshot on every single retry — a repaint every
/// 30 seconds forever, for a chip whose text did not change.
#[test]
fn the_signature_moves_on_transitions_and_not_on_retries() {
    let mut status = ValuationStatus::default();
    let start = 1_000_000;
    let healthy = status.signature(start);

    status.record_failure(
        FaultCause::new(FailureKind::Provider, "binance HTTP 429").at(ValuationStage::Reconcile),
        start,
    );
    let failing = status.signature(start);
    assert_ne!(healthy, failing, "healthy -> failing is a transition");

    status.record_failure(
        FaultCause::new(FailureKind::Provider, "binance HTTP 429").at(ValuationStage::Reconcile),
        start + 30_000,
    );
    assert_eq!(
        failing,
        status.signature(start + 30_000),
        "a retry repeating the same cause is not a transition"
    );

    status.record_failure(
        FaultCause::new(FailureKind::Provider, "binance HTTP 500").at(ValuationStage::Reconcile),
        start + 60_000,
    );
    assert_ne!(
        failing,
        status.signature(start + 60_000),
        "the tooltip renders the detail, so a changed cause must reach it"
    );

    status.record_failure(
        FaultCause::new(FailureKind::CacheWrite, "disk is full").at(ValuationStage::Reconcile),
        start + 90_000,
    );
    assert_ne!(
        failing,
        status.signature(start + 90_000),
        "a different failure class is a transition"
    );

    let before_stall = status.signature(start + 90_000);
    assert_ne!(
        before_stall,
        status.signature(start + STALL_MILLIS + 1),
        "crossing the stall threshold is a transition even with no new failure"
    );
}

/// The worst stalled stage is the one that has been failing longest.
///
/// Breakage: returning the first stalled stage in declaration order instead of the oldest run. The
/// tooltip would name whichever stage happens to sort first rather than the one actually stuck, so
/// a user chasing a five-minute cache failure would be shown a provider code.
///
/// The older run is deliberately given to the LATER-declared stage. With the two orderings
/// agreeing, a `.find()` over declaration order passes this test too and the breakage above goes
/// undetected.
#[test]
fn the_reported_stall_is_the_oldest_failing_stage() {
    let mut status = ValuationStatus::default();
    let start = 1_000_000;
    for attempt in 0..3 {
        status.record_failure(
            FaultCause::new(FailureKind::CacheWrite, "disk is full")
                .at(ValuationStage::DeferredMinute),
            start + attempt * 200_000,
        );
    }
    for attempt in 0..3 {
        status.record_failure(
            FaultCause::new(FailureKind::Provider, "bybit HTTP 502")
                .at(ValuationStage::CacheRecovery),
            start + 300_000 + attempt * 100_000,
        );
    }
    let now = start + 600_000;

    let stalled = status.stalled(now).expect("both stages qualify");
    let fault = stalled
        .fault
        .as_ref()
        .expect("a stalled run keeps its fault");
    assert_eq!(fault.stage, ValuationStage::DeferredMinute);
    assert_eq!(fault.kind.code(), "cache_write");
}

/// A run only waiting out the clock must still report when it will cross the threshold.
///
/// Breakage: `next_stall_ms` returning `None` for an open run, or `worker.rs:until_stall` omitting
/// its deadline cap. The backoff reaches 300 seconds while the threshold is 180, so the
/// worker would sleep straight through the moment its own definition became true and the footer
/// would keep saying "retrying" for minutes after that stopped being the honest word.
#[test]
fn a_run_short_of_the_threshold_reports_when_it_will_cross() {
    let mut status = ValuationStatus::default();
    let start = 1_000_000;
    assert_eq!(status.next_stall_ms(start), None, "nothing is failing");

    status.record_failure(
        FaultCause::new(FailureKind::Provider, "binance HTTP 500").at(ValuationStage::Outbox),
        start,
    );
    status.record_failure(
        FaultCause::new(FailureKind::Provider, "binance HTTP 500").at(ValuationStage::Outbox),
        start + 30_000,
    );
    assert_eq!(
        status.next_stall_ms(start + 30_000),
        None,
        "two failures can never reach the threshold, however long they wait"
    );

    status.record_failure(
        FaultCause::new(FailureKind::Provider, "binance HTTP 500").at(ValuationStage::Outbox),
        start + 60_000,
    );
    assert_eq!(
        status.next_stall_ms(start + 60_000),
        Some(start + STALL_MILLIS),
        "the deadline is measured from the first failure of the run"
    );
    assert!(status.stalled(start + STALL_MILLIS).is_some());
    assert_eq!(
        status.next_stall_ms(start + STALL_MILLIS),
        None,
        "an already-stalled run has no deadline left to wake for"
    );
}

/// Rare log attempts must stay rare forever.
///
/// Breakage: `should_log` returning `true` unconditionally, or the periodic arm dropping its
/// modulus. A permanently unreachable provider retries indefinitely, so one line per attempt writes
/// to `logs/` for as long as the terminal runs and buries every other warning.
#[test]
fn logging_thins_out_as_a_failing_run_continues() {
    let logged: Vec<u32> = (1..=60).filter(|attempt| should_log(*attempt)).collect();
    assert_eq!(logged, vec![1, 2, 3, 5, 10, 20, 40, 60]);
    assert!(!should_log(1_000_001));
}

/// The same fault on two different stages must pack to two different signatures.
///
/// Breakage: `health.rs::ValuationStatus::signature` going back to `packed.rotate_left(16) ^ slot`.
/// That is collision-free only while there are exactly four stages — 64/16 — and there are now
/// five, so the first and fifth would rotate into the same lanes and pack identically. The worker
/// publishes a revision only when this value moves, so an aliased pair means the surfaces never
/// learn that the trouble moved from one stage to another: the footer would keep naming the stage
/// that recovered while the one that broke stays silent.
#[test]
fn every_stage_packs_into_its_own_part_of_the_signature() {
    let now = 1_700_000_000_000;
    let stages = [
        ValuationStage::CacheRecovery,
        ValuationStage::Reconcile,
        ValuationStage::Outbox,
        ValuationStage::DeferredMinute,
        ValuationStage::CurrentRates,
    ];
    let signatures: Vec<u64> = stages
        .iter()
        .map(|stage| {
            let mut status = ValuationStatus::default();
            // Identical cause and text on purpose: the STAGE is then the only thing that differs.
            status.record_failure(
                FaultCause::new(FailureKind::Provider, "the same reason everywhere").at(*stage),
                now,
            );
            status.signature(now)
        })
        .collect();

    for (index, signature) in signatures.iter().enumerate() {
        assert!(
            !signatures[index + 1..].contains(signature),
            "stage {} shares a signature with a later one",
            stages[index].code()
        );
        assert_ne!(
            *signature,
            ValuationStatus::default().signature(now),
            "a failing stage must not pack like a healthy worker"
        );
    }
}
