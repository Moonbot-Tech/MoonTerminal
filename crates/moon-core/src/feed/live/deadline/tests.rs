use super::*;

/// The rule the whole type exists for, and the one a bare `Instant`-and-`bool` pair has now dropped
/// twice in this loop: an owner that cannot act on due work must be able to push it out WITHOUT
/// dropping it, or `wait` keeps answering zero and `live::run` spins at 100% instead of sleeping.
#[test]
fn declined_work_stays_queued_and_stops_answering_zero() {
    let start = Instant::now();
    let mut deadline = CoalescedDeadline::new(Duration::from_millis(250));

    deadline.queue(start);
    assert!(
        deadline.is_due(start),
        "the first trigger after idle runs at once"
    );
    assert_eq!(deadline.wait(start), Some(Duration::ZERO));

    deadline.defer(start);

    assert!(deadline.is_queued(), "the work is still owed");
    assert!(!deadline.is_due(start), "but not right now");
    assert_eq!(deadline.wait(start), Some(Duration::from_millis(250)));
    assert!(deadline.is_due(start + Duration::from_millis(250)));
}

/// Deferring must not invent work: a caller declining something nobody asked for would otherwise
/// queue a run out of nothing, and the loop would wake for it.
#[test]
fn deferring_an_idle_deadline_queues_nothing() {
    let start = Instant::now();
    let mut deadline = CoalescedDeadline::new(Duration::from_millis(250));

    deadline.defer(start);

    assert!(!deadline.is_queued());
    assert_eq!(deadline.wait(start), None);
}

/// A burst of triggers must collapse into ONE run, and must not push its own deadline further out
/// with every arrival — that is the difference between a coalescing deadline and a debounce.
#[test]
fn a_burst_of_triggers_collapses_into_one_run() {
    let start = Instant::now();
    let mut deadline = CoalescedDeadline::new(Duration::from_millis(250));
    deadline.mark_attempt(start);

    deadline.queue(start + Duration::from_millis(10));
    deadline.queue(start + Duration::from_millis(20));
    deadline.queue(start + Duration::from_millis(200));

    assert_eq!(
        deadline.wait(start + Duration::from_millis(200)),
        Some(Duration::from_millis(50)),
        "still the deadline the first trigger set, one interval after the last run"
    );
}

/// A trigger arriving long after the last run waits for nothing: the cooldown has already elapsed,
/// and the table it stands for owes an immediate send.
#[test]
fn a_trigger_after_the_cooldown_runs_at_once() {
    let start = Instant::now();
    let mut deadline = CoalescedDeadline::new(Duration::from_millis(250));
    deadline.mark_attempt(start);

    let later = start + Duration::from_secs(9);
    deadline.queue(later);

    assert!(deadline.is_due(later));
    assert_eq!(deadline.wait(later), Some(Duration::ZERO));
}

/// Running the work clears it. Without this the loop would publish the same set every pass, since
/// nothing else lowers the flag.
#[test]
fn running_the_work_clears_it() {
    let start = Instant::now();
    let mut deadline = CoalescedDeadline::new(Duration::from_millis(250));

    deadline.queue(start);
    deadline.mark_attempt(start);

    assert!(!deadline.is_queued());
    assert!(!deadline.is_due(start + Duration::from_secs(9)));
    assert_eq!(deadline.wait(start), None);
}

/// An authoritative answer arriving on its own satisfies queued work without starting a cooldown —
/// the difference from [`CoalescedDeadline::mark_attempt`], which records that WE asked.
#[test]
fn an_authoritative_answer_satisfies_without_starting_a_cooldown() {
    let start = Instant::now();
    let mut deadline = CoalescedDeadline::new(Duration::from_millis(250));

    deadline.queue(start);
    deadline.satisfy();
    assert!(!deadline.is_queued());

    deadline.queue(start);
    assert!(
        deadline.is_due(start),
        "no attempt was recorded, so the next trigger is still immediate"
    );
}

/// The sibling `defer` on the recurring poll shares a name with the coalesced one and must not
/// share its conditionality: a poll is pending by construction, so there is nothing to check first
/// and nothing that could make a deferral silently do nothing.
#[test]
fn a_polls_deferral_always_moves_it() {
    let start = Instant::now();
    let interval = Duration::from_secs(6 * 60 * 60);
    let mut poll = PollDeadline::new(interval, start);

    assert!(poll.is_due(start), "a fresh poll asks at once");

    poll.defer(start);

    assert!(!poll.is_due(start));
    assert_eq!(poll.wait(start), interval);
}

/// Work that starts life up to date must not fire on its first trigger: the orders table is
/// constructed at the top of a connection, and `new` would let its first publication land a whole
/// interval earlier than it always has. Swapping `idle_since` back for `new` is the edit this
/// catches.
#[test]
fn a_deadline_idle_since_now_makes_its_first_trigger_wait() {
    let start = Instant::now();
    let interval = Duration::from_millis(250);
    let mut deadline = CoalescedDeadline::idle_since(interval, start);

    deadline.queue(start);

    assert!(!deadline.is_due(start), "the cooldown was already running");
    assert_eq!(deadline.wait(start), Some(interval));
    assert!(deadline.is_due(start + interval));
}
