//! Calendar "Month" mode: large day cards (date top-left, PnL + trades + W/L +
//! winrate on the right), grey background + red/green overlay whose alpha
//! tracks |PnL| (the month's top day = 30%). A KPI row with the delta to the
//! previous month on top, a plus/minus-day bar below. Click a day → "Day".

use std::collections::HashMap;

use chrono::Datelike;
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::super::summary::{fmt_signed, fmt_signed_unit, sign_color};
use super::{date_of, days_in_month, month_start, split_i18n, today_start};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::DayCell;

impl AnalyticsView {
    /// Render the selected month with KPI, day grid, and active-day balance.
    ///
    /// Args:
    ///     days: Daily Calendar cells for the displayed month.
    ///     p: Active MoonUI palette.
    ///     cx: GPUI view context used for sizing and listeners.
    ///
    /// Returns:
    ///     The complete Month-mode content below Calendar navigation.
    pub(super) fn calendar_month(
        &self,
        days: &[DayCell],
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let profit: f64 = days.iter().map(|d| d.profit).sum();
        let trades: i64 = days.iter().map(|d| d.trades).sum();
        let wins: i64 = days.iter().map(|d| d.wins).sum();
        let losses = trades - wins;
        let wr = if trades > 0 {
            wins as f64 / trades as f64 * 100.0
        } else {
            0.0
        };
        let pos_days = days
            .iter()
            .filter(|d| d.trades > 0 && d.profit > 0.0)
            .count();
        let neg_days = days
            .iter()
            .filter(|d| d.trades > 0 && d.profit < 0.0)
            .count();
        let active = days.iter().filter(|d| d.trades > 0).count();
        let neutral = active - pos_days - neg_days;
        // Fill scale — the month's largest daily |PnL| (top day = 30% alpha).
        let month_max = days
            .iter()
            .filter(|d| d.trades > 0)
            .map(|d| d.profit.abs())
            .fold(0.0f64, f64::max);
        let today = today_start(self.display_zone);

        let map: HashMap<i64, &DayCell> = days.iter().map(|d| (d.start, d)).collect();
        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .p(design::ui_px(cx, 10.0))
            .gap(design::ui_px(cx, 8.0))
            .child(self.cal_kpi(profit, trades, wins, losses, wr, p, cx))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.cal_grid(&map, month_max, today, p, cx)),
            )
            .child(self.cal_bottom(pos_days, neg_days, active, neutral, p, cx))
            .into_any_element()
    }

    /// Render Month KPI values and deltas against the loaded previous-month aggregate.
    ///
    /// Args:
    ///     profit: Current-month profit.
    ///     trades: Current-month trade count.
    ///     wins: Current-month winning-trade count.
    ///     losses: Current-month losing-trade count.
    ///     wr: Current-month win rate in percent.
    ///     p: Active MoonUI palette.
    ///     cx: GPUI view context used for sizing.
    ///
    /// Returns:
    ///     The Month KPI row.
    #[allow(clippy::too_many_arguments)]
    fn cal_kpi(
        &self,
        profit: f64,
        trades: i64,
        wins: i64,
        losses: i64,
        wr: f64,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        // Deltas are against the PREVIOUS month (not 30 days); None when the
        // previous month is missing or zero.
        let previous = self.cal_prev.data().and_then(|value| **value);
        let (pp, pt, pw) = previous.unwrap_or((0.0, 0, 0));
        let has = previous.is_some();
        let dp = move |c: f64, pr: f64| -> Option<f64> {
            (has && pr.abs() > f64::EPSILON).then(|| (c - pr) / pr.abs() * 100.0)
        };
        let prev_wr = if pt > 0 {
            pw as f64 / pt as f64 * 100.0
        } else {
            0.0
        };
        h_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .items_stretch()
            .child(kpi_tile(
                p,
                cx,
                t!("analytics.cal.kpi_profit").to_string(),
                moon(sign_color(p, profit)),
                fmt_signed_unit(profit),
                dp(profit, pp),
                false,
            ))
            .child(kpi_tile(
                p,
                cx,
                t!("analytics.kpi.trades").to_string(),
                moon(p.text),
                trades.to_string(),
                dp(trades as f64, pt as f64),
                false,
            ))
            .child(kpi_tile(
                p,
                cx,
                t!("analytics.cal.kpi_wins").to_string(),
                moon(p.green),
                wins.to_string(),
                dp(wins as f64, pw as f64),
                false,
            ))
            .child(kpi_tile(
                p,
                cx,
                t!("analytics.cal.kpi_losses").to_string(),
                moon(p.orange),
                losses.to_string(),
                dp(losses as f64, (pt - pw) as f64),
                true,
            ))
            .child(kpi_tile(
                p,
                cx,
                t!("analytics.kpi.winrate").to_string(),
                moon(p.text),
                format!("{wr:.1}%"),
                dp(wr, prev_wr),
                false,
            ))
    }

    /// Render the displayed month as weekday headers and week rows.
    ///
    /// Args:
    ///     map: Day-start timestamps mapped to their trade aggregates.
    ///     month_max: Largest daily `|PnL|` in the month, used to scale cell fills.
    ///     today: Current selected-zone day start, used to classify future cells.
    ///     p: Active MoonUI palette.
    ///     cx: GPUI view context used for sizing and day-selection listeners.
    ///
    /// Returns:
    ///     The complete Month grid.
    fn cal_grid(
        &self,
        map: &HashMap<i64, &DayCell>,
        month_max: f64,
        today: i64,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (y, m) = self.cal_ym;
        let cell_gap = design::ui_px(cx, 6.0);
        let wdays = split_i18n(t!("analytics.heat.weekdays").to_string());
        let mut head = h_flex().w_full().flex_none().gap(cell_gap);
        for wd in wdays.iter().take(7) {
            head = head.child(
                div()
                    .flex_1()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(wd.clone()),
            );
        }

        let first = month_start(y, m, self.display_zone);
        let first_date = date_of(first, self.display_zone);
        let lead = first_date.weekday().num_days_from_monday() as i64;
        let anchor = moon_core::util::display_time::shift_date(first_date, -lead);
        let ndays = days_in_month(y, m) as usize;
        let n_rows = (lead as usize + ndays).div_ceil(7);

        let mut weeks = v_flex().flex_1().min_h_0().w_full().gap(cell_gap);
        for row in 0..n_rows {
            let mut rowel = h_flex().flex_1().w_full().gap(cell_gap);
            for col in 0..7 {
                let dt = moon_core::util::display_time::shift_date(anchor, (row * 7 + col) as i64);
                let t = super::super::exact_secs_of_day(dt, self.display_zone);
                let dom = dt.day();
                let in_month = dt.month() == m && dt.year() == y;
                let is_future = t.is_some_and(|start| start > today);
                let day = if in_month {
                    t.and_then(|start| map.get(&start).copied())
                } else {
                    None
                };
                rowel = rowel.child(cal_cell(t, dom, day, in_month, is_future, month_max, p, cx));
            }
            weeks = weeks.child(rowel);
        }
        v_flex()
            .size_full()
            .gap(cell_gap)
            .child(head)
            .child(weeks)
            .into_any_element()
    }

    /// Render the plus/minus-day bar under the grid: counts, a proportional split, and the
    /// active/neutral tallies.
    ///
    /// Args:
    ///     pos: Days closed in profit.
    ///     neg: Days closed at a loss.
    ///     active: Days that traded at all.
    ///     neutral: Days that traded and closed flat.
    ///     p: Active MoonUI palette.
    ///     cx: GPUI view context used for sizing.
    ///
    /// Returns:
    ///     The bar below the Month grid.
    fn cal_bottom(
        &self,
        pos: usize,
        neg: usize,
        active: usize,
        neutral: usize,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let total = (pos + neg).max(1) as f32;
        v_flex()
            .flex_none()
            .w_full()
            .gap(design::ui_px(cx, 4.0))
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .text_size(design::t_caption(cx))
                    .child(div().text_color(moon(p.green)).child(format!(
                        "{} {}",
                        t!("analytics.cal.pos_days"),
                        pos
                    )))
                    .child(div().text_color(moon(p.red)).child(format!(
                        "{} {}",
                        t!("analytics.cal.neg_days"),
                        neg
                    ))),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(design::ui_px(cx, 6.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .overflow_hidden()
                    .bg(moon_alpha(p.border, 0.4))
                    .child(
                        div()
                            .h_full()
                            .w(relative(pos as f32 / total))
                            .bg(moon(p.green)),
                    )
                    .child(
                        div()
                            .h_full()
                            .w(relative(neg as f32 / total))
                            .bg(moon(p.red)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(format!("{}: {}", t!("analytics.cal.active_days"), active))
                    .child(format!("{}: {}", t!("analytics.cal.neutral_days"), neutral)),
            )
    }
}

/// One day card of the Month grid: the date, and — for a day of this month that is not in the
/// future — its PnL, trades, W/L and winrate over a fill whose alpha tracks `|PnL|`.
///
/// This is a free function because the card's rendering needs no view state.
///
/// Args:
///     dsec: Exact day start, or `None` for a civil date skipped by the selected zone.
///     dom: Day of month, the number drawn in the corner.
///     day: Aggregate for that day, or `None` outside the month, without trades, or when skipped.
///     in_month: Whether the day belongs to the displayed month.
///     is_future: Whether the day is still ahead of today.
///     month_max: Largest daily `|PnL|` in the grid, the fill's scale.
///     p: Active MoonUI palette.
///     cx: GPUI context, for UI-scale sizes and the click listener.
///
/// Returns:
///     The complete day card.
#[allow(clippy::too_many_arguments)]
fn cal_cell(
    dsec: Option<i64>,
    dom: u32,
    day: Option<&DayCell>,
    in_month: bool,
    is_future: bool,
    month_max: f64,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let pad = design::ui_px(cx, 8.0);
    let r = design::ui_px(cx, 8.0);
    let date_only = !in_month || is_future || dsec.is_none();
    let profit = day.map_or(0.0, |d| d.profit);
    let trades = day.map_or(0, |d| d.trades);
    let date_el = div()
        .text_size(design::t_title(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(moon(if date_only { p.text_muted } else { p.text }))
        .child(dom.to_string());
    let inner: AnyElement = if date_only {
        date_el.into_any_element()
    } else {
        let muted = |txt: String| {
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(txt)
        };
        let right = if let Some(d) = day.filter(|d| d.trades > 0) {
            let dwr = d.wins as f64 / d.trades as f64 * 100.0;
            v_flex()
                .items_end()
                .child(
                    div()
                        .text_size(design::t_title(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(sign_color(p, d.profit)))
                        .child(fmt_signed(d.profit)),
                )
                .child(muted(
                    t!("analytics.heat.trades_full", n = d.trades).to_string(),
                ))
                .child(muted(format!("{}W {}L", d.wins, d.trades - d.wins)))
                .child(muted(format!("WR {dwr:.1}%")))
        } else {
            v_flex()
                .items_end()
                .child(
                    div()
                        .text_size(design::t_title(cx))
                        .text_color(moon(p.text_muted))
                        .child("—"),
                )
                .child(muted(t!("analytics.heat.trades_full", n = 0).to_string()))
                .child(muted("0W 0L".to_string()))
                .child(muted("WR —".to_string()))
        };
        h_flex()
            .w_full()
            .items_start()
            .child(date_el)
            .child(div().flex_1())
            .child(right)
            .into_any_element()
    };
    let tint = (!date_only && trades > 0 && profit != 0.0 && month_max > 0.0).then(|| {
        let a = (profit.abs() / month_max).min(1.0) as f32 * 0.30;
        moon_alpha(if profit > 0.0 { p.green } else { p.red }, a)
    });
    let bg = if in_month {
        moon(p.panel)
    } else {
        moon(p.shell)
    };
    let border = if in_month {
        moon_alpha(p.border, 0.5)
    } else {
        moon_alpha(p.border, 0.3)
    };
    let cell = div()
        .id(("mc", dsec.unwrap_or(-i64::from(dom)) as u64))
        .relative()
        .flex_1()
        .h_full()
        .overflow_hidden()
        .rounded(r)
        .bg(bg)
        .border_1()
        .border_color(border);
    // A fully skipped civil date has no instant to open and remains a non-interactive label.
    let cell = if let Some(dsec) = dsec {
        cell.cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.cal_goto_day(dsec, cx)))
    } else {
        cell
    };
    // Highlight only cards that show figures; existing date-only cards remain clickable.
    let cell = if date_only {
        cell
    } else {
        cell.hover(move |s| s.border_color(moon(p.text)))
    };
    cell.children(tint.map(|tc| div().absolute().inset_0().rounded(r).bg(tc)))
        .child(div().absolute().inset_0().p(pad).child(inner))
        .into_any_element()
}

/// Calendar KPI tile: label + large value + delta to the previous period.
/// `invert` — growth in this metric is bad (losing orders). Shared by
/// Month/Day.
pub(super) fn kpi_tile(
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
    label: String,
    value_color: Hsla,
    value: String,
    delta: Option<f64>,
    invert: bool,
) -> impl IntoElement {
    let delta_el = match delta {
        Some(d) if d.is_finite() && d.abs() > 0.05 => {
            let good = if invert { d < 0.0 } else { d > 0.0 };
            let col = if good { p.green } else { p.orange };
            h_flex()
                .gap(design::ui_px(cx, 4.0))
                .items_center()
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(col))
                        .child(format!(
                            "{} {:.1}%",
                            if d > 0.0 { "▲" } else { "▼" },
                            d.abs()
                        )),
                )
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.vs_prev").to_string()),
                )
                .into_any_element()
        }
        _ => div()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_muted))
            .child("—")
            .into_any_element(),
    };
    v_flex()
        .flex_1()
        .min_w(design::font_w_px(cx, 108.0))
        .gap(design::ui_px(cx, 3.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 9.0))
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_soft))
                .child(label),
        )
        .child(
            div()
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(value_color)
                .child(value),
        )
        .child(delta_el)
}
