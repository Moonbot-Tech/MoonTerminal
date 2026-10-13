//! Core Status render-cache pipeline: collect the scoped rows, aggregate them into per-server
//! groups, tag the warning axes, and sort. Split out of `mod.rs` as the
//! data-shaping half of the panel, distinct from its rendering and its interaction handlers.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::rc::Rc;

use gpui::*;

use super::health::{self, HealthFilter};
use super::mode::WARN_LIST_LIMIT;
use super::model::{self, CoreStatusRow, ServerKey, ServerStatusGroup, aggregate_servers};
use super::ordering::{self, assign_server_names, compare_flat_rows, natural_cmp};
use super::{CoreStatusView, table, uniform, warnings};
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
///
/// The key is the snapshot generation, the effective sort, the health chip, the normalized
/// search, and whether addresses are masked. A search computed while addresses were visible
/// must not be reused after the mask flips, because an IP query then matches nothing.
#[derive(Default)]
pub(super) struct FlatViewCache {
    generation: u64,
    sort: Option<(String, bool)>,
    filter: Option<HealthFilter>,
    query: String,
    ip_masked: bool,
    rows: Option<Rc<Vec<CoreStatusRow>>>,
    /// Uniform facts for the filtered projection. Keyed by generation, chip, search, and the
    /// IP mask. Sort is not part of the key: agreement does not depend on row order.
    uniform: Option<(
        u64,
        Option<HealthFilter>,
        String,
        bool,
        Rc<Vec<uniform::UniformFact>>,
    )>,
    /// Widest figure per column plus cell padding. Keyed by the row generation and the locale.
    floors: Option<(u64, String, Rc<HashMap<String, f32>>)>,
}

impl FlatViewCache {
    /// Reuse projected rows on a key hit; build once when any key part changes.
    ///
    /// Args:
    ///     generation: Row-snapshot generation from the last cache rebuild.
    ///     sort: Effective column sort. `None` is the attention order, including a saved sort
    ///         whose column is not on screen.
    ///     filter: Health chip narrowing the rows, or `None` for every row in scope.
    ///     query: Already trimmed and lowercased search text. Empty matches every row.
    ///     ip_masked: Whether endpoint addresses are hidden from search.
    ///     build: Projection used only on a miss.
    ///
    /// Returns:
    ///     The projected rows. The same `Rc` is returned for every hit.
    pub(super) fn get_or_build(
        &mut self,
        generation: u64,
        sort: Option<&(String, bool)>,
        filter: Option<HealthFilter>,
        query: &str,
        ip_masked: bool,
        build: impl FnOnce() -> Vec<CoreStatusRow>,
    ) -> Rc<Vec<CoreStatusRow>> {
        if self.generation == generation
            && self.sort.as_ref() == sort
            && self.filter == filter
            && self.query == query
            && self.ip_masked == ip_masked
            && let Some(rows) = &self.rows
        {
            return rows.clone();
        }
        let rows = Rc::new(build());
        self.generation = generation;
        self.sort = sort.cloned();
        self.filter = filter;
        self.query = query.to_string();
        self.ip_masked = ip_masked;
        self.rows = Some(rows.clone());
        rows
    }

    /// Reuse uniform facts for one filter and search. A miss builds them from that projection.
    ///
    /// Args:
    ///     generation: Row-snapshot generation.
    ///     filter: Health chip, or `None`.
    ///     query: Normalized search text.
    ///     ip_masked: Whether addresses are hidden.
    ///     build: `uniform_columns` over the rows that filter and search keep.
    ///
    /// Returns:
    ///     The facts. The same `Rc` is returned for every hit.
    pub(super) fn cached_uniform(
        &mut self,
        generation: u64,
        filter: Option<HealthFilter>,
        query: &str,
        ip_masked: bool,
        build: impl FnOnce() -> Vec<uniform::UniformFact>,
    ) -> Rc<Vec<uniform::UniformFact>> {
        if let Some((cached_generation, cached_filter, cached_query, cached_mask, facts)) =
            &self.uniform
            && *cached_generation == generation
            && *cached_filter == filter
            && cached_query == query
            && *cached_mask == ip_masked
        {
            return facts.clone();
        }
        let facts = Rc::new(build());
        self.uniform = Some((
            generation,
            filter,
            query.to_string(),
            ip_masked,
            facts.clone(),
        ));
        facts
    }

    /// Facts from the last [`Self::cached_uniform`] hit, or an empty list before the first one.
    pub(super) fn uniform_facts(&self) -> Rc<Vec<uniform::UniformFact>> {
        self.uniform
            .as_ref()
            .map(|(_, _, _, _, facts)| facts.clone())
            .unwrap_or_else(|| Rc::new(Vec::new()))
    }

    /// Content floors for this generation and locale. `build` runs only on a miss.
    ///
    /// Args:
    ///     generation: Row-snapshot generation.
    ///     locale: Active language code. A language switch invalidates the measured strings.
    ///     build: Measurement used only on a miss. It must not touch this cache.
    ///
    /// Returns:
    ///     The floors. The same `Rc` is returned for every hit.
    pub(super) fn cached_floors(
        &mut self,
        generation: u64,
        locale: &str,
        build: impl FnOnce() -> HashMap<String, f32>,
    ) -> Rc<HashMap<String, f32>> {
        if let Some((cached_generation, cached_locale, floors)) = &self.floors
            && *cached_generation == generation
            && cached_locale == locale
        {
            return floors.clone();
        }
        let floors = Rc::new(build());
        self.floors = Some((generation, locale.to_string(), floors.clone()));
        floors
    }

    /// Floors from the last [`Self::cached_floors`] hit, or an empty map before the first one.
    pub(super) fn floors(&self) -> Rc<HashMap<String, f32>> {
        self.floors
            .as_ref()
            .map(|(_, _, floors)| floors.clone())
            .unwrap_or_else(|| Rc::new(HashMap::new()))
    }
}

/// Narrow, search, and sort one flat snapshot.
///
/// Filter and search run before the sort, so a hidden sort column falls back to
/// [`model::ordered_flat_rows`] on the rows that remain, not on the unfiltered scope.
/// A sort key absent from `visible_columns` is that hidden column: the saved preference
/// is left untouched and the attention order is what both drawing and selection see.
///
/// Args:
///     rows: Scoped core snapshots, before the chip and the search.
///     groups: Server groups whose display names the search and the server sort read.
///     filter: Health chip, or `None` to keep every row.
///     query: Search text already trimmed and lowercased. Empty matches all.
///     ip_masked: When true, an endpoint address cannot satisfy `query`.
///     sort: Saved column sort, or `None` for attention order.
///     visible_columns: Column keys the table is showing. A sort key outside this list
///         does not reorder the rows.
///
/// Returns:
///     The rows to draw, in on-screen order.
pub(super) fn project_rows(
    rows: &[CoreStatusRow],
    groups: &[ServerStatusGroup],
    filter: Option<HealthFilter>,
    query: &str,
    ip_masked: bool,
    sort: Option<&(String, bool)>,
    visible_columns: &[&str],
) -> Vec<CoreStatusRow> {
    let matched = matching_rows(rows, groups, filter, query, ip_masked);
    let sort = sort.filter(|(key, _)| visible_columns.contains(&key.as_str()));
    sort_flat_rows(&matched, sort, groups)
}

/// Rows that survive the health chip and the search. Sort and uniform columns are not inputs:
/// membership does not depend on either.
pub(super) fn matching_rows(
    rows: &[CoreStatusRow],
    groups: &[ServerStatusGroup],
    filter: Option<HealthFilter>,
    query: &str,
    ip_masked: bool,
) -> Vec<CoreStatusRow> {
    debug_assert_eq!(query, query.trim().to_lowercase());
    let names: HashMap<ServerKey, &str> = groups
        .iter()
        .map(|group| (group.key, group.display_name.as_str()))
        .collect();
    rows.iter()
        .filter(|row| filter.is_none_or(|chip| health::row_matches(row, chip)))
        .filter(|row| query.is_empty() || row_matches_query(row, &names, query, ip_masked))
        .cloned()
        .collect()
}

/// Uniform facts for the rows [`matching_rows`] keeps.
pub(super) fn projection_uniform(
    rows: &[CoreStatusRow],
    groups: &[ServerStatusGroup],
    filter: Option<HealthFilter>,
    query: &str,
    ip_masked: bool,
) -> Vec<uniform::UniformFact> {
    uniform::uniform_columns(&matching_rows(rows, groups, filter, query, ip_masked))
}

/// Whether one core matches the toolbar search.
///
/// Args:
///     row: Core snapshot being tested.
///     names: Server display names keyed by [`ServerKey`].
///     query: Trimmed, lowercased search text. Not empty.
///     ip_masked: Whether the endpoint address is excluded.
///
/// Returns:
///     Whether the core name, the server display name, or the unmasked address contains `query`.
fn row_matches_query(
    row: &CoreStatusRow,
    names: &HashMap<ServerKey, &str>,
    query: &str,
    ip_masked: bool,
) -> bool {
    if row.name.to_lowercase().contains(query) {
        return true;
    }
    let server = names.get(&ServerKey::for_row(row)).copied().unwrap_or("");
    if server.to_lowercase().contains(query) {
        return true;
    }
    if !ip_masked
        && let Some(endpoint) = row.endpoint
        && endpoint.address.to_string().to_lowercase().contains(query)
    {
        return true;
    }
    false
}

/// Whether one server card matches the toolbar search.
///
/// Empty text matches every card. A card matches its display name, any core name, or its
/// address. A masked panel does not match the address.
///
/// Args:
///     group: Server card being tested.
///     query: Search text. Trimmed and lowercased here, so callers may pass either form.
///     ip_masked: Whether the address is excluded.
///
/// Returns:
///     Whether the card stays on screen.
pub(super) fn card_matches_query(group: &ServerStatusGroup, query: &str, ip_masked: bool) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    if group.display_name.to_lowercase().contains(&query) {
        return true;
    }
    if group
        .cores
        .iter()
        .any(|core| core.name.to_lowercase().contains(&query))
    {
        return true;
    }
    if !ip_masked
        && group
            .address
            .is_some_and(|address| address.to_string().to_lowercase().contains(&query))
    {
        return true;
    }
    false
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
            // Straight off the same store record every other field above reads: the store drops
            // the number and the letter together on any non-Ready status, so a row can never show
            // a build the current connection did not report.
            let (server_version, server_version_suffix) = core
                .map(|core| (core.server_version, core.server_version_suffix.clone()))
                .unwrap_or((None, None));
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
                server_version_suffix,
                // number only: a build letter never makes a core behind (user decision)
                version_behind: model::version_behind_of(server_version, fleet_newest),
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
        let (groups, rows, alarm_starts, alarms) = {
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
            let scope = self.effective_scope(b);
            let core_ids: HashSet<CoreId> = scope.ids().iter().copied().collect();
            let server_addresses: HashSet<IpAddr> =
                groups.iter().filter_map(|group| group.address).collect();
            // The one query for the list the Warnings view draws, capped once. The strip counts
            // starts inside 24 hours; an episode that is still open but started earlier does not
            // count. Resolved here rather than in `render`, which runs on every row hover
            // transition and scroll tick.
            let episodes =
                b.warn_episodes_recent_for_scope(&core_ids, &server_addresses, WARN_LIST_LIMIT);
            let alarm_starts: Vec<i64> = episodes.iter().map(|episode| episode.start_ms).collect();
            let core_names: HashMap<CoreId, String> = b
                .config
                .servers
                .iter()
                .map(|server| (server.id, server.name.clone()))
                .collect();
            let alarms = warnings::AlarmsList::new(
                episodes,
                &groups,
                &b.layout.core_server_names,
                core_names,
            );
            (groups, rows, alarm_starts, alarms)
        };
        self.alarms = alarms;
        self.has_warn = groups.iter().any(|group| group.has_warn());
        replace_snapshots(
            &mut self.cached_rows,
            &mut self.cached_groups,
            &mut self.rows_generation,
            rows,
            groups,
        );
        self.cached_alarm_starts = Rc::new(alarm_starts);
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
    }

    /// Reuse the sorted snapshot and refresh exchange lines from current venues and locale.
    ///
    /// Everything returned is OWNED. [`moon_ui::MoonDataTable`]'s row closure is `'static`, so it
    /// cannot hold a `&CoreVenue` borrowed out of the session's venue map; resolving the sections
    /// here rather than at the call site is what keeps that borrow from ever reaching the closure.
    ///
    /// Args:
    ///     hidden: Column ids the user has unchecked. Computed once per render and passed in.
    ///     cx: Application context used to read the session's venue map.
    ///
    /// Returns:
    ///     The sorted rows, the heading/member lines addressing them by index, and the column
    ///     keys those rows are drawn with.
    pub(super) fn flat_view(
        &self,
        hidden: &HashSet<String>,
        cx: &App,
    ) -> (
        Rc<Vec<CoreStatusRow>>,
        Rc<Vec<ordering::FlatLine>>,
        Vec<&'static str>,
    ) {
        // The build closures must not capture `self`: they run while `flat_cache` is borrowed.
        // A hit check, then a build outside the borrow, then a store whose closure owns the value.
        let generation = self.rows_generation;
        let filter = self.health_filter;
        let ip_masked = self.ip_masked;
        let query = self.search_query.clone();
        let locale = rust_i18n::locale().to_string();
        let uniform = {
            let hit = self.flat_cache.borrow().uniform.as_ref().is_some_and(
                |(cached_generation, cached_filter, cached_query, cached_mask, _)| {
                    *cached_generation == generation
                        && *cached_filter == filter
                        && cached_query == &query
                        && *cached_mask == ip_masked
                },
            );
            if hit {
                self.flat_cache.borrow().uniform_facts()
            } else {
                let facts = projection_uniform(
                    &self.cached_rows,
                    &self.cached_groups,
                    filter,
                    &query,
                    ip_masked,
                );
                self.flat_cache.borrow_mut().cached_uniform(
                    generation,
                    filter,
                    &query,
                    ip_masked,
                    || facts,
                )
            }
        };
        let uniform_keys: Vec<&str> = uniform.iter().map(|fact| fact.key).collect();
        let floors = {
            let hit = self.flat_cache.borrow().floors.as_ref().is_some_and(
                |(cached_generation, cached_locale, _)| {
                    *cached_generation == generation && cached_locale == &locale
                },
            );
            if hit {
                self.flat_cache.borrow().floors()
            } else {
                let built = table::content_floors(&self.cached_rows, cx);
                self.flat_cache
                    .borrow_mut()
                    .cached_floors(generation, &locale, || built)
            }
        };
        // One list for the header and every row. The drag map is read inside this block so the
        // borrow ends before the venue read below. Width 0 (not measured yet) keeps every key.
        let visible = {
            let dragged = &self.table_state.read(cx).column_widths;
            table::projected_column_keys(
                &self.cached_rows,
                hidden,
                &uniform_keys,
                dragged,
                self.flat_measured_w,
                &floors,
            )
        };
        let sort = self
            .flat_sort
            .as_ref()
            .filter(|(key, _)| visible.iter().any(|column| *column == key))
            .cloned();
        debug_assert_eq!(
            self.search_query,
            normalize_query(&self.search_query),
            "set_search stores the comparison form"
        );
        let rows = {
            let hit = {
                let cache = self.flat_cache.borrow();
                cache.generation == generation
                    && cache.sort.as_ref() == sort.as_ref()
                    && cache.filter == filter
                    && cache.query == query
                    && cache.ip_masked == ip_masked
                    && cache.rows.is_some()
            };
            if hit {
                self.flat_cache
                    .borrow()
                    .rows
                    .clone()
                    .expect("hit keeps the projected rows")
            } else {
                let projected = project_rows(
                    &self.cached_rows,
                    &self.cached_groups,
                    filter,
                    &query,
                    ip_masked,
                    sort.as_ref(),
                    &visible,
                );
                self.flat_cache.borrow_mut().get_or_build(
                    generation,
                    sort.as_ref(),
                    filter,
                    &query,
                    ip_masked,
                    || projected,
                )
            }
        };
        let backend = self.backend.read(cx);
        let venues = backend.session.core_venues();
        let lines = Rc::new(ordering::flat_lines(&rows, venues));
        (rows, lines, visible)
    }

    /// Column keys the user has unchecked in the Columns menu.
    ///
    /// No stored entry means nothing is hidden: the table's own data rules still drop an API
    /// column that no row reports. The stored list is the columns the user left checked, so a
    /// column that later gains data stays visible until the user hides it.
    ///
    /// Args:
    ///     cx: Application context used to read the layout.
    ///
    /// Returns:
    ///     Canonical column ids the user has hidden. Empty when the layout has no entry.
    pub(super) fn user_hidden_columns(&self, cx: &App) -> HashSet<String> {
        let Some(shown) =
            crate::persistence::table_persist::visible(self.backend.read(cx), &self.widths_id)
        else {
            return HashSet::new();
        };
        let shown: HashSet<&str> = shown.iter().map(String::as_str).collect();
        table::order_keys()
            .iter()
            .copied()
            .filter(|key| !shown.contains(key))
            .map(str::to_string)
            .collect()
    }
}

/// Trim and lowercase a toolbar search before it joins the projection key.
///
/// Args:
///     query: Raw field text.
///
/// Returns:
///     The comparison form. Empty when the field is blank or only whitespace.
pub(super) fn normalize_query(query: &str) -> String {
    query.trim().to_lowercase()
}
