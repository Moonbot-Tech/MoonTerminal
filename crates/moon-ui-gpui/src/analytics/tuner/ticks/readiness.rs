//! Whether a strategy's trades are enough to tune it on the "Entry/Exit" axis — the ONE count
//! both the summary card's verdict («Итог подбора») and the strategy list's "Tuning" column read.
//!
//! The card used to count its sample inline (`TicksState::search_too_few`) and its fetchable
//! tape apart (`TicksData::fetchable`); the column needs the same three numbers for every
//! strategy of the list. Both now go through [`Readiness::of_rows`], so the column can never
//! say "can tune" where the card says "cannot".

use moon_core::db::tuner::ticks::search::{self, MIN_SEARCH_DEALS};

use super::state::{DealRow, TapeStatus};

/// What a strategy's replayed rows say about tuning it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics) struct Readiness {
    /// Rows the search would learn on: fit ([`DealRow::fit`]) with their tape in memory — the
    /// card's sample (`TicksData::replayable`).
    pub(in crate::analytics) fit: usize,
    /// Rows whose window the tape covers, fit or not.
    pub(in crate::analytics) covered: usize,
    /// Missing rows and short reproduced tails a source can still extend — the card's
    /// "trades with tape available" (`TicksData::fetchable`).
    pub(in crate::analytics) fetchable: usize,
}

impl Readiness {
    /// Count one strategy's (or one scope's) rows.
    ///
    /// Args:
    ///     rows: The rows after their replay.
    ///     now_ms: One clock for the whole population (`DealRow::fetchable_at`).
    ///     min_tail_s: One tail floor for the whole population.
    ///
    /// Returns:
    ///     The sample, the covered rows and the fetchable rows.
    pub(in crate::analytics::tuner) fn of_rows<'a>(
        rows: impl IntoIterator<Item = &'a DealRow>,
        now_ms: i64,
        min_tail_s: u32,
    ) -> Self {
        let mut out = Self::default();
        for row in rows {
            if row.fit() && row.ticks.is_some() {
                out.fit += 1;
            }
            if row.tape == TapeStatus::Covered {
                out.covered += 1;
            }
            if row.fetchable_at(now_ms, min_tail_s) {
                out.fetchable += 1;
            }
        }
        out
    }

    /// The sample while it is too small to search ([`search::sample_floor`]); `None` at or
    /// above the floor. The card's "cannot tune" verdict is exactly this.
    pub(in crate::analytics::tuner) fn too_few(&self) -> Option<usize> {
        search::sample_floor(self.fit).is_err().then_some(self.fit)
    }

    /// The smallest sample the search runs on.
    pub(in crate::analytics::tuner) fn min() -> usize {
        MIN_SEARCH_DEALS
    }
}

#[cfg(test)]
mod tests;
