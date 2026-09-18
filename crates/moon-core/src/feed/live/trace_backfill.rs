//! Paced asking of archived order traces, one queue per core connection.
//!
//! Every `request_traces` this feed sends goes through here — a trade window's ask, a row that
//! just closed, and the startup backfill alike — so the core sees one bounded stream whatever the
//! terminal is doing: at most [`WINDOW`] requests unanswered and at most one new request per
//! [`MIN_INTERVAL`]. The protocol's own advice is the reason: traces are served beside the
//! replica catch-up, and fetching them "for every row during initialization" is what it forbids.
//!
//! A window's ask goes to the FRONT of the queue and the rest to the back: the user is looking at
//! that trade now, while the backfill has all day. A uid already queued or in flight is not queued
//! twice.
//!
//! An old core never answers, and MoonProto turns each silence into a failure after its own
//! timeout; three failures in a row are taken as "this core does not know the command" and the
//! queue is dropped rather than walked through at twelve seconds a slot. A later ask starts over.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Most requests unanswered at once.
const WINDOW: usize = 8;

/// Least time between two sends, so a full window drains at no more than five a second.
const MIN_INTERVAL: Duration = Duration::from_millis(200);

/// Bound on the queue. A backfill hands over a few hundred at most; past this the oldest asks —
/// the back of the queue — are dropped.
const QUEUE_CAP: usize = 1024;

/// Consecutive failures after which the queue is abandoned.
const FAIL_STREAK_LIMIT: u32 = 3;

/// The queue and its in-flight window.
#[derive(Default)]
pub(super) struct TracePacer {
    queue: VecDeque<i64>,
    queued: HashSet<i64>,
    in_flight: HashSet<i64>,
    last_sent: Option<Instant>,
    failed_streak: u32,
}

impl TracePacer {
    /// Queue a uid a window asked for, ahead of everything else.
    pub(super) fn push_front(&mut self, uid: i64) {
        if uid == 0 || self.in_flight.contains(&uid) {
            return;
        }
        if self.queued.insert(uid) {
            self.queue.push_front(uid);
        } else if let Some(pos) = self.queue.iter().position(|q| *q == uid) {
            // Already queued behind others: move it up, the user is waiting on it.
            self.queue.remove(pos);
            self.queue.push_front(uid);
        }
        self.trim();
    }

    /// Queue uids the writer named — a closed row or a backfill — behind whatever is waiting.
    pub(super) fn push_back(&mut self, uids: impl IntoIterator<Item = i64>) {
        for uid in uids {
            if uid == 0 || self.in_flight.contains(&uid) || !self.queued.insert(uid) {
                continue;
            }
            self.queue.push_back(uid);
        }
        self.trim();
    }

    /// Drop the back of the queue past [`QUEUE_CAP`].
    fn trim(&mut self) {
        while self.queue.len() > QUEUE_CAP {
            if let Some(uid) = self.queue.pop_back() {
                self.queued.remove(&uid);
            }
        }
    }

    /// The core answered, one way or the other: free the slot.
    ///
    /// Args:
    ///     uid: The answered row.
    ///     ok: `false` for a failed request. Three in a row abandon the queue.
    pub(super) fn answered(&mut self, uid: i64, ok: bool) {
        self.in_flight.remove(&uid);
        if ok {
            self.failed_streak = 0;
            return;
        }
        self.failed_streak += 1;
        if self.failed_streak >= FAIL_STREAK_LIMIT && !self.queue.is_empty() {
            log::warn!(
                "order traces: {} consecutive failed request(s), {} queued ask(s) dropped — \
                 the core does not answer trace requests",
                self.failed_streak,
                self.queue.len()
            );
            self.queue.clear();
            self.queued.clear();
        }
    }

    /// Whether the next queued uid may be sent now.
    fn may_send(&self, now: Instant) -> bool {
        !self.queue.is_empty()
            && self.in_flight.len() < WINDOW
            && self
                .last_sent
                .is_none_or(|last| now.duration_since(last) >= MIN_INTERVAL)
    }

    /// One uid to send now, or `None` while the window is full, the interval has not passed, or
    /// nothing is queued. The caller sends it; a send that never left is returned through
    /// [`Self::answered`] like any other failure.
    pub(super) fn take_due(&mut self, now: Instant) -> Option<i64> {
        if !self.may_send(now) {
            return None;
        }
        let uid = self.queue.pop_front()?;
        self.queued.remove(&uid);
        self.in_flight.insert(uid);
        self.last_sent = Some(now);
        Some(uid)
    }

    /// How long until the next send could happen: the feed loop's wait bound.
    ///
    /// `None` when nothing is queued or the window is full — then only an answer moves things,
    /// and answers wake the loop by themselves.
    pub(super) fn next_due(&self, now: Instant) -> Option<Duration> {
        if self.queue.is_empty() || self.in_flight.len() >= WINDOW {
            return None;
        }
        Some(self.last_sent.map_or(Duration::ZERO, |last| {
            MIN_INTERVAL.saturating_sub(now.duration_since(last))
        }))
    }

    /// Requests sent and not yet answered.
    #[cfg(test)]
    pub(super) fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Uids waiting to be sent.
    #[cfg(test)]
    pub(super) fn queued(&self) -> usize {
        self.queue.len()
    }
}

#[cfg(test)]
mod tests;
