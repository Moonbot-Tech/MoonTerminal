//! Strategy-tree filters. Every condition is combined with logical AND.
//! Kept separate from the remaining window state as pure, UI-independent predicates.
//! Ported directly from egui's `src/strategies/filter.rs`.

use moon_core::feed::StrategyRow;

pub struct StrategyFilter {
    /// Case-insensitive substring filter over the strategy name.
    pub search: String,
    /// Strategy-kind ordinal, or `None` for every kind.
    pub kind: Option<u8>,
    /// Direction filter: `None` for both, `Some(true)` for short, and `Some(false)` for long.
    pub dir: Option<bool>,
    /// Show only rows whose `checked` checkbox state is true; enabled by default.
    pub only_active: bool,
}

impl Default for StrategyFilter {
    fn default() -> Self {
        Self {
            search: String::new(),
            kind: None,
            dir: None,
            only_active: true,
        }
    }
}

impl StrategyFilter {
    /// Return whether search is active, which temporarily expands the entire tree.
    pub fn searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    /// Apply the kind and direction filters used by active/total counters.
    /// Search text and `only_active` are excluded so core and folder counts reflect kind and side.
    pub fn counts(&self, row: &StrategyRow) -> bool {
        self.kind.is_none_or(|k| row.kind_ordinal == k)
            && self.dir.is_none_or(|s| row.is_short == s)
    }

    /// Return row visibility after applying name, kind, direction, and checked-state filters.
    pub fn matches(&self, row: &StrategyRow) -> bool {
        let q = self.search.trim().to_lowercase();
        let by_name = q.is_empty() || row.name.to_lowercase().contains(&q);
        let by_active = !self.only_active || row.checked;
        self.counts(row) && by_name && by_active
    }
}
