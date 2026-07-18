//! Режим «День» календаря — детализация одного дня. Пока заглушка: показываем
//! дату, суточные PnL/сделки/W-L/winrate; подробная разбивка — следующим этапом.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::super::summary::{fmt_signed, sign_color};
use crate::design;
use crate::design::moon;
use moon_core::db::analytics::DayCell;

impl AnalyticsView {
    pub(super) fn calendar_day(&self, days: &[DayCell], p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // Запрос режима «День» — ровно эти сутки (0..1 ячейки).
        let d = days.iter().find(|d| d.start == self.cal_day);
        let profit = d.map_or(0.0, |d| d.profit);
        let trades = d.map_or(0, |d| d.trades);
        let wins = d.map_or(0, |d| d.wins);
        let wr = if trades > 0 { wins as f64 / trades as f64 * 100.0 } else { 0.0 };

        let stat = |label: String, value: String, color| {
            v_flex()
                .gap(design::ui_px(cx, 3.0))
                .child(div().text_size(design::t_caption(cx)).text_color(moon(p.text_soft)).child(label))
                .child(div().text_size(design::t_title(cx)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(value))
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .p(design::ui_px(cx, 14.0))
            .gap(design::ui_px(cx, 14.0))
            .child(
                h_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 24.0))
                    .items_center()
                    .child(stat(t!("analytics.cal.day").to_string(), super::super::fmt_day(self.cal_day), moon(p.text)))
                    .child(stat(t!("analytics.cal.kpi_profit").to_string(), fmt_signed(profit), moon(sign_color(p, profit))))
                    .child(stat(t!("analytics.kpi.trades").to_string(), trades.to_string(), moon(p.text)))
                    .child(stat(t!("analytics.col.pf").to_string(), format!("{}W {}L", wins, trades - wins), moon(p.text)))
                    .child(stat(t!("analytics.kpi.winrate").to_string(), format!("{wr:.1}%"), moon(p.text))),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .rounded(design::ui_px(cx, 8.0))
                    .bg(moon(p.panel))
                    .border_1()
                    .border_color(moon(p.border))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.cal.day_soon").to_string()),
            )
            .into_any_element()
    }
}
