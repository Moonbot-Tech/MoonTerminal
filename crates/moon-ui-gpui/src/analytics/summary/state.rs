//! Display-only choices of the Summary tab: which core lines the chart draws, which extreme
//! trades the side card lists, and how the cores ranking is filtered and folded.
//!
//! None of these change the query, so none of them reloads anything; they only pick what the
//! already-loaded snapshot shows.

use std::collections::BTreeSet;

/// Which per-core lines the profit chart draws over the total curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics) enum CoreLines {
    /// Only the total curve.
    None,
    /// The five cores with the largest period totals.
    #[default]
    Top5,
    /// The cores the user picked in the ranking table.
    Selected,
}

/// Which five extreme trades the trades card lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics) enum TradeSide {
    /// The five most profitable trades.
    #[default]
    Best,
    /// The five least profitable trades.
    Worst,
}

/// Which cores the ranking table lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics) enum CoreFilter {
    /// Every core, folded in the middle when the list is long.
    #[default]
    All,
    /// Cores whose period total is above zero.
    Pos,
    /// Cores whose period total is below zero.
    Neg,
}

/// View state of the Summary tab, owned by the Analytics view for the window's lifetime.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics) struct SummaryUi {
    /// Which per-core lines the chart draws.
    pub(in crate::analytics) lines: CoreLines,
    /// Core uids picked in the ranking; drawn when `lines` is `Selected`.
    pub(in crate::analytics) picked: BTreeSet<u64>,
    /// Which extreme trades the trades card lists.
    pub(in crate::analytics) trades: TradeSide,
    /// Which cores the ranking lists.
    pub(in crate::analytics) core_filter: CoreFilter,
    /// Whether the ranking lists every core instead of folding its middle.
    pub(in crate::analytics) unfolded: bool,
}

impl SummaryUi {
    /// Add or remove one core from the chart's picked set.
    ///
    /// Adding a core switches the chart to `Selected`, so a click in the table always shows its
    /// line. Removing the last pick keeps `Selected`: the chart then draws only the total, which
    /// is what the user just asked for.
    ///
    /// Args:
    ///     uid: Core clicked in the ranking table.
    pub(in crate::analytics) fn toggle_pick(&mut self, uid: u64) {
        if !self.picked.remove(&uid) {
            self.picked.insert(uid);
            self.lines = CoreLines::Selected;
        }
    }

    /// Drop picks naming cores a freshly applied snapshot no longer has, so a pick cannot
    /// linger invisibly and come back on a later period.
    ///
    /// Args:
    ///     present: Core uids of the new snapshot.
    pub(in crate::analytics) fn retain_picks(&mut self, present: &BTreeSet<u64>) {
        self.picked.retain(|uid| present.contains(uid));
    }
}

#[cfg(test)]
mod tests;
