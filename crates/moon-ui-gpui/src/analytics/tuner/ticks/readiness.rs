//! Whether a strategy's trades are enough to tune it on the "Entry/Exit" axis — the ONE count
//! both the summary card's verdict («Итог подбора») and the strategy list's "Tuning" column read.
//!
//! The card used to count its sample inline (`TicksState::search_too_few`) and its fetchable
//! tape apart (`TicksData::fetchable`); the column needs the same eligibility counts and their
//! exclusion breakdown for every strategy. Both go through [`Readiness::of_rows`], so the column can never
//! say "can tune" where the card says "cannot".

use moon_core::db::tuner::ticks::search::{self, MIN_SEARCH_DEALS};

use super::state::DealRow;
use super::why::{DealChip, DealWhy};

/// What a strategy's replayed rows say about tuning it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics) struct Readiness {
    /// All deals in the period, partitioned by the exclusion counters and `fit`.
    pub(in crate::analytics) deals: usize,
    /// Tape missing, unavailable, fetching or refused.
    pub(in crate::analytics) no_tape: usize,
    /// Reproduced deals whose retained tail is shorter than the sample floor.
    pub(in crate::analytics) short_tail: usize,
    /// Reproduced deals with a sufficient tail but a gap between entry and exit.
    pub(in crate::analytics) holed: usize,
    /// The model judged the deal and missed its entry or exit.
    pub(in crate::analytics) missed: usize,
    /// Unchecked deals, including those whose retained tape the memory cap dropped.
    pub(in crate::analytics) unjudged: usize,
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
    ///     The sample, coverage, fetchable rows and mutually exclusive exclusion counts.
    pub(in crate::analytics::tuner) fn of_rows<'a>(
        rows: impl IntoIterator<Item = &'a DealRow>,
        now_ms: i64,
        min_tail_s: u32,
    ) -> Self {
        let mut out = Self::default();
        let admission = super::tail::Admission::at(min_tail_s, now_ms);
        for row in rows {
            out.deals += 1;
            let chip = DealWhy::of_with(row, &admission).chip();
            out.fit += usize::from(matches!(chip, DealChip::Fit | DealChip::AcceptedShortTail));
            out.no_tape += usize::from(chip == DealChip::NoTape);
            out.short_tail += usize::from(chip == DealChip::ShortTail);
            out.holed += usize::from(chip == DealChip::Hole);
            out.missed += usize::from(chip == DealChip::ModelMiss);
            out.unjudged += usize::from(chip == DealChip::Unjudged);
            if DealChip::tape(row) == Some(DealChip::WithTape) {
                out.covered += 1;
            }
            if row.fetchable_with(&admission) {
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
