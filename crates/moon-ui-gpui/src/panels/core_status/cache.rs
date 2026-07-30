//! Core Status render-cache pipeline: collect the scoped rows, aggregate them into per-server
//! groups, tag the warning axes, sort, and reconcile the MoonTree. Split out of `mod.rs` as the
//! data-shaping half of the panel, distinct from its rendering and its interaction handlers.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::*;

use super::model::{self, CoreStatusRow, ServerKey, aggregate_servers};
use super::ordering::{assign_server_names, compare_flat_rows, natural_cmp};
use super::{CoreStatusView, server_view};
use crate::Backend;
use moon_core::feed::ConnStatus;
use moon_core::session::CoreSysStatus;

impl CoreStatusView {
    /// Collect filtered core rows for the group scope in canonical order.
    ///
    /// Args:
    ///     b: Backend snapshot containing config, sessions, market identity, and the warning engine.
    ///
    /// Returns:
    ///     Canonically ordered visible rows with CPU smoothed by the backend warning engine.
    fn collect(&self, b: &Backend) -> Vec<CoreStatusRow> {
        let store = b.session.store();
        let mut out = Vec::new();
        for (id, name) in self.scope_cores(b) {
            if !self.sel_cores.is_empty() && !self.sel_cores.contains(&id) {
                continue;
            }
            let endpoint = store.core(id).and_then(|core| core.endpoint);
            let (status, mut sys) = store
                .core(id)
                .map(|c| (c.status.clone(), c.sys))
                .unwrap_or((ConnStatus::Disconnected, CoreSysStatus::default()));
            // Smooth the displayed CPU with the engine's rolling average (computed backend-side).
            let (proc, system) = b.warn.avg_cpu(id);
            if let Some(proc) = proc {
                sys.process_cpu_percent = Some(proc);
            }
            if let Some(system) = system {
                sys.system_cpu_percent = Some(system);
            }
            out.push(CoreStatusRow {
                id,
                name,
                status,
                sys,
                endpoint,
                ping_warn: b.warn.core_ping_warn(id),
                exch_warn: b.warn.core_exch_warn(id),
                ping_base: b.warn.core_ping_baseline(id),
                exch_base: b.warn.core_exch_baseline(id),
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
        let (mut groups, rows) = {
            let b = backend.read(cx);
            let rows = self.collect(b);
            let names = b.layout.core_server_names.clone();
            let mut groups = aggregate_servers(&rows);
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
        // Warned servers first, then by server NAME (natural order, so `Server 2` < `Server 10`
        // and custom names like `F1` sort alphabetically). No user-selectable sort.
        groups.sort_by(|a, b| {
            let aw = a.cpu_warn || a.mem_warn || a.conn_warn || a.ping_warn || a.exch_warn;
            let bw = b.cpu_warn || b.mem_warn || b.conn_warn || b.ping_warn || b.exch_warn;
            bw.cmp(&aw)
                .then_with(|| natural_cmp(&a.display_name, &b.display_name))
        });
        self.has_warn = groups.iter().any(|group| {
            group.cpu_warn
                || group.mem_warn
                || group.conn_warn
                || group.ping_warn
                || group.exch_warn
        });
        self.cached_groups = Rc::new(groups);
        self.cached_rows = Rc::new(rows);
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

    /// Order flat-mode rows: the active column sort, or the default attention-first order.
    ///
    /// Args:
    ///     rows: Current filtered core snapshots.
    ///
    /// Returns:
    ///     A sorted copy for the flat table.
    pub(super) fn sorted_flat_rows(&self, rows: &[CoreStatusRow]) -> Vec<CoreStatusRow> {
        let mut out = model::ordered_flat_rows(rows);
        if let Some((key, ascending)) = &self.flat_sort {
            if key == "server" {
                // The "server" column sorts by the displayed server NAME (natural order), which
                // lives on the aggregated groups rather than the row.
                let names: HashMap<ServerKey, String> = self
                    .cached_groups
                    .iter()
                    .map(|group| (group.key, group.display_name.clone()))
                    .collect();
                let name_of = |row: &CoreStatusRow| {
                    names
                        .get(&ServerKey::for_row(row))
                        .cloned()
                        .unwrap_or_default()
                };
                out.sort_by(|a, b| {
                    let ordering = natural_cmp(&name_of(a), &name_of(b));
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
}
