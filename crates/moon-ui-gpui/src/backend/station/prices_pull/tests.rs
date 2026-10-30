use std::time::{Duration, Instant};

use super::{ATTEMPTS, DELAY, Phase, RETRY, next};

#[test]
fn a_failed_attempt_retries_until_the_last_and_a_finished_one_stops() {
    let t0 = Instant::now();
    let due = next(Phase::Unstarted, t0, |_| unreachable!());
    assert_eq!(due, Phase::Waiting(t0 + DELAY, 1));
    // Not due yet: nothing starts.
    assert_eq!(next(due, t0, |_| unreachable!()), due);
    let running = next(due, t0 + DELAY, |n| n == 1);
    assert_eq!(running, Phase::Running(1));
    // A running attempt stays running until its thread says how it ended.
    assert_eq!(next(running, t0 + DELAY, |_| unreachable!()), running);

    let later = t0 + Duration::from_secs(1_000);
    assert_eq!(
        next(Phase::Ended(1, false), later, |_| unreachable!()),
        Phase::Waiting(later + RETRY, 2)
    );
    assert_eq!(
        next(Phase::Ended(ATTEMPTS, false), later, |_| unreachable!()),
        Phase::Done
    );
    assert_eq!(
        next(Phase::Ended(1, true), later, |_| unreachable!()),
        Phase::Done
    );
    // No station, or no thread: done for this run.
    assert_eq!(next(due, t0 + DELAY, |_| false), Phase::Done);
}
