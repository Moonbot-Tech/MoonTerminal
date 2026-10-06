//! Deferred workspace window reconciliation and topology guard transitions.

use super::*;

impl Shell {
    /// Start the blocking exchange-logo cache prewarm off-thread exactly once for this Shell.
    ///
    /// Args:
    ///     cx: Shell context used to spawn work and publish the ready edge on the UI executor.
    ///
    /// Returns:
    ///     Nothing; completion updates only a live weak Shell and requests one repaint.
    pub(super) fn start_exchange_logo_prewarm(&mut self, cx: &mut Context<Self>) {
        if self.exchange_logo_prewarm_started {
            return;
        }
        self.exchange_logo_prewarm_started = true;
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async { crate::media::exchange_logos::prewarm() })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.exchange_logos_ready = true;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Begin one programmatic Auto topology transition and return its guard generation.
    ///
    /// Returns:
    ///     A generation token that only its deferred completion may release.
    pub(super) fn begin_auto_topology_application(&mut self) -> u64 {
        self.auto_topology_guard_generation = self.auto_topology_guard_generation.wrapping_add(1);
        self.applying_auto_topology = true;
        self.auto_topology_guard_generation
    }

    /// Keep the current topology guard through queued Dock events, then release it weakly.
    ///
    /// Args:
    ///     generation: Token returned by [`Self::begin_auto_topology_application`].
    ///     cx: Shell context used to enqueue work after dock-event effects.
    ///
    /// Returns:
    ///     Nothing; a newer generation or closed Shell makes the callback a no-op.
    pub(super) fn finish_auto_topology_application(
        &mut self,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let shell = cx.entity().downgrade();
        defer_auto_topology_guard_release(shell, generation, cx);
    }

    /// Schedule one window-aware reconciliation of persisted mode and addressed chart requests.
    ///
    /// Args:
    ///     cx: Shell context used to defer through the owning native window.
    ///
    /// Returns:
    ///     Nothing; repeated notifications coalesce while one reconciliation is queued.
    pub(in crate::shell) fn defer_workspace_window_sync(&mut self, cx: &mut Context<Self>) {
        if self.workspace_sync_pending {
            return;
        }
        self.workspace_sync_pending = true;
        let shell = cx.entity().downgrade();
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                let _ = shell.update(app, |this, cx| {
                    this.workspace_sync_pending = false;
                    this.reconcile_workspace_window(window, cx);
                });
            });
        });
    }

    /// Apply workspace state and reveal the latest ordered group-local Auto surface when required.
    ///
    /// Args:
    ///     window: Owning group window required by the live DockArea APIs.
    ///     cx: Shell context used to read Backend and update the dock.
    ///
    /// Returns:
    ///     Nothing; Classic observes surface revisions but never changes dock activation for them.
    fn reconcile_workspace_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (mode, surface_request) = {
            let backend = self.backend.read(cx);
            (
                backend.workspace_mode(&self.group),
                backend.auto_workspace_surface_request(&self.group),
            )
        };
        let surface = crate::workspace::resolve_auto_workspace_surface(
            mode,
            &mut self.last_auto_surface_revision,
            surface_request,
        );
        self.apply_workspace_mode(mode, window, cx);
        self.drain_dock_layout_reset(window, cx);
        self.sync_auto_dock_topology(surface.map(|s| s.panel_name()), window, cx);
        self.sync_auto_rail_width(window, cx);

        if let Some(surface) = surface {
            self.dock.update(cx, |dock, cx| {
                dock.activate_panel_by_name(surface.panel_name(), window, cx);
            });
        }
    }

    /// Serve one in-app dock-layout reset for this Shell's currently applied workspace mode.
    ///
    /// Auto only rewrites the shared topology authority; [`Self::sync_auto_dock_topology`] then
    /// applies it. Classic rebuilds the default centre from live instances. A Classic layout
    /// retained while Auto is showing is left alone.
    ///
    /// Args:
    ///     window: Owning group window required by Classic dock reconstruction.
    ///     cx: Shell context used to read the generation and update dock or Backend.
    ///
    /// Returns:
    ///     Nothing; an already-served generation is a no-op.
    fn drain_dock_layout_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let generation = self.backend.read(cx).dock_layout_reset_generation();
        if self.served_dock_layout_reset == generation {
            return;
        }
        self.served_dock_layout_reset = generation;
        self.dock.update(cx, |dock, dock_cx| {
            dock.clear_zoom(window, dock_cx);
        });
        if self.applied_workspace_mode == WorkspaceMode::AutoTrading {
            self.backend.update(cx, |backend, backend_cx| {
                backend.reset_auto_dock_topology(default_auto_workspace_topology(), backend_cx);
            });
        } else {
            self.reset_classic_dock_layout(window, cx);
        }
    }
}
