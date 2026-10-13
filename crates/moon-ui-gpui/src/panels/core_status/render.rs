//! Core Status presentation rendering.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::rc::Rc;

use super::mode::UPDATE_LIST_LIMIT;
use super::model::{CoreStatusRow, ServerKey};
use super::{
    CoreStatusMode, CoreStatusView, chart, health, health_strip, interactions, problems, same_line,
    servers_view, table, updates_list, updates_pane, warnings,
};
use crate::Backend;
use crate::design;
use crate::workspace::scope_marker;
use gpui::*;
use moon_core::session::CoreId;
use moon_core::session::core_update::CoreUpdatePhase;
use moon_ui::{MoonButton, MoonPalette, v_flex};
use rust_i18n::t;

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
        let hidden = self.user_hidden_columns(cx);
        let now_ms = moon_core::util::now_unix_ms_i64();
        let counts = health::health_counts(&rows, &self.cached_alarm_starts, now_ms);
        let verdict = health::verdict(&rows, &counts, groups.len());
        let ping_factor = self.backend.read(cx).warn_params().ping.yellow;

        let marker = self.cached_scope_marker;
        // Built AFTER the mode content, not before: the Problems arm needs `&mut self` to record
        // what it just drew as looked at, and these two hold a borrow of `self` that would outlive
        // the match. The order is not merely free — it is better: the mode strip reads
        // `unseen_problems`, so building it after the mark shows the count the operator has just
        // consumed rather than the one from before they looked.
        let content = if rows.is_empty() {
            no_cores_empty(&self.backend, cx).into_any_element()
        } else {
            match self.mode {
                CoreStatusMode::Servers => {
                    let cpu_history = {
                        let b = self.backend.read(cx);
                        let mut history = HashMap::<IpAddr, Vec<(f32, f32)>>::new();
                        for group in groups.iter() {
                            let Some(ip) = group.address else {
                                continue;
                            };
                            let Some(ring) = b.core_chart_hist.ring(ip) else {
                                continue;
                            };
                            let denom = ring.len().saturating_sub(1).max(1) as f32;
                            let series = ring
                                .iter()
                                .enumerate()
                                .map(|(i, (cpu, _))| (i as f32 / denom, f32::from(*cpu)))
                                .collect();
                            history.insert(ip, series);
                        }
                        history
                    };
                    let grid = servers_view::servers_view(
                        &groups,
                        self.ip_masked,
                        self.editing,
                        self.edit_input.clone(),
                        self.chart_server,
                        self.chart_core,
                        &cpu_history,
                        &self.search_query,
                        self.core_selection.clone(),
                        self.servers_measured_w,
                        cx,
                    );
                    let weak = cx.entity().downgrade();
                    servers_view::servers_measure_host(grid, move |w, app| {
                        let Some(view) = weak.upgrade() else {
                            return;
                        };
                        if (view.read(app).servers_measured_w - w).abs() <= 0.5 {
                            return;
                        }
                        app.defer(move |app| {
                            view.update(app, |this, cx| {
                                if (this.servers_measured_w - w).abs() > 0.5 {
                                    this.servers_measured_w = w;
                                    cx.notify();
                                }
                            });
                        });
                    })
                    .into_any_element()
                }
                CoreStatusMode::Flat => {
                    let server_names: HashMap<ServerKey, String> = groups
                        .iter()
                        .map(|group| (group.key, group.display_name.clone()))
                        .collect();
                    let (flat_rows, flat_lines, column_keys) = self.flat_view(&hidden, cx);
                    let floors = self.flat_cache.borrow().floors();
                    let saved_sort_visible = self
                        .flat_sort
                        .as_ref()
                        .is_none_or(|(key, _)| column_keys.contains(&key.as_str()));
                    let table = table::core_status_table(
                        "core-status-table",
                        flat_rows,
                        flat_lines,
                        Rc::new(column_keys),
                        Rc::new(server_names),
                        self.exchange_logos_ready,
                        self.flat_sort.is_some() && saved_sort_visible,
                        &self.table_state,
                        // The table helper must not read this view while it is rendering.
                        &self.backend,
                        &marker,
                        self.core_selection.clone(),
                        &floors,
                        self.flat_measured_w,
                        cx,
                    )
                    .into_any_element();
                    // Width probe stays in the view's own render: the table helper never reads
                    // this entity. The host is width-capped, so the sample is the dock slot and
                    // a wide row cannot expand it and undo the drop. The write is deferred and
                    // skipped when the width moved by half a pixel or less. `0` stays
                    // "not measured" until this fires.
                    let weak = cx.entity().downgrade();
                    table::flat_measure_host(table, move |w, app| {
                        let Some(view) = weak.upgrade() else {
                            return;
                        };
                        if (view.read(app).flat_measured_w - w).abs() <= 0.5 {
                            return;
                        }
                        app.defer(move |app| {
                            view.update(app, |this, cx| {
                                if (this.flat_measured_w - w).abs() > 0.5 {
                                    this.flat_measured_w = w;
                                    cx.notify();
                                }
                            });
                        });
                    })
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
                        let zone =
                            moon_core::util::display_time::zone_or_utc(b.header_clock_zone());
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
                    // The list and its names come from the cache rebuild, never from here: this
                    // arm runs again on every row hover transition and every scroll tick.
                    let zone = moon_core::util::display_time::zone_or_utc(
                        self.backend.read(cx).header_clock_zone(),
                    );
                    warnings::warnings_table(
                        "core-status-warnings",
                        &self.alarms,
                        &self.warn_table_state,
                        &self.warn_scroll,
                        zone,
                        now_ms,
                        window,
                        cx,
                    )
                }
                CoreStatusMode::Updates => {
                    let visible = self.visible_cores(cx);
                    let selected = self.visible_selected(&visible);
                    let all_empty = self.fleet_update_plan(false, &visible, cx).cores.is_empty();
                    let behind_plan = self.fleet_update_plan(true, &visible, cx);
                    let behind_n = behind_plan.cores.len();
                    let fleet = self.backend.read(cx).session.core_update_summary();
                    let summary = campaign_summary(fleet.updating, fleet.queued, fleet.failed);
                    let shares = updates_list::version_distribution(&rows);
                    let lags = interactions::behind_in_scope(&rows) > 0;
                    let newest = shares
                        .iter()
                        .find(|share| share.newest)
                        .map(|share| super::presentation::build_label(&share.parts))
                        .unwrap_or_else(|| "—".to_string());
                    let weak = cx.entity().downgrade();
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
                    let phases: HashMap<CoreId, CoreUpdatePhase> = rows
                        .iter()
                        .filter_map(|row| row.update.clone().map(|phase| (row.id, phase)))
                        .collect();
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
                                .or_else(|| {
                                    b.layout.core_server_names.get(&ip.to_string()).cloned()
                                })
                                .unwrap_or_else(|| "—".to_string());
                            (ip, name)
                        })
                        .collect();
                    updates_pane::updates_pane(
                        &shares,
                        behind_n,
                        selected,
                        all_empty,
                        behind_n == 0,
                        &summary,
                        lags,
                        Rc::new(live_rows),
                        Rc::new(history),
                        Rc::new(phases),
                        Rc::new(server_names),
                        &self.updates_table_state,
                        moon_core::util::display_time::zone_or_utc(b.header_clock_zone()),
                        now_ms,
                        &newest,
                        weak,
                        &self.backend,
                        cx,
                    )
                    .into_any_element()
                }
            }
        };
        let tail_facts = marker.facts();
        let scope_tail = scope_marker::scope_footer_tail(
            "core-status-health-tail",
            tail_facts.clone(),
            marker.tooltip(&tail_facts),
            design::t_body(cx),
            p.text_soft,
        )
        .map(|tail| tail.into_any_element());
        let weak_health = cx.entity().downgrade();
        let weak_alarms = weak_health.clone();
        let on_filter: Rc<dyn Fn(health::HealthFilter, &mut App)> = Rc::new(move |filter, app| {
            let Some(view) = weak_health.upgrade() else {
                return;
            };
            view.update(app, |this, cx| this.toggle_health_filter(filter, cx));
        });
        let on_alarms: Rc<dyn Fn(&mut App)> = Rc::new(move |app| {
            let Some(view) = weak_alarms.upgrade() else {
                return;
            };
            view.update(app, |this, cx| this.open_alarms(cx));
        });
        let strip = health_strip::health_strip(
            &verdict,
            &counts,
            self.health_filter,
            scope_tail,
            ping_factor,
            cx,
            on_filter,
            on_alarms,
        );
        let segments = health::segment_counts(&verdict, &counts, groups.len());
        let core_bar = self.core_bar(&cores, &hidden, &segments, cx);
        let same = matches!(self.mode, CoreStatusMode::Flat)
            .then(|| same_line::same_line(&self.flat_cache.borrow().uniform_facts(), cx))
            .flatten()
            .map(|line| {
                div()
                    .w_full()
                    .flex_none()
                    .border_b_1()
                    .border_color(rgb(p.border_soft))
                    .child(line)
            });

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
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            // Ctrl+A / Escape for the core-row selection. The Flat grid intercepts the same
            // select-all chord itself when IT holds focus; this route gives a focused descendant
            // that is not that grid the identical gesture.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_selection_key(event, window, cx);
            }))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .child(strip)
            .child(core_bar)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .children(same)
            .child(
                v_flex()
                    .w_full()
                    .min_w(px(0.0))
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
    }
}

/// Joined campaign counters, empty when nothing is updating, queued, or failed.
fn campaign_summary(updating: usize, queued: usize, failed: usize) -> String {
    [
        (updating, "core_update.summary.updating"),
        (queued, "core_update.summary.queued"),
        (failed, "core_update.summary.failed"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, key)| format!("{n} {}", t!(key)))
    .collect::<Vec<_>>()
    .join(" - ")
}

/// Empty state when the panel's scope has no cores. The button opens Settings on Cores.
fn no_cores_empty(backend: &Entity<Backend>, cx: &mut Context<CoreStatusView>) -> impl IntoElement {
    let backend = backend.clone();
    let p = MoonPalette::active(cx);
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(design::ui_px(cx, 8.0))
        .p(design::ui_px(cx, 16.0))
        .child(
            div()
                .flex_none()
                .w(design::ui_px(cx, 40.0))
                .h(design::ui_px(cx, 40.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(p.panel_head))
                .text_color(rgb(p.text_muted))
                .font_weight(FontWeight::BOLD)
                .text_size(design::t_body_lg(cx))
                .child("∅"),
        )
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(design::t_body_lg(cx))
                .text_color(rgb(p.text))
                .child(t!("core_status.empty.no_cores_title").to_string()),
        )
        .child(
            div()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("core_status.empty.no_cores_body").to_string()),
        )
        .child(
            MoonButton::new("core-status-open-settings")
                .label(t!("core_status.empty.open_settings").to_string())
                .primary()
                .on_click(move |_, window, app| {
                    crate::settings::open(
                        backend.clone(),
                        Some(window.window_handle()),
                        crate::window::windowing::window_display_id(window, app),
                        app,
                    );
                })
                .render(),
        )
}
