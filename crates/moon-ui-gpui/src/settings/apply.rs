//! Saving a Settings draft and applying presentation, logging, market, session, and window
//! changes at their appropriate boundaries.

use std::collections::{HashMap, HashSet};

use gpui::*;
use moon_ui::Root;

use super::SettingsView;
use moon_core::config::{AppConfig, SnapshotOutcome};

/// One core, projected to exactly what the window decision reads.
///
/// The three questions this file used to answer with three separate `Vec<(u64, String)>` walks —
/// who moved group, whose bundle changed, who is gone — are one comparison over one projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CoreEntry {
    /// Stable core UID. Equal to `ServerConfig::id` since schema v11, so it is also the `CoreId`
    /// that chart buckets and sessions key on.
    pub uid: u64,
    pub group: String,
    pub bundle: String,
}

/// What one save changed about the set of cores.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct CoreDelta {
    /// Cores that changed group, each with the group it LEFT.
    pub moved: Vec<(u64, String)>,
    /// Cores present before the save and absent after it.
    pub removed: Vec<u64>,
    /// Cores present in both whose `chart_bundle` changed.
    pub rebundled: Vec<u64>,
    /// Cores that appeared in this save.
    pub added: Vec<u64>,
}

/// Compare two core projections.
///
/// Additions and removals are judged separately from value changes, because they pull in opposite
/// directions: an addition must NOT force a window rebuild, a removal must. See
/// [`CoreDelta::needs_window_rebuild`].
pub(super) fn core_delta(before: &[CoreEntry], after: &[CoreEntry]) -> CoreDelta {
    let after_by_uid: HashMap<u64, &CoreEntry> = after.iter().map(|c| (c.uid, c)).collect();
    let before_uids: HashSet<u64> = before.iter().map(|c| c.uid).collect();
    let mut delta = CoreDelta::default();
    for old in before {
        match after_by_uid.get(&old.uid) {
            None => delta.removed.push(old.uid),
            Some(new) => {
                if new.group != old.group {
                    delta.moved.push((old.uid, old.group.clone()));
                }
                if new.bundle != old.bundle {
                    delta.rebundled.push(old.uid);
                }
            }
        }
    }
    delta.added = after
        .iter()
        .filter(|c| !before_uids.contains(&c.uid))
        .map(|c| c.uid)
        .collect();
    delta
}

impl CoreDelta {
    /// Whether this save must close and recreate the group windows.
    ///
    /// A rebuild is destructive in a way the user sees: every group window is recreated, so every
    /// chart tab in it disappears (only Main's settings and detached tabs are restored from
    /// `charts.json` — ordinary stacks are rebuilt from live detects). It is therefore taken only
    /// when nothing else can repair the topology:
    ///
    /// * a core MOVED between groups — its tabs and its old group's persistence must be swept;
    /// * a core was REMOVED — `ChartTabs::ingest` only ever appends stacks for live sessions and
    ///   never retires one, so a dead `CoreId` would otherwise keep its tab;
    /// * a core was REBUNDLED — bucket keys change, and `chart_tabs_sig` does not hash the bundle;
    /// * split-by-core was toggled — every bucket key changes at once.
    ///
    /// An ADDITION is deliberately absent: `chart_tabs_sig` hashes the group's core ids, so the
    /// live window composes the new core's tabs on its own. Adding a server used to land here only
    /// because the old bundle signature compared whole vectors and a longer vector differs.
    pub(super) fn needs_window_rebuild(&self, split_changed: bool) -> bool {
        split_changed
            || !self.moved.is_empty()
            || !self.removed.is_empty()
            || !self.rebundled.is_empty()
    }
}

/// Drop chart persistence that a topology change orphaned, returning whether anything changed.
///
/// Two kinds of orphan, both invisible to the user until they resurface as a tab bound to the wrong
/// settings or to a core that no longer exists:
///
/// * a core that LEFT this group — its dedicated tab, its coins inside a custom multi-market tab,
///   and a compare anchor pointing at it all still name this group's spec while the core now reads
///   another group's settings;
/// * a core that was REMOVED entirely — same, for every group.
///
/// A custom tab emptied of coins is dropped; a compare anchor is cleared without dropping the tab,
/// because the tab still has its own markets.
fn prune_orphaned_chart_specs(
    specs: &mut Vec<crate::persistence::chart_persist::ChartTabSpec>,
    moved: &[(u64, String)],
    removed: &[u64],
) -> bool {
    let orphaned = |core: u64, group: &str| {
        removed.contains(&core)
            || moved
                .iter()
                .any(|(moved_core, old_group)| *moved_core == core && old_group == group)
    };
    let mut changed = false;
    specs.retain_mut(|spec| {
        let group = spec.group.clone();
        if let moon_core::config::ChartBucket::Core(core) = spec.bucket()
            && orphaned(core, &group)
        {
            changed = true;
            return false;
        }
        if let Some(coins) = spec.custom_coins.as_mut() {
            let before_len = coins.len();
            coins.retain(|(core, _)| !orphaned(*core, &group));
            changed |= coins.len() != before_len;
            if coins.is_empty() {
                changed = true;
                return false;
            }
        }
        if spec
            .compare_anchor
            .as_ref()
            .is_some_and(|(core, _)| orphaned(*core, &group))
        {
            // Comparison mode is meaningless without its anchor, so it goes with it.
            spec.compare_anchor = None;
            spec.compare_orderbook_only = false;
            changed = true;
        }
        true
    });
    changed
}

/// Project a saved config down to the cores the window decision compares.
fn cores_of(config: &AppConfig) -> Vec<CoreEntry> {
    config
        .servers
        .iter()
        .map(|server| CoreEntry {
            uid: server.uid,
            group: server.group.clone(),
            bundle: server.chart_bundle.clone(),
        })
        .collect()
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

        // The saved conversion is already in `config`, but two things do not follow from the write:
        // the valuation worker only fetches current rates while they are demanded, and every open
        // Report host and the Analytics window learn about the change through the report revision
        // rather than by polling the config.
        if before.report_valuation_mode != after.report_valuation_mode {
            self.backend
                .update(cx, |b, bcx| b.apply_valuation_mode(bcx));
        }

        // Apply file logging live; purge immediately after toggling it or changing retention.
        if before.log_to_file != after.log_to_file
            || before.log_retention_days != after.log_retention_days
        {
            moon_core::applog::set_file_logging(after.log_to_file, after.log_retention_days);
            moon_core::applog::purge_old();
        }

        let struct_changed = before.structural_sig() != after.structural_sig();
        let mode_changed = before.market_mode != after.market_mode;
        let split_changed = before.charts_split_by_core != after.charts_split_by_core;
        let delta = core_delta(&cores_of(before), &cores_of(&after));
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
            // A rebuild closes every group window, and with it every chart tab those windows
            // hold: only Main's settings and detached tabs come back from `charts.json`, ordinary
            // stacks are recomposed from live detects. So it is taken only when the topology cannot
            // be repaired in place — never for a plain server addition.
            let rebuild = delta.needs_window_rebuild(split_changed);
            log::info!(
                "apply settings: struct change → windows {} (added={} removed={} moved={} rebundled={} split={split_changed})",
                if rebuild { "rebuilt" } else { "reconciled" },
                delta.added.len(),
                delta.removed.len(),
                delta.moved.len(),
                delta.rebundled.len(),
            );
            if rebuild {
                // Bucket keys change only when the split mode or a bundle assignment changes; a
                // move or a removal leaves the surviving keys addressable.
                let bucket_keys_changed = split_changed || !delta.rebundled.is_empty();
                self.rebuild_group_windows(&delta, bucket_keys_changed, cx);
            } else {
                self.reconcile_group_windows(cx);
            }
        } else if mode_changed {
            // Apply market mode live. Cores stay connected and the coordinator reselects providers
            // on its next tick.
            self.backend
                .update(cx, |b, _| b.session.set_market_mode(b.config.market_mode));
        }

        // A bundle or split edit alone does not reach `structural_sig` (both are presentation), so
        // it lands here: the tabs must be recomposed against the new bucket keys, and in GPUI tabs
        // belong to their window, so the window is recreated. Nothing moved or vanished, so the
        // delta carries no pruning work.
        if !struct_changed && delta.needs_window_rebuild(split_changed) {
            log::info!(
                "apply settings: presentation change → windows rebuilt (rebundled={} split={split_changed})",
                delta.rebundled.len(),
            );
            // Both triggers of this branch rekey the buckets by definition.
            self.rebuild_group_windows(&delta, true, cx);
        }
    }

    /// Close every group window and reopen it from the current config, porting egui's
    /// `needs_rebuild` path. Saved layout restores window geometry.
    ///
    /// Also close every detached chart-tab window, because a window outliving its group window
    /// would hold a stale handle. Whether those tabs come back detached depends on
    /// `bucket_keys_changed`: their windows are addressed by bucket, so a split or bundle edit must
    /// return them to the strip, while a move or a removal leaves the surviving buckets valid and
    /// `ChartTabs::new` reopens them from their own specs. The specs carry the geometry in the same
    /// `Option`, so clearing the marker when it is not required would cost the user their saved
    /// window positions for nothing.
    ///
    /// Detached PANEL windows are different: a panel belongs to a group, never to a chart bucket,
    /// so a topology change gives no reason to undo the user's detachment. They are OS-owned by
    /// their group window and die with it regardless, so they are taken off
    /// `detached_panel_windows` first — silencing their repin, which would otherwise return each
    /// panel to the dock and erase its `DetachedSpec` from `detached.json` — and reopened against
    /// the new group windows below.
    fn rebuild_group_windows(
        &mut self,
        delta: &CoreDelta,
        bucket_keys_changed: bool,
        cx: &mut Context<Self>,
    ) {
        let (close, cfg, epoch, layout) = self.backend.update(cx, |b, _| {
            let mut close: Vec<WindowHandle<Root>> = b.group_windows.values().copied().collect();
            b.group_windows.clear();
            // Draining the registry silences the hosts about to be destroyed: nobody asked for
            // their tabs back, and a repin would clear the very `detached` markers this rebuild
            // preserves and resurrect the specs it prunes.
            close.extend(b.detached_chart_windows.drain(..).map(|(_, h)| h));
            close.extend(crate::window::detached::take_windows(b, |_| true));
            // A rebuild can retire a group outright (a rename moves every core out of the old
            // name). Requests still addressed to it would otherwise wait for a future window of
            // that same name and be replayed against it.
            let live: HashSet<String> = crate::window::group_window::groups(&b.config)
                .into_iter()
                .collect();
            crate::window::detached::prune_requests(b, |g| !live.contains(g));
            let mut changed = false;
            // Only a change that rekeys the buckets forces detached chart tabs back into the strip:
            // their windows are keyed by bucket, so a stale one would duplicate or point nowhere.
            // A removal or a move leaves the surviving buckets addressable, and `detached` holds the
            // geometry as well as the marker — clearing it there would cost every detached chart its
            // saved position for nothing, and `ChartTabs::new` reopens them from these very specs.
            if bucket_keys_changed {
                for s in b.chart_specs.iter_mut() {
                    changed |= s.detached.take().is_some();
                }
            }
            changed |= prune_orphaned_chart_specs(&mut b.chart_specs, &delta.moved, &delta.removed);
            b.chart_specs_dirty |= changed;
            (close, b.config.clone(), b.epoch, b.layout.clone())
        });
        crate::window::windowing::close_all(close, cx);
        // Requests already queued before this save belong to windows the user closed, whose tabs
        // this rebuild is about to recompose anyway. The hosts dying right now add nothing to the
        // queue: draining the registry above took them off it, and that is what their release
        // checks before queueing anything.
        self.backend
            .update(cx, |b, _| b.chart_repin_request.clear());
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
        let (close, spawn_groups, cfg, epoch, layout) = self.backend.update(cx, |b, _| {
            let want = crate::window::group_window::groups(&b.config);
            let want_set: HashSet<&str> = want.iter().map(String::as_str).collect();
            // Collect windows belonging to groups that no longer exist.
            let mut close: Vec<WindowHandle<Root>> = b
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
            close.extend(
                b.detached_chart_windows
                    .iter()
                    .filter(|(g, _)| gone.contains(g))
                    .map(|(_, h)| *h),
            );
            b.detached_chart_windows.retain(|(g, _)| !gone.contains(g));
            // Detached panel windows of a removed group have no dock left to return to, and
            // `take_windows` unregisters them before they close so their release stays silent.
            // Their SPECS stay: the group can come back (a deactivated server, a rename undone) and
            // the panel is absent from `dock_states`, so forgetting the spec would leave it nowhere.
            close.extend(crate::window::detached::take_windows(b, |g| {
                gone.contains(g)
            }));
            crate::window::detached::prune_requests(b, |g| gone.contains(g));
            // Same reasoning for the chart tabs of those windows: no `ChartTabs` exists for a
            // departed group, so the request would wait for a future window of that name.
            b.chart_repin_request.retain(|(g, _, _)| !gone.contains(g));
            // Spawn groups requested by the config that do not already have a window.
            let spawn_groups: Vec<String> = want
                .iter()
                .filter(|g| !b.group_windows.contains_key(g.as_str()))
                .cloned()
                .collect();
            (
                close,
                spawn_groups,
                b.config.clone(),
                b.epoch,
                b.layout.clone(),
            )
        });
        crate::window::windowing::close_all(close, cx);
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
