//! Core Status render-cache pipeline: collect the scoped rows, aggregate them into per-server
//! groups, tag the warning axes, sort, and reconcile the MoonTree. Split out of `mod.rs` as the
//! data-shaping half of the panel, distinct from its rendering and its interaction handlers.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::*;

use super::model::{self, CoreStatusRow, ServerKey, ServerStatusGroup, aggregate_servers};
use super::ordering::{self, assign_server_names, compare_flat_rows, compare_groups, natural_cmp};
use super::{CoreStatusView, server_view};
use crate::Backend;
use moon_core::feed::ConnStatus;
use moon_core::session::{CoreId, CoreStartupStatus, CoreSysStatus};

#[cfg(test)]
mod tests;

/// Sort owned flat rows while preserving the attention-first stable tie order.
fn sort_flat_rows(
    rows: &[CoreStatusRow],
    flat_sort: Option<&(String, bool)>,
    groups: &[ServerStatusGroup],
) -> Vec<CoreStatusRow> {
    let mut out = model::ordered_flat_rows(rows);
    if let Some((key, ascending)) = flat_sort {
        if key == "server" {
            let names: HashMap<ServerKey, &str> = groups
                .iter()
                .map(|group| (group.key, group.display_name.as_str()))
                .collect();
            let name_of =
                |row: &CoreStatusRow| names.get(&ServerKey::for_row(row)).copied().unwrap_or("");
            out.sort_by(|a, b| {
                let ordering = natural_cmp(name_of(a), name_of(b));
                if *ascending {
                    ordering
                } else {
                    ordering.reverse()
                }
            });
        } else {
            out.sort_by(|a, b| {
                let ordering = compare_flat_rows(a, b, key);
                if *ascending {
                    ordering
                } else {
                    ordering.reverse()
                }
            });
        }
    }
    out
}

/// Sorted-row snapshot; venue lines deliberately remain outside this cache.
#[derive(Default)]
pub(super) struct FlatViewCache {
    generation: u64,
    sort: Option<(String, bool)>,
    rows: Option<Rc<Vec<CoreStatusRow>>>,
}

impl FlatViewCache {
    /// Reuse sorted rows on a key hit; build once when generation or sort changes.
    pub(super) fn get_or_build(
        &mut self,
        generation: u64,
        sort: Option<&(String, bool)>,
        build: impl FnOnce() -> Vec<CoreStatusRow>,
    ) -> Rc<Vec<CoreStatusRow>> {
        if self.generation == generation
            && self.sort.as_ref() == sort
            && let Some(rows) = &self.rows
        {
            return rows.clone();
        }
        let rows = Rc::new(build());
        self.generation = generation;
        self.sort = sort.cloned();
        self.rows = Some(rows.clone());
        rows
    }
}

/// Publish row and group snapshots under one new generation so flat sorting cannot reuse old rows.
fn replace_snapshots(
    cached_rows: &mut Rc<Vec<CoreStatusRow>>,
    cached_groups: &mut Rc<Vec<ServerStatusGroup>>,
    generation: &mut u64,
    rows: Vec<CoreStatusRow>,
    groups: Vec<ServerStatusGroup>,
) {
    *cached_rows = Rc::new(rows);
    *cached_groups = Rc::new(groups);
    *generation = generation.wrapping_add(1);
}

impl CoreStatusView {
    /// Collect filtered core rows for the group scope in canonical order.
    ///
    /// Args:
    ///     b: Backend snapshot containing config, sessions, market identity, and the warning engine.
    ///     fleet_newest: The newest MoonBot build currently reported anywhere in the fleet, from the
    ///         STORE rather than these (possibly scoped) rows.
    ///     api_axis_on: Whether the user has the ApiExpiry warning axis enabled. A user who switched
    ///         it off asked not to be told about API keys; the notice band respects that.
    ///
    /// Returns:
    ///     Canonically ordered visible rows with CPU smoothed by the backend warning engine.
    fn collect(
        &self,
        b: &Backend,
        fleet_newest: Option<u32>,
        api_axis_on: bool,
    ) -> Vec<CoreStatusRow> {
        let store = b.session.store();
        // One clock reading for the whole snapshot: a per-row `now` could classify two cores
        // against different days in the same frame.
        let now_ms = moon_core::util::now_unix_ms_i64();
        // Whether a core (by id) is currently `Ready`, read once for the whole fleet: the mode
        // suggestion below compares EVERY configured core, not just this (possibly scoped) view.
        let is_ready = |id: CoreId| {
            store
                .core(id)
                .is_some_and(|core| core.status == ConnStatus::Ready)
        };
        let mode_index = crate::conn_diag::FleetModeIndex::new(&b.config.servers, is_ready);
        let mut out = Vec::new();
        for (id, name) in self.query_cores(b) {
            // One store lookup per core: this loop runs for every core on every cache rebuild.
            let core = store.core(id);
            let endpoint = core.and_then(|core| core.endpoint);
            let api_expiry = core.and_then(|core| core.api_expiry);
            let fault = core.and_then(|core| core.fault.clone());
            let (status, mut sys, startup) = core
                .map(|c| (c.status.clone(), c.sys, c.startup))
                .unwrap_or((
                    ConnStatus::Disconnected,
                    CoreSysStatus::default(),
                    CoreStartupStatus::default(),
                ));
            // `rebuild_cache` already runs on every telemetry tick, so a plain read here needs no
            // separate `time_offset_rev` consumer: the next tick always picks up a fresh adoption.
            let time_offset = core.map(|c| c.time_offset).unwrap_or_default();
            // Smooth the displayed CPU with the engine's rolling average (computed backend-side).
            let (proc, system) = b.warn.avg_cpu(id);
            if let Some(proc) = proc {
                sys.process_cpu_percent = Some(proc);
            }
            if let Some(system) = system {
                sys.system_cpu_percent = Some(system);
            }
            // Classified once here, against one clock reading, so every column, colour and sort in
            // this frame agrees about the same key -- the triangle as much as the colour, since both
            // read this state below.
            let api_key = model::ApiKeyState::of(api_expiry, now_ms);
            // Straight off the same store record every other field above reads: the store drops it
            // on any non-Ready status, so a row can never show a build the current connection did
            // not report.
            let server_version = core.and_then(|core| core.server_version);
            out.push(CoreStatusRow {
                id,
                name,
                status,
                sys,
                startup,
                time_offset,
                mode_suggestion: mode_index.suggestion(id),
                fault,
                endpoint,
                ping_warn: b.warn.core_ping_warn(id),
                exch_warn: b.warn.core_exch_warn(id),
                ping_sev: b.warn.core_ping_level(id),
                exch_sev: b.warn.core_exch_level(id),
                api_key,
                // The engine rebuilds its `api_warn` map only on its own tick, while the store
                // clears `api_expiry` the INSTANT a reconnect begins. So a stale `true` can outlive
                // the day count it was about. The classified state is authoritative: no number, no
                // warning -- for the triangle as much as for the colour, since both read this bool.
                api_warn: b.warn.core_api_warn(id) && api_key.days().is_some(),
                // Colour-only band, decided from the SAME classified state the cell prints -- so the
                // number and its colour cannot describe different keys.
                api_notice: api_axis_on && api_key.within_notice(),
                api_quota: core.and_then(|core| core.api_quota),
                // Read from the engine like every other warning flag, and NOT re-derived from the
                // number beside it: the two would disagree for the tick between a fresh quota
                // landing in the store and the engine's next pass over it.
                api_quota_warn: b.warn.core_api_quota_warn(id),
                server_version,
                version_behind: server_version
                    .zip(fleet_newest)
                    .filter(|(mine, newest)| mine < newest)
                    .map(|(_, newest)| newest),
                update: b.session.core_update_phase(id).cloned(),
            });
        }
        out
    }

    /// Rebuild flat and grouped snapshots, tag warnings, sort, then reconcile MoonTree items.
    ///
    /// Args:
    ///     cx: View context used to read the backend and update tree state.
    ///
    /// Returns:
    ///     Nothing; all render caches are replaced atomically.
    pub(super) fn rebuild_cache(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        // Recounted with the rest of the cache, not inside `title_suffix`: the dock asks that on a
        // per-frame path, and it must read a field rather than walk the fleet's findings.
        self.unseen_problems = self.count_unseen_problems(backend.read(cx));
        let (mut groups, rows) = {
            let b = backend.read(cx);
            // From the STORE, not the (possibly scoped) collected rows: a panel scoped to a subset
            // of the fleet must not compute a lower maximum and silently flag nothing, and two Core
            // Status panels with different scopes must not disagree about which cores are stale.
            let fleet_newest: Option<u32> = b.session.fleet_newest_version();
            let api_axis_on = b.warn_axes().api;
            let rows = self.collect(b, fleet_newest, api_axis_on);
            let names = b.layout.core_server_names.clone();
            let mut groups = aggregate_servers(&rows, fleet_newest);
            assign_server_names(&mut groups, &names);
            // All three warning axes come from the backend engine's current state.
            for group in &mut groups {
                group.cpu_warn = group.address.is_some_and(|ip| b.warn.server_cpu_warn(ip));
                group.mem_warn = group.cores.iter().any(|core| b.warn.core_mem_warn(core.id));
                group.conn_warn = group.address.is_some_and(|ip| b.warn.server_conn_warn(ip));
                group.ping_warn = group
                    .cores
                    .iter()
                    .any(|core| b.warn.core_ping_warn(core.id));
                group.exch_warn = group
                    .cores
                    .iter()
                    .any(|core| b.warn.core_exch_warn(core.id));
            }
            (groups, rows)
        };
        // Warned servers ALWAYS lead (the attention pin), then the user's chosen column sort within
        // each partition; the default `(Name, ascending)` reproduces the former fixed order.
        let (field, ascending) = self.group_sort;
        groups.sort_by(|a, b| {
            let aw = a.has_warn();
            let bw = b.has_warn();
            let field_ord = compare_groups(a, b, field);
            let field_ord = if ascending {
                field_ord
            } else {
                field_ord.reverse()
            };
            bw.cmp(&aw).then(field_ord)
        });
        self.has_warn = groups.iter().any(|group| group.has_warn());
        replace_snapshots(
            &mut self.cached_rows,
            &mut self.cached_groups,
            &mut self.rows_generation,
            rows,
            groups,
        );
        // Prune the row selection against the rows that now EXIST. This is the one call that
        // keeps a bulk update honest: a preset change, a group switch or a core simply leaving
        // the scope removes a row from the screen, and a core the user can no longer see must
        // never stay in a set the update menu and the footer are about to enqueue. Placed here,
        // beside the rows themselves, because every path that changes what is displayed ends up
        // in this function.
        let visible: Vec<Option<CoreId>> =
            self.cached_rows.iter().map(|row| Some(row.id)).collect();
        self.core_selection.retain_visible(&visible);
        // `workspace_revision`'s observer (`mod.rs::new`) calls this unconditionally on every
        // change, with no signature gate ahead of it -- unlike the backend observer's 1 s/rev
        // gate above -- so recomputing the marker here keeps it exactly as fresh as the rows and
        // groups it is drawn beside.
        self.cached_scope_marker = self.scope_marker(backend.read(cx));
        self.rebuild_tree(cx);
    }

    /// Replace the tree items, preserving which servers the user has expanded.
    ///
    /// The cache rebuilds on every telemetry tick, so the current expansion is read back and
    /// re-applied; otherwise servers would collapse each tick. New servers stay collapsed.
    ///
    /// Args:
    ///     cx: View context used to update the MoonTree state entity.
    ///
    /// Returns:
    ///     Nothing; only tree items and their expansion change.
    fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        let expanded = self
            .tree_state
            .read(cx)
            .expanded_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        let items = server_view::tree_items(&self.cached_groups);
        self.tree_state.update(cx, |state, cx| {
            state.set_items(items, cx);
            state.set_expanded(expanded, cx);
        });
    }

    /// Reuse the sorted snapshot and refresh exchange lines from current venues and locale.
    ///
    /// Everything returned is OWNED. [`moon_ui::MoonDataTable`]'s row closure is `'static`, so it
    /// cannot hold a `&CoreVenue` borrowed out of the session's venue map; resolving the sections
    /// here rather than at the call site is what keeps that borrow from ever reaching the closure.
    ///
    /// Args:
    ///     cx: Application context used to read the session's venue map.
    ///
    /// Returns:
    ///     The sorted rows, and the heading/member lines addressing them by index.
    pub(super) fn flat_view(
        &self,
        cx: &App,
    ) -> (Rc<Vec<CoreStatusRow>>, Rc<Vec<ordering::FlatLine>>) {
        let rows = self.flat_cache.borrow_mut().get_or_build(
            self.rows_generation,
            self.flat_sort.as_ref(),
            || {
                sort_flat_rows(
                    &self.cached_rows,
                    self.flat_sort.as_ref(),
                    &self.cached_groups,
                )
            },
        );
        let venues = self.backend.read(cx).session.core_venues();
        let lines = ordering::flat_lines(&rows, venues);
        (rows, Rc::new(lines))
    }
}
