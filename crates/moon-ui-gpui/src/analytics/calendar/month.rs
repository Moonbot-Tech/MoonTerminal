//! Calendar "Month" mode: large day cards (date top-left; PnL, win rate with its
//! counts, turnover and execution cost, and average holding time on the right),
//! grey background + red/green overlay whose alpha tracks |PnL| (the month's top
//! day = 30%). A KPI row with the delta to the previous month on top, a
//! plus/minus-day bar below. Click a day → "Day".
//!
//! Money figures are optional throughout: a percent projection has no currency to
//! sum, so the turnover and cost tiles disappear rather than reading zero.

use std::collections::HashMap;

use chrono::Datelike;
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::super::summary::{fmt_signed, fmt_signed_unit, sign_color};
use super::{
    date_of, days_in_month, fmt_amount, fmt_duration_short, fmt_volume, month_start, split_i18n,
    today_start,
};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::{CellTotals, DayCell};

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
        // One fold, so the KPI row and the grid below can never describe different months.
        let mut month = CellTotals::default();
        for day in days {
            month.merge(&day.totals);
        }
        // Day tallies count what the user SEES on a card: a day whose only row was funding shows
        // money and therefore belongs to the plus/minus split, exactly as it does to the profit.
        let pos_days = days
            .iter()
            .filter(|d| d.has_activity() && d.totals.profit > 0.0)
            .count();
        let neg_days = days
            .iter()
            .filter(|d| d.has_activity() && d.totals.profit < 0.0)
            .count();
        let active = days.iter().filter(|d| d.has_activity()).count();
        let neutral = active - pos_days - neg_days;
        // Fill scale — the month's largest daily |PnL| (top day = 30% alpha).
        let month_max = days
            .iter()
            .filter(|d| d.has_activity())
            .map(|d| d.totals.profit.abs())
            .fold(0.0f64, f64::max);
        let today = today_start(self.bound_zone());

        let map: HashMap<i64, &DayCell> = days.iter().map(|d| (d.start, d)).collect();
        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .p(design::ui_px(cx, 10.0))
            .gap(design::ui_px(cx, 8.0))
            .child(self.cal_kpi(&month, p, cx))
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
    ///     month: Current-month totals folded from the visible cells.
    ///     p: Active MoonUI palette.
    ///     cx: GPUI view context used for sizing.
    ///
    /// Returns:
    ///     The Month KPI row.
    fn cal_kpi(&self, month: &CellTotals, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        // Deltas are against the PREVIOUS month (not 30 days); None when the
        // previous month is missing or zero.
        let previous = self.cal_prev.data().and_then(|value| **value);
        let base = previous.unwrap_or_default();
        let has = previous.is_some();
        let dp = move |c: f64, pr: f64| -> Option<f64> {
            (has && pr.abs() > f64::EPSILON).then(|| (c - pr) / pr.abs() * 100.0)
        };
        let wr = month.winrate().unwrap_or(0.0);
        h_flex()
            .w_full()
            .flex_wrap()
            .gap(design::ui_px(cx, 8.0))
            .items_stretch()
            .child(kpi_tile(
                p,
                cx,
                "cal-month-profit",
                t!("analytics.cal.kpi_profit").to_string(),
                None,
                moon(sign_color(p, month.profit)),
                fmt_signed_unit(month.profit),
                dp(month.profit, base.profit),
                false,
            ))
            // One trade is one round trip: the core books an entry and its exit as a single closed
            // order, so this counts orders, not fills. Funding rows are not among them.
            .child(kpi_tile(
                p,
                cx,
                "cal-month-trades",
                t!("analytics.kpi.trades").to_string(),
                None,
                moon(p.text),
                month.trades.to_string(),
                dp(month.trades as f64, base.trades as f64),
                false,
            ))
            .child(kpi_tile(
                p,
                cx,
                "cal-month-winrate",
                t!("analytics.kpi.winrate").to_string(),
                None,
                moon(p.text),
                // The counts ride along so the rate states what it rests on — the tile beside it
                // gives the total, this gives the split: "68.7% (8299/12086)".
                format!("{wr:.1}% ({}/{})", month.wins, month.trades),
                dp(wr, base.winrate().unwrap_or(0.0)),
                false,
            ))
            // Each money tile appears only when THAT figure exists. A percent projection has
            // neither; a legacy source without execution prices has a turnover but no cost, and
            // rendering the absent one as "0" would state a cost that was never measured.
            .children(month.volume.map(|volume| {
                kpi_tile(
                    p,
                    cx,
                    "cal-month-volume",
                    t!("analytics.cal.kpi_volume").to_string(),
                    None,
                    moon(p.text),
                    fmt_volume(volume),
                    base.volume.and_then(|prev| dp(volume, prev)),
                    false,
                )
            }))
            .children(month.fee.map(|fee| {
                kpi_tile(
                    p,
                    cx,
                    "cal-month-costs",
                    t!("analytics.cal.kpi_fee").to_string(),
                    Some(t!("analytics.cal.kpi_fee_tip").to_string()),
                    moon(p.orange),
                    fmt_amount(fee, month.fee_is_complete()),
                    // Growth in cost is bad, so the delta's good direction is inverted.
                    base.fee.and_then(|prev| dp(fee, prev)),
                    true,
                )
            }))
            // Funding is not a trading result — it accrues for holding a position, and a month can
            // be green on trades while red on funding alone. It is signed, so it keeps profit's
            // colouring, and it shows even at zero: "no funding this month" is an answer.
            .children(month.funding.map(|funding| {
                kpi_tile(
                    p,
                    cx,
                    "cal-month-funding",
                    t!("analytics.cal.kpi_funding").to_string(),
                    Some(t!("analytics.cal.kpi_funding_tip").to_string()),
                    moon(sign_color(p, funding)),
                    fmt_signed(funding),
                    base.funding.and_then(|prev| dp(funding, prev)),
                    false,
                )
            }))
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

        let first = month_start(y, m, self.bound_zone());
        let first_date = date_of(first, self.bound_zone());
        let lead = first_date.weekday().num_days_from_monday() as i64;
        let anchor = moon_core::util::display_time::shift_date(first_date, -lead);
        let ndays = days_in_month(y, m) as usize;
        let n_rows = (lead as usize + ndays).div_ceil(7);

        let mut weeks = v_flex().flex_1().min_h_0().w_full().gap(cell_gap);
        for row in 0..n_rows {
            let mut rowel = h_flex().flex_1().w_full().gap(cell_gap);
            for col in 0..7 {
                let dt = moon_core::util::display_time::shift_date(anchor, (row * 7 + col) as i64);
                let t = super::super::exact_secs_of_day(dt, self.bound_zone());
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
    let profit = day.map_or(0.0, |d| d.totals.profit);
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
        // A day is shown in full when it has ANY activity: a funding-only day counts no trades
        // yet moved money, and blanking it would lose a figure the month total still carries.
        let right = if let Some(d) = day.filter(|d| d.has_activity()) {
            let t = &d.totals;
            v_flex()
                .items_end()
                .child(
                    div()
                        .text_size(design::t_title(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(sign_color(p, t.profit)))
                        .child(fmt_signed(t.profit)),
                )
                // Rate first, with the counts it rests on — the shape the month KPI uses too.
                .child(muted(match t.winrate() {
                    Some(wr) => format!("{wr:.1}% ({}/{})", t.wins, t.trades),
                    None => t!("analytics.heat.trades_full", n = 0).to_string(),
                }))
                .children(t.volume.map(|volume| {
                    muted(match t.fee {
                        Some(fee) => format!(
                            "{} · {} {}",
                            fmt_volume(volume),
                            t!("analytics.cal.fee_short"),
                            fmt_amount(fee, t.fee_is_complete())
                        ),
                        None => fmt_volume(volume),
                    })
                }))
                .children(
                    t.funding
                        .filter(|funding| *funding != 0.0)
                        .map(|funding| {
                            div()
                                .text_size(design::t_caption(cx))
                                .text_color(moon(sign_color(p, funding)))
                                .child(format!(
                                    "{} {}",
                                    t!("analytics.cal.funding_short"),
                                    fmt_signed(funding)
                                ))
                        }),
                )
                .children(
                    t.avg_duration_secs()
                        .map(|secs| muted(format!("~{}", fmt_duration_short(secs)))),
                )
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
        };
        h_flex()
            .w_full()
            .items_start()
            .child(date_el)
            .child(div().flex_1())
            .child(right)
            .into_any_element()
    };
    let active = day.is_some_and(|d| d.has_activity());
    let tint = (!date_only && active && profit != 0.0 && month_max > 0.0).then(|| {
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

/// Build a Calendar KPI tile shared by Month and Day views.
///
/// Args:
///     p: Active MoonUI palette.
///     cx: GPUI view context used for sizing.
///     id: Stable element identity required by GPUI's tooltip state.
///     label: Localized metric caption.
///     tooltip: Optional localized explanation attached to the existing tile root.
///     value_color: Colour applied to the primary value.
///     value: Formatted primary value.
///     delta: Percentage change from the previous period, when comparable.
///     invert: Whether growth in the metric should use the adverse delta colour.
///
/// Returns:
///     KPI tile with unchanged geometry and an optional standard MoonUI tooltip.
pub(super) fn kpi_tile(
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
    id: &'static str,
    label: String,
    tooltip: Option<String>,
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
    let tile = v_flex()
        .id(id)
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
        .child(delta_el);
    if let Some(tooltip) = tooltip {
        tile.tooltip(crate::panels::common::text_tooltip(tooltip))
    } else {
        tile
    }
}
