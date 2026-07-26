//! Saving a Settings draft and applying presentation, logging, market, session, and window
//! changes at their appropriate boundaries.

use std::collections::{HashMap, HashSet};

use gpui::*;
use moon_ui::Root;

use super::SettingsView;
use moon_core::config::{AppConfig, SnapshotOutcome};

/// Return each core and old group whose saved group membership changed.
fn moved_cores(before: &[(u64, String)], after: &[(u64, String)]) -> Vec<(u64, String)> {
    let after_by_id: HashMap<u64, &str> = after
        .iter()
        .map(|(id, group)| (*id, group.as_str()))
        .collect();
    before
        .iter()
        .filter(|(id, old_group)| {
            after_by_id
                .get(id)
                .is_some_and(|new_group| *new_group != old_group)
        })
        .map(|(id, old_group)| (*id, old_group.clone()))
        .collect()
}

/// Remove moved cores from old-group chart persistence before rebuilding those windows.
fn sanitize_moved_chart_specs(
    specs: &mut Vec<crate::persistence::chart_persist::ChartTabSpec>,
    moved: &[(u64, String)],
) -> bool {
    let belongs_to_old_group = |core: u64, group: &str| {
        moved
            .iter()
            .any(|(moved_core, old_group)| *moved_core == core && old_group == group)
    };
    let mut changed = false;
    specs.retain_mut(|spec| {
        let group = spec.group.clone();
        if let moon_core::config::ChartBucket::Core(core) = spec.bucket() {
            if belongs_to_old_group(core, &group) {
                changed = true;
                return false;
            }
        }
        if let Some(coins) = spec.custom_coins.as_mut() {
            let before_len = coins.len();
            coins.retain(|(core, _)| !belongs_to_old_group(*core, &group));
            changed |= coins.len() != before_len;
            if coins.is_empty() {
                changed = true;
                return false;
            }
        }
        if spec
            .compare_anchor
            .as_ref()
            .is_some_and(|(core, _)| belongs_to_old_group(*core, &group))
        {
            spec.compare_anchor = None;
            changed = true;
        }
        true
    });
    changed
}

impl SettingsView {
    /// Validate and save the draft, then apply it without closing the Settings window.
    ///
    /// A failed save changes neither the active config nor the draft.
    pub(super) fn save(&mut self, cx: &mut Context<Self>) {
        // Compare the saved candidate with this snapshot to select live updates and required
        // rebuilds.
        let before = self.backend.read(cx).config.clone();
        let res = self.backend.update(cx, |b, _| {
            // Commit the candidate only after validation and I/O succeed. Otherwise the config
            // would change without corresponding session and window reconciliation.
            let mut candidate = b.preview.as_ref().unwrap_or(&b.config).clone();
            // Preserve the preceding on-disk files in `backups/` first: this is a deliberate user
            // save for which rollback must be available.
            let res = candidate.save_with_snapshot();
            if res.is_ok() {
                // Propagate uid normalization from the save back into the draft so the next save
                // cannot roll back `next_uid` and reuse an id from reports.sqlite history.
                if let Some(p) = b.preview.as_mut() {
                    *p = candidate.clone();
                }
                b.config = candidate;
            }
            res
        });
        match res {
            Ok(outcome) => {
                // Snapshot failure does NOT cancel the save, but a normal success message would
                // promise a nonexistent rollback copy precisely when the user relies on one.
                let snapshot_failed = outcome == SnapshotOutcome::Failed;
                let msg = if snapshot_failed {
                    super::StatusMsg::Key("settings.saved_no_backup")
                } else {
                    super::StatusMsg::Key("settings.saved")
                };
                self.status = Some((msg, snapshot_failed));
                self.apply_settings(&before, cx);
            }
            Err(e) => self.status = Some((super::StatusMsg::Text(e.to_string()), true)),
        }
        cx.notify();
    }

    /// Apply saved settings at the narrowest required boundary.
    ///
    /// Presentation, logging, and market-mode changes apply live. Structural server or group
    /// changes reconcile sessions, while chart-topology changes rebuild group windows.
    fn apply_settings(&mut self, before: &AppConfig, cx: &mut Context<Self>) {
        let after = self.backend.read(cx).config.clone();

        // Presentation settings are read during rendering, so update locale/order and redraw
        // without recreating windows or sessions.
        let lang_changed = before.language != after.language;
        let sort_changed = before.core_sort != after.core_sort;
        if lang_changed {
            rust_i18n::set_locale(after.language.code());
        }
        if lang_changed || sort_changed {
            // Notify Backend before redrawing so signature-gated panels recompute their order.
            self.backend.update(cx, |b, bcx| {
                // An order change can replace the canonical first core while the cached core is
                // still live, so bypass the usual liveness early return.
                if sort_changed {
                    b.refresh_header_ticker_default(true);
                }
                bcx.notify();
            });
            cx.refresh_windows();
        }

        // Apply file logging live; purge immediately after toggling it or changing retention.
        if before.log_to_file != after.log_to_file
            || before.log_retention_days != after.log_retention_days
        {
            moon_core::applog::set_file_logging(after.log_to_file, after.log_retention_days);
            moon_core::applog::purge_old();
        }

        let struct_changed = before.structural_sig() != after.structural_sig();
        let server_groups = |config: &AppConfig| {
            config
                .servers
                .iter()
                .map(|server| (server.id, server.group.clone()))
                .collect::<Vec<_>>()
        };
        let moved = moved_cores(&server_groups(before), &server_groups(&after));
        let mode_changed = before.market_mode != after.market_mode;
        let split_changed = before.charts_split_by_core != after.charts_split_by_core;
        // Changing a core's `chart_bundle` alters chart-tab composition without requiring a
        // reconnect. Treat it like split mode and rebuild group windows without restarting sessions.
        let bundle_sig = |c: &AppConfig| {
            let mut v: Vec<(u64, String)> = c
                .servers
                .iter()
                .map(|s| (s.uid, s.chart_bundle.clone()))
                .collect();
            v.sort();
            v
        };
        let bundle_changed = bundle_sig(before) != bundle_sig(&after);
        let ui_theme_changed = before.ui_font_delta != after.ui_font_delta
            || before.ui_theme_mode != after.ui_theme_mode
            || before.ui_scale != after.ui_scale;

        if ui_theme_changed {
            crate::install_moon_theme_for_config(&after, cx);
        }

        if struct_changed {
            // Reconcile sessions incrementally: stop removed or disabled cores and cores in disabled
            // groups, add newly active cores, restart only connections whose `conn_sig` changed,
            // and update other metadata in place. Reconciliation preserves epoch and the existing
            // market mode; the following setter is a no-op unless the saved mode changed, in which
            // case it applies the new mode. Keep `chart_market_refs`: surviving windows retain their
            // subscriptions, closed panels release theirs, and new panels register when opened.
            self.backend.update(cx, |b, _| {
                let reports = b.reports.as_ref().map(|h| &h.tx);
                b.session.reconcile(&b.config, reports);
                b.session.set_market_mode(b.config.market_mode);
            });
            if moved.is_empty() && !split_changed && !bundle_changed {
                self.reconcile_group_windows(cx);
            } else {
                self.rebuild_group_windows(&moved, cx);
            }
        } else if mode_changed {
            // Apply market mode live. Cores stay connected and the coordinator reselects providers
            // on its next tick.
            self.backend
                .update(cx, |b, _| b.session.set_market_mode(b.config.market_mode));
        }

        // When split-by-core or chart bundles change without a structural reconciliation, rebuild
        // group windows so their chart tabs use the new topology. Egui cleared chart tabs directly;
        // in GPUI tabs belong to their window, so the window is recreated.
        if !struct_changed && (split_changed || bundle_changed) {
            self.rebuild_group_windows(&[], cx);
        }
    }

    /// Close every group window and reopen it from the current config, porting egui's
    /// `needs_rebuild` path. Saved layout restores window geometry.
    ///
    /// Also close every detached chart-tab window and clear each spec's `detached` marker. Group
    /// topology can change bucket keys; retaining old windows would leave duplicates and stale
    /// handles. The specs then return to the new group window's tab strip instead of reopening as
    /// detached off-screen windows.
    fn rebuild_group_windows(&mut self, moved: &[(u64, String)], cx: &mut Context<Self>) {
        let (handles, chart_handles, cfg, epoch, layout) = self.backend.update(cx, |b, _| {
            let handles: Vec<WindowHandle<Root>> = b.group_windows.values().copied().collect();
            b.group_windows.clear();
            let chart_handles: Vec<WindowHandle<Root>> =
                b.detached_chart_windows.drain(..).map(|(_, h)| h).collect();
            // Return detached tabs to the strip by clearing every spec marker, preventing fresh
            // group windows from reopening duplicate detached copies.
            for s in b.chart_specs.iter_mut() {
                s.detached = None;
            }
            sanitize_moved_chart_specs(&mut b.chart_specs, moved);
            b.chart_specs_dirty = true;
            (
                handles,
                chart_handles,
                b.config.clone(),
                b.epoch,
                b.layout.clone(),
            )
        });
        for h in handles {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for h in chart_handles {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        // Detached-host release callbacks may enqueue repins while their owning ChartTabs entities
        // are also being destroyed. Those panels cannot be reused after a topology rebuild.
        self.backend.update(cx, |b, _| {
            b.chart_repin_request.clear();
            if sanitize_moved_chart_specs(&mut b.chart_specs, moved) {
                b.chart_specs_dirty = true;
            }
        });
        for (i, g) in crate::window::group_window::groups(&cfg)
            .into_iter()
            .enumerate()
        {
            crate::window::group_window::spawn_group_window(
                cx,
                &self.backend,
                &cfg,
                g,
                epoch,
                &layout,
                i as f32 * 40.0,
            );
        }
    }

    /// Reconcile group windows incrementally instead of calling [`Self::rebuild_group_windows`].
    ///
    /// Close removed groups and their detached hosts, and open new groups.
    ///
    /// Retained group windows stay intact. Their `ChartTabs` signatures pick up added or removed
    /// cores, preserving open tabs and layout across server membership changes.
    fn reconcile_group_windows(&mut self, cx: &mut Context<Self>) {
        let (close_group, close_detached, spawn_groups, cfg, epoch, layout) =
            self.backend.update(cx, |b, _| {
                let want = crate::window::group_window::groups(&b.config);
                let want_set: HashSet<&str> = want.iter().map(String::as_str).collect();
                // Collect windows belonging to groups that no longer exist.
                let close_group: Vec<WindowHandle<Root>> = b
                    .group_windows
                    .iter()
                    .filter(|(g, _)| !want_set.contains(g.as_str()))
                    .map(|(_, h)| *h)
                    .collect();
                let gone: HashSet<String> = b
                    .group_windows
                    .keys()
                    .filter(|g| !want_set.contains(g.as_str()))
                    .cloned()
                    .collect();
                b.group_windows.retain(|g, _| want_set.contains(g.as_str()));
                // A detached host keeps its group for life, so removed groups close their hosts.
                let stale_detached_groups: HashSet<&str> =
                    gone.iter().map(String::as_str).collect();
                let close_detached: Vec<WindowHandle<Root>> = b
                    .detached_chart_windows
                    .iter()
                    .filter(|(g, _)| stale_detached_groups.contains(g.as_str()))
                    .map(|(_, h)| *h)
                    .collect();
                b.detached_chart_windows
                    .retain(|(g, _)| !stale_detached_groups.contains(g.as_str()));
                // Spawn groups requested by the config that do not already have a window.
                let spawn_groups: Vec<String> = want
                    .iter()
                    .filter(|g| !b.group_windows.contains_key(g.as_str()))
                    .cloned()
                    .collect();
                (
                    close_group,
                    close_detached,
                    spawn_groups,
                    b.config.clone(),
                    b.epoch,
                    b.layout.clone(),
                )
            });
        for h in close_group {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for h in close_detached {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for (i, g) in spawn_groups.into_iter().enumerate() {
            crate::window::group_window::spawn_group_window(
                cx,
                &self.backend,
                &cfg,
                g,
                epoch,
                &layout,
                i as f32 * 40.0,
            );
        }
    }
}

#[cfg(test)]
mod tests;
