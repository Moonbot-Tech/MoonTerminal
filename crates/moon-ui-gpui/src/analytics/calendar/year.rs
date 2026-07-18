//! Режим «Год» календаря: ВСЕ года разом — на каждый год сетка 12 месяцев-
//! квадратов (текущий год сверху, будущие месяцы серым). Клик по месяцу →
//! режим «Месяц». Навигация Назад/Вперёд в этом режиме не работает.

use std::collections::HashMap;

use chrono::Datelike;
use gpui::*;
use moon_ui::{MoonPalette, MoonScrollableElement, h_flex, v_flex};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::super::summary::{fmt_signed, sign_color};
use super::{date_of, now_ym, split_i18n};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::DayCell;

impl AnalyticsView {
    pub(super) fn calendar_year(&self, days: &[DayCell], p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // Свёртка посуточной серии всей истории в месячные агрегаты.
        let mut agg: HashMap<(i32, u32), (f64, i64)> = HashMap::new();
        let mut min_year = i32::MAX;
        for d in days {
            let dt = date_of(d.start);
            min_year = min_year.min(dt.year());
            let e = agg.entry((dt.year(), dt.month())).or_insert((0.0, 0));
            e.0 += d.profit;
            e.1 += d.trades;
        }
        let (cy, cm) = now_ym();
        if min_year == i32::MAX {
            min_year = cy;
        }

        // Года сверху вниз: текущий первым, дальше в прошлое.
        let mut list = v_flex().w_full().gap(design::ui_px(cx, 18.0));
        for y in (min_year..=cy).rev() {
            list = list.child(self.year_block(y, cy, cm, &agg, p, cx));
        }
        div()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .p(design::ui_px(cx, 10.0))
            .child(
                div()
                    .id("an-cal-scroll")
                    .size_full()
                    .child(list)
                    .overflow_y_scrollbar(),
            )
            .into_any_element()
    }

    fn year_block(
        &self,
        y: i32,
        cy: i32,
        cm: u32,
        agg: &HashMap<(i32, u32), (f64, i64)>,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let months = split_i18n(t!("analytics.heat.months").to_string());
        let year_total: f64 = (1..=12).filter_map(|m| agg.get(&(y, m))).map(|a| a.0).sum();
        // 1 год = 1 строка из всех 12 месяцев (без переноса; ячейки тянутся).
        let mut grid = h_flex().w_full().gap(design::ui_px(cx, 8.0));
        for m in 1..=12u32 {
            let future = y > cy || (y == cy && m > cm);
            let (profit, trades) = agg.get(&(y, m)).copied().unwrap_or((0.0, 0));
            let name = months.get((m as usize).saturating_sub(1)).cloned().unwrap_or_default();
            grid = grid.child(self.year_month_cell(y, m, name, profit, trades, future, p, cx));
        }
        v_flex()
            .flex_none()
            .w_full()
            .gap(design::ui_px(cx, 6.0))
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(div().text_size(design::t_title(cx)).font_weight(FontWeight::SEMIBOLD).child(y.to_string()))
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(moon(sign_color(p, year_total)))
                            .child(fmt_signed(year_total)),
                    ),
            )
            .child(grid)
    }

    #[allow(clippy::too_many_arguments)]
    fn year_month_cell(
        &self,
        y: i32,
        m: u32,
        name: String,
        profit: f64,
        trades: i64,
        future: bool,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let h = design::ui_px(cx, 118.0);
        let pad = design::ui_px(cx, 10.0);
        let r = design::ui_px(cx, 8.0);
        let empty = future || trades == 0;
        let bg = if future { moon(p.shell) } else { moon(p.panel) };
        let border = if empty { moon_alpha(p.border, 0.35) } else { moon_alpha(sign_color(p, profit), 0.5) };
        // Не переносим на новую строку — ячейки flex_1 ужимаются под 12 в ряд.
        let mut cell = v_flex()
            .id(("ym", (y as u64) * 100 + m as u64))
            .flex_1()
            .min_w_0()
            .h(h)
            .overflow_hidden()
            .p(pad)
            .rounded(r)
            .bg(bg)
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.cal_goto_month(y, m, cx)))
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(if empty { p.text_muted } else { p.text }))
                    .child(name),
            );
        if empty {
            cell = cell.child(div().flex_1()).child(
                div().text_size(design::t_title(cx)).text_color(moon(p.text_muted)).child("—"),
            );
        } else {
            cell = cell
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(design::t_title(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(sign_color(p, profit)))
                        .child(fmt_signed(profit)),
                )
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.heat.trades_full", n = trades).to_string()),
                );
        }
        cell
    }
}
