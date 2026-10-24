//! How many cores list their closed trades for the tape recorder at once.
//!
//! Every Ready core used to start its listing in the same session pass, on startup and again an
//! hour later, since their hourly stamps were taken together: seventy threads read the report
//! replica side by side, held the CPU for minutes, and contended for the eight report readers —
//! a ninth waits three seconds and then that core's whole listing gives up until its next pass
//! (#1081). A listing still starts, and is stamped as started, when it is due; it now waits here
//! for a slot before reading. One whose retention generation went stale while it waited reads
//! nothing: its core relists for the new generation instead.

use std::sync::{Condvar, Mutex, PoisonError};

/// Listings reading the replica at once. Half the report-reader budget, so the Mini App, the
/// bot and the station's own reads keep the other half while every core lists.
pub(super) const LISTING_SLOTS: usize = 4;

/// A first-come, first-served counting gate over the listing threads: a core queued behind
/// seventy others is served in turn, never overtaken indefinitely.
pub(super) struct Slots {
    state: Mutex<Queue>,
    changed: Condvar,
}

/// Free slots, and the tickets that order the waiting listings.
struct Queue {
    free: usize,
    /// The ticket the next arriving listing takes.
    next: u64,
    /// The ticket whose turn it is to take a free slot.
    head: u64,
}

/// The one gate every listing passes.
static SLOTS: Slots = Slots::new(LISTING_SLOTS);

/// Wait for a listing slot; it is given back when the returned guard drops, unwinding included.
pub(super) fn acquire() -> Slot<'static> {
    SLOTS.acquire()
}

impl Slots {
    /// A gate with `slots` free.
    pub(super) const fn new(slots: usize) -> Self {
        Self {
            state: Mutex::new(Queue {
                free: slots,
                next: 0,
                head: 0,
            }),
            changed: Condvar::new(),
        }
    }

    /// Block until this caller's turn comes and a slot is free, then take it.
    pub(super) fn acquire(&self) -> Slot<'_> {
        let mut queue = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let ticket = queue.next;
        queue.next += 1;
        while queue.head != ticket || queue.free == 0 {
            queue = self
                .changed
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
        queue.free -= 1;
        queue.head += 1;
        // The next ticket may already have a free slot waiting for it.
        self.changed.notify_all();
        Slot(self)
    }
}

/// One taken slot.
pub(super) struct Slot<'a>(&'a Slots);

impl Drop for Slot<'_> {
    /// Give the slot back and let the head of the queue re-check.
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .free += 1;
        self.0.changed.notify_all();
    }
}

#[cfg(test)]
mod tests;
