//! Strategy-tree filters. Every condition is combined with logical AND.
//! Kept separate from the remaining window state as pure, UI-independent predicates.
//!
//! [`StrategyFilter`] stores the editable filter state, while [`PreparedFilter`] holds the parsed
//! [`StrategyQuery`] used by per-row predicates — the same parser Analytics and the Report match
//! through in SQL. The tree prepares once per frame so parsing is independent of the number of
//! strategies; row names are folded only while search is active.
//!
//! One dimension is deliberately absent from [`PreparedFilter`]: the exchange. It selects whole
//! CORES rather than rows, and a [`StrategyRow`] carries no venue to test — so `tree::moon::build`
//! evaluates it once per core against the session's venue map, and every row-level predicate here
//! stays a pure function of the row.

use moon_core::feed::StrategyRow;
use moon_core::venue::CoreVenue;

use moon_core::session::core_order::{ExchangeSection, section_of};
use moon_core::strategy_query::StrategyQuery;

/// Editable strategy-filter state retained by the Strategies window.
#[derive(Default)]
pub struct StrategyFilter {
    /// Raw strategy-name query in the shared [`StrategyQuery`] syntax (comma = OR, space = AND,
    /// `!word` = exclude, Unicode caseless).
    pub search: String,
    /// Strategy-kind ordinal, or `None` for every kind.
    pub kind: Option<u8>,
    /// Direction filter: `None` for both, `Some(true)` for short, and `Some(false)` for long.
    pub dir: Option<bool>,
    /// Exchange section whose cores are shown, or `None` for every exchange.
    ///
    /// Held as an owned identity rather than a borrowed venue: current sections derive from the
    /// live venue map, so a selection that outlives one core's connection must borrow nothing.
    /// Evaluated by `tree::moon::build`, never by [`PreparedFilter`].
    pub exchange: Option<ExchangeSection>,
    /// Whether unchecked live strategies are hidden from the tree.
    pub active_only: bool,
}

impl StrategyFilter {
    /// Return whether one core's exchange passes the filter.
    ///
    /// The exchange dimension's ONE predicate, so the tree's two build branches and the reveal path
    /// cannot each spell it differently — three call sites comparing an `Option<ExchangeSection>` by
    /// hand is three places for the meaning of "this core is filtered out" to drift.
    ///
    /// Args:
    ///     venue: What the core reported it is connected to, or `None` before that arrives.
    ///
    /// Returns:
    ///     `true` when no exchange is selected, or when this core belongs to the selected section.
    pub fn core_matches(&self, venue: Option<&CoreVenue>) -> bool {
        self.exchange
            .is_none_or(|selected| selected == section_of(venue))
    }

    /// Parse the search text once, so the per-row predicate does not redo it for every row.
    ///
    /// Returns:
    ///     A prepared predicate carrying every ROW-level filter dimension. The exchange is not one
    ///     of them — see the module docs.
    pub fn prepare(&self) -> PreparedFilter {
        let query = StrategyQuery::parse(&self.search);
        PreparedFilter {
            kind: self.kind,
            dir: self.dir,
            active_only: self.active_only,
            query: (!query.is_empty()).then_some(query),
        }
    }

    /// Return whether this filter narrows the tree at all, across every dimension.
    ///
    /// Derived from the real predicate rather than hand-written: [`Self::prepare`] PARSES the
    /// search query before deciding whether it is active, so text carrying no word (whitespace,
    /// lone commas or `!`) must not be blamed for an empty tree the way a naive
    /// `!self.search.is_empty()` check would.
    ///
    /// Returns:
    ///     `true` when search, kind, direction, active-only, or exchange excludes anything.
    pub(super) fn narrows(&self) -> bool {
        self.prepare().narrows() || self.exchange.is_some()
    }

    /// Returns row visibility for cold single-row callers.
    ///
    /// The per-frame tree pass prepares the filter once and uses [`PreparedFilter`] directly.
    ///
    /// Args:
    ///     row: Live strategy row to evaluate.
    ///
    /// Returns:
    ///     `true` when the row passes search, kind, direction, and active-only visibility. A row on
    ///     a filtered-out EXCHANGE still passes here: its core is what the exchange filter removes,
    ///     and callers that must account for that use [`StrategyFilter::core_matches`].
    pub fn matches(&self, row: &StrategyRow) -> bool {
        self.prepare().matches(row)
    }
}

/// A [`StrategyFilter`] with its search text already parsed.
pub struct PreparedFilter {
    kind: Option<u8>,
    dir: Option<bool>,
    /// Whether unchecked live strategies are excluded from row visibility.
    active_only: bool,
    /// Parsed search query, or `None` when it selects everything.
    query: Option<StrategyQuery>,
}

impl PreparedFilter {
    /// Return whether row filters require a strategy match to retain an otherwise empty core.
    /// Exchange filtering is applied separately to core identities and must not hide empty cores.
    pub(super) fn narrows(&self) -> bool {
        self.query.is_some() || self.kind.is_some() || self.dir.is_some() || self.active_only
    }

    /// Return whether search is active, which temporarily expands the entire tree.
    ///
    /// An exclusion-only query (`!test`) counts as searching by design: it still hides rows, so
    /// the tree force-expands and reordering stays disabled exactly as for a positive query.
    pub fn searching(&self) -> bool {
        self.query.is_some()
    }

    /// Whether a strategy name passes the search query alone, for callers that filter names of
    /// their own (the Deleted folder lists rows that are not `StrategyRow`s).
    ///
    /// Args:
    ///     name: Strategy name as stored.
    ///
    /// Returns:
    ///     `true` when no search is active or the name matches it.
    pub fn name_matches(&self, name: &str) -> bool {
        self.query.as_ref().is_none_or(|q| q.matches(name))
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
    /// The name is folded only when a search is active; full Unicode case folding supports the
    /// Cyrillic strategy names common in this product.
    ///
    /// Args:
    ///     row: Live strategy row to evaluate.
    ///
    /// Returns:
    ///     `true` when the row should be rendered in the current tree.
    pub fn matches(&self, row: &StrategyRow) -> bool {
        self.counts(row) && self.name_matches(&row.name) && (!self.active_only || row.checked)
    }
}

#[cfg(test)]
mod tests;
