//! Main chart stack focus operations.

use super::*;

impl MainChartStack {
    pub(crate) fn active_target(&self, cx: &App) -> Option<(CoreId, String)> {
        self.active
            .and_then(|ix| self.charts.get(ix))
            .and_then(|entry| entry.panel.read(cx).active_target())
    }

    /// Focus handle of the chart currently selected in this stack.
    ///
    /// Exists so an opened chart can be made the keyboard target. Escape is a WINDOW-root hotkey
    /// (`Shell::on_hotkey`), and it only reaches that root when focus sits somewhere inside the
    /// window — so a chart opened while focus is still in the table that opened it receives no
    /// keystrokes at all until the user clicks it.
    ///
    /// Args:
    ///     cx: Application context used to read the selected panel.
    ///
    /// Returns:
    ///     The selected chart's focus handle, or `None` when the stack has no selection.
    pub(crate) fn active_focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        self.active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| entry.panel.read(cx).focus_handle(cx))
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_data_handle(&self, cx: &App) -> Option<crate::chartdx::ChartDataHandle> {
        self.active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| entry.panel.read(cx).debug_data_handle())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_fill_history_to_capacity(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.active else {
            log::warn!("debug fill main chart: no active main chart");
            return false;
        };
        let Some(entry) = self.charts.get(ix) else {
            log::warn!("debug fill main chart: active main chart index is stale");
            return false;
        };
        entry
            .panel
            .update(cx, |panel, pcx| panel.debug_fill_history_to_capacity(pcx))
    }

    /// Record whether the host is showing this stack, and propagate a real change.
    ///
    /// An unchanged value returns immediately. Child prune and layout already update local
    /// visibility, and repeating the hide or reveal on every backend observation would reset
    /// virtual-list children.
    ///
    /// Args:
    ///     visible: Whether the host currently presents this stack.
    ///     cx: Stack context used to update child panels.
    ///
    /// Returns:
    ///     Nothing; an unchanged host flag leaves child visibility untouched. A reveal reuses
    ///     [`Self::sync_visibility`]. A hide forces every child off and clears stack-scroll mode.
    pub(in crate::chart_tabs) fn set_scene_visible(
        &mut self,
        visible: bool,
        cx: &mut Context<Self>,
    ) {
        if self.host_visible == visible {
            return;
        }
        self.host_visible = visible;
        if visible {
            self.sync_visibility(cx);
        } else {
            for entry in &self.charts {
                entry.panel.update(cx, |panel, _| {
                    panel.set_main_stack_scroll(false);
                    panel.set_scene_visible(false);
                });
            }
        }
    }

    /// Push fullscreen or stack visibility down to every child, gated by the host.
    ///
    /// Args:
    ///     cx: Stack context used to update child panels.
    ///
    /// Returns:
    ///     Nothing. A hidden host forces every child off.
    pub(super) fn sync_visibility(&mut self, cx: &mut Context<Self>) {
        for (ix, entry) in self.charts.iter().enumerate() {
            // In fullscreen, only the active chart is visible. In stack mode, visible tiles set
            // themselves visible in `ChartPanel::render`; offscreen virtual-list entries remain
            // hidden and do not run preparation work. `host_visible` keeps a hidden dock or an
            // inactive Main tab from being turned back on by prune, open, or close.
            let visible = self.host_visible && !self.show_stack && Some(ix) == self.active;
            let stack_scroll = self.show_stack;
            entry.panel.update(cx, |panel, _| {
                panel.set_main_stack_scroll(stack_scroll);
                panel.set_scene_visible(visible);
            });
        }
    }

    /// Apply one virtual-list window to stack-mode children, gated by the host.
    ///
    /// Args:
    ///     range: Display indexes the virtual list currently paints.
    ///     cx: Stack context used to update child panels.
    ///
    /// Returns:
    ///     Nothing. Fullscreen ignores the window. A hidden host forces every child in the
    ///     window off.
    pub(super) fn sync_stack_visible_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        if !self.show_stack {
            return;
        }
        for (ix, entry) in self.charts.iter().enumerate() {
            let visible = self.host_visible && range.contains(&ix);
            entry.panel.update(cx, |panel, _| {
                panel.set_main_stack_scroll(true);
                panel.set_scene_visible(visible);
            });
        }
    }

    /// Publish the non-vacant markets currently open in this Main stack.
    ///
    /// `ChartTabs` separately owns the anchor-aware trading target, so this child stack must not
    /// publish or persist a competing target.
    ///
    /// Args:
    ///     cx: Stack context used to update the shared Backend.
    ///
    /// Returns:
    ///     Nothing; the Backend's group market list is replaced.
    pub(super) fn sync_backend_open_markets(&self, cx: &mut Context<Self>) {
        // Publish all non-vacant Main-stack markets so Orders can highlight one row for each.
        let open: Vec<(CoreId, String)> = self
            .charts
            .iter()
            .filter(|e| !e.vacated)
            .map(|e| (e.core, e.market.clone()))
            .collect();
        self.backend
            .update(cx, |b, _| b.set_main_open_markets(&self.group, open));
    }

    /// Republish everything derived from which charts exist and which one is selected.
    ///
    /// Keep these steps together because panel visibility follows the selection, Orders reads the
    /// published open-market list, and the parent only learns of either through the notification.
    ///
    /// Args:
    ///     cx: Stack context used to update child visibility, Backend state, and observers.
    ///
    /// Returns:
    ///     Nothing; all derived state is synchronized before notification.
    pub(super) fn publish(&mut self, cx: &mut Context<Self>) {
        self.sync_visibility(cx);
        self.sync_backend_open_markets(cx);
        cx.notify();
    }

    /// Select one chart, optionally showing it alone.
    ///
    /// Args:
    ///     key: Core and market of the chart to select.
    ///     fullscreen: Whether to leave stack presentation and show it full bleed.
    ///     cx: Stack context used to publish the new state.
    ///
    /// Returns:
    ///     Whether anything changed.
    pub(super) fn select_market(
        &mut self,
        key: &(CoreId, String),
        fullscreen: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ix) = self.index_of(key) else {
            return false;
        };
        if self.active == Some(ix) && !(fullscreen && self.show_stack) {
            return false;
        }
        self.active = Some(ix);
        if fullscreen {
            self.show_stack = false;
        }
        self.publish(cx);
        true
    }

    /// Make one chart the active one without changing presentation mode.
    ///
    /// The active chart is also the group's trading target — `ChartTabs::main_chart_target` reads
    /// it — so picking a tab redirects the F1-F6 / S1-S6 hotkeys and cancel-buy to that market.
    /// That is the point of the row, but it means a stray click moves where an order would land.
    ///
    /// The comparison anchor deliberately does NOT follow because it leads the shared price scale,
    /// while Main's trading target follows the selected chart. The two markers therefore represent
    /// separate responsibilities.
    ///
    /// Args:
    ///     key: Core and market of the chart to select.
    ///     cx: Stack context used to publish visibility and wake the parent.
    ///
    /// Returns:
    ///     Nothing; a missing or already-selected market leaves state unchanged.
    pub(super) fn focus_market(&mut self, key: &(CoreId, String), cx: &mut Context<Self>) {
        self.select_market(key, false, cx);
    }

    /// Select one chart and show it alone, full bleed — the tab row's double-click.
    ///
    /// Args:
    ///     key: Core and market of the chart to show.
    ///     cx: Stack context used to publish visibility and wake the parent.
    ///
    /// Returns:
    ///     Nothing; a missing market leaves state unchanged.
    pub(super) fn fullscreen_market(&mut self, key: &(CoreId, String), cx: &mut Context<Self>) {
        self.select_market(key, true, cx);
    }

    /// Resolve a chart's identity to its CURRENT index, or `None` once it is gone.
    ///
    /// Every tab-row handler goes through this. A handler fires after the render that built it, and
    /// in between an idle chart can expire or a comparison lock can move its anchor to the front —
    /// both of which renumber the stack while leaving the old index perfectly in range, pointing at
    /// somebody else's market.
    pub(super) fn index_of(&self, key: &(CoreId, String)) -> Option<usize> {
        self.charts.iter().position(|entry| entry.is(key))
    }

    /// Toggle one clicked chart between fullscreen and whole-stack presentation.
    ///
    /// With a single chart open the gesture is deliberately inert — see [`stack_toggle_target`].
    ///
    /// Args:
    ///     ix: Index of the clicked chart.
    ///     cx: Stack context used to update visibility and notify the parent.
    ///
    /// Returns:
    ///     Nothing; an out-of-range index, or a gesture with nothing to change, is ignored.
    pub(super) fn toggle_from_chart(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.charts.len() {
            return;
        }
        let Some(show_stack) = stack_toggle_target(self.show_stack, self.charts.len()) else {
            return;
        };
        self.active = Some(ix);
        self.show_stack = show_stack;
        self.publish(cx);
    }
}
