//! Auto-trading workspace authority shared by group panels, singleton tools, and the Shell.
//!
//! Persisted mode, selected-core, and rail-width values stay in `WindowLayout`; this module owns
//! derived effective scope, process-lifetime singleton focus, roster/navigation policy, and the
//! separate scope and Auto-layout revision notifications. None of these helpers mutates a panel's
//! retained Classic filter.

use std::collections::HashMap;

use gpui::Context;
use moon_core::config::{NO_MATCH_CORE_UID, TransportVersion, WorkspaceMode};
use moon_core::feed::{ConnFault, ConnStatus, CoreStartupStatus};
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

/// Dock surface requested by the latest ordered Auto workspace navigation event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoWorkspaceSurface {
    /// The group-owned Main chart host.
    ChartTabs,
    /// The group-owned report panel.
    Report,
    /// The group-owned Assets panel, including wallets and transfers.
    Assets,
}

impl AutoWorkspaceSurface {
    /// Return the stable MoonUI dock panel name for this surface.
    ///
    /// Returns:
    ///     The name consumed by `DockArea::activate_panel_by_name`.
    pub(crate) fn panel_name(self) -> &'static str {
        match self {
            Self::ChartTabs => "ChartTabs",
            Self::Report => "Report",
            Self::Assets => "Assets",
        }
    }
}

/// Latest ordered Auto surface request retained independently for each group window.
#[derive(Default)]
pub(crate) struct AutoWorkspaceSurfaceRequests {
    revision: u64,
    by_group: HashMap<String, (u64, AutoWorkspaceSurface)>,
}

impl AutoWorkspaceSurfaceRequests {
    /// Publish one surface request after every earlier request in the process.
    ///
    /// Args:
    ///     group: Exact group window that owns the requested surface.
    ///     surface: Dock surface that the latest user action should reveal.
    ///
    /// Returns:
    ///     The new wrapping process-wide revision.
    pub(crate) fn request(&mut self, group: &str, surface: AutoWorkspaceSurface) -> u64 {
        self.revision = self.revision.wrapping_add(1);
        self.by_group
            .insert(group.to_string(), (self.revision, surface));
        self.revision
    }

    /// Return one group's latest surface request without consuming another Shell's state.
    ///
    /// Args:
    ///     group: Exact group whose transition is requested.
    ///
    /// Returns:
    ///     The latest revision and surface for that group, or `None` before its first request.
    pub(crate) fn current(&self, group: &str) -> Option<(u64, AutoWorkspaceSurface)> {
        self.by_group.get(group).copied()
    }
}

/// Resolve one unseen group surface request and advance the Shell cursor in every mode.
///
/// Advancing in Classic prevents an event observed there from replaying after a later Auto switch.
/// A newly constructed Shell seeds its cursor from [`AutoWorkspaceSurfaceRequests::current`], so a
/// retained request cannot replay after a Settings rebuild.
///
/// Args:
///     mode: Workspace mode currently requested for the group.
///     cursor: Last request revision observed by this Shell.
///     current: Latest group-local request from the shared authority.
///
/// Returns:
///     The unseen surface only in Auto mode; stale, absent, and Classic requests return `None`.
pub(crate) fn resolve_auto_workspace_surface(
    mode: WorkspaceMode,
    cursor: &mut u64,
    current: Option<(u64, AutoWorkspaceSurface)>,
) -> Option<AutoWorkspaceSurface> {
    let (revision, surface) = current?;
    if revision == *cursor {
        return None;
    }
    *cursor = revision;
    (mode == WorkspaceMode::AutoTrading).then_some(surface)
}

/// Decide whether Escape emptied an Auto group's Main stack and should reveal Report.
///
/// Args:
///     mode: Current workspace mode for the addressed group.
///     closed: Whether the Escape close actually removed one Main chart.
///     main_surface_active: Whether Main, rather than Add or Custom, was the visible chart tab.
///     remaining_main_charts: Authoritative count published after that close.
///
/// Returns:
///     `true` only after a successful last-Main-chart close in Auto mode.
pub(crate) fn should_return_to_report_after_main_close(
    mode: WorkspaceMode,
    closed: bool,
    main_surface_active: bool,
    remaining_main_charts: usize,
) -> bool {
    mode == WorkspaceMode::AutoTrading
        && closed
        && main_surface_active
        && remaining_main_charts == 0
}

/// Notification generation for changes that can invalidate an effective workspace scope.
///
/// Observers use notifications to wake. Report queries reject old results through their own query
/// sequence and filter equality, while Report export captures this generation plus effective core
/// IDs across the native destination picker so a changed scope cannot write the requested file.
#[derive(Default)]
pub(crate) struct WorkspaceRevision {
    generation: u64,
}

impl WorkspaceRevision {
    /// Advance the generation and notify every observing workspace consumer.
    ///
    /// Args:
    ///     cx: GPUI entity context used to publish one coherent invalidation.
    ///
    /// Returns:
    ///     Nothing; observers receive a notification after the generation advances.
    pub(crate) fn advance(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        cx.notify();
    }

    /// Return the current generation for signature-based consumers.
    ///
    /// Returns:
    ///     Monotonic wrapping revision of workspace authority changes.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

/// Notification generation for the process-wide Auto layout authority.
///
/// The one channel covers both the shared dock topology and shared rail width. Shells observe it,
/// compare the Backend values they last applied, and never publish an observed remote update back
/// into the authority.
#[derive(Default)]
pub(crate) struct AutoWorkspaceLayoutRevision {
    generation: u64,
}

impl AutoWorkspaceLayoutRevision {
    /// Advance the shared Auto-layout generation and notify every open Shell.
    ///
    /// Args:
    ///     cx: GPUI entity context used to publish the coherent update.
    ///
    /// Returns:
    ///     Nothing; the wrapping generation advances before observers are notified.
    pub(crate) fn advance(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        cx.notify();
    }
}

/// Resolve whether a requested global Auto rail width changes the persisted normalized value.
///
/// Args:
///     current: Current persisted logical-pixel width.
///     requested: Raw width reported by a Shell resize state.
///
/// Returns:
///     The normalized new value when it differs, otherwise `None` for the equality guard.
pub(crate) fn changed_auto_workspace_rail_width(current: f32, requested: f32) -> Option<f32> {
    let requested = moon_core::config::clamp_auto_workspace_rail_width(requested);
    (current != requested).then_some(requested)
}

/// Process-lifetime owner of the effective scope used by singleton tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceFocus {
    group: String,
}

impl WorkspaceFocus {
    /// Create focus for one live group.
    ///
    /// Args:
    ///     group: Group whose interaction or toolbar launch established ownership.
    ///
    /// Returns:
    ///     Process-lifetime focus record; it is never serialized.
    pub(crate) fn new(group: impl Into<String>) -> Self {
        Self {
            group: group.into(),
        }
    }

    /// Return the owning group name.
    ///
    /// Returns:
    ///     Borrowed group identifier.
    pub(crate) fn group(&self) -> &str {
        &self.group
    }
}

/// Lifecycle of the owning group window during availability resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceWindowState {
    /// No owning group window exists or is being created.
    Missing,
    /// `open_window` is constructing the Shell before its final handle enters the registry.
    Opening,
    /// The completed group-window handle is present in the live registry.
    Live,
}

/// Complete availability facts shared by scope, selectors, trade overlay, and roster policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceCoreAvailability {
    pub(crate) group_active: bool,
    pub(crate) core_active: bool,
    pub(crate) live_session: bool,
    pub(crate) window: WorkspaceWindowState,
}

impl WorkspaceCoreAvailability {
    /// Return whether the core has a configured, live Auto-workspace owner.
    ///
    /// `Opening` is intentionally accepted: GPUI constructs `Shell` inside `open_window` before
    /// the returned handle can enter `group_windows`. Rejecting that lifecycle edge would
    /// transiently resolve a restored selected core as Overview during its first render.
    ///
    /// Returns:
    ///     `true` only for an active core with a live session and opening/live group owner.
    pub(crate) fn is_available(self) -> bool {
        self.group_active
            && self.core_active
            && self.live_session
            && self.window != WorkspaceWindowState::Missing
    }

    /// Return whether a configured core belongs in the session-independent fallback universe.
    ///
    /// `configured_workspace_scope` deliberately ignores `live_session` and `window` here, while
    /// still excluding inactive groups and cores. That keeps an offline fallback from resurrecting
    /// configuration the steady-state scope must never show.
    ///
    /// Returns:
    ///     `true` for an active core in an active group, independent of session and window state.
    pub(crate) fn is_configured_active(self) -> bool {
        self.group_active && self.core_active
    }

    /// Derive the localization-neutral roster status from the shared availability facts.
    ///
    /// Args:
    ///     ready: Whether the live core snapshot reports `ConnStatus::Ready`.
    ///
    /// Returns:
    ///     Disabled, unavailable, problem, or ready status for the rail.
    pub(crate) fn roster_status(self, ready: bool) -> WorkspaceCoreStatus {
        if !self.group_active || !self.core_active {
            WorkspaceCoreStatus::Disabled
        } else if !self.is_available() {
            WorkspaceCoreStatus::Unavailable
        } else if ready {
            WorkspaceCoreStatus::Ready
        } else {
            WorkspaceCoreStatus::Problem
        }
    }
}

/// Which preset a surface displays under, for the per-core membership predicate.
///
/// See `Backend::display_preset`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum DisplayOwner<'a> {
    /// A window that owns a group: Shell, group panels, detached panels (`DetachedSpec.group`),
    /// chart tabs.
    Group(&'a str),
    /// A singleton tool that INHERITS the last group whose window was focused, in either preset:
    /// Analytics, Strategies, Profit Monitor. Global Assets (`AssetsScope::All`) and Settings need
    /// no variant of their own — they never call `display_preset` at all, resolving their own
    /// unscoped views by other means.
    Singleton,
}

/// A panel's retained Classic filter supplied without transferring ownership or mutating it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RetainedCoreScope<'a> {
    /// Every currently valid core in the panel's group.
    All,
    /// The panel's retained explicit core IDs.
    Explicit(&'a [CoreId]),
}

/// Origin and label semantics of a resolved effective core scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectiveScopeKind {
    /// Classic panel filter meaning every valid group core.
    RetainedAll,
    /// Classic panel filter meaning an explicit subset.
    RetainedSelection,
    /// Auto workspace Overview meaning every valid group core.
    AutoOverview,
    /// Auto workspace meaning exactly one selected live core.
    AutoCore,
}

/// Localization-neutral label semantic for an effective scope selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectiveScopeLabel {
    /// The panel's retained all-cores label.
    All,
    /// The panel's retained explicit selection, with the number of valid cores.
    Selection(usize),
    /// The Auto workspace Overview label.
    Overview,
    /// The selected Auto core, whose configured name is resolved by the caller.
    Core(CoreId),
}

/// Concrete, deterministic core IDs a panel must use for its current query or action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EffectiveCoreScope {
    kind: EffectiveScopeKind,
    ids: Vec<CoreId>,
    /// Cores that survived availability and would be shown under the CURRENT preset. Left at 0 by
    /// [`resolve_group_scope`], which has no view of the membership boundary; set by
    /// `Backend::effective_workspace_scope` via [`Self::with_membership_counts`].
    membership_shown: usize,
    /// Cores that survived availability BEFORE the membership filter ran.
    membership_total: usize,
}

impl EffectiveCoreScope {
    /// Attach the membership boundary's shown/total pair, computed by the caller before Classic or
    /// Auto retained filtering narrows `ids` further.
    ///
    /// Args:
    ///     shown: Cores that survived availability and membership together.
    ///     total: Cores that survived availability alone, before the membership filter ran.
    ///
    /// Returns:
    ///     The same scope carrying the membership pair a later marker needs.
    pub(crate) fn with_membership_counts(mut self, shown: usize, total: usize) -> Self {
        self.membership_shown = shown;
        self.membership_total = total;
        self
    }

    /// Return cores that survived availability and membership together.
    pub(crate) fn membership_shown(&self) -> usize {
        self.membership_shown
    }

    /// Return cores that survived availability alone, before the membership filter ran.
    pub(crate) fn membership_total(&self) -> usize {
        self.membership_total
    }
    /// Return the canonical valid IDs in this scope.
    ///
    /// Returns:
    ///     Borrowed IDs in the same canonical order supplied for the group universe.
    pub(crate) fn ids(&self) -> &[CoreId] {
        &self.ids
    }

    /// Return whether one core belongs to the resolved query scope.
    ///
    /// Args:
    ///     core: Stable core UID to test.
    ///
    /// Returns:
    ///     `true` when `core` is one of the effective IDs.
    pub(crate) fn contains(&self, core: CoreId) -> bool {
        self.ids.contains(&core)
    }

    /// Return whether Auto owns and pins the panel's local selector.
    ///
    /// Returns:
    ///     `true` for Overview and selected-core Auto scopes.
    pub(crate) fn is_workspace_owned(&self) -> bool {
        matches!(
            self.kind,
            EffectiveScopeKind::AutoOverview | EffectiveScopeKind::AutoCore
        )
    }

    /// Return whether Auto owns a single selected core rather than Overview.
    ///
    /// Returns:
    ///     `true` only for the explicit `AutoCore` scope kind. This complements
    ///     [`is_auto_overview_scope`]: both use selection provenance rather than the number of
    ///     resolved IDs, because Classic and Auto Overview can each legitimately contain one core.
    pub(crate) fn is_auto_core(&self) -> bool {
        self.kind == EffectiveScopeKind::AutoCore
    }

    /// Return localization-neutral label semantics for the selector.
    ///
    /// Returns:
    ///     Label variant whose visible text is supplied by the panel's locale domain.
    pub(crate) fn label(&self) -> EffectiveScopeLabel {
        match self.kind {
            EffectiveScopeKind::RetainedAll => EffectiveScopeLabel::All,
            EffectiveScopeKind::RetainedSelection => EffectiveScopeLabel::Selection(self.ids.len()),
            EffectiveScopeKind::AutoOverview => EffectiveScopeLabel::Overview,
            EffectiveScopeKind::AutoCore => EffectiveScopeLabel::Core(self.ids[0]),
        }
    }

    /// Fall back to a configured-cores scope when this one has no session-derived universe at all.
    ///
    /// `membership_total == 0` is the ONLY trigger, never the raw `ids.is_empty()` alone: an
    /// explicit user selection naming only offline cores, in a group that still has live cores,
    /// keeps `membership_total > 0` and must stay on this scope's own no-match sentinel rather
    /// than widen to the group's whole configured set. `configured` is a closure rather than an
    /// eager value so the config walk runs only when this scope's session universe is empty.
    ///
    /// Args:
    ///     configured: Lazily resolves the configured-cores fallback scope.
    ///
    /// Returns:
    ///     `self` unchanged, or `configured()`'s scope when this one has no session universe.
    pub(crate) fn or_configured(
        self,
        configured: impl FnOnce() -> EffectiveCoreScope,
    ) -> EffectiveCoreScope {
        if self.ids.is_empty() && self.membership_total == 0 {
            configured()
        } else {
            self
        }
    }
}

/// Turn a resolved scope's IDs into the query a DB read must send, distinguishing an ABSENT
/// scope from a PRESENT-but-empty one.
///
/// `moon-core`'s DB reads treat an empty uid list as UNFILTERED (see
/// `moon_core::db::ReportFilter::core_uids`), which is exactly right when there is no scope at
/// all — but wrong when a scope IS in effect and every core it named has been filtered out.
/// `scoped` is how the caller tells those apart.
///
/// **`scoped` is a per-caller JUDGEMENT, not one fixed expression** — an earlier draft of this
/// contract demanded every caller pass `ScopeMarker::hides_anything()`, and that is now false for
/// one of the three. What every caller MUST answer is "is an empty resolved list here a real
/// exclusion, or simply the absence of a scope"; the right way to answer it depends on what can
/// empty that surface's list:
///
/// - `panels::report::ReportPanel::query_core_uids` passes
///   `EffectiveCoreScope::membership_total() > 0`, applied to the scope AFTER
///   [`EffectiveCoreScope::or_configured`] has already substituted the group's configured
///   universe for an empty session one. Its list can also be emptied by an explicit user
///   selection naming only unavailable cores, which membership never hid, so `hides_anything()`
///   would answer `false` there and wrongly broaden a narrow selection to the whole fleet.
/// - `analytics::profit_monitor::model::scoped_query_core_ids` passes `hides_anything()`, behind
///   a short-circuit to an empty vec. Its scope is purely membership, so no other cause can empty
///   it, and `configured_total > 0` would wrongly filter when nothing is hidden.
/// - `analytics::toolbar::analytics_core_filter_ids` passes `true` at both of its sites, having
///   already established the scope is present at each.
///
/// The invariant that DOES hold everywhere: `scoped` is never the raw `ids.is_empty()`, which
/// would make the sentinel unreachable, and a surface whose rows CAN be emptied by a cause its own
/// marker cannot express owes the user some other statement of why — see that caller's own notes.
///
/// Args:
///     ids: The scope's resolved core IDs, however it got them.
///     scoped: Whether an empty `ids` here means a real exclusion rather than an absent scope.
///         Never the raw `ids.is_empty()`.
///
/// Returns:
///     `ids` unchanged when `scoped` is `false`, or `ids` is non-empty; otherwise
///     `vec![moon_core::config::NO_MATCH_CORE_UID]`, the present-empty encoding that matches no
///     row while core UIDs remain in their normal non-wrapping issued range.
pub(crate) fn query_core_ids(ids: Vec<CoreId>, scoped: bool) -> Vec<CoreId> {
    if scoped && ids.is_empty() {
        vec![NO_MATCH_CORE_UID]
    } else {
        ids
    }
}

/// Resolve a group panel's query scope without rewriting its retained Classic filter.
///
/// Args:
///     mode: Persisted workspace mode for the owning group.
///     selected_core: Persisted Auto core, if any; it is valid only when present in `group_cores`.
///     group_cores: Live group cores in canonical display order.
///     retained: Panel-owned Classic all/subset filter.
///
/// Returns:
///     Concrete valid IDs plus origin semantics. A missing or stale Auto core resolves to Overview
///     while its durable UID remains untouched in the layout.
pub(crate) fn resolve_group_scope(
    mode: WorkspaceMode,
    selected_core: Option<CoreId>,
    group_cores: &[CoreId],
    retained: RetainedCoreScope<'_>,
) -> EffectiveCoreScope {
    // `SessionManager` owns one session per stable CoreId, and `CoreOrder::from_sessions` preserves
    // that uniqueness. Cloning the canonical slice avoids an unnecessary quadratic dedup pass.
    let valid = group_cores.to_vec();

    if mode == WorkspaceMode::AutoTrading {
        // The Auto branch below and [`is_auto_overview_scope`] state ONE rule in two places, and
        // they must move together: this one filters the selection against `valid` itself, while
        // the predicate is handed an already-valid selection. Change either and the header's
        // hidden figures and this scope's own label can start disagreeing about the same group.
        if let Some(core) = selected_core.filter(|core| valid.contains(core)) {
            return EffectiveCoreScope {
                kind: EffectiveScopeKind::AutoCore,
                ids: vec![core],
                membership_shown: 0,
                membership_total: 0,
            };
        }
        return EffectiveCoreScope {
            kind: EffectiveScopeKind::AutoOverview,
            ids: valid,
            membership_shown: 0,
            membership_total: 0,
        };
    }

    match retained {
        RetainedCoreScope::All => EffectiveCoreScope {
            kind: EffectiveScopeKind::RetainedAll,
            ids: valid,
            membership_shown: 0,
            membership_total: 0,
        },
        RetainedCoreScope::Explicit(retained_ids) => EffectiveCoreScope {
            kind: EffectiveScopeKind::RetainedSelection,
            ids: valid
                .into_iter()
                .filter(|core| retained_ids.contains(core))
                .collect(),
            membership_shown: 0,
            membership_total: 0,
        },
    }
}

/// Valid Auto workspace context inherited by application-wide singleton tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SingletonWorkspace {
    /// Live group whose last interaction owns Analytics and Strategies scope.
    pub(crate) group: String,
    /// Valid selected core, or `None` for that group's Overview.
    pub(crate) selected_core: Option<CoreId>,
}

/// Resolve singleton ownership from the already identified focused group.
///
/// Args:
///     group: Focused group identified by the Backend before deriving any core list.
///     owner_registered: Whether that focused group has a live or opening primary window.
///     mode: Persisted workspace mode for the focused group.
///     selected_core: Persisted Auto core for the focused group, if any.
///     live_cores: Canonical available core IDs for the focused group.
///
/// Returns:
///     Valid owning workspace, or `None` so singleton tools keep their retained local filters.
pub(crate) fn resolve_singleton_workspace(
    group: &str,
    owner_registered: bool,
    mode: WorkspaceMode,
    selected_core: Option<CoreId>,
    live_cores: &[CoreId],
) -> Option<SingletonWorkspace> {
    if !owner_registered || mode != WorkspaceMode::AutoTrading {
        return None;
    }
    let selected_core = selected_core.filter(|core| live_cores.contains(core));
    Some(SingletonWorkspace {
        group: group.to_string(),
        selected_core,
    })
}

/// Clear singleton focus when its group's live primary window closes.
///
/// Args:
///     focus: Process-lifetime focus slot to reconcile.
///     closed_group: Group that lost its live primary window.
///
/// Returns:
///     `true` when this group was the current owner and its focus was cleared.
pub(crate) fn close_workspace_owner(
    focus: &mut Option<WorkspaceFocus>,
    closed_group: &str,
) -> bool {
    if focus
        .as_ref()
        .is_some_and(|current| current.group() == closed_group)
    {
        *focus = None;
        true
    } else {
        false
    }
}

/// Make one group the singleton workspace owner without repeating the same transition.
///
/// Args:
///     focus: Process-lifetime singleton owner slot to update.
///     group: Group receiving active user interaction.
///
/// Returns:
///     `true` only when ownership moved, so callers publish no revision for repeated activity in
///     the already owning group.
pub(crate) fn focus_workspace_owner(focus: &mut Option<WorkspaceFocus>, group: &str) -> bool {
    if focus
        .as_ref()
        .is_some_and(|current| current.group() == group)
    {
        return false;
    }
    *focus = Some(WorkspaceFocus::new(group));
    true
}

/// Reconcile singleton focus against one final combined config/window validity decision.
///
/// Args:
///     focus: Process-lifetime singleton owner to reconcile in place.
///     owner_valid: Whether the owner remains configured and window-backed after all work.
///
/// Returns:
///     `true` only when an invalid owner was cleared during this final transition.
pub(crate) fn reconcile_workspace_focus(
    focus: &mut Option<WorkspaceFocus>,
    owner_valid: bool,
) -> bool {
    if focus.is_some() && !owner_valid {
        *focus = None;
        true
    } else {
        false
    }
}

/// Localization-neutral availability of one configured roster row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceCoreStatus {
    /// Core or its group is deliberately disabled in Settings.
    Disabled,
    /// Core has no live session or owning group window.
    Unavailable,
    /// Core has a live session that is not ready.
    Problem,
    /// Core session is ready for operator actions.
    Ready,
}

/// Pure input for one configured core in the all-core navigation roster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceRosterInput {
    pub(crate) core: CoreId,
    pub(crate) name: String,
    pub(crate) group: String,
    pub(crate) venue: Option<CoreVenue>,
    pub(crate) availability: WorkspaceCoreAvailability,
    pub(crate) ready: bool,
    pub(crate) connection: Option<ConnStatus>,
    pub(crate) startup: CoreStartupStatus,
    /// Why this core's last connection attempt ended, when one has ended.
    pub(crate) fault: Option<ConnFault>,
    /// A fleet-wide "try this mode instead" suggestion for this core, when the evidence supports
    /// one (`conn_diag::fleet_mode_suggestion`).
    pub(crate) mode_suggestion: Option<TransportVersion>,
}

/// Derived roster row rendered by the Auto workspace rail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceRosterRow {
    pub(crate) core: CoreId,
    pub(crate) name: String,
    pub(crate) group: String,
    pub(crate) status: WorkspaceCoreStatus,
    pub(crate) selectable: bool,
    pub(crate) selected: bool,
    pub(crate) connection: Option<ConnStatus>,
    pub(crate) startup: CoreStartupStatus,
    /// Why this core's last connection attempt ended, when one has ended.
    pub(crate) fault: Option<ConnFault>,
    /// A fleet-wide "try this mode instead" suggestion for this core, when the evidence supports
    /// one (`conn_diag::fleet_mode_suggestion`).
    pub(crate) mode_suggestion: Option<TransportVersion>,
}

/// One venue section in the all-core roster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceRosterSection {
    pub(crate) venue: Option<CoreVenue>,
    pub(crate) rows: Vec<WorkspaceRosterRow>,
}

/// Application-wide health counts displayed above the roster.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceRosterSummary {
    pub(crate) configured: usize,
    /// Configured cores before the membership filter ran, for the rail's own scope marker.
    pub(crate) configured_total: usize,
    pub(crate) ready: usize,
    pub(crate) problem: usize,
}

/// Exchange-grouped all-core roster plus the current group's Overview selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceRoster {
    pub(crate) sections: Vec<WorkspaceRosterSection>,
    pub(crate) summary: WorkspaceRosterSummary,
    pub(crate) overview_selected: bool,
}

/// Derive visible roster rows while retaining inactive and unavailable configured cores.
///
/// Args:
///     inputs: Configured cores in canonical member order, already scoped to the rail's own
///         Auto-Trading membership filter.
///     current_group: Group window whose Overview row is rendered.
///     selected_core: Valid selected Auto core, or `None` for Overview.
///     configured_total: Configured cores BEFORE the membership filter ran, supplied by the
///         caller — this function stays a pure "render every input you were given" and never
///         filters or recomputes the pre-membership count itself.
///
/// Returns:
///     Exchange-grouped roster and application-wide configured/ready/problem counts. Unknown
///     exchange identities remain an explicit first section instead of being guessed from names.
pub(crate) fn derive_workspace_roster(
    inputs: &[WorkspaceRosterInput],
    current_group: &str,
    selected_core: Option<CoreId>,
    configured_total: usize,
) -> WorkspaceRoster {
    let mut summary = WorkspaceRosterSummary {
        configured: inputs.len(),
        configured_total,
        ..WorkspaceRosterSummary::default()
    };
    let exchange_sections = crate::core_order::exchange_sections(
        inputs
            .iter()
            .enumerate()
            .map(|(index, input)| (index, input.venue.as_ref())),
    );
    let mut sections = Vec::with_capacity(exchange_sections.len());
    for (venue, indices) in exchange_sections {
        let rows = indices
            .into_iter()
            .map(|index| {
                let input = &inputs[index];
                let status = input.availability.roster_status(input.ready);
                if status == WorkspaceCoreStatus::Ready {
                    summary.ready += 1;
                } else if matches!(
                    status,
                    WorkspaceCoreStatus::Unavailable | WorkspaceCoreStatus::Problem
                ) {
                    summary.problem += 1;
                }
                WorkspaceRosterRow {
                    core: input.core,
                    name: input.name.clone(),
                    group: input.group.clone(),
                    status,
                    selectable: input.availability.is_available(),
                    selected: input.group == current_group && selected_core == Some(input.core),
                    connection: input.connection.clone(),
                    startup: input.startup,
                    fault: input.fault.clone(),
                    mode_suggestion: input.mode_suggestion,
                }
            })
            .collect();
        sections.push(WorkspaceRosterSection {
            venue: venue.cloned(),
            rows,
        });
    }

    WorkspaceRoster {
        sections,
        summary,
        overview_selected: selected_core.is_none(),
    }
}

/// Pure navigation decision for one selectable roster row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceNavigationAction {
    /// Change scope inside the already active group window.
    SelectCurrent { group: String, core: CoreId },
    /// Persist scope for another group and activate its existing window.
    ActivateGroup { group: String, core: CoreId },
}

/// Plan a core-row click without reparenting any group-owned panel.
///
/// Args:
///     current_group: Group window receiving the click.
///     row: Derived roster row targeted by the click.
///
/// Returns:
///     Same-group selection or cross-group activation, or `None` for a disabled row.
pub(crate) fn plan_workspace_navigation(
    current_group: &str,
    row: &WorkspaceRosterRow,
) -> Option<WorkspaceNavigationAction> {
    if !row.selectable {
        return None;
    }
    if row.group == current_group {
        Some(WorkspaceNavigationAction::SelectCurrent {
            group: current_group.to_string(),
            core: row.core,
        })
    } else {
        Some(WorkspaceNavigationAction::ActivateGroup {
            group: row.group.clone(),
            core: row.core,
        })
    }
}

/// Three-rung Auto workspace rail density.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceRailDensity {
    /// Full names; non-ready rows also keep explicit status text.
    Full,
    /// Truncated server names with every status retained in the tooltip.
    Compact,
    /// Icon rows whose complete name and status move to tooltips.
    Icon,
}

/// Minimum expanded rail width that shows status text beside full server names.
pub(crate) const WORKSPACE_RAIL_FULL_MIN_WIDTH: f32 = 300.0;
/// Minimum expanded rail width that keeps server names instead of icon initials.
pub(crate) const WORKSPACE_RAIL_COMPACT_MIN_WIDTH: f32 = 140.0;

/// Choose rail presentation from the width currently owned by the draggable sidebar.
///
/// Args:
///     rail_width: Current logical sidebar width after UI scaling.
///
/// Returns:
///     Full, compact, or icon presentation without changing the user's chosen width.
pub(crate) fn workspace_rail_density(rail_width: f32) -> WorkspaceRailDensity {
    if rail_width >= WORKSPACE_RAIL_FULL_MIN_WIDTH {
        WorkspaceRailDensity::Full
    } else if rail_width >= WORKSPACE_RAIL_COMPACT_MIN_WIDTH {
        WorkspaceRailDensity::Compact
    } else {
        WorkspaceRailDensity::Icon
    }
}

/// Return whether a dock layout event may update the persisted normal topology.
///
/// Args:
///     mode: Current workspace preset for the group.
///
/// Returns:
///     `true` only in Classic mode; Auto layout events use the separate shared topology authority.
pub(crate) fn should_persist_normal_dock(mode: WorkspaceMode) -> bool {
    mode == WorkspaceMode::Classic
}

/// Return whether a trade-core producer may update the durable Classic selection.
///
/// Args:
///     mode: Current workspace preset for the group.
///
/// Returns:
///     `true` only when Classic owns the header/manual selection.
pub(crate) fn should_remember_classic_trade_core(mode: WorkspaceMode) -> bool {
    mode == WorkspaceMode::Classic
}

/// Return whether the group's VISIBLE scope names NO single core — the Auto workspace Overview.
///
/// Kept in step with [`resolve_group_scope`]'s Auto branch by hand: that function answers the same
/// question as a full [`EffectiveCoreScope`], which a chrome row cannot pay for on every repaint,
/// so this is the cheap form of the identical rule and the two are edited together.
///
/// This is the honest counterpart to [`crate::Backend::active_trade_core`], which never answers
/// `None` while the group has any live core: in Overview it falls through to the group's FIRST
/// core, so a per-core figure read through it belongs to one arbitrary server while the header
/// pill reads "Полная сводка". Chrome that prints money, leverage, or an exchange limit asks THIS
/// first and prints its own unknown mark when the answer is `true`.
///
/// The mode conjunct cannot be dropped: [`crate::Backend::valid_auto_workspace_core`] reads the
/// persisted Auto map regardless of the current mode, so it answers `None` for a Classic group
/// that never had an Auto selection — and without the mode check that group would lose its
/// balance too.
///
/// Classic is deliberately `false` even with several cores in the group: its header selector is an
/// interactive control that NAMES the single core every trading control is addressed to, and that
/// core was chosen by the user. The condition is scope-shaped, not cardinality-shaped, so an Auto
/// group holding exactly one live core still reads Overview and still hides — the same rule
/// [`EffectiveCoreScope::is_auto_core`] already states for itself.
///
/// Args:
///     mode: Persisted workspace preset for the group.
///     valid_selected_core: The group's VALID Auto selection — `Backend::valid_auto_workspace_core`,
///         which is `None` for Overview and for a stale persisted reference.
///
/// Returns:
///     `true` only for Auto Overview, where no single core owns the visible scope.
pub(crate) fn is_auto_overview_scope(
    mode: WorkspaceMode,
    valid_selected_core: Option<CoreId>,
) -> bool {
    mode == WorkspaceMode::AutoTrading && valid_selected_core.is_none()
}

pub(crate) mod scope_marker;

#[cfg(test)]
mod tests;
