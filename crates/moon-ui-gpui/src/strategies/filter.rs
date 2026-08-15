//! Strategy-tree filters. Every condition is combined with logical AND.
//! Kept separate from the remaining window state as pure, UI-independent predicates.
//!
//! [`StrategyFilter`] stores the editable filter state, while [`PreparedFilter`] holds the lowered
//! search query used by per-row predicates. The tree prepares once per frame so query normalization
//! is independent of the number of strategies; row names are lowered only while search is active.

use moon_core::feed::StrategyRow;

/// Editable strategy-filter state retained by the Strategies window.
#[derive(Default)]
pub struct StrategyFilter {
    /// Case-insensitive substring filter over the strategy name.
    pub search: String,
    /// Strategy-kind ordinal, or `None` for every kind.
    pub kind: Option<u8>,
    /// Direction filter: `None` for both, `Some(true)` for short, and `Some(false)` for long.
    pub dir: Option<bool>,
    /// Whether unchecked live strategies are hidden from the tree.
    pub active_only: bool,
}

impl StrategyFilter {
    /// Lower the search text once, so the per-row predicate does not redo it for every row.
    ///
    /// Returns:
    ///     A prepared predicate carrying every resolved filter dimension.
    pub fn prepare(&self) -> PreparedFilter {
        let query = self.search.trim();
        PreparedFilter {
            kind: self.kind,
            dir: self.dir,
            active_only: self.active_only,
            query: (!query.is_empty()).then(|| query.to_lowercase()),
        }
    }

    /// Returns row visibility for cold single-row callers.
    ///
    /// The per-frame tree pass prepares the filter once and uses [`PreparedFilter`] directly.
    ///
    /// Args:
    ///     row: Live strategy row to evaluate.
    ///
    /// Returns:
    ///     `true` when the row passes search, kind, direction, and active-only visibility.
    pub fn matches(&self, row: &StrategyRow) -> bool {
        self.prepare().matches(row)
    }
}

/// A [`StrategyFilter`] with its search text already trimmed and lowered.
pub struct PreparedFilter {
    kind: Option<u8>,
    dir: Option<bool>,
    /// Whether unchecked live strategies are excluded from row visibility.
    active_only: bool,
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
    /// Search text and active-only visibility are excluded so core and folder counts reflect kind
    /// and side without changing when rows are hidden.
    ///
    /// Args:
    ///     row: Live strategy row to count.
    ///
    /// Returns:
    ///     `true` when the row belongs in the current kind/direction counts.
    pub fn counts(&self, row: &StrategyRow) -> bool {
        self.kind.is_none_or(|k| row.kind_ordinal == k)
            && self.dir.is_none_or(|s| row.is_short == s)
    }

    /// Return row visibility after applying name, kind, direction, and active-only filters.
    /// The name is lowered only when a search is active; full-Unicode lowering supports the
    /// Cyrillic strategy names common in this product.
    ///
    /// Args:
    ///     row: Live strategy row to evaluate.
    ///
    /// Returns:
    ///     `true` when the row should be rendered in the current tree.
    pub fn matches(&self, row: &StrategyRow) -> bool {
        let by_name = self
            .query
            .as_ref()
            .is_none_or(|q| row.name.to_lowercase().contains(q));
        self.counts(row) && by_name && (!self.active_only || row.checked)
    }
}

#[cfg(test)]
mod tests;
