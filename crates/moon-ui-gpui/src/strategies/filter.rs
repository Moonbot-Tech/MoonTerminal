//! Strategy-tree filters. Every condition is combined with logical AND.
//! Kept separate from the remaining window state as pure, UI-independent predicates.
//!
//! [`StrategyFilter`] stores the editable filter state, while [`PreparedFilter`] holds the lowered
//! search query used by per-row predicates. The tree prepares once per frame so query normalization
//! is independent of the number of strategies; row names are lowered only while search is active.

use moon_core::feed::StrategyRow;

/// Editable strategy-filter state retained by the Strategies window.
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
    /// Lower the search text once, so the per-row predicate does not redo it for every row.
    pub fn prepare(&self) -> PreparedFilter {
        let query = self.search.trim();
        PreparedFilter {
            kind: self.kind,
            dir: self.dir,
            only_active: self.only_active,
            query: (!query.is_empty()).then(|| query.to_lowercase()),
        }
    }

    /// Returns row visibility for cold single-row callers.
    ///
    /// The per-frame tree pass prepares the filter once and uses [`PreparedFilter`] directly.
    pub fn matches(&self, row: &StrategyRow) -> bool {
        self.prepare().matches(row)
    }
}

/// A [`StrategyFilter`] with its search text already trimmed and lowered.
pub struct PreparedFilter {
    kind: Option<u8>,
    dir: Option<bool>,
    only_active: bool,
    /// Trimmed and lowercased search text, or `None` when the search is empty.
    query: Option<String>,
}

impl PreparedFilter {
    /// Return whether search is active, which temporarily expands the entire tree.
    pub fn searching(&self) -> bool {
        self.query.is_some()
    }

    /// The lowered search text, reused by callers that filter names of their own (the Deleted
    /// folder lists rows that are not `StrategyRow`s).
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// Apply the kind and direction filters used by active/total counters.
    /// Search text and `only_active` are excluded so core and folder counts reflect kind and side.
    pub fn counts(&self, row: &StrategyRow) -> bool {
        self.kind.is_none_or(|k| row.kind_ordinal == k)
            && self.dir.is_none_or(|s| row.is_short == s)
    }

    /// Return row visibility after applying name, kind, direction, and checked-state filters.
    /// The name is lowered only when a search is active; full-Unicode lowering supports the
    /// Cyrillic strategy names common in this product.
    pub fn matches(&self, row: &StrategyRow) -> bool {
        let by_name = self
            .query
            .as_ref()
            .is_none_or(|q| row.name.to_lowercase().contains(q));
        let by_active = !self.only_active || row.checked;
        self.counts(row) && by_name && by_active
    }
}

#[cfg(test)]
mod tests;
