//! Вкладка «Стратегии» окна «Аналитика» — рабочее место анализа стратегии.
//! Слева всегда список (сравнение по ID: сделки/WR/прибыль/ср./PF/best/worst,
//! свой скролл), режимы кнопками в шапке списка (дефолт — «Фильтры»):
//! - «Фильтры» — справа тюнер порогов (KPI Факт vs v1/v2 + сетка от/до) в
//!   СКОУПЕ выбранной стратегии, внизу прибитая гистограмма поля;
//! - «Монеты» — справа таблица по монетам выбранной (или всех сделок).

use gpui::*;
use moon_ui::{MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::AnalyticsView;
use super::summary::{fmt_signed, sign_color};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::GroupStat;

/// Показываем не больше стольких групп (реплика может держать тысячи имён;
/// хвост за пределами топа по |прибыли| малоинформативен, а DOM — не резиновый).
const MAX_ROWS: usize = 300;

/// Режим правой панели вкладки «Стратегии» (дефолт — «Фильтры»).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StratMode {
    Filters,
    Coins,
}

impl AnalyticsView {
    /// Смена выбранной стратегии: детализация + скоуп тюнера.
    fn set_sel_strategy(&mut self, sel: Option<(String, String)>, cx: &mut Context<Self>) {
        self.sel_strategy = sel;
        // detail НЕ обнуляем: старая карточка остаётся до прихода новой —
        // иначе всё окно «мерцало» вспышками «Загрузка…».
        self.reload_detail(cx);
        // Скоуп тюнера сменился — старые расчёты (включая автоподбор) неверны.
        self.tuner.invalidate();
        if self.strat_mode == StratMode::Filters {
            self.reload_tuner(cx);
            self.reload_hist(cx);
        }
        cx.notify();
    }

    fn set_strat_mode(&mut self, mode: StratMode, cx: &mut Context<Self>) {
        if self.strat_mode == mode {
            return;
        }
        self.strat_mode = mode;
        if mode == StratMode::Filters && self.tuner.needs_reload() {
            self.reload_tuner(cx);
            self.reload_hist(cx);
        }
        cx.notify();
    }

    /// Тело вкладки: высоту делит само (внешний скролл окна отключён) —
    /// нижняя плашка всегда на экране.
    pub(super) fn strategies_tab(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode = self.strat_mode;
        // Левая половина: список; в «Фильтрах» под ним прибитая гистограмма,
        // в «Обзоре» — вклад по монетам. Правая колонка (Фильтры/Монеты) —
        // на ВСЮ высоту вкладки.
        let mut left = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .gap(design::ui_px(cx, 8.0))
            .child(self.strat_list_card(p, cx));
        match mode {
            StratMode::Filters => left = left.child(self.hist_card(p, cx)),
            StratMode::Coins => {}
        }

        let mut main = h_flex()
            .size_full()
            .p(design::ui_px(cx, 10.0))
            .gap(design::ui_px(cx, 8.0))
            .child(left);
        match mode {
            StratMode::Filters => {
                // KPI прибит сверху; скроллится ТОЛЬКО контейнер порогов.
                main = main.child(
                    v_flex()
                        .w(design::font_w_px(cx, 470.0))
                        .flex_none()
                        .h_full()
                        .min_h_0()
                        .gap(design::ui_px(cx, 8.0))
                        .child(self.kpi_matrix(p, cx))
                        .child(
                            div()
                                .id("an-tuner-col")
                                .w_full()
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .child(self.fields_grid(p, window, cx)),
                        ),
                );
            }
            StratMode::Coins => {
                main = main.child(self.strat_coins_table(p, cx));
            }
        }
        // Окно подтверждения сохранения — оверлеем поверх вкладки.
        let dialog = self.save_dialog_overlay(p, cx);
        div()
            .relative()
            .size_full()
            .child(main)
            .children(dialog)
            .into_any_element()
    }

    /// Оверлей «Записать в стратегию»: список правок + Да/Нет.
    fn save_dialog_overlay(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let dlg = self.tuner.save_dialog.clone()?;
        let mut list = v_flex().w_full().gap_0();
        for (i, (k, v)) in dlg.changes.iter().enumerate() {
            // «сейчас → будет»: текущее значение стратегии мутным, новое — янтарным.
            let old = dlg.olds.get(i).cloned().flatten();
            list = list.child(
                h_flex()
                    .w_full()
                    .h(design::fit_h_px(cx, 22.0, 13.0, 4.5))
                    .px(design::ui_px(cx, 10.0))
                    .gap(design::ui_px(cx, 6.0))
                    .items_center()
                    .border_t_1()
                    .border_color(moon_alpha(p.border, 0.5))
                    .child(div().flex_1().min_w_0().truncate().child(k.clone()))
                    .child(
                        div()
                            .flex_none()
                            .max_w(design::font_w_px(cx, 90.0))
                            .truncate()
                            .text_color(moon(p.text_muted))
                            .child(old.unwrap_or_else(|| "—".to_string())),
                    )
                    .child(div().flex_none().text_color(moon(p.text_muted)).child("→"))
                    .child(
                        div()
                            .flex_none()
                            .max_w(design::font_w_px(cx, 120.0))
                            .truncate()
                            .text_color(moon(p.amber))
                            .child(v.clone()),
                    ),
            );
        }
        // Предупреждения (перезапись чужого слот-типа и т.п.) — оранжевым,
        // с переносом строк (не обрезаем).
        for w in &dlg.warns {
            list = list.child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 10.0))
                    .py(design::ui_px(cx, 5.0))
                    .border_t_1()
                    .border_color(moon_alpha(p.border, 0.5))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.orange))
                    .child(w.clone()),
            );
        }
        Some(
            div()
                .id("an-save-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(moon_alpha(p.shell, 0.6))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tuner.save_dialog = None;
                    cx.notify();
                }))
                .child(
                    v_flex()
                        .id("an-save-dialog")
                        .w(design::font_w_px(cx, 360.0))
                        .max_h(design::ui_px(cx, 480.0))
                        .rounded(design::ui_px(cx, 8.0))
                        .bg(moon(p.panel_high))
                        .border_1()
                        .border_color(moon(p.border))
                        .overflow_hidden()
                        .occlude()
                        .child(
                            h_flex()
                                .px(design::ui_px(cx, 12.0))
                                .py(design::ui_px(cx, 8.0))
                                .items_center()
                                .gap(design::ui_px(cx, 8.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(design::t_title(cx))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(
                                            t!(
                                                "analytics.tuner.save_title",
                                                name = dlg.name.as_str()
                                            )
                                            .to_string(),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(design::t_caption(cx))
                                        .text_color(moon(p.text_muted))
                                        .child(t!("analytics.tuner.save_now_next").to_string()),
                                ),
                        )
                        .child(
                            div()
                                .id("an-save-list")
                                .w_full()
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .child(list),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .px(design::ui_px(cx, 12.0))
                                .py(design::ui_px(cx, 8.0))
                                .gap(design::ui_px(cx, 8.0))
                                .justify_end()
                                .border_t_1()
                                .border_color(moon_alpha(p.border, 0.6))
                                .child(
                                    MoonButton::new("an-save-no")
                                        .variant(MoonButtonVariant::Ghost)
                                        .size(MoonButtonSize::Micro)
                                        .label(t!("analytics.tuner.save_no").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.tuner.save_dialog = None;
                                            cx.notify();
                                        }))
                                        .render(),
                                )
                                .child(
                                    MoonButton::new("an-save-yes")
                                        .variant(MoonButtonVariant::Blue)
                                        .size(MoonButtonSize::Micro)
                                        .label(t!("analytics.tuner.save_yes").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_save_dialog(cx);
                                        }))
                                        .render(),
                                ),
                        ),
                ),
        )
    }

    /// Карточка списка: шапка (заголовок + режимы + счётчик), свой скролл.
    fn strat_list_card(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let (list, total, shown): (AnyElement, usize, usize) = match self.data.clone() {
            None => (
                div()
                    .p(design::ui_px(cx, 18.0))
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.loading").to_string())
                    .into_any_element(),
                0,
                0,
            ),
            Some(d) if d.strategies.is_empty() => (
                div()
                    .p(design::ui_px(cx, 18.0))
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.empty_period").to_string())
                    .into_any_element(),
                0,
                0,
            ),
            Some(d) => {
                let total = d.strategies.len();
                let shown = total.min(MAX_ROWS);
                let mut list = v_flex().w_full().gap_0();
                for g in d.strategies.iter().take(MAX_ROWS) {
                    list = list.child(self.strategy_row(g, p, cx));
                }
                (list.into_any_element(), total, shown)
            }
        };

        let mode_btn = |id: &'static str, mode: StratMode, label: String| {
            let on = self.strat_mode == mode;
            MoonButton::new(id)
                .variant(if on {
                    MoonButtonVariant::Amber
                } else {
                    MoonButtonVariant::Soft
                })
                .size(MoonButtonSize::Micro)
                .selected(on)
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_strat_mode(mode, cx)))
                .render()
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
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
                            .child(t!("analytics.strat.title").to_string()),
                    )
                    .child(mode_btn(
                        "sm-filters",
                        StratMode::Filters,
                        t!("analytics.tab.tuner").to_string(),
                    ))
                    .child(mode_btn(
                        "sm-coins",
                        StratMode::Coins,
                        t!("analytics.tab.coins").to_string(),
                    ))
                    .child(div().flex_1())
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
            .child(header_row(p, cx))
            .child(
                div()
                    .id("an-strat-list")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .into_any_element()
    }

    /// Строка таблицы сравнения; клик — выбрать/снять группу.
    fn strategy_row(&self, g: &GroupStat, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let selected = self.sel_strategy.as_ref().is_some_and(|(k, _)| *k == g.key);
        let key = g.key.clone();
        let name = g.name.clone();
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
        // Имя — слева (гибкое, с обрезкой), все фиксированные колонки — одной
        // жёсткой обоймой справа (justify_between): при глюках распределения
        // флекса на ресайзе колонки не разъезжаются между строками.
        let mut row = h_flex()
            .id(SharedString::from(format!("an-strat-{}", g.key)))
            .w_full()
            .h(design::fit_h_px(cx, 25.0, 14.0, 5.5))
            .px(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .justify_between()
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
                    // flex_1 обязателен: div с truncate() без флекс-базиса
                    // схлопывается в «…» (так пропадали имена стратегий).
                    .child(div().flex_1().min_w_0().truncate().child(g.name.clone())),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .w(design::font_w_px(cx, 72.0))
                            .flex_none()
                            .truncate()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(g.kind.clone()),
                    )
                    .child(
                        div()
                            .w(design::font_w_px(cx, 88.0))
                            .flex_none()
                            .truncate()
                            .text_color(moon(p.text_soft))
                            .child(core_label),
                    )
                    .child(num_cell(cx, 56.0, g.n.to_string(), p.text_soft))
                    .child(num_cell(cx, 56.0, format!("{:.1}%", g.winrate()), p.text_soft))
                    .child(num_cell(cx, 84.0, fmt_signed(g.profit), sign_color(p, g.profit)))
                    .child(num_cell(cx, 70.0, fmt_signed(g.avg()), sign_color(p, g.avg())))
                    .child(num_cell(cx, 52.0, format!("{:.2}", g.pf), p.text_soft))
                    .child(num_cell(cx, 70.0, fmt_signed(g.best), sign_color(p, g.best)))
                    .child(num_cell(cx, 70.0, fmt_signed(g.worst), sign_color(p, g.worst))),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.sel_strategy.as_ref().is_some_and(|(k, _)| *k == key) {
                    this.set_sel_strategy(None, cx);
                } else {
                    this.set_sel_strategy(Some((key.clone(), name.clone())), cx);
                }
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

    /// Правая панель «Монеты»: таблица по монетам выбранной (или всех сделок).
    fn strat_coins_table(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // Скоуп: детализация выбранной стратегии либо общий разрез по монетам.
        let (coins, scope): (Option<Vec<GroupStat>>, String) = match &self.sel_strategy {
            Some((_, name)) => (
                self.detail.clone().map(|d| d.coins.clone()),
                name.clone(),
            ),
            None => (
                self.data.clone().map(|d| d.coins.clone()),
                t!("analytics.strat.scope_all").to_string(),
            ),
        };
        let cell = |w: f32, key: &str| {
            div()
                .w(design::font_w_px(cx, w))
                .flex_none()
                .text_right()
                .child(t!(key).to_string())
        };
        let head = h_flex()
            .w_full()
            .flex_none()
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .px(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(div().flex_1().child(t!("analytics.col.coin").to_string()))
            .child(cell(52.0, "analytics.kpi.trades"))
            .child(cell(56.0, "analytics.kpi.winrate"))
            .child(cell(84.0, "analytics.col.profit"))
            .child(cell(52.0, "analytics.col.pf"))
            .child(cell(70.0, "analytics.col.worst"));

        let body: AnyElement = match coins {
            None => div()
                .p(design::ui_px(cx, 10.0))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.loading").to_string())
                .into_any_element(),
            Some(coins) if coins.is_empty() => div()
                .p(design::ui_px(cx, 10.0))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.empty_period").to_string())
                .into_any_element(),
            Some(coins) => {
                let mut list = v_flex().w_full();
                for c in coins.iter().take(MAX_ROWS) {
                    list = list.child(
                        h_flex()
                            .w_full()
                            .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                            .px(design::ui_px(cx, 8.0))
                            .gap(design::ui_px(cx, 8.0))
                            .items_center()
                            .border_t_1()
                            .border_color(moon_alpha(p.border, 0.5))
                            .child(div().flex_1().min_w_0().truncate().child(c.name.clone()))
                            .child(num_cell(cx, 52.0, c.n.to_string(), p.text_soft))
                            .child(num_cell(
                                cx,
                                56.0,
                                format!("{:.1}%", c.winrate()),
                                p.text_soft,
                            ))
                            .child(num_cell(
                                cx,
                                84.0,
                                fmt_signed(c.profit),
                                sign_color(p, c.profit),
                            ))
                            .child(num_cell(cx, 52.0, format!("{:.2}", c.pf), p.text_soft))
                            .child(num_cell(
                                cx,
                                70.0,
                                fmt_signed(c.worst),
                                sign_color(p, c.worst),
                            )),
                    );
                }
                list.into_any_element()
            }
        };

        v_flex()
            .w(design::font_w_px(cx, 460.0))
            .flex_none()
            .h_full()
            .min_h_0()
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
                            .child(t!("analytics.tab.coins").to_string()),
                    )
                    .child(
                        div()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .min_w_0()
                            .truncate()
                            .child(scope),
                    ),
            )
            .child(head)
            .child(
                div()
                    .id("an-strat-coins")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
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
        .flex_none()
        .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
        .px(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .justify_between()
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_soft))
        .bg(moon(p.table_head))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(t!("analytics.col.strategy").to_string()),
        )
        // Обойма фиксированных колонок — зеркально строкам (justify_between).
        .child(
            h_flex()
                .flex_none()
                .gap(design::ui_px(cx, 8.0))
                .items_center()
                .child(
                    div()
                        .w(design::font_w_px(cx, 72.0))
                        .flex_none()
                        .child(t!("analytics.col.kind").to_string()),
                )
                .child(
                    div()
                        .w(design::font_w_px(cx, 88.0))
                        .flex_none()
                        .child(t!("analytics.col.core").to_string()),
                )
                .child(cell(56.0, "analytics.kpi.trades"))
                .child(cell(56.0, "analytics.kpi.winrate"))
                .child(cell(84.0, "analytics.col.profit"))
                .child(cell(70.0, "analytics.kpi.avg_short"))
                .child(cell(52.0, "analytics.col.pf"))
                .child(cell(70.0, "analytics.col.best"))
                .child(cell(70.0, "analytics.col.worst")),
        )
}

fn num_cell(
    cx: &Context<AnalyticsView>,
    w: f32,
    text: String,
    color: u32,
) -> impl IntoElement {
    div()
        .w(design::font_w_px(cx, w))
        .flex_none()
        .text_right()
        .text_color(moon(color))
        .child(text)
}
