//! Analytics session extracted from the window module.

use super::*;

/// Process-lifetime Analytics choices restored when its OS window is recreated.
#[derive(Clone)]
pub(crate) struct AnalyticsSessionState {
    /// Last top-level page selected by the user.
    pub(super) tab: Tab,
    /// Explicit core filter, preserving empty, complete, and stale selections.
    pub(super) sel_cores: HashSet<u64>,
    /// Explicit saved-group provenance for the caption beside the numeric core trigger.
    pub(super) core_caption: toolbar::CoreSelectionCaption,
    /// Last Strategies axis selected while this process is running.
    pub(super) strat_mode: tuner::StratMode,
    /// Whether the "closed trades the core never dated" notice is expanded.
    ///
    /// Deliberately here and NOT in `WindowLayout`: the notice starts collapsed in every
    /// process, for every user, so a restart cannot leave a warning about money missing from
    /// the figures silently switched off. Expanding it is a look-at-it-now action, not a
    /// preference — it survives closing the window and nothing more.
    pub(super) undated_expanded: bool,
}

/// Effective Auto-workspace scope inherited from the last live singleton owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AnalyticsWorkspaceScope {
    /// Selected core, or `None` when the owning group is on Overview.
    pub(super) selected_core: Option<u64>,
    /// Concrete live group cores that bound actions and pinned-filter reads.
    pub(super) core_ids: Vec<u64>,
    /// Cores that survived availability and membership together, for the summary's scope marker.
    pub(super) membership_shown: usize,
    /// Cores that survived availability alone, before the membership filter ran.
    pub(super) membership_total: usize,
}

impl AnalyticsWorkspaceScope {
    /// Whether this scope also PINS which cores the window reads, not just what it may write to.
    ///
    /// The two answers part on the rail's Overview row. Overview is not a narrower question the
    /// user asked of Analytics — it is the absence of one, and answering it with the whole group
    /// would overrule a core filter the user set here and lock the selector against changing it,
    /// on the one surface where "every core in the group" and "the cores I want to compare"
    /// routinely differ. Action authority is unaffected: Save, Copy and purge stay confined to
    /// the group for the whole of Auto.
    ///
    /// Returns:
    ///     `true` only while the rail is on a concrete core.
    pub(super) fn pins_core_filter(&self) -> bool {
        self.selected_core.is_some()
    }

    /// Build the summary's scope marker.
    ///
    /// Returns:
    ///     A marker built from the membership boundary's own counts. The preset is always
    ///     [`WorkspaceMode::AutoTrading`]: [`analytics_workspace_scope`] only produces `Some` while
    ///     `Backend::singleton_workspace` has resolved a live Auto-focused group.
    pub(super) fn scope_marker(&self) -> crate::workspace::scope_marker::ScopeMarker {
        crate::workspace::scope_marker::ScopeMarker::new(
            Some(moon_core::config::WorkspaceMode::AutoTrading),
            self.membership_shown,
            self.membership_total,
        )
    }
}

/// Configured cores the singleton's VIEWING preset hides, with the counts its marker states.
///
/// Deliberately the HIDDEN set rather than the shown one: this window queries the REPLICA core
/// universe (`AnalyticsView::cores`), which can name a core whose server was deleted and which
/// therefore has no `workspace_membership` at all. `Backend::core_displayed` admits such a core,
/// so subtracting a hidden set keeps it; intersecting with a config-derived shown set would drop
/// it and lose money from the figures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AnalyticsDisplayScope {
    /// Preset resolved through `Backend::display_preset(DisplayOwner::Singleton)`.
    pub(super) preset: Option<moon_core::config::WorkspaceMode>,
    /// Configured cores this preset hides from Analytics' scoped data reads. Never empty — see
    /// `analytics_display_scope`.
    pub(super) hidden_core_ids: Vec<u64>,
    /// Every `config.servers` id, in configuration order, captured in the same walk as
    /// `hidden_core_ids`.
    ///
    /// The bootstrap universe for the implicit "All" row: the replica core list arrives only WITH
    /// a query's result, while this is complete the moment the window is built, so a first query
    /// can be scoped exactly as the marker states instead of running unfiltered. See
    /// `toolbar::analytics_core_filter_ids`, which consults it only while that replica list is
    /// still empty.
    pub(super) configured_core_ids: Vec<u64>,
    /// `config.servers.len()`, captured BEFORE the membership filter ran.
    pub(super) configured_total: usize,
}

impl AnalyticsDisplayScope {
    /// Build the summary's scope marker.
    ///
    /// Returns:
    ///     A marker built from the membership boundary's own counts.
    pub(super) fn scope_marker(&self) -> crate::workspace::scope_marker::ScopeMarker {
        crate::workspace::scope_marker::ScopeMarker::new(
            self.preset,
            self.configured_total - self.hidden_core_ids.len(),
            self.configured_total,
        )
    }
}

/// Resolve which configured cores the singleton's VIEWING preset hides.
///
/// Independent of [`analytics_workspace_scope`] by construction: `Backend::display_preset` and
/// `Backend::singleton_workspace` apply the SAME focus-and-live test, so
/// `display_preset(Singleton) == Some(AutoTrading)` holds exactly when `singleton_workspace()`
/// is `Some`. The two never both answer.
///
/// This is a READ narrowing only. Analytics' ACTION authority stays `analytics_workspace_scope`,
/// which is `None` in Classic on purpose (`AnalyticsView::action_core_ids`).
pub(super) fn analytics_display_scope(backend: &Backend) -> Option<AnalyticsDisplayScope> {
    let preset = backend.display_preset(crate::workspace::DisplayOwner::Singleton);
    if preset != Some(moon_core::config::WorkspaceMode::Classic) {
        // The Auto arm is already answered group-scoped by `analytics_workspace_scope`; without a
        // live singleton owner, this window remains unscoped and shows every core.
        return None;
    }
    let configured_core_ids: Vec<u64> = backend.config.servers.iter().map(|s| s.id).collect();
    let configured_total = configured_core_ids.len();
    let hidden_core_ids: Vec<u64> = backend
        .config
        .servers
        .iter()
        .filter(|server| !backend.core_displayed(preset, server.id))
        .map(|server| server.id)
        .collect();
    if hidden_core_ids.is_empty() {
        // Load-bearing: keeps every unaffected Classic state today byte-identical to before
        // this fix.
        return None;
    }
    Some(AnalyticsDisplayScope {
        preset,
        hidden_core_ids,
        configured_core_ids,
        configured_total,
    })
}

/// Resolve the current singleton workspace without changing Analytics' retained selection.
///
/// This is Analytics' ACTION authority and stays present for the whole of Auto, Overview
/// included — it is what confines Save, Copy and strategy purge to the focused group's cores and
/// what invalidates a captured dialog when the group moves. Which cores the window READS is a
/// separate, narrower question answered by [`AnalyticsView::core_filter_pin`].
///
/// Args:
///     backend: Shared authority for singleton ownership and group-effective scope.
///
/// Returns:
///     Concrete Auto scope, or `None` when Analytics owns its Classic filter.
pub(super) fn analytics_workspace_scope(backend: &Backend) -> Option<AnalyticsWorkspaceScope> {
    let workspace = backend.singleton_workspace()?;
    let scope = backend
        .effective_workspace_scope(&workspace.group, crate::workspace::RetainedCoreScope::All);
    Some(AnalyticsWorkspaceScope {
        selected_core: workspace.selected_core,
        core_ids: scope.ids().to_vec(),
        membership_shown: scope.membership_shown(),
        membership_total: scope.membership_total(),
    })
}

impl Default for AnalyticsSessionState {
    /// Create the state used for the first Analytics open in a fresh process.
    ///
    /// Returns:
    ///     Summary-tab state with the empty core set that represents all current cores.
    fn default() -> Self {
        Self {
            tab: Tab::Summary,
            sel_cores: HashSet::new(),
            core_caption: toolbar::CoreSelectionCaption::default(),
            strat_mode: tuner::StratMode::Filters,
            undated_expanded: false,
        }
    }
}
