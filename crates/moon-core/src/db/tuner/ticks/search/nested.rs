//! "Search all" over the Entry and the Exit group at once (LinKvo, 2026-09-25): every entry point
//! the descent visits is scored by a whole descent of the exit under it, so a point is judged with
//! the exit that suits it rather than with the one that suited the point before.
//!
//! One descent over both groups turns one field at a time, and a move of the entry is then judged
//! under the exit tuned for the OLD entry: a better entry that needs its own exit reads as worse,
//! is refused, and the exit settles around the entry it started from (the bench of 2026-09-25,
//! `ENTRY_EXIT_TUNER.md` §11). Nesting the exit under each entry point is what reaches it.
//!
//! The price is the product: every entry point costs a whole exit descent. Two things keep it
//! down without changing what a point scores. The exit descent of an entry point starts from the
//! exit found under the best entry point so far — the neighbour's exit is the nearest start there
//! is, and a start near the answer converges in a pass or two. And an entry point is scored once
//! per restart: the exit found under it is kept ([`ExitCache`]). Not across restarts: each walks
//! from its own start, and an exit found from another restart's start would cap an entry point at
//! what that start could reach. An entry point the corridor rules refuse is refused before its exit
//! is searched: those rules read the entry alone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use super::{Point, Walked, better_score, coupled, descend};
use crate::db::metrics::Tally;
use crate::db::tuner::threshold_search::SearchHandle;
use crate::db::tuner::ticks::params::TickParam;
use crate::db::tuner::ticks::params::range::Grids;

/// An entry point in one spelling whatever order its fields were set in.
type EntryKey = Vec<(&'static str, String)>;

fn key_of(point: &Point) -> EntryKey {
    let mut key: EntryKey = point.iter().map(|(k, v)| (*k, v.clone())).collect();
    key.sort();
    key
}

/// The exit found under each entry point one restart scored, and what the two scored together.
#[derive(Default)]
struct ExitCache {
    found: Mutex<HashMap<EntryKey, (Option<Tally>, Point)>>,
}

impl ExitCache {
    fn get(&self, key: &EntryKey) -> Option<(Option<Tally>, Point)> {
        self.found
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    fn put(&self, key: EntryKey, found: (Option<Tally>, Point)) {
        self.found
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, found);
    }
}

/// What one nested descent walks and how: the two groups' fields in the order this restart visits
/// them, the moves each group makes, and what refuses an entry point outright.
pub(super) struct Nested<'a, 'c> {
    pub grids: &'a Grids,
    /// Where each number field starts on its grid (`deps::dependents_of`).
    pub start: &'a HashMap<&'static str, usize>,
    /// The Entry group's fields, the outer descent's.
    pub entry: &'a [&'static TickParam],
    /// The Exit group's fields, the inner descent's.
    pub exit: &'a [&'static TickParam],
    /// The Entry fields that move in pairs (`descend`).
    pub pairs: &'a [&'static TickParam],
    /// The coupling of the entry fields — none of them is in the Delta Modifiers section, so it
    /// couples nothing; it keeps the exit's diagonals out of the outer descent.
    pub entry_coupling: &'a coupled::Coupling<'c>,
    /// The coupling of the exit fields, the Delta Modifiers section's.
    pub exit_coupling: &'a coupled::Coupling<'c>,
    pub min_n: i64,
    pub max_passes: usize,
    pub handle: &'a SearchHandle,
    /// Whether an entry point is out before its exit is searched — the corridor rules, which
    /// read the entry alone; the search counts the refusal there.
    pub refused: &'a (dyn Fn(&Point) -> bool + Sync),
    /// Entry points scored by an exit descent of their own, over every restart.
    pub searched: &'a AtomicUsize,
}

/// The descent of one restart over both groups, from `point`: the outer descent walks the entry
/// fields, and scores each entry point by an inner descent of the exit fields under it.
///
/// Args:
///     point: The restart's start — its entry fields start the outer descent, its exit fields
///         the first inner one.
///     nested: What to walk and how.
///     evaluate: The score of a whole point, entry and exit.
///
/// Returns:
///     Where it stopped — the entry and the exit found under it — or `None` when the run was
///     stopped.
pub(super) fn descend_nested(
    point: Point,
    nested: &Nested<'_, '_>,
    evaluate: &(dyn Fn(&Point) -> Option<Tally> + Sync),
) -> Option<Walked> {
    let is_exit = |key: &str| nested.exit.iter().any(|f| f.key == key);
    let (exit, entry): (Point, Point) = point.into_iter().partition(|(key, _)| is_exit(key));
    // The exit found under the best entry point so far, with that point's score: where the next
    // entry point's exit descent starts. The outer descent keeps a point only when it beats the
    // score, so the best so far is the point it stands on.
    let warm: Mutex<(Option<Tally>, Point)> = Mutex::new((None, exit));
    let cache = ExitCache::default();
    // A point better than the best so far hands its exit on.
    let keep_if_best = |score: &Option<Tally>, found: &Point| {
        let mut best = warm.lock().unwrap_or_else(PoisonError::into_inner);
        if better_score(score, &best.0, nested.min_n) {
            *best = (score.clone(), found.clone());
        }
    };
    let score_entry = |entry: &Point| -> Option<Tally> {
        if (nested.refused)(entry) {
            return None;
        }
        let key = key_of(entry);
        // Scored before in this restart, and offered as the best then.
        if let Some((score, _)) = cache.get(&key) {
            return score;
        }
        let mut start = warm
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1
            .clone();
        start.extend(entry.iter().map(|(k, v)| (*k, v.clone())));
        // A stop inside reads here as a refused point; the outer descent notices the stop at its
        // next field and answers nothing.
        let walked = descend(
            start,
            nested.grids,
            nested.exit,
            &[],
            nested.exit_coupling,
            nested.start,
            evaluate,
            nested.min_n,
            nested.max_passes,
            nested.handle,
        )?;
        nested.searched.fetch_add(1, Ordering::Relaxed);
        nested.handle.record_point();
        let found: Point = walked
            .point
            .into_iter()
            .filter(|(key, _)| is_exit(key))
            .collect();
        keep_if_best(&walked.score, &found);
        cache.put(key, (walked.score.clone(), found));
        walked.score
    };
    let walked = descend(
        entry,
        nested.grids,
        nested.entry,
        nested.pairs,
        nested.entry_coupling,
        nested.start,
        &score_entry,
        nested.min_n,
        nested.max_passes,
        nested.handle,
    )?;
    // The exit scored with the entry the descent stopped on; a refused one never ran its own, and
    // keeps the last exit found.
    let exit = cache
        .get(&key_of(&walked.point))
        .map(|(_, exit)| exit)
        .unwrap_or_else(|| {
            warm.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .1
                .clone()
        });
    let mut point = walked.point;
    point.extend(exit);
    Some(Walked { point, ..walked })
}

#[cfg(test)]
mod tests;
