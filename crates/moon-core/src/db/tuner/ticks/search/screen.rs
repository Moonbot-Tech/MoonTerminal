//! The screen of a search of both groups (LinKvo, 2026-10-02: "a switch in the settings, on by
//! default"): of the entry moves one step of the descent tries — every other value of a field,
//! or every pair move — each is first scored under the exit of the best entry point so far, one
//! replay, and only the [`KEEP`] best are scored by a whole exit descent under them
//! ([`super::nested`]).
//!
//! A whole exit descent is some 640 replays (the bench of 2026-09-25), and it ran under every
//! entry value of every field. The screen keeps the reason the descent is nested — an entry that
//! pays only with its own exit is still searched with its own exit, as long as it ranks among the
//! best under the old one — and drops the long tail of moves that read worse than the rest even
//! there. It can change the answer: a move that ranks low under the old exit and pays with its
//! own is not reached. Off, every move runs its exit descent, as before.

use super::{Point, better_score};
use crate::db::metrics::Tally;

/// Entry moves per step kept past the screen for a whole exit descent.
pub(super) const KEEP: usize = 3;

/// What the screen makes of one move.
#[derive(Clone, Debug)]
pub(super) enum Screened {
    /// The move's whole score is known not to beat the point the descent stands on — an entry
    /// the corridor rules refuse, or one an exit descent already ran under — so it takes no
    /// place among the kept.
    Skip,
    /// The move's score under the exit found so far; `None` when that exit leaves it refused.
    Scored(Option<Tally>),
}

/// The screen's scoring of a move.
pub(super) type Screen<'a> = dyn Fn(&Point) -> Screened + Sync + 'a;

/// The `keep` best of `scored`, best first; of two that score alike the earlier comes first — the
/// caller lists the moves nearest the point first, so a step none of whose moves scores keeps the
/// nearest. A skipped move is never kept.
///
/// Picked by repeated scans rather than a sort: the order [`better_score`] gives is a preorder
/// over floats, and a sort handed a comparator that is not a total order may panic.
pub(super) fn best<T>(scored: Vec<(T, Screened)>, keep: usize, min_n: i64) -> Vec<T> {
    let mut rest: Vec<(T, Option<Tally>)> = scored
        .into_iter()
        .filter_map(|(item, screened)| match screened {
            Screened::Skip => None,
            Screened::Scored(score) => Some((item, score)),
        })
        .collect();
    let mut out = Vec::with_capacity(keep.min(rest.len()));
    while out.len() < keep && !rest.is_empty() {
        let mut at = 0;
        for i in 1..rest.len() {
            if better_score(&rest[i].1, &rest[at].1, min_n) {
                at = i;
            }
        }
        out.push(rest.remove(at).0);
    }
    out
}

#[cfg(test)]
mod tests;
