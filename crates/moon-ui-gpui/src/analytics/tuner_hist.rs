//! Карточки тюнера: матрица KPI «Факт vs варианты» и гистограмма (нижняя
//! плашка режима «Фильтры») — распределение профита/убытка и сделок по
//! квантильным вёдрам выбранного поля. Вынесено из tuner.rs (лимит размера).

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::AnalyticsView;
use super::summary::{fmt_signed, sign_color};
use super::tuner::card;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::{FIELDS, VarStats};

impl AnalyticsView {
    /// Матрица KPI: строки — показатели, колонки — Факт / v1 / v2.
    pub(super) fn kpi_matrix(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // Подзаголовок = скоуп: имя выбранной стратегии или «все стратегии».
        let scope = self
            .sel_strategy
            .as_ref()
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| t!("analytics.strat.scope_all").to_string());
        let Some(stats) = self.tuner.stats.clone() else {
            return card(
                t!("analytics.tuner.kpi_title").to_string(),
                scope,
                div()
                    .p(design::ui_px(cx, 8.0))
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.loading").to_string())
                    .into_any_element(),
                p,
                cx,
            );
        };
        // (подпись, значение, больше=лучше; None — без сравнения с фактом)
        type Row = (String, fn(&VarStats) -> f64, Option<bool>, bool);
        let rows: Vec<Row> = vec![
            (t!("analytics.kpi.trades").to_string(), |s| s.n as f64, None, true),
            (t!("analytics.kpi.profit").to_string(), |s| s.profit, Some(true), false),
            (t!("analytics.kpi.winrate").to_string(), |s| s.winrate(), Some(true), false),
            (t!("analytics.col.pf").to_string(), |s| s.pf, Some(true), false),
            (t!("analytics.kpi.avg_short").to_string(), |s| s.avg, Some(true), false),
            (t!("analytics.tuner.avg_win").to_string(), |s| s.avg_win, Some(true), false),
            (t!("analytics.tuner.avg_loss").to_string(), |s| s.avg_loss, Some(false), false),
            (t!("analytics.kpi.maxdd").to_string(), |s| s.max_dd, Some(false), false),
        ];
        let col_w = 92.0;
        let mut head = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(div().flex_1().child(t!("analytics.tuner.metric").to_string()));
        for i in 0..stats.len() {
            let name = if i == 0 {
                t!("analytics.tuner.fact").to_string()
            } else {
                format!("v{i}")
            };
            head = head.child(div().w(design::font_w_px(cx, col_w)).flex_none().text_right().child(name));
        }

        let mut body = v_flex().w_full().child(head);
        for (label, get, better, int) in rows {
            let fact = get(&stats[0]);
            let mut row = h_flex()
                .w_full()
                .px(design::ui_px(cx, 8.0))
                .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .border_t_1()
                .border_color(moon_alpha(p.border, 0.5))
                .child(div().flex_1().text_color(moon(p.text_soft)).child(label));
            for (i, s) in stats.iter().enumerate() {
                let v = get(s);
                let text = if int { format!("{}", v as i64) } else { fmt_signed(v) };
                let color = match better {
                    // Вариант красим относительно факта; сам факт — по знаку.
                    Some(hb) if i > 0 => {
                        if (v > fact) == hb && v != fact {
                            p.green
                        } else if v != fact {
                            p.orange
                        } else {
                            p.text
                        }
                    }
                    Some(_) => sign_color(p, v),
                    None => p.text,
                };
                row = row.child(
                    div()
                        .w(design::font_w_px(cx, col_w))
                        .flex_none()
                        .text_right()
                        .text_color(moon(color))
                        .child(text),
                );
            }
            body = body.child(row);
        }
        card(
            t!("analytics.tuner.kpi_title").to_string(),
            scope,
            body.into_any_element(),
            p,
            cx,
        )
    }

    /// Гистограмма выбранного поля: выигрыши вверх, убытки вниз, счётчик и края.
    pub(super) fn hist_card(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // В заголовке — поле и скоуп (имя стратегии / все).
        let scope = self
            .sel_strategy
            .as_ref()
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| t!("analytics.strat.scope_all").to_string());
        let title = format!(
            "{} — {} — {}",
            t!("analytics.tuner.hist_title"),
            FIELDS[self.tuner.sel_field].1,
            scope,
        );
        let body: AnyElement = match self.tuner.hist.clone() {
            None => div()
                .p(design::ui_px(cx, 8.0))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.loading").to_string())
                .into_any_element(),
            Some(h) if h.is_empty() => div()
                .p(design::ui_px(cx, 8.0))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.empty_period").to_string())
                .into_any_element(),
            Some(h) => {
                let max = h
                    .iter()
                    .map(|b| b.wsum.max(b.lsum))
                    .fold(1e-9f64, f64::max);
                let half = design::ui_px(cx, 74.0);
                let mut row = h_flex().w_full().gap(design::ui_px(cx, 3.0)).items_start();
                for b in h.iter() {
                    let up = ((b.wsum / max) as f32).clamp(0.0, 1.0);
                    let dn = ((b.lsum / max) as f32).clamp(0.0, 1.0);
                    row = row.child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(2.0))
                            // Выигрыши (вверх от оси).
                            .child(
                                div()
                                    .w_full()
                                    .h(half)
                                    .flex()
                                    .items_end()
                                    .justify_center()
                                    .child(
                                        div()
                                            .w(relative(0.62))
                                            .h(relative(
                                                up.max(if b.wsum > 0.0 { 0.02 } else { 0.0 }),
                                            ))
                                            .rounded_t(px(2.0))
                                            .bg(moon(p.green)),
                                    ),
                            )
                            // Убытки (вниз от оси).
                            .child(
                                div()
                                    .w_full()
                                    .h(half)
                                    .flex()
                                    .items_start()
                                    .justify_center()
                                    .border_t_1()
                                    .border_color(moon_alpha(p.border, 0.8))
                                    .child(
                                        div()
                                            .w(relative(0.62))
                                            .h(relative(
                                                dn.max(if b.lsum > 0.0 { 0.02 } else { 0.0 }),
                                            ))
                                            .rounded_b(px(2.0))
                                            .bg(moon(p.orange)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(sign_color(p, b.wsum - b.lsum)))
                                    .child(fmt_signed(b.wsum - b.lsum)),
                            )
                            .child(
                                div()
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(p.text_soft))
                                    .child(b.n.to_string()),
                            )
                            .child(
                                div()
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(p.text_muted))
                                    .child(short_num(b.lo)),
                            ),
                    );
                }
                v_flex()
                    .w_full()
                    .px(design::ui_px(cx, 8.0))
                    .pb(design::ui_px(cx, 6.0))
                    .child(row)
                    .into_any_element()
            }
        };
        card(title, t!("analytics.tuner.hist_sub").to_string(), body, p, cx)
    }
}

/// Короткий формат числа для краёв вёдер (объёмы до миллиардов).
fn short_num(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.1}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}k", v / 1e3)
    } else if a >= 100.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}
