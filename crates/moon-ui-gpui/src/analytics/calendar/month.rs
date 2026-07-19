//! Режим «Месяц» календаря: крупные карточки-дни (дата слева сверху, справа —
//! PnL + сделки + W/L + winrate), серый фон + красно/зелёный оверлей
//! прозрачностью по |PnL| (топ месяца = 30%). Сверху ряд KPI с дельтой к
//! предыдущему месяцу, снизу — полоса плюс/минус-дней. Клик по дню → «День».

use std::collections::HashMap;

use chrono::Datelike;
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::super::summary::{fmt_signed, sign_color};
use super::{date_of, days_in_month, month_start, split_i18n, today_start};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::DayCell;

impl AnalyticsView {
    pub(super) fn calendar_month(&self, days: &[DayCell], p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let profit: f64 = days.iter().map(|d| d.profit).sum();
        let trades: i64 = days.iter().map(|d| d.trades).sum();
        let wins: i64 = days.iter().map(|d| d.wins).sum();
        let losses = trades - wins;
        let wr = if trades > 0 { wins as f64 / trades as f64 * 100.0 } else { 0.0 };
        let pos_days = days.iter().filter(|d| d.trades > 0 && d.profit > 0.0).count();
        let neg_days = days.iter().filter(|d| d.trades > 0 && d.profit < 0.0).count();
        let active = days.iter().filter(|d| d.trades > 0).count();
        let neutral = active - pos_days - neg_days;
        // Масштаб заливки — максимум |PnL| дня месяца (топ = 30% прозрачности).
        let month_max = days.iter().filter(|d| d.trades > 0).map(|d| d.profit.abs()).fold(0.0f64, f64::max);
        let today = today_start();

        let map: HashMap<i64, &DayCell> = days.iter().map(|d| (d.start, d)).collect();
        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .p(design::ui_px(cx, 10.0))
            .gap(design::ui_px(cx, 8.0))
            .child(self.cal_kpi(profit, trades, wins, losses, wr, p, cx))
            .child(div().flex_1().w_full().min_h(px(0.0)).child(self.cal_grid(&map, month_max, today, p, cx)))
            .child(self.cal_bottom(pos_days, neg_days, active, neutral, p, cx))
            .into_any_element()
    }

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
        // Дельты — к ПРЕДЫДУЩЕМУ месяцу (не 30 дней); None — прошлого нет/ноль.
        let (pp, pt, pw) = self.cal_prev.unwrap_or((0.0, 0, 0));
        let has = self.cal_prev.is_some();
        let dp = move |c: f64, pr: f64| -> Option<f64> {
            (has && pr.abs() > f64::EPSILON).then(|| (c - pr) / pr.abs() * 100.0)
        };
        let prev_wr = if pt > 0 { pw as f64 / pt as f64 * 100.0 } else { 0.0 };
        h_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .items_stretch()
            .child(kpi_tile(p, cx, t!("analytics.cal.kpi_profit").to_string(), moon(sign_color(p, profit)), fmt_signed(profit), dp(profit, pp), false))
            .child(kpi_tile(p, cx, t!("analytics.kpi.trades").to_string(), moon(p.text), trades.to_string(), dp(trades as f64, pt as f64), false))
            .child(kpi_tile(p, cx, t!("analytics.cal.kpi_wins").to_string(), moon(p.green), wins.to_string(), dp(wins as f64, pw as f64), false))
            .child(kpi_tile(p, cx, t!("analytics.cal.kpi_losses").to_string(), moon(p.orange), losses.to_string(), dp(losses as f64, (pt - pw) as f64), true))
            .child(kpi_tile(p, cx, t!("analytics.kpi.winrate").to_string(), moon(p.text), format!("{wr:.1}%"), dp(wr, prev_wr), false))
    }

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

        let first = month_start(y, m);
        let lead = date_of(first).weekday().num_days_from_monday() as i64;
        let anchor = first - lead * 86_400; // понедельник недели 1-го числа
        let ndays = days_in_month(y, m) as usize;
        let n_rows = (lead as usize + ndays).div_ceil(7);

        let mut weeks = v_flex().flex_1().min_h_0().w_full().gap(cell_gap);
        for row in 0..n_rows {
            let mut rowel = h_flex().flex_1().w_full().gap(cell_gap);
            for col in 0..7 {
                let t = anchor + ((row * 7 + col) as i64) * 86_400;
                let dt = date_of(t);
                let dom = dt.day();
                let in_month = dt.month() == m && dt.year() == y;
                let is_future = t > today;
                let day = if in_month { map.get(&t).copied() } else { None };
                rowel = rowel.child(self.cal_cell(t, dom, day, in_month, is_future, month_max, p, cx));
            }
            weeks = weeks.child(rowel);
        }
        v_flex().size_full().gap(cell_gap).child(head).child(weeks).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn cal_cell(
        &self,
        dsec: i64,
        dom: u32,
        day: Option<&DayCell>,
        in_month: bool,
        is_future: bool,
        month_max: f64,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let pad = design::ui_px(cx, 8.0);
        let r = design::ui_px(cx, 8.0);
        let hovered = self.cal_hover == Some(dsec);
        let date_only = !in_month || is_future;
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
                div().text_size(design::t_caption(cx)).text_color(moon(p.text_muted)).child(txt)
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
                    .child(muted(t!("analytics.heat.trades_full", n = d.trades).to_string()))
                    .child(muted(format!("{}W {}L", d.wins, d.trades - d.wins)))
                    .child(muted(format!("WR {dwr:.1}%")))
            } else {
                v_flex()
                    .items_end()
                    .child(div().text_size(design::t_title(cx)).text_color(moon(p.text_muted)).child("—"))
                    .child(muted(t!("analytics.heat.trades_full", n = 0).to_string()))
                    .child(muted("0W 0L".to_string()))
                    .child(muted("WR —".to_string()))
            };
            h_flex().w_full().items_start().child(date_el).child(div().flex_1()).child(right).into_any_element()
        };
        let tint = (!date_only && trades > 0 && profit != 0.0 && month_max > 0.0).then(|| {
            let a = (profit.abs() / month_max).min(1.0) as f32 * 0.30;
            moon_alpha(if profit > 0.0 { p.green } else { p.red }, a)
        });
        let bg = if in_month { moon(p.panel) } else { moon(p.shell) };
        let border = if !date_only && hovered {
            moon(p.text)
        } else if in_month {
            moon_alpha(p.border, 0.5)
        } else {
            moon_alpha(p.border, 0.3)
        };
        let cell = div()
            .id(("mc", dsec as u64))
            .relative()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .cursor_pointer()
            .rounded(r)
            .bg(bg)
            .border_1()
            .border_color(border)
            // Клик по дню → детализация «День».
            .on_click(cx.listener(move |this, _, _, cx| this.cal_goto_day(dsec, cx)));
        let cell = if date_only { cell } else { cell.on_hover(self.cell_hover(dsec, cx)) };
        cell.children(tint.map(|tc| div().absolute().inset_0().rounded(r).bg(tc)))
            .child(div().absolute().inset_0().p(pad).child(inner))
            .into_any_element()
    }

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
                    .child(div().text_color(moon(p.green)).child(format!("{} {}", t!("analytics.cal.pos_days"), pos)))
                    .child(div().text_color(moon(p.red)).child(format!("{} {}", t!("analytics.cal.neg_days"), neg))),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(design::ui_px(cx, 6.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .overflow_hidden()
                    .bg(moon_alpha(p.border, 0.4))
                    .child(div().h_full().w(relative(pos as f32 / total)).bg(moon(p.green)))
                    .child(div().h_full().w(relative(neg as f32 / total)).bg(moon(p.red))),
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

/// KPI-плитка календаря: подпись + крупное значение + дельта к пред. периоду.
/// `invert` — рост метрики это плохо (минусовые ордера). Общая для Месяц/День.
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
                        .child(format!("{} {:.1}%", if d > 0.0 { "▲" } else { "▼" }, d.abs())),
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
        .child(div().text_size(design::t_caption(cx)).text_color(moon(p.text_soft)).child(label))
        .child(
            div()
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(value_color)
                .child(value),
        )
        .child(delta_el)
}
