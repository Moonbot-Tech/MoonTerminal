//! Calendar "Day" mode: an hourly grid of 24 (hours) × 7 (days). The selected
//! day sits in the middle, but never higher — near the "today" edge it drops
//! to the bottom row (`day_window`). KPI tiles on top as in "Month", but
//! against the PREVIOUS day. Columns are labelled with hours, rows with the
//! date and weekday. Cells stretch.

use std::collections::HashMap;

use chrono::Datelike;
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{h_flex, v_flex, MoonPalette};
use rust_i18n::t;

use super::super::summary::{fmt_signed, sign_color};
use super::super::AnalyticsView;
use super::{date_of, day_window, split_i18n};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::DayCell;
use moon_core::util::fmt::compact;

/// Sign + compact form with no fraction — the hour cells are narrow.
fn hour_pnl(v: f64) -> String {
    let suffix = crate::analytics::pnl_suffix();
    if v > 0.0 {
        format!("+{}{}", compact(v, 0), suffix)
    } else {
        format!("{}{}", compact(v, 0), suffix)
    }
}

impl AnalyticsView {
    pub(super) fn calendar_day(
        &self,
        hours: &[DayCell],
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let map: HashMap<i64, &DayCell> = hours.iter().map(|c| (c.start, c)).collect();
        // Daily aggregate (over the window's hourly cells).
        let day_agg = |d0: i64| -> (f64, i64, i64) {
            hours
                .iter()
                .filter(|c| c.start >= d0 && c.start < d0 + 86_400)
                .fold((0.0f64, 0i64, 0i64), |a, c| {
                    (a.0 + c.profit, a.1 + c.trades, a.2 + c.wins)
                })
        };
        let sel = self.cal_day;
        let (profit, trades, wins) = day_agg(sel);
        let (pp, pt, pw) = day_agg(sel - 86_400);
        let has_prev = pt > 0 || pp != 0.0;

        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .p(design::ui_px(cx, 10.0))
            .gap(design::ui_px(cx, 8.0))
            .child(self.day_kpi(profit, trades, wins, (pp, pt, pw), has_prev, p, cx))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.day_grid(&map, p, cx)),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn day_kpi(
        &self,
        profit: f64,
        trades: i64,
        wins: i64,
        prev: (f64, i64, i64),
        has_prev: bool,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let (pp, pt, pw) = prev;
        let losses = trades - wins;
        let wr = if trades > 0 {
            wins as f64 / trades as f64 * 100.0
        } else {
            0.0
        };
        let prev_wr = if pt > 0 {
            pw as f64 / pt as f64 * 100.0
        } else {
            0.0
        };
        let dp = move |c: f64, pr: f64| -> Option<f64> {
            (has_prev && pr.abs() > f64::EPSILON).then(|| (c - pr) / pr.abs() * 100.0)
        };
        let tile = super::month::kpi_tile;
        h_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .items_stretch()
            .child(tile(
                p,
                cx,
                t!("analytics.cal.kpi_profit_day").to_string(),
                moon(sign_color(p, profit)),
                fmt_signed(profit),
                dp(profit, pp),
                false,
            ))
            .child(tile(
                p,
                cx,
                t!("analytics.kpi.trades").to_string(),
                moon(p.text),
                trades.to_string(),
                dp(trades as f64, pt as f64),
                false,
            ))
            .child(tile(
                p,
                cx,
                t!("analytics.cal.kpi_wins").to_string(),
                moon(p.green),
                wins.to_string(),
                dp(wins as f64, pw as f64),
                false,
            ))
            .child(tile(
                p,
                cx,
                t!("analytics.cal.kpi_losses").to_string(),
                moon(p.orange),
                losses.to_string(),
                dp(losses as f64, (pt - pw) as f64),
                true,
            ))
            .child(tile(
                p,
                cx,
                t!("analytics.kpi.winrate").to_string(),
                moon(p.text),
                format!("{wr:.1}%"),
                dp(wr, prev_wr),
                false,
            ))
    }

    /// Render the visible day window as weekday gutters and 24 hourly columns.
    ///
    /// Args:
    ///     map: Hour-start timestamps mapped to their trade aggregates.
    ///     p: Active MoonUI palette.
    ///     cx: GPUI view context used for sizing and day-selection listeners.
    ///
    /// Returns:
    ///     The complete Day grid.
    fn day_grid(
        &self,
        map: &HashMap<i64, &DayCell>,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let cell_gap = design::ui_px(cx, 4.0);
        let gutter_w = design::font_w_px(cx, 84.0);
        let wdays = split_i18n(t!("analytics.heat.weekdays").to_string());
        let (top, bottom) = day_window(self.cal_day);

        // Largest hourly |PnL| (for the fill) + the average profit per hour
        // across the visible days (only days that traded in that hour) — shown
        // under the hour in the header.
        let mut hour_max = 0.0f64;
        let mut hour_sum = [0.0f64; 24];
        let mut hour_cnt = [0i64; 24];
        {
            let mut d = top;
            while d <= bottom {
                for h in 0..24usize {
                    if let Some(c) = map.get(&(d + h as i64 * 3600)) {
                        if c.trades > 0 {
                            hour_max = hour_max.max(c.profit.abs());
                            hour_sum[h] += c.profit;
                            hour_cnt[h] += 1;
                        }
                    }
                }
                d += 86_400;
            }
        }

        // Header: empty left corner + hours 00..23 with the average below each.
        let mut head = h_flex().w_full().flex_none().gap(cell_gap);
        head = head.child(div().w(gutter_w).flex_none());
        for h in 0..24usize {
            let mut hc = v_flex().flex_1().min_w_0().items_center().child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(format!("{h:02}")),
            );
            if hour_cnt[h] > 0 {
                let avg = hour_sum[h] / hour_cnt[h] as f64;
                hc = hc.child(
                    div()
                        .text_size(design::t_caption(cx))
                        .whitespace_nowrap()
                        .text_color(moon(sign_color(p, avg)))
                        .child(format!(
                            "{}{}",
                            compact(avg, 1),
                            if crate::analytics::pnl_is_pct() {
                                "%"
                            } else {
                                "$"
                            }
                        )),
                );
            }
            head = head.child(hc);
        }

        // 7 day rows (top to bottom, chronological).
        let mut rows = v_flex().flex_1().min_h_0().w_full().gap(cell_gap);
        let mut d = top;
        while d <= bottom {
            let sel = d == self.cal_day;
            let dt = date_of(d);
            let wd = wdays
                .get(dt.weekday().num_days_from_monday() as usize)
                .cloned()
                .unwrap_or_default();
            // Date + weekday on ONE line; the whole column is clickable, so
            // ANY visible date can be picked (not just via Prev/Next).
            let gutter = h_flex()
                .id(("gd", d as u64))
                .w(gutter_w)
                .flex_none()
                .h_full()
                .overflow_hidden()
                .items_center()
                .justify_center()
                .gap(design::ui_px(cx, 4.0))
                .px(design::ui_px(cx, 6.0))
                .rounded(design::ui_px(cx, 4.0))
                .cursor_pointer()
                .when(sel, |el| {
                    el.bg(moon_alpha(p.amber, 0.16))
                        .border_1()
                        .border_color(moon_alpha(p.amber, 0.6))
                })
                .on_click(cx.listener(move |this, _, _, cx| this.cal_goto_day(d, cx)))
                .child(
                    div()
                        .text_size(design::t_body(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .whitespace_nowrap()
                        .text_color(moon(if sel { p.amber } else { p.text }))
                        .child(dt.format("%d.%m").to_string()),
                )
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .whitespace_nowrap()
                        .text_color(moon(if sel { p.amber } else { p.text_muted }))
                        .child(wd),
                );

            // min_h_0 — without it rows refuse to shrink below their content
            // and all 30 of them do not fit.
            let mut rowel = h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .gap(cell_gap)
                .child(gutter);
            for h in 0..24i64 {
                let hsec = d + h * 3600;
                rowel = rowel.child(hour_cell(
                    hsec,
                    map.get(&hsec).copied(),
                    hour_max,
                    sel,
                    p,
                    cx,
                ));
            }
            rows = rows.child(rowel);
            d += 86_400;
        }

        v_flex()
            .size_full()
            .gap(cell_gap)
            .child(head)
            .child(rows)
            .into_any_element()
    }
}

/// One hour cell of the Day grid: hourly PnL over a fill whose alpha tracks `|PnL|`.
///
/// This is a free function because the cell's highlight and rendering need no view state.
///
/// Args:
///     hsec: Cell's hour start, in unix seconds.
///     cell: Aggregate for that hour, or `None` when the hour holds no trades.
///     hour_max: Largest `|PnL|` across the visible grid, the fill's scale.
///     sel_day: Whether this cell belongs to the selected day's row.
///     p: Active MoonUI palette.
///     cx: Application context, for the UI-scale sizes only.
///
/// Returns:
///     The complete hour cell.
fn hour_cell(
    hsec: i64,
    cell: Option<&DayCell>,
    hour_max: f64,
    sel_day: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let r = design::ui_px(cx, 4.0);
    let profit = cell.map_or(0.0, |c| c.profit);
    let trades = cell.map_or(0, |c| c.trades);
    let tint = (trades > 0 && profit != 0.0 && hour_max > 0.0).then(|| {
        let a = (profit.abs() / hour_max).min(1.0) as f32 * 0.35;
        moon_alpha(if profit > 0.0 { p.green } else { p.red }, a)
    });
    // The selected day's row stands out: lighter background + amber cell border.
    let bg = if sel_day {
        moon(p.panel_high)
    } else {
        moon(p.panel)
    };
    let border = if sel_day {
        moon_alpha(p.amber, 0.45)
    } else {
        moon_alpha(p.border, 0.4)
    };
    let label: AnyElement = if trades > 0 {
        div()
            .text_size(design::t_caption(cx))
            .text_color(moon(sign_color(p, profit)))
            .child(hour_pnl(profit))
            .into_any_element()
    } else {
        div().into_any_element()
    };
    div()
        .id(("hc", hsec as u64))
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .rounded(r)
        .bg(bg)
        .border_1()
        .border_color(border)
        // Keep this self-highlight in element state. The stable id persists that state between
        // frames; `calendar_hover_is_element_state_not_view_state` guards the full contract.
        .hover(move |s| s.border_color(moon(p.text)))
        .children(tint.map(|tc| div().absolute().inset_0().rounded(r).bg(tc)))
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(label),
        )
        .into_any_element()
}
