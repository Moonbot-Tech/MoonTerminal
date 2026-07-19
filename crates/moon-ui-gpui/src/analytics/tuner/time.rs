//! Режим «По времени» вкладки «Тюнинг стратегий». Слева — тот же список
//! стратегий, что в «По фильтру» (клик = скоуп профиля); в правом верхнем углу
//! пока пустой контейнер (зарезервирован); внизу на всю ширину — тепловая карта
//! «по часам дня»: 4 столбца-периода (текущий/7д/30д/90д), в каждом 24 строки
//! (час → бар PnL от центра + значение). Данные — `hourly_profiles`.

use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::super::summary::{fmt_signed, sign_color};
use super::hist::kpi_matrix_card;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::{HourStat, hourly_profiles};
use moon_core::db::tuner::variant_stats;

/// Число столбцов-периодов тепловой карты.
const COLS: usize = 4;

impl AnalyticsView {
    /// Фоновый расчёт профилей «час дня» для всех столбцов сразу (один снапшот).
    /// База — фильтры «Тюнинга» + скоуп выбранной стратегии (`tuner_query`);
    /// столбцы: активный период вкладки, 7д, 30д, 90д.
    pub(in crate::analytics) fn reload_time(&mut self, cx: &mut Context<Self>) {
        self.time_dirty = false;
        self.time_seq = self.time_seq.wrapping_add(1);
        let req = self.time_seq;
        self.time_stats.begin();
        let base = self.tuner_query();
        let now = moon_core::util::now_unix_ms_i64() / 1000;
        let tomorrow = now.div_euclid(86_400) * 86_400 + 86_400;
        // Порядок столбцов = порядок подписей в `time_heatmap`.
        let ranges = vec![
            (base.from, base.to),
            (tomorrow - 7 * 86_400, tomorrow),
            (tomorrow - 30 * 86_400, tomorrow),
            (tomorrow - 90 * 86_400, tomorrow),
        ];
        // Варианты KPI строит СЕТКА расписания (грид), а не профиль — выборки
        // независимы: профили для тепловой карты, variant_stats для «Факт vs v1/v2».
        let variants = self.time_tuner.variants();
        // sid выбранной стратегии — для чтения сырых текущих значений полей.
        let sid = self
            .sel_strategy
            .as_ref()
            .and_then(|(k, _)| super::parse_strat_key(k))
            .map(|(s, _)| s);
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let (profiles, stats, current, slider, ig_time, ig_filters) = executor
                .spawn(async move {
                    let profiles = hourly_profiles(&base, &ranges);
                    let stats = variant_stats(&base, &variants);
                    // Профили раскраски ползунков (неделя×час / час / минута в часе).
                    let slider = moon_core::db::tuner::slider_profiles(&base);
                    // Сырые значения полей + флаги игнора (для показа и Save-логики).
                    let (current, ig_time, ig_filters): ([String; 2], bool, bool) = sid
                        .map(|sid| {
                            let m = moon_core::db::tuner::strategy_current_values(
                                sid,
                                &[
                                    "WorkingWeekTime".to_string(),
                                    "WorkingTime".to_string(),
                                    "IgnoreTime".to_string(),
                                    "IgnoreFilters".to_string(),
                                ],
                            );
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
                        })
                        .unwrap_or_default();
                    (profiles, stats, current, slider, ig_time, ig_filters)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.time_seq != req {
                        return; // период/фильтры/стратегия/сетка уже сменили
                    }
                    // Профиль (heatmap) и KPI независимы: ошибку профиля показываем
                    // как «нет данных», ошибку KPI — как Failed в матрице.
                    this.time_profiles = profiles.ok().map(Arc::new);
                    this.time_slider = slider.ok().map(Arc::new);
                    this.time_stats.apply(stats);
                    this.time_tuner.current_raw = current;
                    this.time_tuner.ignore_cur = ig_time;
                    this.time_tuner.ign_filters_cur = ig_filters;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Тело режима «По времени» — раскладка ЗЕРКАЛИТ «По фильтру»: слева список
    /// (верх) + тепловая карта (низ, на месте гистограммы); справа на всю высоту
    /// колонка — KPI «Факт vs варианты» (верх) + сетка расписания (низ).
    pub(super) fn strat_time(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Элементы в локали: &self-виджеты считаем до &mut-сетки (time_grid лениво
        // создаёт инпуты), чтобы заимствования не пересекались.
        let list = self.strat_list_card(p, cx);
        let heatmap = self.time_heatmap(p, cx);
        let kpi = self.time_kpi(p, cx);
        let grid = self.time_grid(p, window, cx);
        // Окно подтверждения записи — ОБЩЕЕ с «По фильтру» (`save_dialog_overlay`);
        // рисуем его и здесь, т.к. «По времени» выходит из `strategies_tab` раньше,
        // чем тот путь добавляет оверлей.
        let dialog = self.save_dialog_overlay(p, cx);
        // Overlay ловли мыши на время drag ползунка (поверх всего тела).
        let drag_overlay = self.slider_drag_overlay(cx);
        let body = v_flex()
            .size_full()
            .p(design::ui_px(cx, 10.0))
            .gap(design::ui_px(cx, 8.0))
            // Верх: список стратегий (слева) + KPI/сетка/ползунки (справа, 470).
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .gap(design::ui_px(cx, 8.0))
                    .child(div().flex_1().min_w_0().h_full().min_h_0().child(list))
                    .child(
                        v_flex()
                            .w(design::font_w_px(cx, 500.0))
                            .flex_none()
                            .h_full()
                            .min_h_0()
                            .gap(design::ui_px(cx, 8.0))
                            .child(kpi)
                            .child(grid),
                    ),
            )
            // Низ: тепловая карта «По часам» на ВСЮ ширину (высота как гистограмма фильтра).
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

    /// Верх правой колонки: УНИВЕРСАЛЬНАЯ KPI-матрица «Факт vs варианты» (Факт /
    /// v1 / v2 по недельному расписанию из сетки) по активному периоду и скоупу
    /// выбранной стратегии. Тот же виджет, что в «По фильтру».
    fn time_kpi(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let scope = self
            .sel_strategy
            .as_ref()
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| t!("analytics.strat.scope_all").to_string());
        // Пустой список подписей → «v1»/«v2» (как в «По фильтру»).
        kpi_matrix_card(&self.time_stats, scope, &[], p, cx)
    }

    /// Нижняя тепловая карта «по часам»: шапка + 4 столбца-периода со скроллом.
    fn time_heatmap(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // Подписи столбцов = порядок диапазонов в `reload_time`.
        let labels: [String; COLS] = [
            self.strat_period.title(),
            t!("analytics.time.week").to_string(),
            t!("analytics.time.month").to_string(),
            t!("analytics.time.d90").to_string(),
        ];
        let profiles = self.time_profiles.clone();
        // Скоуп: имя выбранной стратегии либо «все стратегии».
        let scope = self
            .sel_strategy
            .as_ref()
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| t!("analytics.strat.scope_all").to_string());

        let mut cols = h_flex().w_full().gap(design::ui_px(cx, 8.0));
        for (idx, label) in labels.into_iter().enumerate() {
            let prof = profiles.as_ref().and_then(|v| v.get(idx)).copied();
            cols = cols.child(time_column(label, prof, p, cx));
        }

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
                    .overflow_y_scroll()
                    .child(cols),
            )
            .into_any_element()
    }
}

/// Один столбец-период: подпись + 24 строки-часа (или прочерк «нет данных»).
fn time_column(
    label: String,
    prof: Option<[HourStat; 24]>,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    // Масштаб бара — по максимуму |PnL| часа В ЭТОМ столбце (каждый период свой).
    let max = prof.map_or(0.0, |pr| {
        pr.iter().fold(0.0f64, |m, h| m.max(h.profit.abs()))
    });
    let mut col = v_flex()
        .flex_1()
        .min_w_0()
        .gap(design::ui_px(cx, 2.0))
        .child(
            // Фикс. высота + truncate: длинная подпись «Custom» (дд.мм.гг – дд.мм.гг)
            // в узком столбце иначе переносится на 2 строки и сбивает выравнивание
            // строк-часов относительно соседних столбцов.
            div()
                .w_full()
                .flex_none()
                .h(design::fit_h_px(cx, 16.0, 11.0, 3.0))
                .min_w_0()
                .truncate()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_soft))
                .child(label),
        );
    match prof {
        None => {
            col = col.child(
                div()
                    .py(design::ui_px(cx, 6.0))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.time.no_data").to_string()),
            );
        }
        Some(pr) => {
            for (h, stat) in pr.iter().enumerate() {
                col = col.child(time_row(h, *stat, max, p, cx));
            }
        }
    }
    col.into_any_element()
}

/// Строка часа: «HH | бар от центра | +PnL$ (n)». Бар вправо = прибыль (зелёный),
/// влево = убыток (красный); длина ∝ |PnL| / максимум столбца.
fn time_row(
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
    // Убыток = оранжевый (как sign_color и бары «Сводки»), НЕ красный.
    let bar_color = if profit > 0.0 { p.green } else { p.orange };
    let value = if has {
        format!("{}$ ({})", fmt_signed(profit), stat.trades)
    } else {
        "—".to_string()
    };
    let r = design::ui_px(cx, 2.0);
    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, 18.0, 11.0, 3.5))
        .items_center()
        .gap(design::ui_px(cx, 4.0))
        .child(
            div()
                .flex_none()
                .w(design::font_w_px(cx, 18.0))
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(format!("{h:02}")),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(design::ui_px(cx, 12.0))
                .rounded(r)
                .bg(moon_alpha(p.panel_high, 0.6))
                // Центральная риска (ноль).
                .child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .left(relative(0.5))
                        .w(px(1.0))
                        .bg(moon_alpha(p.border, 0.8)),
                )
                .when(has && frac > 0.0 && profit > 0.0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .left(relative(0.5))
                            .w(relative(0.5 * frac))
                            .rounded(r)
                            .bg(moon(bar_color)),
                    )
                })
                .when(has && frac > 0.0 && profit < 0.0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .right(relative(0.5))
                            .w(relative(0.5 * frac))
                            .rounded(r)
                            .bg(moon(bar_color)),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .w(design::font_w_px(cx, 92.0))
                .text_right()
                .text_size(design::t_caption(cx))
                .text_color(moon(if has {
                    sign_color(p, profit)
                } else {
                    p.text_muted
                }))
                .child(value),
        )
}
