//! Group-owned Main chart targets and surface requests.

use super::ChartHistoryScope;
use crate::Backend;
use gpui::Context;
use moon_core::config::WorkspaceMode;
use moon_core::market::MarketLimits;
use moon_core::session::CoreId;

impl Backend {
    /// Return whether a live core currently belongs to a Main window group.
    ///
    /// Args:
    ///     group: Window group that must own the core.
    ///     core: Stable core UID to validate.
    ///
    /// Returns:
    ///     `true` only while the matching live session remains in `group`.
    pub(crate) fn core_belongs_to_group(&self, group: &str, core: CoreId) -> bool {
        self.session
            .sessions()
            .iter()
            .any(|session| session.id == core && session.group == group)
    }

    /// Resolve the pending Main request's current owner from live session state.
    ///
    /// Returns:
    ///     Current group of the pending target core, or `None` after removal/consumption.
    fn current_open_main_group(&self) -> Option<&str> {
        let core = self.open_main_request.pending_core()?;
        self.session
            .sessions()
            .iter()
            .find(|session| session.id == core)
            .map(|session| session.group.as_str())
    }

    /// Reconcile pending Main routing after session topology changes.
    ///
    /// Returns:
    ///     `true` when a moved core retargeted the request or a removed core cancelled it.
    pub(crate) fn reconcile_open_main_request_group(&mut self) -> bool {
        let current_group = self.current_open_main_group().map(str::to_string);
        let authority_valid = match (
            self.open_main_request.authority_group(),
            self.open_main_request.pending_core(),
        ) {
            (Some(authority_group), Some(core)) => {
                current_group.as_deref() == Some(authority_group)
                    && self.workspace_action_allows_core(Some(authority_group), core)
            }
            (None, Some(_)) => true,
            (_, None) => false,
        };
        let current_group = authority_valid.then_some(current_group).flatten();
        self.open_main_request.reconcile_group(current_group)
    }

    /// Return a pending Main target only to its current live group.
    ///
    /// Args:
    ///     group: ChartTabs group requesting a read-phase target copy.
    ///
    /// Returns:
    ///     Borrowed target only when the target core currently belongs to `group`.
    pub(crate) fn pending_open_main_request_for_group(
        &self,
        group: &str,
    ) -> Option<&(CoreId, String)> {
        (self.current_open_main_group() == Some(group))
            .then_some(())
            .and(self.open_main_request.pending_target())
    }

    /// Return the pending Main revision only to its current live group.
    ///
    /// Args:
    ///     group: ChartTabs group assembling its observer signature.
    ///
    /// Returns:
    ///     Request revision when the pending core currently belongs to `group`, otherwise zero.
    pub(crate) fn pending_open_main_revision_for_group(&self, group: &str) -> u64 {
        if self.current_open_main_group() == Some(group) {
            self.open_main_request.revision()
        } else {
            0
        }
    }

    /// Revalidate and drain a matching Main request from its current live group.
    ///
    /// Args:
    ///     group: ChartTabs group attempting consumption.
    ///     expected: Target copied during the preceding read phase.
    ///     cx: Backend context used to wake the request's newly resolved owner.
    ///
    /// Returns:
    ///     Owned target, history scope, and activation bit only when current session ownership
    ///     still matches.
    pub(crate) fn take_open_main_request_if_matches(
        &mut self,
        group: &str,
        expected: &(CoreId, String),
        cx: &mut Context<Self>,
    ) -> Option<(CoreId, String, ChartHistoryScope, bool)> {
        if self.reconcile_open_main_request_group() {
            cx.notify();
        }
        self.open_main_request.take_if_matches(group, expected)
    }

    /// Return whether this panel is currently recorded as detached from `group`.
    ///
    /// The question every detach route asks before opening a window: a panel already pulled out
    /// must not be detached twice, or the second window takes over `detached_panel_windows` and
    /// leaves the first unable to repin.
    ///
    /// Args:
    ///     group: Window group the panel would be detached from.
    ///     panel: Stable panel name shared by `DetachedSpec` and the dock.
    ///
    /// Returns:
    ///     `true` while a `DetachedSpec` for this pair exists.
    pub(crate) fn is_detached(&self, group: &str, panel: &str) -> bool {
        self.detached
            .iter()
            .any(|spec| spec.group == group && spec.panel == panel)
    }

    /// Seed the group's runtime Main target without replacing a durable manual selection.
    ///
    /// Construction publishes restored Main state once after startup. Treating that baseline as a
    /// user-visible target change would immediately overwrite the core restored from layout.toml.
    ///
    /// Args:
    ///     group: Window group whose initial target is being published.
    ///     target: Restored active core and market, or `None`. Invalid cross-group targets are
    ///         treated as absent.
    ///
    /// Returns:
    ///     Nothing; only the process-lifetime target cache is initialized.
    pub(crate) fn initialize_main_chart_target(
        &mut self,
        group: &str,
        target: Option<(CoreId, String)>,
    ) {
        let target = target.filter(|(core, _)| self.core_belongs_to_group(group, *core));
        self.store_main_chart_target(group, target);
    }

    /// Publish the group's current Main target and remember a genuine Classic core change.
    ///
    /// Repeated synchronization of the same target preserves a manual header selection. In
    /// Classic, moving Main or a locked comparison anchor to another core makes that core the new
    /// durable selection. In Auto, the target remains runtime chart context and cannot overwrite
    /// `active_trade_core_by_group`.
    ///
    /// Args:
    ///     group: Window group whose target changed.
    ///     target: Active core and market, or `None` when no Main trading target exists. A target
    ///         whose live core no longer belongs to `group` is treated as `None`.
    ///
    /// Returns:
    ///     Nothing; runtime target state and, only for a Classic core change, layout state update.
    pub(crate) fn set_main_chart_target(&mut self, group: &str, target: Option<(CoreId, String)>) {
        let target = target.filter(|(core, _)| self.core_belongs_to_group(group, *core));
        if self.main_chart_targets.get(group) == target.as_ref() {
            return;
        }
        let prev_core = self.main_chart_targets.get(group).map(|(core, _)| *core);
        if let Some(new_core) = target.as_ref().and_then(|(new_core, _)| {
            Self::classic_trade_core_for_main_transition(
                self.workspace_mode(group),
                prev_core,
                *new_core,
            )
        }) {
            self.set_active_trade_core(group, new_core);
        }
        self.store_main_chart_target(group, target);
    }

    /// Resolve whether a Main target transition may update durable Classic trade state.
    ///
    /// This is the exact production guard used by [`Self::set_main_chart_target`]. It remains a
    /// narrow associated function so the Auto chart-open regression can mutate and prove the real
    /// decision without constructing the unrelated report, chart, and window fields of Backend.
    ///
    /// Args:
    ///     mode: Current group workspace mode.
    ///     previous_core: Core previously targeted by Main, if any.
    ///     new_core: Newly published Main core.
    ///
    /// Returns:
    ///     Core to remember only for a genuine Classic transition, otherwise `None`.
    pub(super) fn classic_trade_core_for_main_transition(
        mode: WorkspaceMode,
        previous_core: Option<CoreId>,
        new_core: CoreId,
    ) -> Option<CoreId> {
        (previous_core != Some(new_core)
            && crate::workspace::should_remember_classic_trade_core(mode))
        .then_some(new_core)
    }

    /// Replace one group's process-lifetime Main target cache entry.
    ///
    /// Args:
    ///     group: Window group that owns the entry.
    ///     target: Validated core and market, or `None` to remove the entry.
    ///
    /// Returns:
    ///     Nothing; durable selection state is not changed here.
    fn store_main_chart_target(&mut self, group: &str, target: Option<(CoreId, String)>) {
        match target {
            Some(target) => {
                if self.main_chart_targets.get(group) != Some(&target) {
                    self.main_chart_targets.insert(group.to_string(), target);
                }
            }
            None => {
                self.main_chart_targets.remove(group);
            }
        }
    }

    /// Return the group's current Main trading target while it still belongs to that live group.
    ///
    /// Args:
    ///     group: Window group whose target is requested.
    ///
    /// Returns:
    ///     The stored core and market, or `None` when absent or stale after a group move.
    pub(crate) fn main_chart_target(&self, group: &str) -> Option<(CoreId, String)> {
        self.main_chart_target_ref(group)
            .map(|(core, market)| (core, market.to_owned()))
    }

    /// Borrow the Main target only while its live core remains in the owning group.
    pub(crate) fn main_chart_target_ref(&self, group: &str) -> Option<(CoreId, &str)> {
        self.main_chart_targets
            .get(group)
            .filter(|(core, _)| self.core_belongs_to_group(group, *core))
            .map(|(core, market)| (*core, market.as_str()))
    }

    /// Return one market's exchange trading limits, for the leverage control.
    ///
    /// Takes the core and market EXPLICITLY rather than resolving a group's current chart, because
    /// its two consumers ask different questions: the toolbar readout wants the market on screen
    /// now, while an open leverage popup must ask about the address it was SEEDED from — the chart
    /// can move to another coin while that popup stands open, and answering with the new coin's cap
    /// is how a limit gets read against the wrong market. A group-shaped convenience here would
    /// make the wrong call the easy one.
    ///
    /// Args:
    ///     core: Core whose provider owns the market data.
    ///     market: Canonical market name.
    ///
    /// Returns:
    ///     The limits, or `None` while the provider snapshot or the market has not arrived.
    pub(crate) fn market_limits(&self, core: CoreId, market: &str) -> Option<MarketLimits> {
        self.session.market_source().market_limits(core, market)
    }

    /// Publish the markets open in a group's Main stack from `MainChartStack`.
    pub(crate) fn set_main_open_markets(&mut self, group: &str, markets: Vec<(CoreId, String)>) {
        if markets.is_empty() {
            self.main_open_markets.remove(group);
        } else {
            self.main_open_markets.insert(group.to_string(), markets);
        }
    }

    /// Return the markets open in a group's Main stack for highlighting and sorting in Orders.
    pub(crate) fn main_open_markets(&self, group: &str) -> &[(CoreId, String)] {
        self.main_open_markets
            .get(group)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Return one group's latest ordered Auto dock-surface transition.
    ///
    /// Args:
    ///     group: Exact group window whose navigation cursor is being reconciled.
    ///
    /// Returns:
    ///     Latest revision and surface, or `None` before the group publishes its first transition.
    pub(crate) fn auto_workspace_surface_request(
        &self,
        group: &str,
    ) -> Option<(u64, crate::workspace::AutoWorkspaceSurface)> {
        self.auto_workspace_surface_requests.current(group)
    }

    /// Publish ChartTabs after a group successfully consumes and opens a Main request.
    ///
    /// Args:
    ///     group: Exact group whose `ChartTabs` accepted the request.
    ///
    /// Returns:
    ///     Nothing; the shared sequence advances after every successful Main open.
    pub(crate) fn request_chart_tabs_after_main_open(&mut self, group: &str) {
        self.auto_workspace_surface_requests
            .request(group, crate::workspace::AutoWorkspaceSurface::ChartTabs);
    }

    /// Publish Report only after Escape actually empties an Auto group's Main stack.
    ///
    /// Args:
    ///     group: Exact group addressed by the Escape request.
    ///     closed: Whether `MainChartStack::close_active` removed a chart.
    ///     main_surface_active: Whether Main was visible instead of an Add or Custom chart tab.
    ///
    /// Returns:
    ///     `true` when a new Report transition was published and observers need notification.
    pub(crate) fn request_report_after_main_close(
        &mut self,
        group: &str,
        closed: bool,
        main_surface_active: bool,
    ) -> bool {
        if !crate::workspace::should_return_to_report_after_main_close(
            self.workspace_mode(group),
            closed,
            main_surface_active,
            self.main_open_markets(group).len(),
        ) {
            return false;
        }
        self.auto_workspace_surface_requests
            .request(group, crate::workspace::AutoWorkspaceSurface::Report);
        true
    }

    /// Publish Assets so Auto can reveal the group-owned panel instead of a detached window.
    ///
    /// Classic is a no-op: the request is not published, so a header double-click opens the Assets
    /// window on its own path rather than relying on this method.
    ///
    /// Args:
    ///     group: Exact group whose Assets surface should become visible.
    ///
    /// Returns:
    ///     `true` when a new Auto Assets transition was published and observers need notification.
    pub(crate) fn request_assets_surface(&mut self, group: &str) -> bool {
        if self.workspace_mode(group) != WorkspaceMode::AutoTrading {
            return false;
        }
        self.auto_workspace_surface_requests
            .request(group, crate::workspace::AutoWorkspaceSurface::Assets);
        true
    }

    /// Record group-wide activity and reset Main's inactivity-close timer.
    ///
    /// Any active group-owned window can refresh this timestamp, including the primary window,
    /// detached chart windows, and detached panel windows.
    pub(crate) fn note_main_input(&mut self, group: &str) {
        self.last_main_input
            .insert(group.to_string(), std::time::Instant::now());
    }

    /// Return the last group-wide activity time used by Main's inactivity timeout.
    pub(crate) fn main_input_at(&self, group: &str) -> Option<std::time::Instant> {
        self.last_main_input.get(group).copied()
    }

    /// Return Main's configured inactivity-close timeout in seconds, where zero disables it.
    pub(crate) fn main_idle_close_secs(&self) -> u32 {
        self.preview
            .as_ref()
            .unwrap_or(&self.config)
            .main_idle_close_secs
    }
}
