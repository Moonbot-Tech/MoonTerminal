//! Core Status presentation rendering.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::rc::Rc;

use super::mode::{UPDATE_LIST_LIMIT, WARN_LIST_LIMIT};
use super::model::{CoreStatusRow, ServerKey};
use super::{
    CoreStatusMode, CoreStatusView, chart, model, ordering, problems, server_view, table,
    updates_list, warnings,
};
use crate::design;
use gpui::*;
use moon_core::session::CoreId;
use moon_core::session::core_update::CoreUpdatePhase;
use moon_ui::{MoonPalette, v_flex};

impl Render for CoreStatusView {
    /// Render the active presentation with shared filters and counters.
    ///
    /// Args:
    ///     window: Host window; its ACTIVE state gates the Problems read-mark, so a tab left in
    ///         front while the operator works in another app does not consume the badge.
    ///     cx: View context used for backend reads, palette, and callbacks.
    ///
    /// Returns:
    ///     Full dock, detached-window, or group-host panel contents.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::CORE_STATUS_RENDER);
        let _render_us = crate::diag::scope(&crate::diag::CORE_STATUS_RENDER_US);
        // The interactive picker must agree with the rows below it, which already route through
        // `effective_workspace_scope`; `scope_cores` stays unfiltered for the retained-selection
        // callers that still need the full list.
        let cores = self.displayed_scope_cores(self.backend.read(cx));
        let rows = self.cached_rows.clone();
        let groups = self.cached_groups.clone();
        let p = MoonPalette::active(cx);
        let total_cores = rows.len();

        let marker = self.cached_scope_marker;
        // Built AFTER the mode content, not before: the Problems arm needs `&mut self` to record
        // what it just drew as looked at, and these two hold a borrow of `self` that would outlive
        // the match. The order is not merely free — it is better: the mode strip reads
        // `unseen_problems`, so building it after the mark shows the count the operator has just
        // consumed rather than the one from before they looked.
        let content = match self.mode {
            CoreStatusMode::ByIp => server_view::grouped_server_view(
                groups.clone(),
                self.ip_masked,
                self.editing,
                self.edit_input.clone(),
                self.chart_server,
                self.core_selection.clone(),
                self.group_sort,
                self.by_ip_width,
                // Row insets are `rems`, so the By-IP width budget needs the window's rem size —
                // MoonUI's Root sets it from the theme font size, which the legacy font-delta
                // channel moves.
                f32::from(window.rem_size()),
                // The user's dragged widths, BORROWED: the callee resolves them into `Copy`
                // geometries synchronously and nothing in the render tree holds the map, so a
                // per-frame clone of it would buy nothing on a path that repaints on every hover.
                &self.by_ip_col_widths.read(cx).column_widths,
                &self.tree_state,
                // Handed down rather than read off the view inside the callee: we are already
                // inside this view's own update here, and `cx.entity().read(cx)` there is a
                // process-killing panic. We hold the handle, so we pass it.
                &self.backend,
                &marker,
                // `&Window` is enough: `Window::listener_for` takes `&self`, so the header's
                // drag-move listener needs no mutable borrow.
                window,
                cx,
            ),
            CoreStatusMode::Flat => {
                let server_names: HashMap<ServerKey, String> = groups
                    .iter()
                    .map(|group| (group.key, group.display_name.clone()))
                    .collect();
                let column_keys = table::visible_column_keys(&rows);
                let saved_sort_visible = self
                    .flat_sort
                    .as_ref()
                    .is_none_or(|(key, _)| column_keys.contains(&key.as_str()));
                let (flat_rows, flat_lines) = if saved_sort_visible {
                    self.flat_view(cx)
                } else {
                    // A temporarily absent API column must not erase its persisted sort. Render the
                    // historical attention order until that column has real data and returns.
                    let flat_rows = model::ordered_flat_rows(&rows);
                    let venues = self.backend.read(cx).session.core_venues();
                    let flat_lines = ordering::flat_lines(&flat_rows, venues);
                    (Rc::new(flat_rows), Rc::new(flat_lines))
                };
                table::core_status_table(
                    "core-status-table",
                    flat_rows,
                    flat_lines,
                    Rc::new(column_keys),
                    Rc::new(server_names),
                    self.exchange_logos_ready,
                    self.flat_sort.is_some() && saved_sort_visible,
                    &self.table_state,
                    // Same reason as the By-IP arm above: the callee must not read this view.
                    &self.backend,
                    &marker,
                    self.core_selection.clone(),
                    cx,
                )
                .into_any_element()
            }
            CoreStatusMode::Problems => {
                // Everything the arm needs is collected inside this block so the backend borrow
                // ENDS before the read-mark, which needs `&mut self`. The alternative — marking up
                // in `render` — is what let the cap consume rows the surface never drew.
                let (rows, core_names, scope, picked, zone) = {
                    let b = self.backend.read(cx);
                    let effective = self.effective_scope(b);
                    // Scope order, then each core's own listing, is only the tie-break. The active
                    // column sort — newest confirmation first, when nothing was saved — runs on the
                    // whole list. The cap is applied after that sort, so a new confirmation on a
                    // late core is not dropped in favour of an old one on an early core.
                    let scope_ids = effective.ids();
                    let core_names: HashMap<CoreId, String> = b
                        .config
                        .servers
                        .iter()
                        .map(|server| (server.id, server.name.clone()))
                        .collect();
                    let (sort_key, sort_ascending) =
                        problems::shown_sort(self.problems_sort.as_ref());
                    let zone = moon_core::util::display_time::zone_or_utc(b.header_clock_zone());
                    let mut refs: Vec<problems::ProblemRef<'_>> = Vec::new();
                    let mut silent: Vec<String> = Vec::new();
                    // `matched` counts the cores in scope whatever their connection, `targets` only
                    // the reachable ones: the command channel outlives a disconnect, so a command
                    // queued for a core that is down waits there and fires on reconnect — for the
                    // reset, against findings gathered during the outage that nobody ever saw. The
                    // difference is what lets a shut gate say "the core you picked is down" rather
                    // than "pick one".
                    let mut matched = 0usize;
                    let mut matched_only = None;
                    let mut targets = 0usize;
                    // The same dash the table uses for an unnamed core, so one core cannot appear
                    // under two different spellings on the one surface.
                    let name_of = |core: CoreId| problems::core_label(&core_names, core);
                    for core in scope_ids.iter().copied() {
                        // Counted for EVERY core in scope, before anything can skip the rest of the
                        // body. A core with no store entry is still a core this panel covers, and
                        // counting only the ones that have connected would let a twenty-six-core
                        // scope report "exactly one core" and open the test on a core nobody chose.
                        matched += 1;
                        matched_only = (matched == 1).then_some(core);
                        // ONE lookup per core: this loop runs on every repaint of the arm, and the
                        // store search is the expensive half of it.
                        let Some(data) = b.session.store().core(core) else {
                            // No retained state at all is the same "not known" case as an answer
                            // without support: named, never read as clean.
                            silent.push(name_of(core));
                            continue;
                        };
                        // A core that has never delivered a list is still one all three actions
                        // must reach — see `ProblemsScope::targets` for why `supported` does not
                        // narrow this.
                        if moon_core::session::CoreRunState::from_core(data).online {
                            targets += 1;
                        }
                        if !data.problems.supported {
                            silent.push(name_of(core));
                            continue;
                        }
                        problems::extend_supported(&mut refs, core, &data.problems);
                    }
                    // Sort first, then cut. The notice states a cut list rather than shortening it
                    // in silence, for the same reason a silent core is named. Only the rows that
                    // survive the cut are cloned.
                    let truncated = problems::order_and_cap(
                        &mut refs,
                        &sort_key,
                        sort_ascending,
                        &core_names,
                        zone,
                    );
                    let rows: Vec<problems::ProblemRow> = refs
                        .into_iter()
                        .map(|row| problems::ProblemRow {
                            core: row.core,
                            problem: row.problem.clone(),
                        })
                        .collect();
                    // A CLICKED FINDING narrows all three actions to its core, and that pick is
                    // the operator's own — set by the click, held as a CORE, never re-derived from
                    // a row index into a list this arm rebuilds every repaint. No pick keeps the
                    // whole scope, which is this panel's own rule for a bulk command
                    // (`selected_or_visible`).
                    //
                    // Dropped as soon as the surface can no longer SHOW it. Clicking a row of its
                    // core is the only gesture that clears a pick, so a pick whose findings have
                    // all gone — reset on the core, or its `supported` flipped back to unknown —
                    // would sit there narrowing an irreversible action with nothing on screen
                    // saying so and no way to undo it. Losing it widens the actions back to the
                    // scope instead, which the tooltip counts and the confirm names core by core.
                    // Checked against the DRAWN rows, so the cap cannot keep a pick alive that the
                    // operator cannot reach either.
                    let picked = self
                        .problems_picked
                        .filter(|core| rows.iter().any(|row| row.core == *core));
                    // One store lookup, only on the frames where a pick is live.
                    let (matched, matched_only, targets) = match picked {
                        None => (matched, matched_only, targets),
                        Some(core) => {
                            let online = b.session.store().core(core).is_some_and(|d| {
                                moon_core::session::CoreRunState::from_core(d).online
                            });
                            (1, Some(core), usize::from(online))
                        }
                    };
                    let scope = problems::ProblemsScope {
                        cores: scope_ids.len(),
                        silent,
                        truncated,
                        // The test needs ONE core and says which reason it lacks: nothing narrowed
                        // to one, or the one it has is down.
                        actions: match (matched, matched_only, targets) {
                            (1, Some(core), 1) => problems::ActionGate::Ready(core),
                            (1, Some(_), _) => problems::ActionGate::NotConnected,
                            _ => problems::ActionGate::NoSingleChoice,
                        },
                        // A live pick answers for ITSELF, so a scope that still holds connected
                        // cores must not be reported as offline just because the picked one is —
                        // that is the conflation the two refusals exist to keep apart.
                        picked: picked.is_some(),
                        targets,
                    };
                    (rows, core_names, scope, picked, zone)
                };
                // A pick whose core left the scope is dropped for good, not just for this frame:
                // left behind, it would silently re-arm the moment that core came back.
                if self.problems_picked != picked {
                    self.problems_picked = picked;
                }
                // Drawing the findings IS looking at them — the News panel's rule, and for its
                // reason. The window-active guard is what stops the badge being consumed unseen: an
                // inactive window still repaints on the shell's clock tick, and "the tab was in
                // front while you worked elsewhere" is not "you looked".
                if window.is_window_active() {
                    self.mark_problems_seen(&rows, cx);
                }
                problems::problems_view(
                    "core-status-problems",
                    Rc::new(rows),
                    Rc::new(core_names),
                    &scope,
                    picked,
                    &self.problems_table_state,
                    zone,
                    cx,
                )
                .into_any_element()
            }
            CoreStatusMode::Warnings => {
                let b = self.backend.read(cx);
                let scope = self.effective_scope(b);
                let core_ids: HashSet<CoreId> = scope.ids().iter().copied().collect();
                let server_addresses: HashSet<IpAddr> =
                    groups.iter().filter_map(|group| group.address).collect();
                let episodes =
                    b.warn_episodes_recent_for_scope(&core_ids, &server_addresses, WARN_LIST_LIMIT);
                // Resolve each server IP to its display name (connected group, then a saved custom
                // name), never the raw IP — matching how the panel masks addresses.
                let server_names: HashMap<IpAddr, String> = episodes
                    .iter()
                    .filter_map(|episode| episode.server_ip)
                    .map(|ip| {
                        let name = groups
                            .iter()
                            .find(|group| group.address == Some(ip))
                            .map(|group| group.display_name.clone())
                            .or_else(|| b.layout.core_server_names.get(&ip.to_string()).cloned())
                            .unwrap_or_else(|| "—".to_string());
                        (ip, name)
                    })
                    .collect();
                let core_names: HashMap<CoreId, String> = b
                    .config
                    .servers
                    .iter()
                    .map(|server| (server.id, server.name.clone()))
                    .collect();
                warnings::warnings_table(
                    "core-status-warnings",
                    Rc::new(episodes),
                    Rc::new(server_names),
                    Rc::new(core_names),
                    &self.warn_table_state,
                    moon_core::util::display_time::zone_or_utc(b.header_clock_zone()),
                    cx,
                )
                .into_any_element()
            }
            CoreStatusMode::Updates => {
                let b = self.backend.read(cx);
                let scope = self.effective_scope(b);
                let core_ids: HashSet<CoreId> = scope.ids().iter().copied().collect();
                let now_ms = moon_core::util::now_unix_ms_i64();
                // Live rows come straight off `rows`: `CoreStatusRow.update` is already this
                // core's current phase, scoped by `query_cores` the same way every other mode
                // here is scoped. `Done`/`None` are excluded -- a finished attempt is already a
                // history record, and a core the queue has never touched has nothing to show.
                let live_rows: Vec<CoreStatusRow> = rows
                    .iter()
                    .filter(|row| {
                        matches!(
                            row.update,
                            Some(CoreUpdatePhase::Queued { .. })
                                | Some(CoreUpdatePhase::Sent { .. })
                                | Some(CoreUpdatePhase::Waiting { .. })
                                | Some(CoreUpdatePhase::Verifying { .. })
                        )
                    })
                    .cloned()
                    .collect();
                // Admit a record when EITHER: its core is still configured and in `core_ids` --
                // this keeps the sibling leak closed, so a still-configured core the user did not
                // select never shows through on a shared IP -- OR the core has been removed from
                // configuration entirely (it can no longer appear in `core_ids` at all, see
                // `EffectiveCoreScope::ids`) and its lane ran on an address this panel's scope
                // still covers. Without the second half, `reconcile_vanished_updates`'s
                // `Failed(Gone)` row -- and every other history row for that core -- becomes
                // unreachable the instant the core leaves the config, even though
                // `CoreUpdateRecord::core_name` was snapshotted at enqueue precisely so the row
                // could keep naming a core that no longer exists.
                let configured_core_ids: HashSet<CoreId> =
                    b.config.servers.iter().map(|server| server.id).collect();
                // The SCOPE's server addresses, not the live rows' (`update_ips`, below, is
                // downstream of this filter and only ever covers cores already admitted). Reuses
                // the same `groups` aggregate the Warnings branch above already derives its own
                // server-address set from.
                let scope_server_addrs: HashSet<IpAddr> =
                    groups.iter().filter_map(|group| group.address).collect();
                let history: Vec<moon_core::session::core_update::CoreUpdateRecord> = b
                    .session
                    .core_update_history()
                    .iter()
                    .rev()
                    .filter(|record| {
                        core_ids.contains(&record.core)
                            || (!configured_core_ids.contains(&record.core)
                                && scope_server_addrs.contains(&record.lane_addr))
                    })
                    .take(UPDATE_LIST_LIMIT)
                    .cloned()
                    .collect();
                let update_ips: HashSet<IpAddr> = live_rows
                    .iter()
                    .filter_map(|row| row.endpoint.map(|ep| ep.address))
                    .chain(history.iter().map(|record| record.lane_addr))
                    .collect();
                let server_names: HashMap<IpAddr, String> = update_ips
                    .into_iter()
                    .map(|ip| {
                        let name = groups
                            .iter()
                            .find(|group| group.address == Some(ip))
                            .map(|group| group.display_name.clone())
                            .or_else(|| b.layout.core_server_names.get(&ip.to_string()).cloned())
                            .unwrap_or_else(|| "—".to_string());
                        (ip, name)
                    })
                    .collect();
                updates_list::updates_table(
                    "core-status-updates",
                    Rc::new(live_rows),
                    Rc::new(history),
                    Rc::new(server_names),
                    &self.updates_table_state,
                    moon_core::util::display_time::zone_or_utc(b.header_clock_zone()),
                    now_ms,
                    cx,
                )
                .into_any_element()
            }
        };
        let core_bar = self.core_bar(&cores, cx);
        let footer = self.footer(&groups, total_cores, &marker, cx);

        // A detached window gets a live CPU/memory chart for ONE subject: a clicked core, else the
        // clicked (or first) server's machine aggregate — never per-core overlays. The dock tab builds
        // nothing here, so it pays no chart cost.
        let chart_el: Option<AnyElement> = self
            .detached
            .then(|| {
                let b = self.backend.read(cx);
                let now_sec = moon_chart::paint::now_unix_ms() as i64 / 1000;
                // A selected core, if it is still in scope, charts ITS OWN samples — CPU/RAM plus its
                // own client↔core and core→exchange pings, not the server-wide worst. Falls through to
                // the server aggregate otherwise.
                if let Some(core) = self.chart_core.and_then(|core_id| {
                    groups
                        .iter()
                        .flat_map(|group| group.cores.iter())
                        .find(|core| core.id == core_id)
                }) {
                    // Split the core's 4-metric samples into the (cpu, mem) machine lines and its own
                    // ping/exch series, all from the same per-core ring.
                    let (points, ping_points, exch_points): (
                        std::collections::VecDeque<(u8, u8)>,
                        std::collections::VecDeque<u16>,
                        std::collections::VecDeque<u16>,
                    ) = b
                        .core_line_hist
                        .ring(core.id)
                        .map(|r| {
                            (
                                r.iter().map(|m| (m.cpu, m.mem)).collect(),
                                r.iter().map(|m| m.ping).collect(),
                                r.iter().map(|m| m.exch).collect(),
                            )
                        })
                        .unwrap_or_default();
                    return Some(chart::server_chart(
                        &points,
                        &ping_points,
                        &exch_points,
                        core.name.clone(),
                        self.chart_window,
                        now_sec,
                        cx.entity().downgrade(),
                        p,
                        cx,
                    ));
                }
                // Follows the clicked server (chart_server), else the first server that HAS an address
                // — an address-less (unknown-endpoint) server has no history ring to chart, so it must
                // fall through rather than blank the chart.
                let target = self
                    .chart_server
                    .and_then(|key| groups.iter().find(|group| group.key == key))
                    .filter(|group| group.address.is_some())
                    .or_else(|| groups.iter().find(|group| group.address.is_some()))?;
                let ip = target.address?;
                let points = b.core_chart_hist.ring(ip).cloned().unwrap_or_default();
                let ping_points = b.server_ping_hist.ring(ip).cloned().unwrap_or_default();
                let exch_points = b.server_exch_hist.ring(ip).cloned().unwrap_or_default();
                Some(chart::server_chart(
                    &points,
                    &ping_points,
                    &exch_points,
                    target.display_name.clone(),
                    self.chart_window,
                    now_sec,
                    cx.entity().downgrade(),
                    p,
                    cx,
                ))
            })
            .flatten();

        v_flex()
            .id("core-status-panel")
            .size_full()
            .relative()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            // Ctrl+A / Escape for the core-row selection. The Flat grid intercepts the same
            // select-all chord itself when IT holds focus; this route is what gives the By-IP
            // tree, which is not a `MoonDataTable`, the identical gesture.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_selection_key(event, window, cx);
            }))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .child(core_bar)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(
                v_flex()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(content),
            )
            .children(chart_el.map(|el| {
                v_flex()
                    .w_full()
                    .flex_none()
                    .child(div().w_full().h(px(1.0)).bg(rgb(p.border)))
                    .child(el)
            }))
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(footer)
    }
}
