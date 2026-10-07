//! How a search of the Entry group reads a point that leaves deals open inside the tape (LinKvo,
//! 2026-10-07): it steers by it instead of refusing it, and answers only with a point that closes
//! every deal.
//!
//! An entry that moves the fill moves every exit after it, and on a sample of hundreds of deals
//! nearly every entry move left at least one of them open at the tape's end: 36 697 of 37 342
//! points a search of six MoonShot strategies scored were refused for it (2026-10-07), and the
//! search answered the strategies as they stand every time. So the entry is steered by what it
//! makes on the deals it CLOSED — an open deal adds nothing and is no trade, so the `min_n` floor
//! is also the floor on how many it must close — and the exit searched under each entry point
//! closes deals first and earns second ([`closing_score`]): it walks the open deals down to none,
//! where a point that refuses on any of them gave the walk nothing to follow.
//!
//! The answer is still a point that closes every deal it buys (`closing`): the best such point
//! the search met, in any restart ([`Tracker`]). The best one that leaves deals open is the
//! [`super::Candidate`] beside it, when it earns more than the answer on the deals it closed.

use std::sync::{Mutex, PoisonError};

use super::closing::Replayed;
use super::{Point, Tally, better};

/// Which score a restart walks by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Steer {
    /// The answer's own: a point that leaves a deal open is refused. An exit search.
    Answer,
    /// What the point makes on the deals it closed — an open deal adds nothing and is no trade.
    /// The entry points of a search of the Entry group.
    Closed,
    /// Deals closed first, profit second ([`closing_score`]). The exit under an entry point.
    Closing,
}

/// What one open deal weighs in the exit descent under an entry point ([`closing_score`]): more
/// than the profit any sample can sum to, in either metric, so one deal fewer open always beats
/// any profit, and profit decides among points that leave as many open.
pub(super) const OPEN_PENALTY: f64 = 1e9;

/// The score the exit descent under an entry point walks by: the trades the point closed, then a
/// loss of [`OPEN_PENALTY`] for every deal it left open. Never shown and never an answer — the
/// figures past the penalty are not money.
pub(super) fn closing_score(replayed: &Replayed) -> Tally {
    let mut tally = replayed.closed.clone();
    for _ in 0..replayed.open {
        tally.push(-OPEN_PENALTY);
    }
    tally
}

/// A point the search met, with its score and the restart that met it.
#[derive(Clone, Debug)]
pub(super) struct Met {
    pub(super) point: Point,
    /// The trades it closed inside the tape.
    pub(super) score: Tally,
    /// The deals it left open there.
    pub(super) open: usize,
    pub(super) restart: usize,
}

/// The best points a search met: the one that closes every deal, which it may answer, and the
/// one that leaves some open, which it hands back beside the answer. Among equal scores the LOWEST restart keeps
/// its place, so the parallel restarts answer as a sequential run would.
#[derive(Default)]
pub(super) struct Tracker {
    answer: Mutex<Option<Met>>,
    candidate: Mutex<Option<Met>>,
}

impl Tracker {
    /// Offer a point scored inside a restart, past its refusals and within the risk limits on the
    /// deals it closed.
    ///
    /// Args:
    ///     point: The point as scored.
    ///     replayed: Its replay of the training slice.
    ///     restart: The restart that scored it.
    ///     min_n: The trade floor; a point that leaves deals open and closes fewer than this is
    ///         no candidate.
    pub(super) fn offer(&self, point: &Point, replayed: &Replayed, restart: usize, min_n: i64) {
        let slot = if replayed.open == 0 {
            &self.answer
        } else if replayed.closed.n >= min_n {
            &self.candidate
        } else {
            return;
        };
        // Called on every replay of every restart: the point is cloned only when it takes the
        // slot, and the lock is held for a comparison.
        let mut best = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if takes(best.as_ref(), &replayed.closed, restart, min_n) {
            *best = Some(Met {
                point: point.clone(),
                score: replayed.closed.clone(),
                open: replayed.open,
                restart,
            });
        }
    }

    /// The best point that closes every deal, if the search met one.
    pub(super) fn answer(&self) -> Option<Met> {
        self.answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The best point that leaves deals open, if the search met one.
    pub(super) fn candidate(&self) -> Option<Met> {
        self.candidate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Whether a point scoring `score` in `restart` takes the place of `old`: it beats it, or ties it
/// from a lower restart.
fn takes(old: Option<&Met>, score: &Tally, restart: usize, min_n: i64) -> bool {
    match old {
        None => true,
        Some(old) => {
            better(score, &old.score, min_n)
                || (!better(&old.score, score, min_n) && restart < old.restart)
        }
    }
}

#[cfg(test)]
mod tests;
