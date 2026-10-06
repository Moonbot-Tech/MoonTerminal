//! Chart panel lifecycle operations.

use super::*;

impl ChartPanel {
    /// Where and when a click last closed a chart anywhere in the process, for `render_input`.
    pub(in crate::panels) fn backend_close_mark(&self, cx: &App) -> Option<(f64, (f32, f32))> {
        self.backend.read(cx).last_chart_close
    }

    /// Move the close mark onto a press that turned out to belong to that closing.
    ///
    /// Keeps a drifting stab sequence anchored where it actually is now rather than at the pixel
    /// the first × sat at. The mark's TIME is deliberately left alone: refreshing it on every
    /// rejected press would restart the window each time and make a spot the user keeps clicking
    /// untradeable for as long as they keep trying.
    pub(in crate::panels) fn mark_close_residue(
        &mut self,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        self.backend.update(cx, |b, _| {
            if let Some((at_ms, _)) = b.last_chart_close {
                b.last_chart_close = Some((at_ms, pos));
            }
        });
    }

    /// Mark on the shared backend where a click just closed a chart, so no panel trades on the rest
    /// of the presses that click belongs to.
    ///
    /// The mark is deliberately global: the chart those presses reach is never the one that closed
    /// — the stack drops the closed panel and walks the next one under the cursor. It is placed at
    /// the press that caused this close, so a close no press explains leaves nothing behind and
    /// blocks nothing. Written without a notify; nothing renders from it.
    pub(super) fn note_chart_closed(&mut self, cx: &mut Context<Self>) {
        let now = now_unix_ms();
        let Some(pos) = self.click_series.fresh_press_pos(now) else {
            return;
        };
        self.backend.update(cx, |b, _| {
            b.last_chart_close = Some((now, pos));
        });
    }

    /// Closes a pane through its close button. When no remaining pane uses its market, release this
    /// panel's market ownership and order-book demand; backend refcounts decide whether the market
    /// remains desired by another panel.
    pub(super) fn remove_pane(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some((core, market)) = self.chart.remove_pane(idx) else {
            return;
        };
        self.note_chart_closed(cx);
        self.view_dirty = true;
        if !self.chart.uses_market(core, &market) {
            self.release_market_ref(core, &market, cx);
        }
        self.clear_history_target_if_unused(cx);
        cx.notify();
    }

    /// Toggles a pane's pin. Pinning exempts it from TTL auto-close; unpinning restores its deadline,
    /// which may already have elapsed, so the deadline timer is re-armed.
    pub(super) fn toggle_pin(&mut self, idx: usize, cx: &mut Context<Self>) {
        if self.chart.toggle_pane_pin(idx) {
            self.view_dirty = true;
            self.arm_ttl_timer(cx);
            cx.notify();
        }
    }

    /// Closes every market in this panel and releases their market and order-book references.
    pub fn close_all_panes(&mut self, cx: &mut Context<Self>) {
        let removed = self.chart.clear_panes();
        if removed.is_empty() {
            return;
        }
        self.view_dirty = true;
        for (core, market) in removed {
            self.release_market_ref(core, &market, cx);
        }
        self.clear_history_target_if_unused(cx);
        cx.notify();
    }

    pub(super) fn mark_input_changed(&mut self, cx: &mut Context<Self>) {
        self.chart.sync_follow_from_views();
        // A historical viewer PUBLISHES nothing: the application-wide Live flag describes the live
        // charts, and this window has no live edge to have left. Without the guard, panning inside
        // a trade window switched the MAIN chart's Live off, because this pane is permanently
        // manual by construction.
        if !self.historical {
            let follow = self.chart.follow();
            let now = now_unix_ms();
            self.backend.update(cx, |b, bcx| {
                b.last_live_chart_interaction_ms = Some(now);
                if b.follow != follow {
                    b.follow = follow;
                    if follow {
                        b.follow_persistent = false;
                    }
                    bcx.notify();
                }
            });
        }
        self.camera_dirty = true;
    }

    /// Returns the tab caption. Numbered AddToChart and Custom panels use `N · market`, falling back
    /// to the localized numbered label; Main uses the active market and then `Main`. Group and core
    /// belong to ChartTabs or DetachedChartHost and are not available here.
    pub fn title_text(&self) -> String {
        let market = self
            .chart
            .active_market()
            .filter(|m| !m.is_empty())
            .or_else(|| self.market.clone());
        if let Some(n) = self.num {
            return match market {
                Some(m) => format!("{n} · {m}"),
                None => t!("chartwin.tab_title", n = n).to_string(),
            };
        }
        market.unwrap_or_else(|| "Main".into())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub fn debug_fill_history_to_capacity(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((core, market)) = self.chart.active_target() else {
            log::warn!("debug history fill: current chart has no active market");
            return false;
        };
        let now_ms = now_unix_ms();
        log::info!(
            "debug history fill: requesting core={} market={market} span_ms={DEBUG_HISTORY_FILL_SPAN_MS}",
            moon_core::feed::core_label(core)
        );
        let filled = self
            .backend
            .read(cx)
            .session
            .diag_fill_market_history_to_capacity(
                core,
                &market,
                now_ms.round() as i64,
                DEBUG_HISTORY_FILL_SPAN_MS,
            );
        if !filled {
            log::warn!(
                "debug history fill: failed core={} market={market}",
                moon_core::feed::core_label(core)
            );
            return false;
        }
        self.chart.force_history_reupload();
        self.view_dirty = true;
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
        log::info!(
            "debug history fill: force reupload core={} market={market}",
            moon_core::feed::core_label(core)
        );
        true
    }
}
