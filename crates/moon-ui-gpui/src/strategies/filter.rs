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

use std::collections::HashSet;

use moon_core::feed::{StrategyRow, strategy_path};
use moon_core::session::store::CoreData;
use moon_core::venue::CoreVenue;

use moon_core::session::core_order::{ExchangeSection, section_of};
use moon_core::strategy_query::StrategyQuery;

/// One of the overview's quick filters, toggled from its chips and removed from the chip under the
/// tree's toolbar.
///
/// Process-only, like the kind and side filters: a quick filter is a question asked now ("which
/// cores are stopped?"), not a view the window should reopen on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuickFilter {
    /// Only cores whose strategy engine reported stopped. Selects CORES, so like the exchange it
    /// is evaluated by `tree::moon::build` and never by a row predicate.
    Stopped,
    /// Only strategies holding at least one open order.
    WithOrders,
    /// Only strategies the core holds unchecked.
    Disabled,
    /// Only strategies with an edit the core has not confirmed yet (pending or timed out).
    Unconfirmed,
}

impl QuickFilter {
    /// Every quick filter, in the order the overview offers them.
    pub const ALL: [Self; 4] = [
        Self::Stopped,
        Self::WithOrders,
        Self::Disabled,
        Self::Unconfirmed,
    ];

    /// The chip's localized caption.
    pub fn label(self) -> String {
        match self {
            Self::Stopped => rust_i18n::t!("strat.quick_stopped"),
            Self::WithOrders => rust_i18n::t!("strat.quick_with_orders"),
            Self::Disabled => rust_i18n::t!("strat.quick_disabled"),
            Self::Unconfirmed => rust_i18n::t!("strat.quick_unconfirmed"),
        }
        .to_string()
    }

    /// Stable element-id suffix, independent of the caption's language.
    pub fn key(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::WithOrders => "orders",
            Self::Disabled => "disabled",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}

/// Editable strategy-filter state retained by the Strategies window.
#[derive(Default)]
pub struct StrategyFilter {
    /// Raw strategy-or-folder-name query in the shared [`StrategyQuery`] syntax (comma = OR, space = AND,
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
    /// Quick filter picked from the overview, or `None`.
    pub quick: Option<QuickFilter>,
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
            quick: self.quick,
            core_ids: None,
        }
    }

    /// Prepare the filter for one core, so the quick filters that need that core's orders or
    /// edits can answer.
    ///
    /// Args:
    ///     cd: The core whose rows the prepared filter will be asked about.
    ///
    /// Returns:
    ///     A prepared predicate scoped to that core — see [`PreparedFilter::scope_to_core`].
    pub fn prepare_for_core(&self, cd: &CoreData) -> PreparedFilter {
        let mut prepared = self.prepare();
        prepared.scope_to_core(cd);
        prepared
    }

    /// Return whether this filter narrows the tree at all, across every dimension.
    ///
    /// Derived from the real predicate rather than hand-written: [`Self::prepare`] PARSES the
    /// search query before deciding whether it is active, so text carrying no word (whitespace,
    /// lone commas or `!`) must not be blamed for an empty tree the way a naive
    /// `!self.search.is_empty()` check would.
    ///
    /// Returns:
    ///     `true` when search, kind, direction, active-only, a quick filter, or exchange excludes
    ///     anything.
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
    /// Quick filter in force, or `None`.
    quick: Option<QuickFilter>,
    /// Strategy ids of the core this filter was scoped to that pass a per-core quick filter
    /// ([`QuickFilter::WithOrders`], [`QuickFilter::Unconfirmed`]); `None` when unscoped.
    core_ids: Option<HashSet<u64>>,
}

impl PreparedFilter {
    /// Return whether row filters require a strategy match to retain an otherwise empty core.
    /// Exchange filtering is applied separately to core identities and must not hide empty cores.
    pub(super) fn narrows(&self) -> bool {
        self.query.is_some()
            || self.kind.is_some()
            || self.dir.is_some()
            || self.active_only
            || self.quick.is_some()
    }

    /// Whether row filters can leave a core with no matching row, which drops an empty core.
    ///
    /// [`Self::narrows`] minus the core-level [`QuickFilter::Stopped`]: that filter keeps or drops a
    /// WHOLE core by its engine state and never narrows rows, so a stopped core holding no
    /// strategy must still be shown under it.
    ///
    /// Returns:
    ///     `true` when some row-level filter is in force.
    pub(super) fn narrows_rows(&self) -> bool {
        self.query.is_some()
            || self.kind.is_some()
            || self.dir.is_some()
            || self.active_only
            || matches!(
                self.quick,
                Some(QuickFilter::WithOrders | QuickFilter::Unconfirmed | QuickFilter::Disabled)
            )
    }

    /// Whether the quick filter in force keeps a core with this engine state.
    ///
    /// Args:
    ///     engine: The core's confirmed engine state, `Some(false)` when it reported stopped.
    ///
    /// Returns:
    ///     `false` only under [`QuickFilter::Stopped`] for a core that did not report stopped.
    pub fn core_matches_quick(&self, engine: Option<bool>) -> bool {
        self.quick != Some(QuickFilter::Stopped) || engine == Some(false)
    }

    /// Scope the per-core quick filters to one core's open orders or open edits.
    ///
    /// A [`StrategyRow`] carries neither, so the tree build calls this once per core before asking
    /// about that core's rows. An UNSCOPED filter lets those two quick filters pass every row:
    /// callers that never scope it (the selection prune) narrow only, and failing open there keeps
    /// a selection the operator can still see rather than dropping it on a guess.
    ///
    /// Args:
    ///     cd: The core whose rows follow.
    pub fn scope_to_core(&mut self, cd: &CoreData) {
        self.core_ids = match self.quick {
            Some(QuickFilter::WithOrders) => Some(
                cd.orders
                    .iter()
                    .filter(|order| !order.job_is_done)
                    .map(|order| order.strat_id)
                    .collect(),
            ),
            Some(QuickFilter::Unconfirmed) => {
                Some(cd.strategy_edits.iter().map(|edit| edit.id).collect())
            }
            _ => None,
        };
    }

    /// Return whether one row passes the quick filter in force.
    ///
    /// Args:
    ///     row: Live strategy row to evaluate.
    ///
    /// Returns:
    ///     `true` with no quick filter, for the core-level [`QuickFilter::Stopped`], and for a
    ///     per-core quick filter on an unscoped predicate.
    fn quick_matches(&self, row: &StrategyRow) -> bool {
        match self.quick {
            None | Some(QuickFilter::Stopped) => true,
            Some(QuickFilter::Disabled) => !row.checked,
            Some(QuickFilter::WithOrders | QuickFilter::Unconfirmed) => self
                .core_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&row.id)),
        }
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

    /// Whether a positive query matches any folder in this path, retaining its whole subtree.
    /// Each folder name is evaluated independently through the shared query grammar. Exclusion-only
    /// searches never expand a subtree, so excluded strategy names keep their existing semantics.
    pub fn folder_path_matches(&self, path: &str) -> bool {
        self.query.as_ref().is_some_and(|query| {
            query.has_positive()
                && strategy_path::path_segments(path).any(|name| query.matches(name))
        })
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

    /// Return row visibility after applying strategy-or-folder name, kind, direction, active-only and
    /// quick filters.
    /// The name is folded only when a search is active; full Unicode case folding supports the
    /// Cyrillic strategy names common in this product.
    ///
    /// Args:
    ///     row: Live strategy row to evaluate.
    ///
    /// Returns:
    ///     `true` when the row should be rendered in the current tree.
    pub fn matches(&self, row: &StrategyRow) -> bool {
        self.counts(row)
            && (self.name_matches(&row.name) || self.folder_path_matches(&row.folder_path))
            && (!self.active_only || row.checked)
            && self.quick_matches(row)
    }
}

#[cfg(test)]
mod tests;
