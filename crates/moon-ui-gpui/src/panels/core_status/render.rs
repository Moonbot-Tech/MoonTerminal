//! Core Status presentation rendering.

use std::rc::Rc;

use super::{
    CoreStatusMode, CoreStatusView, chart, frame_cache, health, health_strip, problems, same_line,
    servers_view, table, updates_pane, warnings,
};
use crate::Backend;
use crate::design;
use crate::workspace::scope_marker;
use gpui::*;
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
        let scope_model = self.frame_scope(self.backend.read(cx));
        let rows = self.cached_rows.clone();
        let p = MoonPalette::active(cx);
        let hidden = self.user_hidden_columns(cx);
        let now_ms = moon_core::util::now_unix_ms_i64();
        let strip_model = self.frame_strip(now_ms);
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
                    let cpu_history = self.frame_cpu_history(self.backend.read(cx));
                    let cards = self.frame_server_cards();
                    let grid = servers_view::servers_view(
                        &cards,
                        self.ip_masked,
                        self.editing,
                        self.edit_input.clone(),
                        self.chart_server,
                        self.chart_core,
                        &cpu_history,
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
                    let server_names = self.frame_server_names();
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
                        server_names,
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
                    // Resolved from the store once per snapshot, sort and pick
                    // (`CoreStatusView::frame_problems`). The backend borrow ends here, before
                    // the read-mark, which needs `&mut self`.
                    let model = self.frame_problems(self.backend.read(cx));
                    // A pick whose core left the scope is dropped for good, not just for this
                    // frame: left behind, it would silently re-arm the moment that core came back.
                    if self.problems_picked != model.picked {
                        self.problems_picked = model.picked;
                    }
                    // Drawing the findings IS looking at them, the News panel's rule. The
                    // window-active guard stops the badge being consumed unseen: an inactive
                    // window still repaints on the shell's clock tick.
                    if window.is_window_active() {
                        self.mark_problems_seen(&model.rows, model.mark_sig, cx);
                    }
                    problems::problems_view(
                        "core-status-problems",
                        model.rows.clone(),
                        model.core_names.clone(),
                        &model.scope,
                        model.picked,
                        &self.problems_table_state,
                        model.zone,
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
                    let model = self.frame_updates(self.backend.read(cx));
                    // Only the selection-dependent counts are per frame, and they walk the selected
                    // cores alone; an empty selection reads the memoized counts.
                    let (selected, all_n, behind_n) =
                        frame_cache::selection_counts(&model, self.core_selection.iter());
                    let weak = cx.entity().downgrade();
                    updates_pane::updates_pane(
                        &model.shares,
                        behind_n,
                        selected,
                        all_n == 0,
                        behind_n == 0,
                        &model.summary,
                        model.lags,
                        model.live_rows.clone(),
                        model.history.clone(),
                        model.history_newest.clone(),
                        model.phases.clone(),
                        model.server_names.clone(),
                        &self.updates_table_state,
                        model.zone,
                        now_ms,
                        &model.newest,
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
            &strip_model.verdict,
            &strip_model.counts,
            self.health_filter,
            scope_tail,
            ping_factor,
            cx,
            on_filter,
            on_alarms,
        );
        let core_bar = self.core_bar(&scope_model, &hidden, &strip_model.segments, cx);
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
                let model = self.frame_chart(self.backend.read(cx));
                let model = model.as_ref().as_ref()?;
                let now_sec = moon_chart::paint::now_unix_ms() as i64 / 1000;
                Some(chart::server_chart(
                    &model.points,
                    &model.ping_points,
                    &model.exch_points,
                    model.title.clone(),
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
pub(super) fn campaign_summary(updating: usize, queued: usize, failed: usize) -> String {
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
