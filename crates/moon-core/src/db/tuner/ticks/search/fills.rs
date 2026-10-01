//! The entry fills of the points a search scores, read once per entry and reused (2026-10-02).
//!
//! A scored point replays every training deal through the entry and then the exit, and half of a
//! replay is the entry's fill (the bench of 2026-10-02: 47–52 % on three MoonShot strategies).
//! The fill depends on the entry's parameters alone, and most points a search scores share them
//! with the point before: every point of an exit search, every point of the exit descent under
//! an entry point ([`super::nested`]). Those points read the fills of their entry here instead of
//! replaying it.
//!
//! The key is the entry's parameters as built for each strategy of the sample, never the entry
//! fields of the point: `MaxModifier`, a field of the Exit group, also caps the entry corridor's
//! sum (`params::mshot_params`) — the search leaves it alone on a MoonShot (`not_kinds`), but a
//! held edit can set it — and the built parameters say what the entry is whatever field reached
//! it. The answer is the one an uncached search gives, bit for bit (the bench of 2026-10-02).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rayon::prelude::*;

use super::PreparedDeal;
use crate::db::tuner::ticks::exit::ExitParams;
use crate::db::tuner::ticks::{EntryParams, Fill, entry_fill};

/// Entries whose fills are kept: the restarts run side by side, each with its own entry point
/// (and a screened one with a dozen candidates, [`super::screen`]), and a miss costs one replay
/// of the entry, never a wrong answer.
const KEPT: usize = 64;

/// One entry's fills: the entry per strategy, and each deal's fill in the deals' order.
type Entry = (Vec<EntryParams>, Arc<[Option<Fill>]>);

/// The fills of the last [`KEPT`] entries one search scored, the latest first.
#[derive(Default)]
pub(super) struct FillCache {
    recent: Mutex<VecDeque<Entry>>,
    /// Points scored on fills read before, for the run's statistics.
    reused: AtomicUsize,
}

impl FillCache {
    /// Each deal's entry fill under `params`, read from the cache or replayed.
    ///
    /// Args:
    ///     deals: The deals scored — always the same slice for one cache: the search's training
    ///         slice.
    ///     of_deal: Each deal's index into `params`.
    ///     params: The point's parameters per strategy.
    pub(super) fn fills(
        &self,
        deals: &[PreparedDeal],
        of_deal: &[usize],
        params: &[(EntryParams, ExitParams)],
    ) -> Arc<[Option<Fill>]> {
        let matches = |entry: &Entry| {
            entry.0.len() == params.len() && entry.0.iter().zip(params).all(|(a, (b, _))| a == b)
        };
        {
            let mut recent = self.recent.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(entry) = recent
                .iter()
                .position(matches)
                .and_then(|at| recent.remove(at))
            {
                let fills = Arc::clone(&entry.1);
                recent.push_front(entry);
                self.reused.fetch_add(1, Ordering::Relaxed);
                return fills;
            }
        }
        // Replayed outside the lock: two restarts asking for the same entry at once both replay
        // it, and the second insert only refreshes the first.
        let fills: Arc<[Option<Fill>]> = deals
            .par_iter()
            .zip(of_deal.par_iter())
            .map(|(d, &base)| {
                entry_fill(&d.deal, &d.ticks, &params[base].0, d.entry_line.as_deref())
            })
            .collect::<Vec<_>>()
            .into();
        let key: Vec<EntryParams> = params.iter().map(|(entry, _)| entry.clone()).collect();
        let mut recent = self.recent.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = recent.iter().position(matches) {
            recent.remove(at);
        }
        recent.push_front((key, Arc::clone(&fills)));
        recent.truncate(KEPT);
        fills
    }

    /// Points scored on fills read before.
    pub(super) fn reused(&self) -> usize {
        self.reused.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests;
