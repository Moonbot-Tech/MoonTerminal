//! Main chart stack focus operations.

use super::*;

impl MainChartStack {
    /// Exercise the real Main stack with two synthetic markets backed by the same generated history.
    pub(crate) fn firetest_dense_pair(
        &mut self,
        core: CoreId,
        cx: &mut Context<Self>,
    ) -> [crate::chartdx::ChartDataHandle; 2] {
        use moon_core::fixture::dense::{CAPACITY, MARKETS};
        use moon_core::market::{CandleViewCfg, candles::CANDLE_ZONE_MAX};
        let handles = MARKETS.map(|market| {
            self.open_or_focus(
                core,
                market.to_string(),
                crate::backend::ChartHistoryScope::Default,
                cx,
            );
            let entry = &self.charts[self.active.expect("opened Main chart")];
            entry.panel.update(cx, |panel, cx| {
                let mut graphics = panel.effective_chart_graphics(cx);
                graphics.hvol_enabled = true;
                graphics.candle_volume_sides = true;
                panel.set_chart_graphics(Some(graphics), cx);
                panel.set_candle_view(
                    Some(CandleViewCfg {
                        tf_min: 1,
                        trade_candles: if market == MARKETS[1] {
                            CANDLE_ZONE_MAX
                        } else {
                            0
                        },
                        hide_candles: 0,
                        trades_limit: CAPACITY as u32,
                        ..CandleViewCfg::default()
                    }),
                    cx,
                );
                panel.debug_data_handle()
            })
        });
        self.set_layout_columns(Some(1), Some(true), None, cx);
        self.toggle_from_chart(self.active.expect("opened Main chart"), cx);
        handles
    }

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

    /// Hand the keyboard to the full-bleed chart when a chart this frame no longer draws holds it.
    ///
    /// Fullscreen draws ONE chart, yet every other chart's panel stays alive in `charts`. Once the
    /// shown chart changes without a click — the `switch_charts` hotkey, a detect retargeting Main —
    /// the chart that was clicked last keeps the focus while its element leaves the frame. Its handle
    /// still resolves, so the window reads as focused, but GPUI finds no node for it and dispatches
    /// every key to the bare root: no window-root listener runs, and every hotkey (the switch key
    /// itself included) stays dead until the user clicks a chart again. `restore_root_focus` cannot
    /// see this — it acts only when nothing holds focus — so the host that stops drawing the element
    /// is the one that must move the focus, as `hotkeys::restore_root_focus` documents.
    ///
    /// Called from `render`, the first thing to run after the change, and before the body is built,
    /// so the frame that paints the new chart is also the one whose dispatch path holds it. It only
    /// moves a focus that was already inside one of this stack's hidden charts, so it never takes the
    /// keyboard from a field elsewhere. Stack mode is not decided here: which tiles it draws is
    /// known only after layout, so [`Self::hand_focus_into_range`] repairs it from the virtual
    /// list's range report.
    ///
    /// Args:
    ///     window: Window whose focus may be moved.
    ///     cx: Application context used to read the panels' focus handles.
    pub(super) fn hand_focus_to_shown_chart(&self, window: &mut Window, cx: &mut App) {
        if self.show_stack || self.charts.is_empty() {
            return;
        }
        // The same index `render_body` draws, so the target is the chart actually on screen.
        let shown = self.active.unwrap_or(0).min(self.charts.len() - 1);
        self.rescue_stranded_focus(|ix| ix == shown, shown, window, cx);
    }

    /// The stack-mode half of [`Self::hand_focus_to_shown_chart`]: a focused tile the virtual list
    /// scrolled out of `range` stops being drawn while its panel lives on, which strands the window
    /// keyboard exactly as a hidden fullscreen chart does.
    ///
    /// Runs from the list's `on_visible_range`, the one place that knows which tiles this frame
    /// draws; the list contract names evicting focus outside the range as that observer's use. The
    /// keyboard goes to the active chart when it is drawn, otherwise to the first drawn tile.
    ///
    /// Args:
    ///     range: Chart indexes the virtual list draws this frame.
    ///     window: Window whose focus may be moved.
    ///     cx: Application context used to read the panels' focus handles.
    pub(super) fn hand_focus_into_range(
        &self,
        range: &Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if !self.show_stack || range.is_empty() {
            return;
        }
        let target = self
            .active
            .filter(|ix| range.contains(ix))
            .unwrap_or(range.start);
        self.rescue_stranded_focus(|ix| range.contains(&ix), target, window, cx);
    }

    /// Move the focus to chart `target` when it sits inside a chart for which `drawn` is false.
    ///
    /// Only a focus already inside one of this stack's undrawn charts is moved, so a field anywhere
    /// else keeps the keyboard. `contains_focused` also covers an input inside the chart; it reads
    /// the PREVIOUS frame, the one that still drew the chart now leaving, and a chart's own handle
    /// matches whatever that frame held.
    ///
    /// Args:
    ///     drawn: Whether the chart at an index is drawn this frame.
    ///     target: Index of the drawn chart that takes the keyboard.
    ///     window: Window whose focus may be moved.
    ///     cx: Application context used to read the panels' focus handles.
    fn rescue_stranded_focus(
        &self,
        drawn: impl Fn(usize) -> bool,
        target: usize,
        window: &mut Window,
        cx: &mut App,
    ) {
        if window.focused(cx).is_none() {
            return;
        }
        let stranded = self.charts.iter().enumerate().any(|(ix, entry)| {
            !drawn(ix)
                && entry
                    .panel
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
        });
        if stranded && let Some(entry) = self.charts.get(target) {
            let focus = entry.panel.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
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
