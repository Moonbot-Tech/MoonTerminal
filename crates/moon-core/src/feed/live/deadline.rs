//! The two deadline shapes the live loop schedules work with.
//!
//! Both exist for one reason: the loop at the heart of [`super::run`] sleeps on the EARLIEST of
//! every deadline it owns, so each one has to answer "may I run now" and "how long until I may"
//! from the same state — and an owner that CANNOT act on due work has to be able to push it out
//! without dropping it. Written as a bare `Instant` plus a `bool` that last operation is a line
//! somebody has to remember, and both times it was forgotten in this loop the result was the same:
//! the wait stayed zero and the thread spun at 100% instead of sleeping. As a type it is a method
//! that cannot be forgotten, only not called — and not calling it fails a test.
//!
//! - [`CoalescedDeadline`] — work a TRIGGER queues: an order event, an account change. The first
//!   trigger after an idle period runs at once; later ones collapse into the cooldown.
//! - [`PollDeadline`] — work nothing announces, so it recurs on its own: a value that ages without
//!   telling anyone.

use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

/// Work queued by a trigger, run at most once per interval.
///
/// The first trigger after an idle period is due immediately; the ones that arrive during the
/// cooldown collapse into a single run at its end. `due_at` carries BOTH "is anything owed" and
/// "when may it run", which is what keeps the two from being answered by separate fields that can
/// disagree.
#[derive(Clone, Copy, Debug)]
pub(super) struct CoalescedDeadline {
    interval: Duration,
    due_at: Option<Instant>,
    last_attempt: Option<Instant>,
}

impl CoalescedDeadline {
    /// Creates an idle deadline with the supplied cooldown.
    pub(super) fn new(interval: Duration) -> Self {
        Self {
            interval,
            due_at: None,
            last_attempt: None,
        }
    }

    /// Creates an idle deadline whose cooldown is ALREADY running, as if the work had just been
    /// done at `now`.
    ///
    /// For work that starts life up to date — a table with nothing to publish until something
    /// changes. Without it the first trigger of the process fires at once instead of one interval
    /// after the owner began, which is a different first frame for anyone watching.
    pub(super) fn idle_since(interval: Duration, now: Instant) -> Self {
        Self {
            interval,
            due_at: None,
            last_attempt: Some(now),
        }
    }

    /// Queues an immediate run after idle, or the earliest one the cooldown allows.
    ///
    /// Idempotent: a trigger arriving while work is already queued does not move it, so a burst
    /// cannot push its own deadline further and further out.
    pub(super) fn queue(&mut self, now: Instant) {
        if self.due_at.is_some() {
            return;
        }
        self.due_at = Some(
            self.last_attempt
                .map(|last_attempt| (last_attempt + self.interval).max(now))
                .unwrap_or(now),
        );
    }

    /// Records that something authoritative already did the work, and clears what was queued.
    pub(super) fn satisfy(&mut self) {
        self.due_at = None;
    }

    /// Records a run and starts the cooldown.
    pub(super) fn mark_attempt(&mut self, now: Instant) {
        self.due_at = None;
        self.last_attempt = Some(now);
    }

    /// Pushes queued work out by one interval, for an owner that cannot act on it yet.
    ///
    /// KEEPS the work queued — it is still owed, only not now. This is the operation a
    /// hand-rolled `Instant`-and-`bool` pair forgets, and forgetting it is what leaves
    /// [`Self::wait`] returning zero on every pass while the owner keeps declining, which spins the
    /// thread. Deferring an idle deadline queues nothing: work nobody asked for must not appear
    /// because somebody declined to do it — which is the one difference from
    /// [`PollDeadline::defer`], where there is no such thing as idle and every deadline is pending
    /// by construction.
    pub(super) fn defer(&mut self, now: Instant) {
        if self.due_at.is_some() {
            self.due_at = Some(now + self.interval);
        }
    }

    /// Whether anything is queued at all, due or not.
    pub(super) fn is_queued(&self) -> bool {
        self.due_at.is_some()
    }

    /// Whether queued work may run at `now`.
    pub(super) fn is_due(&self, now: Instant) -> bool {
        self.due_at.is_some_and(|deadline| now >= deadline)
    }

    /// The remaining wait for queued work, or `None` when nothing is queued.
    ///
    /// Preserves the original deadline: asking how long is left must never move it.
    pub(super) fn wait(&self, now: Instant) -> Option<Duration> {
        self.due_at
            .map(|deadline| deadline.saturating_duration_since(now))
    }
}

/// A plain recurring deadline: unlike [`CoalescedDeadline`] it needs no trigger, because nothing in
/// the event stream announces that a value went stale.
#[derive(Clone, Copy, Debug)]
pub(super) struct PollDeadline {
    interval: Duration,
    due_at: Instant,
    last_attempt: Option<Instant>,
}

impl PollDeadline {
    /// Creates a poll that is due immediately, so the first value arrives as soon as the core can
    /// answer rather than one interval later.
    pub(super) fn new(interval: Duration, now: Instant) -> Self {
        Self {
            interval,
            due_at: now,
            last_attempt: None,
        }
    }

    /// Brings the poll due at once, unless one ran within `min_gap`.
    ///
    /// For an event that means "this value may have changed" — a core reaching Ready. The gap is
    /// what keeps a flapping core from asking on every reconnect.
    pub(super) fn poll_now_unless_recent(&mut self, now: Instant, min_gap: Duration) {
        let recent = self
            .last_attempt
            .is_some_and(|last| now.saturating_duration_since(last) < min_gap);
        if !recent {
            self.due_at = now;
        }
    }

    /// Brings the next poll forward to `delay` from now, for a retry after an unanswered check.
    /// Never pushes it further out than it already is.
    pub(super) fn retry_in(&mut self, now: Instant, delay: Duration) {
        let retry_at = now + delay;
        if retry_at < self.due_at {
            self.due_at = retry_at;
        }
    }

    /// Pushes a due poll out by one interval, for a caller that cannot act on it yet.
    ///
    /// Unconditional, unlike [`Self::retry_in`]: a deadline that stays due while its caller keeps
    /// declining it leaves the loop with a zero-length wait, and the thread spins. A poll has
    /// nothing to be idle about, so unlike [`CoalescedDeadline::defer`] there is no queued state to
    /// check first — and the interval is the poll's own, not the caller's to name twice.
    pub(super) fn defer(&mut self, now: Instant) {
        self.due_at = now + self.interval;
    }

    /// Returns whether the next poll may run at `now`.
    pub(super) fn is_due(&self, now: Instant) -> bool {
        now >= self.due_at
    }

    /// Records a poll and schedules the next one a full interval out.
    pub(super) fn mark_attempt(&mut self, now: Instant) {
        self.due_at = now + self.interval;
        self.last_attempt = Some(now);
    }

    /// Returns the remaining wait before the next poll.
    pub(super) fn wait(&self, now: Instant) -> Duration {
        self.due_at.saturating_duration_since(now)
    }
}
