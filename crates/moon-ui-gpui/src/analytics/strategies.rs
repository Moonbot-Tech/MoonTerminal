//! Вкладка «Стратегии» окна «Аналитика»: таблица сравнения групп по ID
//! стратегии (сделки, winrate с мини-баром, прибыль, ср./сделка, PF, best/worst)
//! + drill-down по клику: вклад по монетам и последние сделки группы.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonPalette, MoonTone, h_flex, v_flex};
use rust_i18n::t;

use super::AnalyticsView;
use super::summary::{fmt_signed, sign_color};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::GroupStat;

/// Показываем не больше стольких групп (реплика может держать тысячи имён;
/// хвост за пределами топа по |прибыли| малоинформативен, а DOM — не резиновый).
const MAX_ROWS: usize = 300;

impl AnalyticsView {
    pub(super) fn strategies_tab(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let Some(data) = self.data.clone() else {
            return div()
                .p(design::ui_px(cx, 18.0))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.loading").to_string())
                .into_any_element();
        };
        if data.strategies.is_empty() {
            return div()
                .p(design::ui_px(cx, 18.0))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.empty_period").to_string())
                .into_any_element();
        }

        // Таблица: прибыль по убыванию (как отдаёт запрос); кап на MAX_ROWS.
        let total = data.strategies.len();
        let shown = total.min(MAX_ROWS);
        let mut list = v_flex().w_full().gap_0().child(header_row(p, cx));
        for g in data.strategies.iter().take(MAX_ROWS) {
            list = list.child(self.strategy_row(g, p, cx));
        }

        let mut col = v_flex()
            .w_full()
            .p(design::ui_px(cx, 10.0))
            .gap(design::ui_px(cx, 8.0))
            .child(
                v_flex()
                    .w_full()
                    .rounded(design::ui_px(cx, 8.0))
                    .bg(moon(p.panel))
                    .border_1()
                    .border_color(moon(p.border))
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .w_full()
                            .px(design::ui_px(cx, 12.0))
                            .py(design::ui_px(cx, 8.0))
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(design::t_title(cx))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t!("analytics.strat.title").to_string()),
                            )
                            .child(
                                div()
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(p.text_muted))
                                    .child(if total > shown {
                                        t!("analytics.strat.shown", shown = shown, total = total)
                                            .to_string()
                                    } else {
                                        t!("analytics.strat.hint").to_string()
                                    }),
                            ),
                    )
                    .child(list),
            );

        // Drill-down выбранной группы.
        if let Some((_, name)) = &self.sel_strategy {
            col = col.child(self.detail_cards(name, p, cx));
        }
        col.into_any_element()
    }

    /// Строка таблицы сравнения; клик — выбрать/снять группу для детализации.
    fn strategy_row(&self, g: &GroupStat, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let selected = self.sel_strategy.as_ref().is_some_and(|(k, _)| *k == g.key);
        let key = g.key.clone();
        let name = g.name.clone();
        let wr = g.winrate();
        // Группа = id стратегии; имя — подпись, id — мутным рядом (различает
        // одноимённые стратегии и переживает переименования).
        let show_id = g.name != g.key;
        // Индикатор «жива сейчас»: ● зелёная — есть в ядре и включена,
        // ● мутная — есть, но выключена, ○ контур — удалена из ядер.
        let alive_dot = g.alive.map(|a| {
            let dot = div()
                .flex_none()
                .w(design::ui_px(cx, 6.0))
                .h(design::ui_px(cx, 6.0))
                .rounded_full();
            match a {
                2 => dot.bg(moon(p.green)),
                1 => dot.bg(moon_alpha(p.text_muted, 0.8)),
                _ => dot.border_1().border_color(moon_alpha(p.text_muted, 0.6)),
            }
        });
        let core_label = if g.cores_n > 1 {
            t!("report.cores_n", n = g.cores_n).to_string()
        } else {
            g.core.clone()
        };
        let mut row = h_flex()
            .id(SharedString::from(format!("an-strat-{}", g.key)))
            .w_full()
            .h(design::fit_h_px(cx, 25.0, 14.0, 5.5))
            .px(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .cursor_pointer()
            .bg(moon(p.table_body))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.6))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(design::ui_px(cx, 6.0))
                    .items_center()
                    .children(alive_dot)
                    .child(div().min_w_0().truncate().child(g.name.clone()))
                    .when(show_id, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .text_size(design::t_caption(cx))
                                .text_color(moon(p.text_muted))
                                .child(format!("#{}", g.key)),
                        )
                    }),
            )
            .child(
                div()
                    .w(design::font_w_px(cx, 88.0))
                    .flex_none()
                    .truncate()
                    .text_color(moon(p.text_soft))
                    .child(core_label),
            )
            .child(num_cell(p, cx, 56.0, g.n.to_string(), p.text_soft))
            // Winrate: мини-бар + процент.
            .child(
                h_flex()
                    .w(design::font_w_px(cx, 92.0))
                    .flex_none()
                    .gap(design::ui_px(cx, 6.0))
                    .items_center()
                    .justify_end()
                    .child(
                        div()
                            .w(design::font_w_px(cx, 40.0))
                            .h(px(3.0))
                            .rounded(px(2.0))
                            .bg(moon(p.border))
                            .overflow_hidden()
                            .child(
                                div()
                                    .w(relative((wr / 100.0) as f32))
                                    .h_full()
                                    .bg(moon(p.green)),
                            ),
                    )
                    .child(
                        div()
                            .text_color(moon(p.text_soft))
                            .child(format!("{wr:.1}%")),
                    ),
            )
            .child(num_cell(p, cx, 84.0, fmt_signed(g.profit), sign_color(p, g.profit)))
            .child(num_cell(p, cx, 70.0, fmt_signed(g.avg()), sign_color(p, g.avg())))
            .child(num_cell(p, cx, 52.0, format!("{:.2}", g.pf), p.text_soft))
            .child(num_cell(p, cx, 70.0, fmt_signed(g.best), sign_color(p, g.best)))
            .child(num_cell(p, cx, 70.0, fmt_signed(g.worst), sign_color(p, g.worst)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.sel_strategy.as_ref().is_some_and(|(k, _)| *k == key) {
                    this.sel_strategy = None;
                    this.detail = None;
                } else {
                    this.sel_strategy = Some((key.clone(), name.clone()));
                    this.detail = None;
                    this.reload_detail(cx);
                }
                cx.notify();
            }));
        if selected {
            row = row
                .bg(moon_alpha(p.amber, 0.12))
                .border_color(moon_alpha(p.amber, 0.5));
        } else {
            row = row.hover(move |s| s.bg(moon_alpha(p.panel_high, 0.9)));
        }
        row
    }

    /// Карточки детализации: вклад по монетам + последние сделки.
    fn detail_cards(&self, name: &str, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let Some(detail) = self.detail.clone() else {
            return div()
                .p(design::ui_px(cx, 12.0))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.loading").to_string())
                .into_any_element();
        };
        // По монетам: топ-8 по |вкладу|, мини-бары от максимума.
        let mut coins: Vec<&GroupStat> = detail.coins.iter().collect();
        coins.sort_by(|a, b| b.profit.abs().total_cmp(&a.profit.abs()));
        coins.truncate(8);
        let max_abs = coins
            .iter()
            .map(|c| c.profit.abs())
            .fold(1e-9f64, f64::max);
        let mut coin_list = v_flex().w_full().gap(design::ui_px(cx, 4.0));
        for c in &coins {
            coin_list = coin_list.child(
                h_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .w(design::font_w_px(cx, 64.0))
                            .flex_none()
                            .truncate()
                            .child(c.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(5.0))
                            .rounded(px(2.0))
                            .bg(moon_alpha(p.border, 0.6))
                            .overflow_hidden()
                            .child(
                                div()
                                    .w(relative((c.profit.abs() / max_abs) as f32))
                                    .h_full()
                                    .bg(moon(sign_color(p, c.profit))),
                            ),
                    )
                    .child(
                        div()
                            .w(design::font_w_px(cx, 76.0))
                            .flex_none()
                            .text_color(moon(sign_color(p, c.profit)))
                            .child(format!("{} ({})", fmt_signed(c.profit), c.n)),
                    ),
            );
        }

        // Последние сделки группы.
        let mut last = v_flex().w_full().gap_0();
        for tr in detail.last.iter() {
            last = last.child(
                h_flex()
                    .w_full()
                    .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .border_t_1()
                    .border_color(moon_alpha(p.border, 0.5))
                    .child(
                        div()
                            .w(design::font_w_px(cx, 74.0))
                            .text_color(moon(p.text_soft))
                            .child(super::summary::fmt_dm_hm(tr.closedate)),
                    )
                    .child(
                        h_flex()
                            .w(design::font_w_px(cx, 80.0))
                            .gap(design::ui_px(cx, 4.0))
                            .items_center()
                            .child(div().min_w_0().truncate().child(tr.coin.clone()))
                            .child(
                                MoonBadge::new(if tr.is_short { "S" } else { "L" })
                                    .tone(if tr.is_short {
                                        MoonTone::Negative
                                    } else {
                                        MoonTone::Positive
                                    })
                                    .variant(MoonBadgeVariant::Soft)
                                    .size(MoonBadgeSize::Tiny)
                                    .render_with_palette(p),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(moon(p.text_soft))
                            .child(tr.core_name.clone()),
                    )
                    .child(
                        div()
                            .text_color(moon(sign_color(p, tr.profit)))
                            .child(fmt_signed(tr.profit)),
                    ),
            );
        }

        h_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .items_start()
            .child(detail_card(
                t!("analytics.strat.by_coins", name = name).to_string(),
                coin_list.into_any_element(),
                p,
                cx,
            ))
            .child(detail_card(
                t!("analytics.strat.last_trades").to_string(),
                last.into_any_element(),
                p,
                cx,
            ))
            .into_any_element()
    }
}

/// Шапка таблицы сравнения.
fn header_row(p: MoonPalette, cx: &Context<AnalyticsView>) -> impl IntoElement {
    let cell = |w: f32, key: &str| {
        div()
            .w(design::font_w_px(cx, w))
            .flex_none()
            .text_right()
            .child(t!(key).to_string())
    };
    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
        .px(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_soft))
        .bg(moon(p.table_head))
        .child(div().flex_1().child(t!("analytics.col.strategy").to_string()))
        .child(
            div()
                .w(design::font_w_px(cx, 88.0))
                .flex_none()
                .child(t!("analytics.col.core").to_string()),
        )
        .child(cell(56.0, "analytics.kpi.trades"))
        .child(cell(92.0, "analytics.kpi.winrate"))
        .child(cell(84.0, "analytics.col.profit"))
        .child(cell(70.0, "analytics.kpi.avg_short"))
        .child(cell(52.0, "analytics.col.pf"))
        .child(cell(70.0, "analytics.col.best"))
        .child(cell(70.0, "analytics.col.worst"))
}

fn num_cell(
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
    w: f32,
    text: String,
    color: u32,
) -> impl IntoElement {
    let _ = p;
    div()
        .w(design::font_w_px(cx, w))
        .flex_none()
        .text_right()
        .text_color(moon(color))
        .child(text)
}

/// Карточка детализации с заголовком.
fn detail_card(
    title: String,
    body: AnyElement,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .child(
            div()
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(body)
}

