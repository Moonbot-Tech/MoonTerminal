//! Main chart stack close operations.

use super::*;

impl MainChartStack {
    /// Arm the one-shot inactivity auto-close timer unless it is already armed.
    /// It ticks at about 1 Hz while `main_idle_close_secs` is positive and charts exist, then rearms
    /// itself in the callback. Chart open/focus events and the timer callback start it, not render.
    pub(super) fn arm_idle_timer(&mut self, cx: &mut Context<Self>) {
        if self.idle_timer_armed
            || self.charts.is_empty()
            || self.backend.read(cx).main_idle_close_secs() == 0
        {
            return;
        }
        self.idle_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(Duration::from_secs(1)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.idle_timer_armed = false;
                    this.prune_idle(cx);
                    this.arm_idle_timer(cx);
                })
                .is_ok()
            });
        })
        .detach();
    }

    /// Close the active Main chart for Escape in either fullscreen or tiled-stack presentation.
    /// Remaining charts return to stack view with the chart that replaced the removed slot active.
    ///
    /// Args:
    ///     cx: Stack context used to close the panel and publish the new visible/open state.
    ///
    /// Returns:
    ///     Whether a chart closed.
    pub(crate) fn close_active(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(idx) =
            take_active_close_index(&mut self.active, &mut self.show_stack, self.charts.len())
        else {
            return false;
        };
        self.disarm_sells_zone_on_user_close(cx);
        // Escape has already decided what stays active and that the remainder returns to stack
        // view; this only performs the removal.
        self.remove_chart_at(idx, cx);
        self.publish(cx);
        true
    }

    /// Leave the Sells-to-zone mode because the USER closed a chart.
    ///
    /// Deliberately not inside `remove_chart_at`: that teardown also runs for the inactivity TTL
    /// and for an Auto-rail retarget, and a background timer closing some other chart must not
    /// cancel a band the user is in the middle of drawing on this one. The mode is global and its
    /// only marks are the crosshair badge and the tool picker, so one armed on a chart the user
    /// just closed would otherwise land on whatever chart is opened next.
    pub(super) fn disarm_sells_zone_on_user_close(&mut self, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            if b.sells_zone_armed() {
                b.disarm_sells_zone();
                bcx.notify();
            }
        });
    }

    /// Remove one chart and release everything hanging off it, deciding nothing about what is
    /// active afterwards.
    ///
    /// All single-chart close paths use this teardown so they release the panel's panes and
    /// order-book subscriptions, plus any comparison lock the removed chart anchored. Selection
    /// remains a caller decision because Escape and the tab-row close button choose differently.
    ///
    /// Args:
    ///     idx: Index of the chart to remove; assumed in range.
    ///     cx: Stack context used to close the panel and refresh comparison peers.
    ///
    /// Returns:
    ///     Nothing; selection and publication remain the caller's responsibility.
    pub(super) fn remove_chart_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        let entry = self.charts.remove(idx);
        entry.panel.update(cx, |p, pcx| p.close_all_panes(pcx));
        let had_compare_anchor = self.compare_anchor.is_some();
        if self
            .compare_anchor
            .as_ref()
            .is_some_and(|key| key.0 == entry.core && key.1 == entry.market)
        {
            self.compare_anchor = None;
            self.compare_orderbook_only = false;
            self.compare_y = None;
        }
        if had_compare_anchor {
            // Refresh comparison peers and clear any locks left by a removed anchor.
            self.sync_compare(cx);
        }
    }

    /// Close one chart chosen from the tab row, keeping the selection where the user left it.
    ///
    /// Unlike Escape this does not change presentation mode: closing a sibling from the row is not
    /// a request to leave fullscreen. The chart that stays active is tracked by IDENTITY rather
    /// than by index, because removing an earlier entry shifts every index after it — following
    /// the number would silently select a different market.
    ///
    /// Args:
    ///     idx: Index of the chart to close.
    ///     cx: Stack context used to release the chart and publish the new state.
    ///
    /// Returns:
    ///     Whether a chart closed.
    pub(super) fn close_at(&mut self, idx: usize, cx: &mut Context<Self>) -> bool {
        if idx >= self.charts.len() {
            return false;
        }
        // The identity to keep active — unless it is the one going away, in which case the numeric
        // fallback picks up whatever slid into its place.
        let keep = self
            .active
            .filter(|&active| active != idx)
            .and_then(|active| self.charts.get(active))
            .map(|entry| (entry.core, entry.market.clone()));
        let fallback = self.active;
        self.disarm_sells_zone_on_user_close(cx);
        self.remove_chart_at(idx, cx);
        let charts = &self.charts;
        self.active = remap_active_index(charts.len(), keep.as_ref(), fallback, |ix, key| {
            charts[ix].is(key)
        });
        if self.charts.is_empty() {
            self.show_stack = false;
        }
        self.publish(cx);
        true
    }

    /// Close every chart in the Main stack for the built-in Shift+Escape action.
    /// This releases each panel's order-book subscriptions and clears the stack. Returns whether
    /// anything was available to close.
    pub(crate) fn close_all(&mut self, cx: &mut Context<Self>) -> bool {
        if self.charts.is_empty() {
            return false;
        }
        self.disarm_sells_zone_on_user_close(cx);
        for entry in self.charts.drain(..) {
            entry.panel.update(cx, |p, pcx| p.close_all_panes(pcx));
        }
        self.active = None;
        self.show_stack = false;
        // The anchor left with the charts. Left set, it names a market that no longer exists, and
        // the next charts opened here are classified as its followers — locked to a price window
        // and, in broom mode, reduced to order books — with no leader on screen to explain it.
        self.compare_anchor = None;
        self.compare_y = None;
        self.compare_orderbook_only = false;
        // Orders highlights and prioritizes rows for the markets Main has open. Nothing else writes
        // that list, so without this it keeps pointing at charts that are gone.
        self.sync_backend_open_markets(cx);
        cx.notify();
        true
    }

    /// Auto-close unpinned charts after the configured window inactivity interval in seconds.
    /// Each deadline is `max(last_input, arrived_at) + interval`, with `arrived_at` used when no
    /// input exists. Closing the active fullscreen chart returns the remainder to stack view and
    /// immediately releases its order-book subscriptions. A focused detached chart window refreshes
    /// group activity because its input does not reach Main directly.
    ///
    /// Args:
    ///     cx: Stack context used to inspect windows, close panels, and publish open markets.
    ///
    /// Returns:
    ///     `true` when at least one idle chart was removed.
    pub(super) fn prune_idle(&mut self, cx: &mut Context<Self>) -> bool {
        let secs = self.backend.read(cx).main_idle_close_secs();
        if secs == 0 || self.charts.is_empty() {
            return false;
        }
        // Keep Main charts open while any detached chart window in this group is focused. Activity
        // in that separate OS window does not produce mouse movement in Main, so refresh the group
        // timestamp and grant Main a full TTL after the detached window loses focus.
        let group = self.group.clone();
        let chart_handles: Vec<_> = self
            .backend
            .read(cx)
            .detached_chart_windows
            .iter()
            .filter(|(g, _)| *g == group)
            .map(|(_, h)| *h)
            .collect();
        let chart_focused = chart_handles
            .into_iter()
            .any(|h| h.is_active(cx).unwrap_or(false));
        if chart_focused {
            self.backend.update(cx, |b, bcx| {
                b.note_main_input(&group);
                b.focus_singleton_owner(&group, bcx);
            });
            return false;
        }
        let ttl = Duration::from_secs(secs as u64);
        let last_input = self.backend.read(cx).main_input_at(&self.group);
        let now = Instant::now();
        // Inactivity does not close pinned charts, matching `prune_ttl`. Read pin flags through
        // `cx` before filtering to avoid borrowing panels during mutation.
        let pinned: Vec<bool> = self
            .charts
            .iter()
            .map(|e| e.panel.read(cx).is_pinned())
            .collect();
        let expired: Vec<usize> = self
            .charts
            .iter()
            .enumerate()
            .filter(|(ix, e)| {
                if pinned[*ix] {
                    return false;
                }
                let base = match last_input {
                    Some(t) => t.max(e.arrived_at),
                    None => e.arrived_at,
                };
                now.duration_since(base) >= ttl
            })
            .map(|(ix, _)| ix)
            .collect();
        if expired.is_empty() {
            return false;
        }
        let active_closed = self.active.is_some_and(|a| expired.contains(&a));
        // Track the surviving selection by IDENTITY. A numeric index may shift when an earlier
        // chart expires and would then highlight a different market and redirect trading hotkeys.
        let keep = self
            .active
            .filter(|active| !expired.contains(active))
            .and_then(|active| self.charts.get(active))
            .map(|entry| (entry.core, entry.market.clone()));
        let fallback = self.active;
        // Remove from the end to preserve indices. Routed through the shared removal so an expiring
        // comparison anchor tears its lock down with it, exactly as closing one by hand does.
        for &ix in expired.iter().rev() {
            self.remove_chart_at(ix, cx);
        }
        if self.charts.is_empty() {
            self.active = None;
            self.show_stack = false;
        } else {
            // If the active fullscreen chart closed, return the remaining charts to stack view.
            if active_closed && !self.show_stack {
                self.show_stack = true;
            }
            let charts = &self.charts;
            self.active = remap_active_index(charts.len(), keep.as_ref(), fallback, |ix, key| {
                charts[ix].is(key)
            });
        }
        self.publish(cx);
        true
    }
}
