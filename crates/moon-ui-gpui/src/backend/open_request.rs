//! Atomic chart-open requests and their exact durable-history scope.

use moon_core::session::CoreId;

/// Atomic identity of the most recent request to open a market on a group's Main chart.
///
/// The target, routing authority, owning group, activation bit, and revision move together.
/// Draining clears `pending` and `activate`; no parallel field can retain stale routing data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OpenMainRequest {
    target: Option<(CoreId, String)>,
    history: ChartHistoryScope,
    group: Option<String>,
    authority_group: Option<String>,
    revision: u64,
    activate: bool,
    pending: bool,
}

/// Durable history policy applied to one exact Main-chart target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ChartHistoryScope {
    /// Load all durable closed trades for the exact target core and market aliases.
    #[default]
    Default,
    /// Refine durable history with the published Report query that produced the clicked row.
    Report {
        /// Published filter snapshot; core, coin, and closed-only predicates are enforced again by
        /// the durable chart query boundary.
        filter: moon_core::db::ReportFilter,
        /// Exact historical coin identity stored on the clicked Report row.
        exact_coin: String,
        /// Stable row identity used to choose the initial time focus when present.
        focus_record_id: Option<i64>,
    },
}

impl OpenMainRequest {
    /// Replace the current request with one internally consistent identity.
    ///
    /// Args:
    ///     target: Live core and canonical market to open.
    ///     history: Default or published Report durable-history scope for that same target.
    ///     group: Owning group resolved from that same live core.
    ///     authority_group: Immutable group that owned the producer, or `None` for an unscoped
    ///         global/internal request that may follow a core moved by Settings.
    ///     activate: Whether the consumer should raise the group window.
    ///
    /// Returns:
    ///     Nothing; the wrapping revision advances and the request becomes pending.
    pub(super) fn request(
        &mut self,
        target: (CoreId, String),
        history: ChartHistoryScope,
        group: String,
        authority_group: Option<String>,
        activate: bool,
    ) {
        self.target = Some(target);
        self.history = history;
        self.group = Some(group);
        self.authority_group = authority_group;
        self.revision = self.revision.wrapping_add(1);
        self.activate = activate;
        self.pending = true;
    }

    /// Return whether an undrained Main-open request exists.
    ///
    /// Returns:
    ///     `true` between the producer API call and the owning `ChartTabs` drain.
    pub(crate) fn is_pending(&self) -> bool {
        self.pending
    }

    /// Return the core carried by the current pending request.
    ///
    /// Returns:
    ///     Pending core identity, or `None` after consumption/cancellation.
    pub(super) fn pending_core(&self) -> Option<CoreId> {
        self.pending
            .then_some(())
            .and(self.target.as_ref().map(|(core, _)| *core))
    }

    /// Return the target carried by the current pending request without trusting stored routing.
    ///
    /// Returns:
    ///     Borrowed target while the request is pending.
    pub(super) fn pending_target(&self) -> Option<&(CoreId, String)> {
        self.pending.then_some(()).and(self.target.as_ref())
    }

    /// Return the immutable group authority captured by a group-owned producer.
    ///
    /// Returns:
    ///     Captured group, or `None` for an explicitly unscoped request.
    pub(super) fn authority_group(&self) -> Option<&str> {
        self.authority_group.as_deref()
    }

    /// Retarget or cancel a pending request after session/config reconciliation.
    ///
    /// Args:
    ///     current_group: Current live owner resolved from the target core, or `None` when the core
    ///         no longer has a session.
    ///
    /// Returns:
    ///     `true` when routing/reveal metadata changed and observers need a new revision.
    pub(super) fn reconcile_group(&mut self, current_group: Option<String>) -> bool {
        if !self.pending {
            return false;
        }
        let current_group = match self.authority_group.as_deref() {
            Some(authority) if current_group.as_deref() != Some(authority) => None,
            _ => current_group,
        };
        if current_group.is_some() && self.group == current_group {
            return false;
        }
        self.revision = self.revision.wrapping_add(1);
        if let Some(group) = current_group {
            self.group = Some(group);
        } else {
            self.target = None;
            self.group = None;
            self.activate = false;
            self.pending = false;
        }
        true
    }

    /// Return the pending target only to its atomically recorded owning group.
    ///
    /// Args:
    ///     group: Group whose `ChartTabs` is asking for work.
    ///
    /// Returns:
    ///     Borrowed target while this exact group owns a pending request.
    #[cfg(test)]
    pub(crate) fn pending_for_group(&self, group: &str) -> Option<&(CoreId, String)> {
        (self.pending && self.group.as_deref() == Some(group))
            .then_some(())
            .and(self.target.as_ref())
    }

    /// Return the request revision relevant to one group's chart-tab signature.
    ///
    /// Args:
    ///     group: Group whose signature is being assembled.
    ///
    /// Returns:
    ///     Current revision for a pending request owned by `group`, otherwise zero.
    #[cfg(test)]
    pub(crate) fn pending_revision_for_group(&self, group: &str) -> u64 {
        if self.pending && self.group.as_deref() == Some(group) {
            self.revision
        } else {
            0
        }
    }

    /// Return the group addressed by the latest revision for routing regressions.
    ///
    /// Returns:
    ///     Borrowed owning group, or `None` before the first valid request and after reconciliation
    ///     cancels a request whose core no longer has an owner.
    #[cfg(test)]
    pub(crate) fn addressed_group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    /// Return the latest request revision for pending ChartTabs signatures and regressions.
    ///
    /// Returns:
    ///     Wrapping revision, zero before the first request.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// Drain a still-matching request from its owning group.
    ///
    /// Args:
    ///     group: Group whose `ChartTabs` is consuming the request.
    ///     expected: Target copied during the preceding read phase.
    ///
    /// Returns:
    ///     Owned core, market, history scope, and activation bit, or `None` if another producer
    ///     replaced it.
    pub(crate) fn take_if_matches(
        &mut self,
        group: &str,
        expected: &(CoreId, String),
    ) -> Option<(CoreId, String, ChartHistoryScope, bool)> {
        if !self.pending
            || self.group.as_deref() != Some(group)
            || self.target.as_ref() != Some(expected)
        {
            return None;
        }
        self.pending = false;
        let activate = std::mem::take(&mut self.activate);
        self.target
            .as_ref()
            .map(|(core, market)| (*core, market.clone(), self.history.clone(), activate))
    }
}

/// One comparison-tab navigation plus the immutable group authority of its producer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OpenCompareRequest {
    pub(super) target: (CoreId, String),
    /// The chart the comparison was started FROM, when the producer had one.
    ///
    /// A comparison of one chart is not a comparison: an arbitrage click means "this coin here
    /// versus this coin there", and without the anchor the tab would open holding only the
    /// destination. Absent for producers that have no chart of their own — a detect card names a
    /// coin, not a comparison.
    pub(super) anchor: Option<(CoreId, String)>,
    pub(super) authority_group: Option<String>,
}

impl OpenCompareRequest {
    /// Capture one comparison target and its optional group workspace authority.
    ///
    /// Args:
    ///     target: Live core and canonical market selected by the producer.
    ///     authority_group: Immutable owning group, or `None` for an explicitly unscoped route.
    ///
    /// Returns:
    ///     One atomic request that cannot be retargeted across a scoped workspace boundary.
    pub(super) fn new(target: (CoreId, String), authority_group: Option<String>) -> Self {
        Self {
            target,
            anchor: None,
            authority_group,
        }
    }

    /// The chart this comparison started from, for the test that pins it.
    #[cfg(test)]
    pub(super) fn anchor_for_test(&self) -> Option<&(CoreId, String)> {
        self.anchor.as_ref()
    }

    /// The same request, stated as a comparison BETWEEN two charts.
    pub(super) fn pair(
        anchor: (CoreId, String),
        target: (CoreId, String),
        authority_group: Option<String>,
    ) -> Self {
        Self {
            target,
            anchor: Some(anchor),
            authority_group,
        }
    }

    /// Decide whether one group may consume this request after live ownership revalidation.
    ///
    /// Args:
    ///     group: Group whose `ChartTabs` is attempting to consume the request.
    ///     live_group: Current group resolved from the target core.
    ///     workspace_allowed: Whether the target remains in the current Auto scope.
    ///
    /// Returns:
    ///     `true` only when live ownership matches and any captured authority remains valid.
    pub(super) fn allows_group(
        &self,
        group: &str,
        live_group: Option<&str>,
        workspace_allowed: bool,
    ) -> bool {
        live_group == Some(group)
            && self
                .authority_group
                .as_deref()
                .is_none_or(|authority| authority == group && workspace_allowed)
    }
}
