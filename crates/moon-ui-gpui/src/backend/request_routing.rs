//! Workspace-authorized Main and comparison request routing.

use super::ChartHistoryScope;
use super::OpenCompareRequest;
use crate::Backend;
use gpui::App;
use moon_core::config::WorkspaceMode;
use moon_core::session::CoreId;

impl Backend {
    /// Authorize a delayed core-specific action against the current Auto workspace.
    ///
    /// Args:
    ///     group: Optional owning group. Group panels and charts pass their owner; standalone and
    ///         deliberately global callers pass `None` and preserve their existing authority.
    ///     core: Core captured by the row, menu, dialog, or asynchronous request.
    ///
    /// Returns:
    ///     `false` only when an Auto-owned group no longer exposes `core`; Classic and unscoped
    ///     callers retain their existing behavior.
    pub(crate) fn workspace_action_allows_core(&self, group: Option<&str>, core: CoreId) -> bool {
        let Some(group) = group else {
            return true;
        };
        self.core_belongs_to_group(group, core)
            && self.core_displayed_in_group(group, core)
            && (self.workspace_mode(group) != WorkspaceMode::AutoTrading
                || self
                    .effective_workspace_scope(group, crate::workspace::RetainedCoreScope::All)
                    .contains(core))
    }

    /// Whether the window that armed the cancel hold is the platform's active window.
    ///
    /// The hold's first fail-safe: a hold armed in one window is inert the moment focus moves
    /// anywhere else — another window of ours, the Settings window, or another application.
    pub(crate) fn cancel_hold_window_active(&self, cx: &App) -> bool {
        match (self.cancel_hold.armed_window(), cx.active_window()) {
            (Some(armed), Some(active)) => armed == active,
            _ => false,
        }
    }

    /// Whether the chart under the pointer belongs to the window that armed the cancel hold.
    ///
    /// The SWEEP's gate and only the sweep's (see AM-2): `hovered_chart` is application-global while
    /// a keystroke is not, so without this a hold taken in one window would sweep a chart in
    /// another. A fresh single press is deliberately NOT gated on this — it keeps today's behaviour.
    pub(crate) fn cancel_hold_owns_hovered_chart(&self, _cx: &App) -> bool {
        let Some(armed) = self.cancel_hold.armed_window() else {
            return false;
        };
        let (Some(hovered), Some(last)) =
            (self.hovered_chart.as_ref(), self.last_chart.get(&armed))
        else {
            return false;
        };
        hovered.entity_id() == last.entity_id()
    }

    /// Queue one Main-chart navigation only while its captured core remains workspace-visible.
    ///
    /// Args:
    ///     group: Group that owned the rendered callback, or `None` for a standalone/global host.
    ///     target: Captured core and market to reveal on Main.
    ///     activate: Whether the receiving group window may be raised for this request.
    ///
    /// Returns:
    ///     `true` when the request was authorized and queued; stale Auto callbacks return `false`.
    pub(crate) fn open_on_main_if_authorized(
        &mut self,
        group: Option<&str>,
        target: (CoreId, String),
        activate: bool,
    ) -> bool {
        if !self.workspace_action_allows_core(group, target.0) {
            return false;
        }
        self.queue_open_on_main(
            target,
            ChartHistoryScope::Default,
            group.map(str::to_string),
            activate,
        );
        true
    }

    /// Queue a Report-refined Main-chart navigation while preserving its exact captured core.
    ///
    /// Args:
    ///     group: Group that owned the published Report row, or `None` for standalone Report.
    ///     target: Captured core and catalog-verified market.
    ///     history: Published Report scope and clicked-row identity.
    ///     activate: Whether the receiving group window may be raised.
    ///
    /// Returns:
    ///     `true` when current workspace authority accepted the exact core.
    pub(crate) fn open_report_on_main_if_authorized(
        &mut self,
        group: Option<&str>,
        target: (CoreId, String),
        history: ChartHistoryScope,
        activate: bool,
    ) -> bool {
        if !self.workspace_action_allows_core(group, target.0) {
            return false;
        }
        self.queue_open_on_main(target, history, group.map(str::to_string), activate);
        true
    }

    /// Queue one comparison navigation only while its captured core remains workspace-visible.
    ///
    /// Args:
    ///     group: Group that owned the rendered callback, or `None` for an unscoped host.
    ///     target: Captured core and market used to seed the comparison tab.
    ///
    /// Returns:
    ///     `true` when the current authority accepted and published the request.
    pub(crate) fn open_compare_if_authorized(
        &mut self,
        group: Option<&str>,
        target: (CoreId, String),
    ) -> bool {
        if !self.workspace_action_allows_core(group, target.0) {
            return false;
        }
        self.open_compare_request =
            Some(OpenCompareRequest::new(target, group.map(str::to_string)));
        self.open_compare_request_rev = self.open_compare_request_rev.wrapping_add(1);
        true
    }

    /// Ask for a comparison BETWEEN two charts: the one it was started from, and the target.
    ///
    /// Both cores are checked against the workspace rail, not just the target: the anchor is put on
    /// the same tab, and a comparison that quietly dropped it would answer a different question
    /// from the one asked.
    pub(crate) fn open_compare_pair_if_authorized(
        &mut self,
        group: Option<&str>,
        anchor: (CoreId, String),
        target: (CoreId, String),
    ) -> bool {
        if !self.workspace_action_allows_core(group, target.0)
            || !self.workspace_action_allows_core(group, anchor.0)
        {
            return false;
        }
        self.open_compare_request = Some(OpenCompareRequest::pair(
            anchor,
            target,
            group.map(str::to_string),
        ));
        self.open_compare_request_rev = self.open_compare_request_rev.wrapping_add(1);
        true
    }

    /// Revalidate and drain one comparison request only for its live authorized group.
    ///
    /// Args:
    ///     group: ChartTabs group attempting to consume the request.
    ///
    /// Returns:
    ///     Owned target when live ownership and captured authority still match; stale scoped
    ///     requests are discarded rather than rerouted.
    pub(crate) fn take_open_compare_request_for_group(
        &mut self,
        group: &str,
    ) -> Option<((CoreId, String), Option<(CoreId, String)>)> {
        let request = self.open_compare_request.as_ref()?;
        let (core, _) = &request.target;
        let live_group = self
            .session
            .sessions()
            .iter()
            .find(|session| session.id == *core)
            .map(|session| session.group.as_str());
        let workspace_allowed = request
            .authority_group
            .as_deref()
            .is_none_or(|authority| self.workspace_action_allows_core(Some(authority), *core));
        if !request.allows_group(group, live_group, workspace_allowed) {
            if request.authority_group.is_some() {
                self.open_compare_request = None;
            }
            return None;
        }
        self.open_compare_request
            .take()
            .map(|request| (request.target, request.anchor))
    }

    /// Return the comparison revision only to the request's current authorized group.
    ///
    /// Args:
    ///     group: ChartTabs group assembling its observer signature.
    ///
    /// Returns:
    ///     Current request revision when this group may consume it, otherwise zero.
    pub(crate) fn pending_open_compare_revision_for_group(&self, group: &str) -> u64 {
        let Some(request) = self.open_compare_request.as_ref() else {
            return 0;
        };
        let (core, _) = &request.target;
        let live_group = self
            .session
            .sessions()
            .iter()
            .find(|session| session.id == *core)
            .map(|session| session.group.as_str());
        let workspace_allowed = request
            .authority_group
            .as_deref()
            .is_none_or(|authority| self.workspace_action_allows_core(Some(authority), *core));
        if request.allows_group(group, live_group, workspace_allowed) {
            self.open_compare_request_rev
        } else {
            0
        }
    }
}
