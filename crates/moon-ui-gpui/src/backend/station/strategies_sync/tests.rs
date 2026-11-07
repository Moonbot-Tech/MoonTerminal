//! When the strategy history pass runs, and what it does about one strategy.

use std::time::{Duration, Instant};

use moon_core::station_api::StrategySummary;

use super::{
    ATTEMPTS, DELAY, Failures, LIST_CHANGE_DELAY, PERIOD, Phase, RETRY, STOP_AFTER_FAILURES, Step,
    after_list_change, next, step,
};

fn summary(versions: u32, last_from: i64) -> StrategySummary {
    StrategySummary {
        core_uid: 1,
        strategy_id: 2,
        versions,
        first_from: 100,
        last_from,
    }
}

/// Regression target: stopping after one pass, as the trace and price pulls do. Those fill a gap
/// once per run; this one must also carry what a station missed during a long-running terminal.
#[test]
fn a_finished_pass_comes_again_later() {
    let t0 = Instant::now();
    assert_eq!(
        next(Phase::Unstarted, t0, |_| unreachable!()),
        Phase::Waiting(t0 + DELAY, 1)
    );
    assert_eq!(
        next(Phase::Ended(1, true), t0, |_| unreachable!()),
        Phase::Waiting(t0 + PERIOD, 1)
    );
}

#[test]
fn a_failed_attempt_retries_until_the_last_then_waits_for_the_next_pass() {
    let t0 = Instant::now();
    assert_eq!(
        next(Phase::Ended(1, false), t0, |_| unreachable!()),
        Phase::Waiting(t0 + RETRY, 2)
    );
    assert_eq!(
        next(Phase::Ended(ATTEMPTS, false), t0, |_| unreachable!()),
        Phase::Waiting(t0 + PERIOD, 1)
    );
}

/// A running pass is not started again, and a due one starts exactly once.
#[test]
fn a_pass_starts_when_due_and_only_then() {
    let t0 = Instant::now();
    let due = Phase::Waiting(t0 + Duration::from_secs(10), 2);
    assert_eq!(next(due, t0, |_| unreachable!()), due);
    let running = next(due, t0 + Duration::from_secs(10), |n| n == 2);
    assert_eq!(running, Phase::Running(2));
    assert_eq!(next(running, t0, |_| unreachable!()), running);
}

/// No station yet is no end: one set up later must be found without a restart.
#[test]
fn no_station_looks_again_later() {
    let t0 = Instant::now();
    assert_eq!(
        next(Phase::Waiting(t0, 1), t0, |_| false),
        Phase::Waiting(t0 + PERIOD, 1)
    );
}

/// A core added on a new machine should not wait hours for its history.
#[test]
fn a_changed_core_list_brings_the_pass_forward() {
    let t0 = Instant::now();
    assert_eq!(
        after_list_change(Phase::Waiting(t0 + PERIOD, 1), t0),
        Phase::Waiting(t0 + LIST_CHANGE_DELAY, 1)
    );
    // Nothing to bring forward, and a running pass is left to end.
    let soon = Phase::Waiting(t0 + Duration::from_secs(1), 3);
    assert_eq!(after_list_change(soon, t0), soon);
    assert_eq!(after_list_change(Phase::Running(1), t0), Phase::Running(1));
}

#[test]
fn settled_chains_are_left_alone_and_the_rest_exchanged() {
    let held = summary(3, 300);
    assert_eq!(step(Some(&held), Some(&held), false), Step::Nothing);
    assert_eq!(step(None, Some(&held), false), Step::Send);
    assert_eq!(step(Some(&held), None, false), Step::Exchange);
    assert_eq!(
        step(Some(&held), Some(&summary(3, 301)), false),
        Step::Exchange
    );
    assert_eq!(
        step(Some(&held), Some(&summary(4, 300)), false),
        Step::Exchange
    );
    assert_eq!(step(None, None, false), Step::Nothing);
}

/// Regression target: one strategy that always fails stopping the pass at it. Every attempt would
/// hit it first, and the strategies after it would never exchange anything.
#[test]
fn one_failing_strategy_does_not_stop_the_pass() {
    let mut failures = Failures::default();
    assert!(failures.note(1, Err(anyhow::anyhow!("bad chain"))).is_ok());
    assert!(failures.note(2, Ok(())).is_ok());
    assert_eq!(
        failures.total, 1,
        "still counted, so the attempt is retried"
    );
    assert_eq!(failures.in_a_row, 0);
}

/// A station that went away fails everything: say so rather than asking about each of the rest.
#[test]
fn many_failures_in_a_row_stop_the_pass() {
    let mut failures = Failures::default();
    for n in 1..STOP_AFTER_FAILURES {
        assert!(
            failures
                .note(i64::from(n), Err(anyhow::anyhow!("gone")))
                .is_ok()
        );
    }
    assert!(failures.note(0, Err(anyhow::anyhow!("gone"))).is_err());
}

/// Regression target: fetching a forgotten chain every pass for nothing.
#[test]
fn what_was_forgotten_here_is_not_fetched() {
    let held = summary(3, 300);
    assert_eq!(step(Some(&held), None, true), Step::Nothing);
    // A strategy this machine holds again goes on: it is the merge that drops the forgotten part.
    assert_eq!(
        step(Some(&held), Some(&summary(1, 900)), true),
        Step::Exchange
    );
}
