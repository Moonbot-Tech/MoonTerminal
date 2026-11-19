//! The status mark a popup row carries: what the row's last run on this chart could not do.
//!
//! Read from the outcomes the chart panels last PUBLISHED (`ChartPanel::auto_row_outcomes`), never
//! recomputed here — the popup reports what the chart is actually drawing from. A hidden row and a
//! row of an unknown detector publish nothing and carry no mark of this kind; the unknown one has
//! its own line in the list.

use std::sync::Arc;

use moon_chart::auto_figures::tf::BarSource;

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

/// One mark per row of a `rows`-long configuration, the worst across every published outcome.
///
/// An outcome naming a row past the end — published before a removal reached the panel — is
/// ignored rather than shifted onto another row.
pub(crate) fn merge(
    outcomes: impl IntoIterator<Item = (usize, Arc<AutoRowOutcome>)>,
    rows: usize,
) -> Vec<Option<RowStatus>> {
    let mut out = vec![None; rows];
    for (ix, outcome) in outcomes {
        let Some(slot) = out.get_mut(ix) else {
            continue;
        };
        if let Some(status) = RowStatus::of(&outcome) {
            *slot = Some(slot.map_or(status, |s: RowStatus| s.max(status)));
        }
    }
    out
}
