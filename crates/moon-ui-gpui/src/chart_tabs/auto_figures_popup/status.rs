//! The status mark a popup row carries: what the row's last run on this chart could not do.
//!
//! Read from the outcomes the chart panels last PUBLISHED (`ChartPanel::auto_row_outcomes`), never
//! recomputed here — the popup reports what the chart is actually drawing from. A hidden row and a
//! row of an unknown detector publish nothing and carry no mark of this kind; the unknown one has
//! its own line in the list.

use std::sync::Arc;

use moon_chart::auto_figures::tf::BarSource;
use moon_core::config::auto_figures::AutoFigureRow;

use crate::chartdx::auto_figures_sync::job::{AutoRowOutcome, reads_cache};

/// Why a row draws less than it was asked for. Ordered by severity, mildest first: one row shown
/// on several panes reports the worst of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RowStatus {
    /// The newest bucket of the 5-minute cache is still filling; the answer is usable, not final.
    Lagging,
    /// Fewer closed bars than the detector asks for.
    ShortHistory,
    /// Nothing the terminal holds yields these bars: a timeframe finer than the chart's own.
    Unavailable,
    /// The row reads the local candle cache, and this market has none.
    NoCache,
}

impl RowStatus {
    /// The mark one outcome earns, or `None` for a complete answer.
    ///
    /// A cache read that came back with no bars at all is "no cache" rather than "short history":
    /// the row cannot get better by waiting for more bars of the chart's own series, and the
    /// reader's next step is different.
    pub(crate) fn of(outcome: &AutoRowOutcome) -> Option<Self> {
        if outcome.source == BarSource::Unavailable {
            return Some(RowStatus::Unavailable);
        }
        if reads_cache(outcome.source) && outcome.bars == 0 && !outcome.read_failed {
            return Some(RowStatus::NoCache);
        }
        if outcome.short_history() {
            return Some(RowStatus::ShortHistory);
        }
        outcome.lagging.then_some(RowStatus::Lagging)
    }

    /// Locale key of the words the popup prints for this mark.
    pub(crate) fn locale_key(self) -> &'static str {
        match self {
            RowStatus::Lagging => "auto_figures.status.lagging",
            RowStatus::ShortHistory => "auto_figures.status.short_history",
            RowStatus::Unavailable => "auto_figures.status.unavailable",
            RowStatus::NoCache => "auto_figures.status.no_cache",
        }
    }
}

/// One mark per row of `rows`, the worst across every published outcome.
///
/// An outcome counts for a row only when it is an answer FOR that row: its index is in range and
/// its detector parameters are the row's. Outcomes are published by position, and the popup moves
/// and removes rows — between a reorder and the chart's next publication, an outcome names a
/// position whose row is now a different one, and is ignored rather than shifted onto it.
pub(crate) fn merge(
    outcomes: impl IntoIterator<Item = (usize, Arc<AutoRowOutcome>)>,
    rows: &[AutoFigureRow],
) -> Vec<Option<RowStatus>> {
    let mut out = vec![None; rows.len()];
    for (ix, outcome) in outcomes {
        let (Some(slot), Some(row)) = (out.get_mut(ix), rows.get(ix)) else {
            continue;
        };
        if row.detector.known() != Some(&outcome.key.detector) {
            continue;
        }
        if let Some(status) = RowStatus::of(&outcome) {
            *slot = Some(slot.map_or(status, |s: RowStatus| s.max(status)));
        }
    }
    out
}
