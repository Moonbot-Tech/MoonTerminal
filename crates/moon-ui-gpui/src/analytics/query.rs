//! Analytics query extracted from the window module.

use super::*;

impl AnalyticsView {
    /// Current filters in the structure shared by Summary and Tuning, using the active tab's
    /// period (`active_period`).
    pub(super) fn query(&self) -> Query {
        self.query_for(self.active_period())
    }

    /// Current filters over one EXPLICIT time window.
    ///
    /// Split out from [`Self::query`] because the strategy base records
    /// `strategy_data_period = self.strat_period`, so its read must use that same window rather
    /// than whichever tab's period happens to be active.
    ///
    /// Args:
    ///     period: Window the caller is building a result for.
    ///
    /// Returns:
    ///     Shared filters over that window.
    pub(super) fn query_for(&self, period: Period) -> Query {
        let (from, to) = period.range(self.bound_zone());
        Query {
            axis: self.report_axis(),
            previous_period_basis: if matches!(period, Period::Custom(..)) {
                PreviousPeriodBasis::Elapsed
            } else {
                PreviousPeriodBasis::Civil
            },
            from,
            to,
            cores: self.cores_selected(),
            side: self.side,
            emulator: self.emu,
            strategies: Vec::new(),
            strategy_name_mask: self.strategy_mask.clone(),
            metric: self.metric,
            valuation: self.valuation_mode,
            prefer_usdt: self.prefer_usdt,
            core_names: self.core_names.clone(),
        }
    }

    /// Return the Auto scope only while it also PINS which cores this window reads.
    ///
    /// Deliberately NARROWER than [`Self::workspace_scope`], which stays present for the whole of
    /// Auto: unpinning the filter must never unpin the ACTION authority that confines Save, Copy
    /// and purge to the focused group. The narrowing rule itself lives on
    /// [`AnalyticsWorkspaceScope::pins_core_filter`], beside the struct it constrains.
    ///
    /// Returns:
    ///     The scope while the rail is on a concrete core, or `None` on Overview and in Classic.
    pub(super) fn core_filter_pin(&self) -> Option<&AnalyticsWorkspaceScope> {
        self.workspace_scope
            .as_ref()
            .filter(|scope| scope.pins_core_filter())
    }

    /// Whether the core selector is pinned and its edit paths must refuse.
    pub(super) fn core_filter_pinned(&self) -> bool {
        self.core_filter_pin().is_some()
    }

    /// Cores this window may READ, or `None` while the filter is the user's to set.
    ///
    /// Pairs with [`Self::action_core_ids`]; naming the two authorities is what keeps a call site
    /// from picking the wrong one by copying a neighbouring borrow chain. Anything that DESCRIBES
    /// the selection takes this one, because the tuner is only ever meaningful scoped to concrete
    /// strategies: an empty strategy list is not "no scope", it is EVERY strategy the core filter
    /// admits, which measures the mixture instead of the tuning. So a row the user can see
    /// highlighted must reach the query even when the Auto group may not write to its core —
    /// otherwise selecting it silently widens the analysis while the page still shows one row
    /// selected. Writing to it is refused separately, and visibly, through `action_core_ids`.
    pub(in crate::analytics) fn read_core_ids(&self) -> Option<&[u64]> {
        self.core_filter_pin()
            .map(|scope| scope.core_ids.as_slice())
    }

    /// Cores this window may WRITE to, or `None` in Classic, where nothing confines it.
    pub(in crate::analytics) fn action_core_ids(&self) -> Option<&[u64]> {
        self.workspace_scope
            .as_ref()
            .map(|scope| scope.core_ids.as_slice())
    }

    /// Configured cores the viewing preset hides from scoped Analytics data reads, or `None` when
    /// it hides none.
    pub(in crate::analytics) fn hidden_core_ids(&self) -> Option<&[u64]> {
        self.display_scope
            .as_ref()
            .map(|s| s.hidden_core_ids.as_slice())
    }

    /// Configured cores the viewing preset counted, used only to expand an implicit "All" before
    /// this window's replica core list has arrived.
    ///
    /// Empty whenever no preset narrows this window, which is also exactly when
    /// [`Self::hidden_core_ids`] is `None` and the expansion never happens.
    pub(super) fn configured_core_ids(&self) -> &[u64] {
        self.display_scope
            .as_ref()
            .map_or(&[], |s| s.configured_core_ids.as_slice())
    }

    /// The scope marker the Summary states, from whichever authority narrowed the read.
    /// The two are mutually exclusive by construction (`analytics_display_scope`).
    pub(super) fn summary_scope_marker(
        &self,
    ) -> Option<crate::workspace::scope_marker::ScopeMarker> {
        self.workspace_scope
            .as_ref()
            .map(AnalyticsWorkspaceScope::scope_marker)
            .or_else(|| {
                self.display_scope
                    .as_ref()
                    .map(AnalyticsDisplayScope::scope_marker)
            })
    }

    /// Return the effective core filter without overwriting the retained Classic selection.
    ///
    /// Returns:
    ///     Pinned Auto workspace ids, the no-match sentinel for an empty pinned scope or a Classic
    ///     membership narrowing that hides everything selected, retained user-selected ids narrowed
    ///     by Classic membership when it hides at least one core, or an empty vector for an
    ///     unfiltered query. A window whose replica core list has not arrived yet expands its
    ///     implicit All against the CONFIG instead, so its very first query already states what
    ///     the scope marker beside it claims.
    pub(super) fn cores_selected(&self) -> Vec<u64> {
        toolbar::analytics_core_filter_ids(
            &self.sel_cores,
            self.read_core_ids(),
            self.hidden_core_ids(),
            &self.cores,
            self.configured_core_ids(),
        )
    }
}
