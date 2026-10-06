//! Workspace presets, selected trading cores, and effective membership.

use crate::Backend;
use moon_core::config::WorkspaceMode;
use moon_core::session::CoreId;
use moon_core::session::core_order::CoreOrder;

/// Missing metadata remains visible; configured entries apply the viewing window's preset.
fn displays(
    server: Option<&moon_core::config::ServerConfig>,
    preset: Option<WorkspaceMode>,
) -> bool {
    preset.is_none_or(|preset| {
        server.is_none_or(|server| server.workspace_membership.displays_in(preset))
    })
}

/// Keep first-id/group lookups distinct from the first-id rule used by viewing presets.
fn server_identity(server: &moon_core::config::ServerConfig) -> (CoreId, &str) {
    (server.id, server.group.as_str())
}

/// Render-local membership snapshot preserving both first-id and first-id/group semantics.
pub(crate) struct WorkspaceLiveIndex<'a> {
    live: std::collections::HashSet<(CoreId, &'a str)>,
    first_server: std::collections::HashMap<(CoreId, &'a str), &'a moon_core::config::ServerConfig>,
    first_by_id: std::collections::HashMap<CoreId, &'a moon_core::config::ServerConfig>,
}

impl<'a> WorkspaceLiveIndex<'a> {
    /// Capture config and session membership once without copying configured secrets.
    pub(crate) fn new(
        servers: &'a [moon_core::config::ServerConfig],
        live: impl IntoIterator<Item = (CoreId, &'a str)>,
    ) -> Self {
        let mut first_server = std::collections::HashMap::with_capacity(servers.len());
        let mut first_by_id = std::collections::HashMap::with_capacity(servers.len());
        for server in servers {
            first_server
                .entry(server_identity(server))
                .or_insert(server);
            first_by_id.entry(server.id).or_insert(server);
        }
        Self {
            live: live.into_iter().collect(),
            first_server,
            first_by_id,
        }
    }

    /// Return the first configured entry for this identity in the requested group.
    pub(crate) fn server(
        &self,
        group: &str,
        core: CoreId,
    ) -> Option<&'a moon_core::config::ServerConfig> {
        self.first_server.get(&(core, group)).copied()
    }

    /// Test live membership independently of configured duplicates.
    pub(crate) fn live(&self, group: &str, core: CoreId) -> bool {
        self.live.contains(&(core, group))
    }

    /// Apply the viewing preset using the first configured identity across every group.
    pub(crate) fn displayed(&self, preset: Option<WorkspaceMode>, core: CoreId) -> bool {
        displays(self.first_by_id.get(&core).copied(), preset)
    }
}

impl Backend {
    /// Resolve one concrete trading address for commands and core-settings controls.
    ///
    /// A valid selected Auto workspace core takes precedence. Auto Overview and Classic then use
    /// the still-valid remembered header selection, followed by the visible chart target and the
    /// group's first displayed core. Every branch consults workspace-preset membership, so a core
    /// hidden by the current preset can never be armed even though it still connects and streams;
    /// display surfaces must additionally gate per-core figures with
    /// [`Self::is_auto_overview_scope`] before reading it.
    ///
    /// Args:
    ///     group: Window group whose command or core-settings control needs a concrete core.
    ///
    /// Returns:
    ///     A live, displayed core belonging to the group, or `None` when the group has no live
    ///     core the current preset displays.
    pub(crate) fn active_trade_core(&self, group: &str) -> Option<CoreId> {
        if self.workspace_mode(group) == WorkspaceMode::AutoTrading
            && let Some(core) = self.valid_auto_workspace_core(group)
        {
            return Some(core);
        }
        if let Some(&core) = self.layout.active_trade_core_by_group.get(group)
            && self.core_belongs_to_group(group, core)
            && self.core_displayed_in_group(group, core)
        {
            return Some(core);
        }
        self.main_chart_target(group)
            .map(|(core, _)| core)
            // A chart target is sticky and survives a settings save, so it needs the same
            // membership gate as the remembered selection above: without it a core hidden from
            // every surface would still be the one commands address.
            .filter(|core| self.core_displayed_in_group(group, *core))
            // Match the visible first core so the trade fallback and header selector agree; a
            // hidden group leaves nothing to fall back to.
            .or_else(|| {
                self.group_cores(group)
                    .iter()
                    .find(|pair| self.core_displayed_in_group(group, pair.0))
                    .map(|(id, _)| *id)
            })
    }

    /// Set the group's active trading core in the shared durable Classic layout.
    ///
    /// Args:
    ///     group: Window group that owns the selection.
    ///     core: Stable UID of the selected live core. Values outside `group` are ignored.
    ///
    /// Returns:
    ///     Nothing; Auto mode refuses this legacy writer, while a changed Classic selection marks
    ///     layout persistence dirty.
    pub(crate) fn set_active_trade_core(&mut self, group: &str, core: CoreId) {
        // The Auto Shell rail uses `select_auto_workspace_core`, whose revision wakes scoped
        // consumers. Refusing the legacy writer here is the final guard against chart/header paths
        // silently replacing the user's remembered Classic manual-trading core.
        if !crate::workspace::should_remember_classic_trade_core(self.workspace_mode(group)) {
            return;
        }
        if !self.core_belongs_to_group(group, core) {
            return;
        }
        if self.layout.active_trade_core_by_group.get(group) != Some(&core) {
            self.layout
                .active_trade_core_by_group
                .insert(group.to_string(), core);
            self.layout_dirty = true;
        }
    }

    /// Return the persisted workspace mode for one group, defaulting legacy layouts to Classic.
    ///
    /// Args:
    ///     group: Group window whose preset is requested.
    ///
    /// Returns:
    ///     The group's own saved mode, else the layout-wide default. First-run loading is what
    ///     seeds that default; a layout carrying none resolves to [`WorkspaceMode::Classic`],
    ///     which is every layout written before the preset existed. The fallback lives here
    ///     because this is the single read choke point every workspace-mode consumer goes through.
    pub(crate) fn workspace_mode(&self, group: &str) -> WorkspaceMode {
        self.layout
            .workspace_mode_by_group
            .get(group)
            .copied()
            .unwrap_or_else(|| self.layout.default_workspace_mode())
    }

    /// Return the raw persisted Auto top-tab preference for one group.
    ///
    /// Args:
    ///     group: Group window whose Auto preference is requested.
    ///
    /// Returns:
    ///     Saved stable panel name. The Shell validates eligibility and applies its safe fallback.
    pub(crate) fn auto_workspace_tab(&self, group: &str) -> Option<&str> {
        self.layout
            .auto_workspace_tab_by_group
            .get(group)
            .map(String::as_str)
    }

    /// Persist a Shell-validated eligible Auto top-tab name for one group.
    ///
    /// This preference changes neither effective workspace scope nor another live Shell, so it
    /// marks only `layout.toml` dirty and deliberately publishes no workspace revision.
    ///
    /// Args:
    ///     group: Owning Auto workspace group.
    ///     panel_name: Stable eligible panel name already validated by the Shell.
    ///
    /// Returns:
    ///     `true` only when the saved preference changed.
    pub(crate) fn set_auto_workspace_tab(&mut self, group: &str, panel_name: &str) -> bool {
        if self.auto_workspace_tab(group) == Some(panel_name) {
            return false;
        }
        self.layout
            .auto_workspace_tab_by_group
            .insert(group.to_string(), panel_name.to_string());
        self.layout_dirty = true;
        true
    }

    /// Return the shared Auto-workspace availability facts for one configured core.
    ///
    /// Args:
    ///     group: Owning group expected by the caller.
    ///     core: Stable core UID to resolve across config, session, and window lifecycle.
    ///
    /// Returns:
    ///     Complete availability record consumed by scope, setters, trade overlay, and roster.
    pub(crate) fn workspace_core_availability(
        &self,
        group: &str,
        core: CoreId,
    ) -> crate::workspace::WorkspaceCoreAvailability {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server_identity(server) == (core, group));
        self.workspace_core_availability_resolved(
            group,
            server,
            self.core_belongs_to_group(group, core),
        )
    }

    /// Compose availability from resolved first-group config and live-session membership.
    pub(crate) fn workspace_core_availability_resolved(
        &self,
        group: &str,
        server: Option<&moon_core::config::ServerConfig>,
        live_session: bool,
    ) -> crate::workspace::WorkspaceCoreAvailability {
        let window = if self.group_windows.contains_key(group) {
            crate::workspace::WorkspaceWindowState::Live
        } else if self.opening_group_windows.contains(group) {
            crate::workspace::WorkspaceWindowState::Opening
        } else {
            crate::workspace::WorkspaceWindowState::Missing
        };
        crate::workspace::WorkspaceCoreAvailability {
            // Missing group metadata means the configured server uses GroupConfig's active
            // defaults; requiring an explicit groups.toml row would disable legacy groups.
            group_active: self
                .config
                .group_ref(group)
                .is_none_or(|group| group.active),
            core_active: server.is_some_and(|server| server.active),
            live_session,
            window,
        }
    }

    /// The preset a surface displays UNDER — its own window's, never the core's group's.
    ///
    /// Membership is tested against the preset of the window DOING THE DISPLAYING, never against
    /// the preset of the core's own group (the H3 rule).
    ///
    /// Args:
    ///     owner: What kind of surface is asking — a group-owning window, or a singleton that
    ///         inherits the last group whose window was focused, in either preset.
    ///
    /// Returns:
    ///     The resolved preset, or `None` when a singleton has no live focus — meaning show
    ///     everything.
    pub(crate) fn display_preset(
        &self,
        owner: crate::workspace::DisplayOwner<'_>,
    ) -> Option<WorkspaceMode> {
        match owner {
            crate::workspace::DisplayOwner::Group(group) => Some(self.workspace_mode(group)),
            crate::workspace::DisplayOwner::Singleton => {
                let focus = self.workspace_focus.as_ref()?;
                let group = focus.group();
                let live = self.group_windows.contains_key(group)
                    || self.opening_group_windows.contains(group);
                live.then(|| self.workspace_mode(group))
            }
        }
    }

    /// Whether one configured core is displayed under a resolved preset.
    ///
    /// Args:
    ///     preset: Resolved viewing preset, or `None` for unscoped (show everything).
    ///     core: Core to test.
    ///
    /// Returns:
    ///     `true` when `preset` is `None`, when the core is absent from `self.config.servers` (an
    ///     unconfigured core cannot be hidden by a setting it does not have), or when its
    ///     `workspace_membership` displays in `preset`.
    pub(crate) fn core_displayed(&self, preset: Option<WorkspaceMode>, core: CoreId) -> bool {
        displays(
            self.config.servers.iter().find(|server| server.id == core),
            preset,
        )
    }

    /// Convenience for group-owning callers: `display_preset(Group(group))` + `core_displayed`.
    pub(crate) fn core_displayed_in_group(&self, group: &str, core: CoreId) -> bool {
        self.core_displayed(
            self.display_preset(crate::workspace::DisplayOwner::Group(group)),
            core,
        )
    }

    /// Return a saved Auto core only while it remains a live member of the owning group.
    ///
    /// Args:
    ///     group: Group whose Auto workspace selection is requested.
    ///
    /// Returns:
    ///     Valid selected core, or `None` for Overview and stale persisted references.
    pub(crate) fn valid_auto_workspace_core(&self, group: &str) -> Option<CoreId> {
        self.layout
            .auto_workspace_core_by_group
            .get(group)
            .copied()
            .filter(|core| {
                self.workspace_core_availability(group, *core)
                    .is_available()
            })
            .filter(|core| self.core_displayed_in_group(group, *core))
    }

    /// Return whether the group's visible scope names NO single core — the Auto Overview state.
    ///
    /// The chrome that prints a per-core figure — the header balance, the toolbar's leverage and
    /// the exchange max-order readout — asks THIS rather than [`Self::active_trade_core`], whose
    /// Overview fallback silently answers with the group's first core. See
    /// [`crate::workspace::is_auto_overview_scope`] for why the two must differ.
    ///
    /// Args:
    ///     group: Group whose header and toolbar are being rendered.
    ///
    /// Returns:
    ///     `true` only in Auto Overview, where no single core owns the visible scope.
    pub(crate) fn is_auto_overview_scope(&self, group: &str) -> bool {
        crate::workspace::is_auto_overview_scope(
            self.workspace_mode(group),
            self.valid_auto_workspace_core(group),
        )
    }

    /// Resolve one group panel's effective scope without mutating its retained Classic filter.
    ///
    /// Args:
    ///     group: Owning group window.
    ///     retained: Panel-owned Classic all/subset filter.
    ///
    /// Returns:
    ///     Canonical live core IDs selected by Classic, Auto Overview, or Auto selected-core mode.
    pub(crate) fn effective_workspace_scope(
        &self,
        group: &str,
        retained: crate::workspace::RetainedCoreScope<'_>,
    ) -> crate::workspace::EffectiveCoreScope {
        let available: Vec<CoreId> = self
            .group_cores(group)
            .into_iter()
            .map(|(core, _)| core)
            .filter(|core| {
                self.workspace_core_availability(group, *core)
                    .is_available()
            })
            .collect();
        let membership_total = available.len();
        let cores: Vec<CoreId> = available
            .into_iter()
            .filter(|core| self.core_displayed_in_group(group, *core))
            .collect();
        let membership_shown = cores.len();
        crate::workspace::resolve_group_scope(
            self.workspace_mode(group),
            self.valid_auto_workspace_core(group),
            &cores,
            retained,
        )
        .with_membership_counts(membership_shown, membership_total)
    }

    /// Resolve one group panel's fallback scope from its configured cores.
    ///
    /// The fallback universe for [`crate::workspace::EffectiveCoreScope::or_configured`] is built
    /// from configuration rather than [`Self::group_cores`].
    /// [`crate::workspace::WorkspaceCoreAvailability::is_configured_active`] deliberately ignores
    /// the `live_session` value that [`Self::workspace_core_availability`] still computes, so an
    /// offline group resolves to its own configured active cores instead of an empty
    /// session-derived universe.
    ///
    /// `valid_auto_workspace_core` stays session-derived even here: [`resolve_group_scope`]'s Auto
    /// branch and [`Self::is_auto_overview_scope`] state one rule in two places, and resolving a
    /// pinned-but-offline Auto core against config would make the Report show core X while the
    /// header still says Overview.
    ///
    /// Args:
    ///     group: Owning group window.
    ///     retained: Panel-owned Classic all/subset filter.
    ///
    /// Returns:
    ///     Canonical configured core IDs selected by Classic, Auto Overview, or Auto selected-core
    ///     mode.
    pub(crate) fn configured_workspace_scope(
        &self,
        group: &str,
        retained: crate::workspace::RetainedCoreScope<'_>,
    ) -> crate::workspace::EffectiveCoreScope {
        let mut available: Vec<CoreId> = self
            .config
            .servers
            .iter()
            .filter(|s| s.group == group)
            .map(|s| s.id)
            .collect();
        CoreOrder::new(&self.config).sort_by(&mut available, |id| *id);
        let available: Vec<CoreId> = available
            .into_iter()
            .filter(|core| {
                self.workspace_core_availability(group, *core)
                    .is_configured_active()
            })
            .collect();
        let membership_total = available.len();
        let cores: Vec<CoreId> = available
            .into_iter()
            .filter(|core| self.core_displayed_in_group(group, *core))
            .collect();
        let membership_shown = cores.len();
        crate::workspace::resolve_group_scope(
            self.workspace_mode(group),
            self.valid_auto_workspace_core(group),
            &cores,
            retained,
        )
        .with_membership_counts(membership_shown, membership_total)
    }
}
