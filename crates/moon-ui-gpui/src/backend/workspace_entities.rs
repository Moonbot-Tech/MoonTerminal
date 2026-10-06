//! Workspace entities, dock topology, and revision publication.

use crate::Backend;
use gpui::Context;
use gpui::WindowId;
use moon_core::config::WorkspaceMode;
use moon_core::session::CoreId;
use moon_ui::DockAreaState;
use moon_ui::DockTopologyByName;
use std::collections::HashSet;

impl Backend {
    /// Resolve the live Auto owner inherited by Analytics and Strategies.
    ///
    /// Returns:
    ///     Last focused live Auto group plus its valid selected core, or `None` so the singleton
    ///     retains its own Classic filter.
    pub(crate) fn singleton_workspace(&self) -> Option<crate::workspace::SingletonWorkspace> {
        let focus = self.workspace_focus.as_ref()?;
        let group = focus.group();
        let owner_registered =
            self.group_windows.contains_key(group) || self.opening_group_windows.contains(group);
        let live_cores = self
            .group_cores(group)
            .into_iter()
            .map(|(core, _)| core)
            .filter(|core| {
                self.workspace_core_availability(group, *core)
                    .is_available()
            })
            .filter(|core| self.core_displayed_in_group(group, *core))
            .collect::<Vec<_>>();
        crate::workspace::resolve_singleton_workspace(
            group,
            owner_registered,
            self.workspace_mode(group),
            self.layout.auto_workspace_core_by_group.get(group).copied(),
            &live_cores,
        )
    }

    /// Return the dedicated entity observed by cached and asynchronous workspace consumers.
    ///
    /// Returns:
    ///     Shared revision entity whose notifications describe effective-scope invalidations.
    pub(crate) fn workspace_revision(&self) -> gpui::Entity<crate::workspace::WorkspaceRevision> {
        self.workspace_revision.clone()
    }

    /// Return the revision entity notified after retained market data changes.
    ///
    /// Returns:
    ///     A narrow wake channel for catalog-sensitive consumers such as Auto chart retargeting.
    pub(crate) fn market_data_revision(&self) -> gpui::Entity<crate::MarketDataRevision> {
        self.market_data_revision.clone()
    }

    /// Return the terminal's one reader of the crowd's public statistics.
    ///
    /// Returns:
    ///     The shared service. It is never observed FOR the backend's sake: it carries its own two
    ///     wake channels, one per audience, so nothing here wakes the other sixteen views.
    pub(crate) fn crowd(&self) -> gpui::Entity<crate::crowd::service::CrowdService> {
        self.crowd.clone()
    }

    /// Return the cores the Profit Monitor is currently broadcasting.
    ///
    /// Returns:
    ///     Selected core ids; empty means every core, matching each panel's own retained filter.
    pub(crate) fn core_filter(&self) -> &HashSet<CoreId> {
        &self.core_filter
    }

    /// Return the wake channel every core-selector panel observes for the broadcast filter.
    ///
    /// Returns:
    ///     Shared notification-only entity advanced by [`Self::set_core_filter`].
    pub(crate) fn core_filter_revision(&self) -> gpui::Entity<crate::CoreFilterRevision> {
        self.core_filter_revision.clone()
    }

    /// Publish a new cross-window core filter to every panel that owns a core selector.
    ///
    /// Equality-guarded: a click that resolves to the selection already on air must not wake five
    /// panels into rebuilding rows that cannot have changed.
    ///
    /// Args:
    ///     cores: Replacement selection; empty releases every panel back to all cores.
    ///     cx: Backend context used to notify the dedicated revision entity.
    ///
    /// Returns:
    ///     Nothing; observers see the new value only after the notification.
    pub(crate) fn set_core_filter(&mut self, cores: HashSet<CoreId>, cx: &mut Context<Self>) {
        if self.core_filter == cores {
            return;
        }
        self.core_filter = cores;
        self.core_filter_revision
            .update(cx, |_revision, revision_cx| revision_cx.notify());
    }

    /// Return the revision entity observed by every Auto Shell layout consumer.
    ///
    /// Returns:
    ///     Shared notification authority for topology and global rail-width changes.
    pub(crate) fn auto_workspace_layout_revision(
        &self,
    ) -> gpui::Entity<crate::workspace::AutoWorkspaceLayoutRevision> {
        self.auto_workspace_layout_revision.clone()
    }

    /// Borrow the persisted process-wide Auto dock topology authority.
    ///
    /// Returns:
    ///     Normalized panel-name topology loaded from or destined for `auto_dock.json`, or `None`
    ///     so a Shell uses the deterministic safe preset for missing or protected invalid data.
    pub(crate) fn auto_dock_topology(&self) -> Option<&DockTopologyByName> {
        self.auto_dock_topology.as_ref()
    }

    /// Accept a user-edited shared Auto dock topology when its normalized value changed.
    ///
    /// Args:
    ///     topology: Topology-only panel-name tree projected from the user-edited Auto dock.
    ///     cx: Backend context used to notify every other open Auto Shell.
    ///
    /// Returns:
    ///     `true` when the authority changed and now awaits persistence to `auto_dock.json`.
    pub(crate) fn set_auto_dock_topology(
        &mut self,
        topology: DockTopologyByName,
        cx: &mut Context<Self>,
    ) -> bool {
        let topology = topology.normalized();
        if self.auto_dock_topology.as_ref() == Some(&topology) {
            return false;
        }
        self.auto_dock_automatic_persistence_allowed = true;
        self.auto_dock_topology = Some(topology);
        self.auto_dock_dirty = true;
        self.publish_auto_workspace_layout_revision(cx);
        true
    }

    /// Force the shared Auto topology back to `topology` and unlock persistence even on equality.
    ///
    /// [`Self::set_auto_dock_topology`] returns before unlocking when the in-memory tree already
    /// matches, which is exactly the locked-invalid-file case a user reset has to repair.
    ///
    /// Args:
    ///     topology: First-run Auto topology to install as the shared authority.
    ///     cx: Backend context used to notify every open Auto Shell.
    ///
    /// Returns:
    ///     Nothing; persistence is unlocked and dirtied regardless of equality.
    pub(crate) fn reset_auto_dock_topology(
        &mut self,
        topology: DockTopologyByName,
        cx: &mut Context<Self>,
    ) {
        self.auto_dock_topology = Some(topology.normalized());
        self.auto_dock_automatic_persistence_allowed = true;
        self.auto_dock_dirty = true;
        self.publish_auto_workspace_layout_revision(cx);
    }

    /// Store a live dock dump only while the group is in Classic mode.
    ///
    /// Args:
    ///     group: Group whose live DockArea produced the state.
    ///     state: Complete serialized live dock state.
    ///
    /// Returns:
    ///     `true` when Classic persistence accepted the state; Auto callers are ignored so their
    ///     shared topology and temporary panel instances cannot overwrite `docks.json`.
    pub(crate) fn store_classic_dock_state(&mut self, group: String, state: DockAreaState) -> bool {
        if self.workspace_mode(&group) == WorkspaceMode::AutoTrading {
            return false;
        }
        self.dock_states.insert(group, state);
        self.dock_dirty = true;
        true
    }

    /// Reconcile topology produced by a programmatic Auto install or name-based repair.
    ///
    /// Missing first-run state and valid loaded state may persist this automatic transition.
    /// Invalid or unreadable startup state remains protected until
    /// [`Self::set_auto_dock_topology`] receives a distinct user-edited topology.
    ///
    /// Args:
    ///     topology: Actual normalized topology resolved against one Shell's live panel names.
    ///     cx: Backend context used to notify other Auto Shells when the authority changes.
    ///
    /// Returns:
    ///     `true` when the in-memory authority changed.
    pub(crate) fn reconcile_auto_dock_topology(
        &mut self,
        topology: DockTopologyByName,
        cx: &mut Context<Self>,
    ) -> bool {
        let topology = topology.normalized();
        if self.auto_dock_topology.as_ref() == Some(&topology) {
            return false;
        }
        self.auto_dock_topology = Some(topology);
        if self.auto_dock_automatic_persistence_allowed {
            self.auto_dock_dirty = true;
        }
        self.publish_auto_workspace_layout_revision(cx);
        true
    }

    /// Return the persisted logical-pixel Auto rail width shared by every group window.
    ///
    /// Returns:
    ///     Finite width clamped by the layout decoder and every runtime setter.
    pub(crate) fn auto_workspace_rail_width(&self) -> f32 {
        self.layout.auto_workspace_rail_width()
    }

    /// Persist and publish a global Auto rail resize only when its normalized value changed.
    ///
    /// Args:
    ///     requested: Raw logical-pixel width reported by one Shell resize state.
    ///     cx: Backend context used to notify every other open Shell.
    ///
    /// Returns:
    ///     `true` when the clamped preference changed; repeated equal samples are ignored.
    pub(crate) fn set_auto_workspace_rail_width(
        &mut self,
        requested: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(width) = crate::workspace::changed_auto_workspace_rail_width(
            self.auto_workspace_rail_width(),
            requested,
        ) else {
            return false;
        };
        self.layout.auto_workspace_rail_width = Some(width);
        self.layout_dirty = true;
        self.publish_auto_workspace_layout_revision(cx);
        true
    }

    /// Whether the core-settings gear opens the expert window instead of the compact popup.
    pub(crate) fn core_settings_expert(&self) -> bool {
        self.layout.core_settings_expert.unwrap_or(false)
    }

    /// Persist which face the core-settings gear opens.
    ///
    /// Only the shared backend is woken: both surfaces that read this observe it, and neither is on
    /// the frame path — the gear reads the flag when it is CLICKED, not while it renders. An
    /// unchanged value exits without touching layout state.
    pub(crate) fn set_core_settings_expert(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.core_settings_expert() == on {
            return;
        }
        self.layout.core_settings_expert = Some(on);
        self.layout_dirty = true;
        cx.notify();
    }

    /// Persist a group workspace mode and publish one effective-scope transition.
    ///
    /// Args:
    ///     group: Live configured group whose preset changes.
    ///     mode: New workspace preset.
    ///     cx: Backend context used to publish the dedicated revision.
    ///
    /// Returns:
    ///     `true` when mode or singleton ownership changed.
    pub(crate) fn set_workspace_mode(
        &mut self,
        group: &str,
        mode: WorkspaceMode,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.group_is_configured(group) {
            return false;
        }
        let mode_changed = self.workspace_mode(group) != mode;
        let focus_changed = match mode {
            WorkspaceMode::Classic => false,
            WorkspaceMode::AutoTrading => {
                crate::workspace::focus_workspace_owner(&mut self.workspace_focus, group)
            }
        };
        if !mode_changed && !focus_changed {
            return false;
        }
        if mode_changed {
            self.layout
                .workspace_mode_by_group
                .insert(group.to_string(), mode);
            self.layout_dirty = true;
        }
        self.publish_workspace_revision(cx);
        true
    }

    /// Ask every open group window to reset the current workspace mode's dock layout once.
    ///
    /// Settings has no handle on those windows, so the request rides the existing workspace
    /// revision channel. A generation (not a bool) is what lets each Shell compare-and-serve
    /// exactly once, including when several group windows are open.
    ///
    /// Args:
    ///     cx: Backend context used to publish the dedicated revision.
    ///
    /// Returns:
    ///     Nothing; each Shell that existed at the request serves it on its next reconcile.
    pub(crate) fn request_dock_layout_reset(&mut self, cx: &mut Context<Self>) {
        self.dock_layout_reset_generation = self.dock_layout_reset_generation.wrapping_add(1);
        self.publish_workspace_revision(cx);
    }

    /// Return the runtime dock-layout reset generation.
    ///
    /// Returns:
    ///     The current generation, including `0` when no reset has been requested this process.
    pub(crate) fn dock_layout_reset_generation(&self) -> u64 {
        self.dock_layout_reset_generation
    }

    /// Select one live core or Overview for an already active Auto workspace.
    ///
    /// Args:
    ///     group: Owning group window.
    ///     core: Live group core, or `None` for Overview.
    ///     cx: Backend context used to publish one revision.
    ///
    /// Returns:
    ///     `true` when persisted selection or singleton ownership changed.
    pub(crate) fn select_auto_workspace_core(
        &mut self,
        group: &str,
        core: Option<CoreId>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.workspace_mode(group) != WorkspaceMode::AutoTrading
            || core
                .is_some_and(|core| !self.workspace_core_availability(group, core).is_available())
        {
            return false;
        }
        let previous = self.layout.auto_workspace_core_by_group.get(group).copied();
        if let Some(core) = core {
            self.layout
                .auto_workspace_core_by_group
                .insert(group.to_string(), core);
        } else {
            self.layout.auto_workspace_core_by_group.remove(group);
        }
        let selection_changed = previous != core;
        let focus_changed =
            crate::workspace::focus_workspace_owner(&mut self.workspace_focus, group);
        if !selection_changed && !focus_changed {
            return false;
        }
        if selection_changed {
            self.layout_dirty = true;
        }
        self.publish_workspace_revision(cx);
        true
    }

    /// Enter Auto mode and select a destination core as one cross-group transition.
    ///
    /// Args:
    ///     group: Destination group whose existing window will be activated by the caller.
    ///     core: Live core in that destination group.
    ///     cx: Backend context used to publish one revision.
    ///
    /// Returns:
    ///     `true` when the validated transition changed state.
    pub(crate) fn activate_auto_workspace_core(
        &mut self,
        group: &str,
        core: CoreId,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.workspace_core_availability(group, core).is_available() {
            return false;
        }
        let mode_changed = self.workspace_mode(group) != WorkspaceMode::AutoTrading;
        let selection_changed = self.valid_auto_workspace_core(group) != Some(core);
        let focus_changed =
            crate::workspace::focus_workspace_owner(&mut self.workspace_focus, group);
        if !mode_changed && !selection_changed && !focus_changed {
            return false;
        }
        self.layout
            .workspace_mode_by_group
            .insert(group.to_string(), WorkspaceMode::AutoTrading);
        self.layout
            .auto_workspace_core_by_group
            .insert(group.to_string(), core);
        self.layout_dirty |= mode_changed || selection_changed;
        self.publish_workspace_revision(cx);
        true
    }

    /// Record a live group's window as the current singleton-tool owner.
    ///
    /// Any group whose window is live may own singleton scope now — this is the display-membership
    /// question (`Backend::display_preset(DisplayOwner::Singleton)`), independent of Auto's own
    /// selected-core question (`Backend::singleton_workspace`), which re-gates on
    /// `WorkspaceMode::AutoTrading` itself inside `workspace::resolve_singleton_workspace` and is
    /// therefore unaffected by widening this setter.
    ///
    /// Args:
    ///     group: Group whose toolbar or interaction established ownership.
    ///     cx: Backend context used to publish one revision.
    ///
    /// Returns:
    ///     `true` when focus moved to this group.
    pub(crate) fn focus_singleton_owner(&mut self, group: &str, cx: &mut Context<Self>) -> bool {
        if !self.group_windows.contains_key(group)
            || !crate::workspace::focus_workspace_owner(&mut self.workspace_focus, group)
        {
            return false;
        }
        self.publish_workspace_revision(cx);
        true
    }

    /// Remove one registered primary group window and publish its workspace transition once.
    ///
    /// Args:
    ///     closed_id: Native window identity reported by GPUI.
    ///     cx: Backend context used for the single workspace revision publication.
    ///
    /// Returns:
    ///     Closed group and whether it was the last primary window, or `None` for detached and
    ///     already-processed window identities.
    pub(crate) fn close_group_window(
        &mut self,
        closed_id: WindowId,
        cx: &mut Context<Self>,
    ) -> Option<(String, bool)> {
        let group = self
            .group_windows
            .iter()
            .find(|(_, handle)| handle.window_id() == closed_id)
            .map(|(group, _)| group.clone())?;
        self.group_windows.remove(&group)?;
        self.opening_group_windows.remove(&group);
        crate::workspace::close_workspace_owner(&mut self.workspace_focus, &group);
        self.publish_workspace_revision(cx);
        Some((group, self.group_windows.is_empty()))
    }

    /// Publish one final combined config and group-window lifecycle transition.
    ///
    /// Args:
    ///     closed_groups: Groups removed in this final transition; empty for a completed open.
    ///     cx: Backend context used to publish exactly one transition.
    ///
    /// Returns:
    ///     Nothing; ownership is reconciled once against final state before the notification.
    pub(crate) fn publish_workspace_window_change(
        &mut self,
        closed_groups: &[String],
        cx: &mut Context<Self>,
    ) {
        let focus_valid = self.workspace_focus.as_ref().is_none_or(|focus| {
            self.group_is_configured(focus.group())
                && !closed_groups.iter().any(|group| group == focus.group())
                && (self.group_windows.contains_key(focus.group())
                    || self.opening_group_windows.contains(focus.group()))
        });
        crate::workspace::reconcile_workspace_focus(&mut self.workspace_focus, focus_valid);
        self.publish_workspace_revision(cx);
    }

    /// Return whether a group still has an active configured window owner.
    ///
    /// Args:
    ///     group: Group name to validate against the committed configuration.
    ///
    /// Returns:
    ///     `true` when the group appears in the canonical group-window enumeration.
    fn group_is_configured(&self, group: &str) -> bool {
        crate::window::group_window::groups(&self.config)
            .iter()
            .any(|configured| configured == group)
    }

    /// Advance and notify the dedicated workspace revision entity.
    ///
    /// Args:
    ///     cx: Backend context whose app handle updates the revision entity.
    ///
    /// Returns:
    ///     Nothing; one call produces one generation increment and one notification.
    fn publish_workspace_revision(&mut self, cx: &mut Context<Self>) {
        self.workspace_revision
            .update(cx, |revision, revision_cx| revision.advance(revision_cx));
    }

    /// Publish one equality-guarded Auto topology or rail-width transition.
    ///
    /// Args:
    ///     cx: Backend context whose app handle updates the shared revision entity.
    ///
    /// Returns:
    ///     Nothing; callers must update their authority before publishing.
    fn publish_auto_workspace_layout_revision(&mut self, cx: &mut Context<Self>) {
        self.auto_workspace_layout_revision
            .update(cx, |revision, revision_cx| revision.advance(revision_cx));
    }
}
