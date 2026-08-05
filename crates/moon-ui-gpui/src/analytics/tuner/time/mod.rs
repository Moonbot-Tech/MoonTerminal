//! "By time" mode of the "Strategy tuning" tab. On the left — the same strategy
//! list as in "By filter" (a click sets the profile scope); on the right — KPI +
//! the schedule grid; at the bottom, full width — a SINGLE "by hour of day" chart
//! for the SELECTED period: 24 hour columns left to right, each with a vertical
//! PnL bar growing from the center line (green up / orange down) + the value.
//! Data comes from one bounded streaming time-profile scan of the selected period.

// Files of this axis.
/// Its weekly schedule grid.
mod grid;
/// Its Save / Make-a-copy actions (feeding the shared confirmation dialog).
mod save;
/// Its three colour-profiled range sliders.
mod sliders;
/// Its own state: the schedule values, their parsers and the suggestion settings.
pub(in crate::analytics::tuner) mod state;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::super::summary::{fmt_signed, sign_color};
use super::kpi::kpi_matrix_card;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::HourStat;

impl AnalyticsView {
    /// Recompute the time axis after a user-controlled draft or scope change.
    ///
    /// Args:
    ///     cx: GPUI context used to run and publish the combined background read.
    pub(in crate::analytics) fn reload_time(&mut self, cx: &mut Context<Self>) {
        self.reload_time_inner(false, true, cx);
    }

    /// Recompute report-stale time data and retry transient database contention.
    ///
    /// Args:
    ///     show_overlay: Whether queued user work requires blocking progress feedback.
    ///     cx: GPUI context used to run and publish the combined background read.
    pub(in crate::analytics) fn reload_time_after_report(
        &mut self,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        self.reload_time_inner(true, show_overlay, cx);
    }

    /// Start one time-axis snapshot with behavior selected by its reload cause.
    ///
    /// Args:
    ///     after_report: Whether to preserve report-style draft and retry semantics.
    ///     show_overlay: Whether this refresh must block interaction with visible progress feedback.
    ///     cx: GPUI context used to run and publish the combined background read.
    fn reload_time_inner(
        &mut self,
        after_report: bool,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        if !after_report {
            self.report_busy_retries.reset();
            self.tuner.mark_dialog_draft_changed();
        }
        self.time_tuner.dirty = false;
        self.time_tuner.seq = self.time_tuner.seq.wrapping_add(1);
        let req = self.time_tuner.seq;
        let report_req = self.current_report_generation();
        self.time_tuner.profiles.begin();
        self.time_tuner.slider.begin();
        self.time_tuner.stats.begin();
        let base = self.tuner_query();
        // The KPI variants are built by the schedule GRID, not by the profile — the two
        // queries are independent: profiles feed the heatmap, variant_stats feeds
        // "Fact vs v1/v2".
        let variants = self.time_tuner.variants();
        // (sid, core) of the selected per-core row — the raw current values must be read from
        // the SAME core the schedule save targets, not the newest-checked one.
        let sid_core = self
            .sel_strategy
            .as_ref()
            .and_then(|(k, _)| super::parse_strat_key(k));
        let sid = sid_core.map(|(s, _)| s);
        let core = sid_core.and_then(|(_, c)| c);
        self.spawn_latest_db(
            &[crate::analytics::bg::ReadLane::Time],
            show_overlay,
            cx,
            move || {
                let report = moon_core::db::tuner::time_tuner_data(&base, &variants);
                // Raw field values + the ignore flags (for display and the Save logic).
                let current: Option<([String; 2], bool, bool)> = match sid {
                    Some(sid) => moon_core::db::tuner::strategy_current_values_opt(
                        sid,
                        core,
                        &[
                            "WorkingWeekTime".to_string(),
                            "WorkingTime".to_string(),
                            "IgnoreTime".to_string(),
                            "IgnoreFilters".to_string(),
                        ],
                    )
                    .map(|m| {
                        let truthy = |k: &str| {
                            matches!(
                                m.get(k).map(|s| s.trim().to_ascii_uppercase()).as_deref(),
                                Some("YES" | "TRUE" | "1")
                            )
                        };
                        (
                            [
                                m.get("WorkingWeekTime").cloned().unwrap_or_default(),
                                m.get("WorkingTime").cloned().unwrap_or_default(),
                            ],
                            truthy("IgnoreTime"),
                            truthy("IgnoreFilters"),
                        )
                    }),
                    None => Some(Default::default()),
                };
                (report.profiles, report.stats, current, report.slider)
            },
            move |this, (profiles, stats, current, slider), cx| {
                if this.time_tuner.seq != req {
                    return; // the period/filters/strategy/grid have already changed
                }
                let retry = profiles
                    .as_ref()
                    .err()
                    .filter(|error| error.kind() == Some(moon_core::db::FailKind::Busy))
                    .or_else(|| {
                        stats
                            .as_ref()
                            .err()
                            .filter(|error| error.kind() == Some(moon_core::db::FailKind::Busy))
                    })
                    .or_else(|| {
                        slider
                            .as_ref()
                            .err()
                            .filter(|error| error.kind() == Some(moon_core::db::FailKind::Busy))
                    })
                    .cloned();
                this.time_tuner.dirty = super::super::refresh::report_result_is_stale(
                    report_req,
                    this.current_report_generation(),
                    profiles.is_err() || stats.is_err() || slider.is_err() || current.is_none(),
                );
                // The profile, slider colors, and KPI are independent load surfaces, so each
                // retains its own classified error instead of collapsing it to "no data".
                this.time_tuner.profiles.apply(profiles);
                this.time_tuner.slider.apply(slider);
                this.time_tuner.stats.apply(stats);
                this.time_tuner.apply_current_read(current, after_report);
                if after_report {
                    this.settle_report_refresh_retry(retry.as_ref(), cx);
                }
                cx.notify();
            },
        );
    }

    /// Body of the "By time" mode — the layout MIRRORS "By filter": on the left the
    /// list (top) + the heatmap (bottom, where the histogram sits); on the right a
    /// full-height column — the "Fact vs variants" KPI (top) + the schedule grid (bottom).
    pub(in crate::analytics::tuner) fn strat_time(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Elements built up front: the &self widgets are computed before the &mut grid
        // (time_grid lazily creates inputs) so the borrows do not overlap.
        let list = self.strat_list_card(p, window, cx);
        let heatmap = self.time_heatmap(p, cx);
        let kpi = self.time_kpi(p, cx);
        let grid = self.time_grid(p, window, cx);
        // The write-confirmation dialog is SHARED with "By filter" (`save_dialog_overlay`);
        // we render it here too, because "By time" returns from `strategies_tab` earlier
        // than the point where that path adds the overlay.
        let dialog = self.save_dialog_overlay(p, cx);
        // Mouse-capture overlay for the duration of a slider drag (on top of the whole body).
        let drag_overlay = self.slider_drag_overlay(cx);
        let body = v_flex()
            .size_full()
            .p(design::ui_px(cx, 10.0))
            .gap(design::ui_px(cx, 8.0))
            // Top: the strategy list (left) + KPI/grid/sliders (right, 470).
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .gap(design::ui_px(cx, 8.0))
                    .child(div().flex_1().min_w_0().h_full().min_h_0().child(list))
                    .child(
                        // Just wide enough for the "WorkingWeekTime" name column plus the v1/v2
                        // inputs on one line — no slack for the "in strategy" column to inflate.
                        v_flex()
                            .w(design::font_w_px(cx, 540.0))
                            .flex_none()
                            .h_full()
                            .min_h_0()
                            .gap(design::ui_px(cx, 8.0))
                            .child(kpi)
                            .child(grid),
                    ),
            )
            // Bottom: the "By hour" heatmap at FULL width (same height as the filter histogram).
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .h(design::ui_px(cx, 210.0))
                    .child(heatmap),
            );
        div()
            .relative()
            .size_full()
            .child(body)
            .children(dialog)
            .children(drag_overlay)
            .into_any_element()
    }

    /// Top of the right column: the UNIVERSAL "Fact vs variants" KPI matrix (Fact /
    /// v1 / v2 over the weekly schedule taken from the grid) for the active period and
    /// the scope of the selected strategy. The same widget as in "By filter".
    fn time_kpi(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // An empty label list → "v1"/"v2" (as in "By filter").
        kpi_matrix_card(
            &self.time_tuner.stats,
            self.scope_label(),
            &[],
            self.kpi_collapsed,
            p,
            cx,
        )
    }

    /// Build the bottom "by hour" chart for the selected period.
    ///
    /// Args:
    ///     p: Active Moon palette.
    ///     cx: GPUI application context used for scaled dimensions and labels.
    ///
    /// Returns:
    ///     The chart card or its classified loading/error placeholder.
    fn time_heatmap(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // The single profile = the selected period (see `reload_time`).
        let profiles = match self.time_tuner.profiles.view(|_| false) {
            Ok(profiles) => profiles,
            Err(note) => {
                return crate::load_state::note_el("an-time-profile-note", note, 8.0, p, cx);
            }
        };
        let prof = profiles.first().copied();
        // Scope: the selected strategy's name, their count, or "all strategies".
        let scope = self.scope_label();
        // Bar scale — max |PnL| across the 24 hours of the selected period.
        let max = prof.map_or(0.0, |pr| {
            pr.iter().fold(0.0f64, |m, h| m.max(h.profit.abs()))
        });

        let body: AnyElement = match prof {
            None => h_flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.time.no_data").to_string())
                .into_any_element(),
            Some(pr) => {
                let mut row = h_flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .gap(design::ui_px(cx, 2.0));
                for (h, stat) in pr.iter().enumerate() {
                    row = row.child(hour_column(h, *stat, max, p, cx));
                }
                row.into_any_element()
            }
        };

        v_flex()
            .size_full()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .px(design::ui_px(cx, 12.0))
                    .py(design::ui_px(cx, 8.0))
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        div()
                            .text_size(design::t_title(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("analytics.time.by_hour").to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(scope),
                    ),
            )
            .child(
                div()
                    .id("an-time-body")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .px(design::ui_px(cx, 8.0))
                    .pb(design::ui_px(cx, 8.0))
                    .child(body),
            )
            .into_any_element()
    }
}

/// Height of a column's bar area (px before font scaling); the bar grows from the center by ≤ half.
const BAR_H: f32 = 108.0;

/// A single hour column: exact-unit PnL, trade count, center-origin bar, and hour.
/// Bar up = profit (green), down = loss (orange); length ∝ |PnL| / maximum.
///
/// Args:
///     h: UTC hour index.
///     stat: Hour aggregate in the active comparable unit.
///     max: Largest absolute profit across the rendered profile.
///     p: Active MoonUI palette.
///     cx: Analytics view context.
///
/// Returns:
///     One complete hour column.
fn hour_column(
    h: usize,
    stat: HourStat,
    max: f64,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    let profit = stat.profit;
    let has = stat.trades > 0;
    let frac = if max > 0.0 {
        (profit.abs() / max).min(1.0) as f32
    } else {
        0.0
    };
    // Loss = orange (same as sign_color and the "Summary" bars), NOT red.
    let bar_color = if profit > 0.0 { p.green } else { p.orange };
    let value = if has {
        let formatted = fmt_signed(profit);
        if crate::analytics::pnl_is_pct() {
            formatted
        } else {
            format!("{} {}", formatted, crate::analytics::pnl_unit_label())
        }
    } else {
        "—".to_string()
    };
    let count = if has {
        format!("({})", stat.trades)
    } else {
        String::new()
    };
    let r = design::ui_px(cx, 2.0);
    let half = BAR_H / 2.0;
    v_flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(design::ui_px(cx, 1.0))
        // Value (on top).
        .child(
            div()
                .flex_none()
                .w_full()
                .truncate()
                .text_center()
                .text_size(design::t_caption(cx))
                .text_color(moon(if has {
                    sign_color(p, profit)
                } else {
                    p.text_muted
                }))
                .child(value),
        )
        // Trade count (smaller, muted).
        .child(
            div()
                .flex_none()
                .w_full()
                .truncate()
                .text_center()
                .text_size(design::t_caption(cx))
                .text_color(moon_alpha(p.text_muted, 0.7))
                .child(count),
        )
        // Fixed-height bar area: the center tick + the bar up/down (px, not a relative
        // height — that one is unreliable inside a flex context).
        .child(
            div()
                .relative()
                .w_full()
                .flex_none()
                .h(design::ui_px(cx, BAR_H))
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .right(px(0.0))
                        .top(design::ui_px(cx, half))
                        .h(px(1.0))
                        .bg(moon_alpha(p.border, 0.8)),
                )
                .when(has && frac > 0.0 && profit > 0.0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .left(relative(0.2))
                            .right(relative(0.2))
                            .bottom(design::ui_px(cx, half))
                            .h(design::ui_px(cx, half * frac))
                            .rounded(r)
                            .bg(moon(bar_color)),
                    )
                })
                .when(has && frac > 0.0 && profit < 0.0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .left(relative(0.2))
                            .right(relative(0.2))
                            .top(design::ui_px(cx, half))
                            .h(design::ui_px(cx, half * frac))
                            .rounded(r)
                            .bg(moon(bar_color)),
                    )
                }),
        )
        // Hour (at the bottom).
        .child(
            div()
                .flex_none()
                .w_full()
                .text_center()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(format!("{h:02}")),
        )
}
