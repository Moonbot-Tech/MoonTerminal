//! Вкладка «Стратегии» окна «Аналитика» — рабочее место анализа стратегии.
//! Слева всегда список (сравнение по ID: сделки/WR/прибыль/ср./PF/best/worst,
//! свой скролл), режимы кнопками в шапке списка (дефолт — «Фильтры»):
//! - «Фильтры» — справа тюнер порогов (KPI Факт vs v1/v2 + сетка от/до) в
//!   СКОУПЕ выбранной стратегии, внизу прибитая гистограмма поля;
//! - «Монеты» — справа таблица по монетам выбранной (или всех сделок).
//!
//! Страница «Тюнинг стратегий» целиком: этот модуль-корень — список стратегий +
//! диспетчер режимов, подмодули — тюнеры и их общая оболочка:
//! `state` (состояние + `TunerKind`), `shell` (общий тулбар/строка подбора),
//! `fields`/`actions`/`hist` — ось «По фильтру», `time`/`grid` — ось «По времени».

// Подмодули страницы тюнинга.
mod actions;
mod fields;
mod grid;
mod hist;
mod shell;
mod sliders;
mod state;
mod time;

// Типы состояния, которые держит `AnalyticsView` (родитель).
pub(super) use grid::TimeTunerState;
pub(super) use state::TunerState;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use super::AnalyticsView;
use super::LoadState;
use super::summary::{fmt_signed, sign_color};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::GroupStat;

/// Показываем не больше стольких групп (реплика может держать тысячи имён;
/// хвост за пределами топа по |прибыли| малоинформативен, а DOM — не резиновый).
const MAX_ROWS: usize = 300;

/// One NUMERIC column of the comparison tables.
///
/// The heading and the body cell come from the SAME descriptor, so such a column cannot exist in
/// one and not the other, and its value cannot end up rendered under a neighbour's heading — a
/// misalignment no layout would reveal, since the table renders cleanly either way and only the
/// titles are wrong.
///
/// The two leading identity columns (kind, core) stay hand-paired: they are left-aligned, carry
/// their own tones and caption size, and modelling that here would add three optional fields for
/// two columns that no reordering touches.
#[derive(Clone, Copy)]
struct MetricCol {
    /// i18n key of the heading.
    key: &'static str,
    /// Column width, in font-scaled px.
    w: f32,
    /// Cell text for one aggregate.
    text: fn(&GroupStat) -> String,
    /// Value whose sign colours the cell; `None` renders neutral.
    signed: Option<fn(&GroupStat) -> f64>,
}

const COL_TRADES: MetricCol = MetricCol {
    key: "analytics.kpi.trades",
    w: 56.0,
    text: |g| g.n.to_string(),
    signed: None,
};
const COL_PROFIT: MetricCol = MetricCol {
    key: "analytics.col.profit",
    w: 84.0,
    text: |g| fmt_signed(g.profit),
    signed: Some(|g| g.profit),
};
const COL_AVG: MetricCol = MetricCol {
    key: "analytics.kpi.avg_short",
    w: 70.0,
    text: |g| fmt_signed(g.avg()),
    signed: Some(|g| g.avg()),
};
const COL_WINRATE: MetricCol = MetricCol {
    key: "analytics.kpi.winrate",
    w: 56.0,
    text: |g| format!("{:.1}%", g.winrate()),
    signed: None,
};
const COL_PF: MetricCol = MetricCol {
    key: "analytics.col.pf",
    w: 52.0,
    text: |g| format!("{:.2}", g.pf),
    signed: None,
};
const COL_BEST: MetricCol = MetricCol {
    key: "analytics.col.best",
    w: 70.0,
    text: |g| fmt_signed(g.best),
    signed: Some(|g| g.best),
};
const COL_WORST: MetricCol = MetricCol {
    key: "analytics.col.worst",
    w: 70.0,
    text: |g| fmt_signed(g.worst),
    signed: Some(|g| g.worst),
};

/// Strategy comparison columns, in reading order: identity, then how many trades back the figure,
/// then the result, then how good it is, then the tails. Profit leads the numbers because the
/// table is sorted by it — the sort key sitting mid-row leaves the ranking without an anchor, and
/// winrate placed ahead of it reads a 100%-on-two-trades row as the winner.
const METRIC_COLS: &[MetricCol] = &[
    COL_TRADES,
    COL_PROFIT,
    COL_AVG,
    COL_WINRATE,
    COL_PF,
    COL_BEST,
    COL_WORST,
];

/// Per-coin columns: the same order, minus the per-trade average and the best case. Both tables
/// are visible at once in "Монеты" mode, so a differing order would cost the reader the position
/// cue they just learned on the left.
const COIN_COLS: &[MetricCol] = &[COL_TRADES, COL_PROFIT, COL_WINRATE, COL_PF, COL_WORST];

/// Geometry of the coins panel. The coin-name column is whatever [`COIN_COLS`] leaves over, so
/// these are the terms a width test has to read rather than re-state.
const COIN_PANEL_W: f32 = 460.0;
const COIN_ROW_PAD_X: f32 = 8.0;
const COIN_ROW_GAP: f32 = 8.0;

/// Разобрать ключ строки списка стратегий `"strategyid@core_uid"` (список разбит
/// ПО ЯДРАМ) → `(strategyid, Some(core_uid))`. Легаси без ядра (`"5"`) → `(5, None)`;
/// нечисловой ключ (не стратегия) → `None`.
pub(super) fn parse_strat_key(key: &str) -> Option<(i64, Option<u64>)> {
    match key.split_once('@') {
        Some((sid, core)) => Some((sid.parse().ok()?, core.parse::<u64>().ok())),
        None => Some((key.parse().ok()?, None)),
    }
}

/// Режим правой панели вкладки «Тюнинг стратегий» (дефолт — «По фильтру»):
/// по фильтру (пороги), по монете (таблица), по времени (профиль «час дня»).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StratMode {
    Filters,
    Coins,
    Time,
}

impl AnalyticsView {
    /// Смена выбранной стратегии: детализация + скоуп тюнера.
    fn set_sel_strategy(&mut self, sel: Option<(String, String)>, cx: &mut Context<Self>) {
        self.sel_strategy = sel;
        // Retain ready detail while another strategy loads to avoid a loading
        // flash; deselection or a completed non-data result clears it.
        self.reload_detail(cx);
        // Скоуп тюнера/профиля сменился — старые расчёты (включая автоподбор) неверны.
        self.tuner.invalidate();
        // Сетка расписания «По времени» — СВОЯ на стратегию: сбрасываем, чтобы v1
        // предыдущей стратегии не «утёк» на новую (иначе Save записал бы чужое).
        self.time_tuner.reset_grid();
        self.time_dirty = true;
        if self.strat_mode == StratMode::Filters {
            self.reload_tuner(cx);
            self.reload_hist(cx);
        }
        if self.strat_mode == StratMode::Time {
            self.reload_time(cx);
        }
        cx.notify();
    }

    /// Change strategy mode and refresh dirty tuner data when entering Filters.
    fn set_strat_mode(&mut self, mode: StratMode, cx: &mut Context<Self>) {
        if self.strat_mode == mode {
            return;
        }
        self.strat_mode = mode;
        if mode == StratMode::Filters && self.tuner.needs_reload() {
            self.reload_tuner(cx);
            self.reload_hist(cx);
        }
        if mode == StratMode::Time && (self.time_profiles.is_none() || self.time_dirty) {
            self.reload_time(cx);
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
        // «По времени» — своя раскладка (список + карта + KPI + сетка расписания).
        if mode == StratMode::Time {
            return self.strat_time(p, window, cx);
        }
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
            StratMode::Time => unreachable!("Time mode returns early above"),
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
            StratMode::Time => unreachable!("Time mode returns early above"),
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
        // Окно копии: редактируемое имя новой стратегии первой строкой.
        if dlg.copy {
            if let Some(input) = self.tuner.inputs.get("copy-name") {
                list = list.child(
                    h_flex()
                        .w_full()
                        .px(design::ui_px(cx, 10.0))
                        .py(design::ui_px(cx, 5.0))
                        .gap(design::ui_px(cx, 8.0))
                        .items_center()
                        .child(
                            div()
                                .flex_none()
                                .text_size(design::t_caption(cx))
                                .text_color(moon(p.text_muted))
                                .child(t!("analytics.tuner.copy_name_lbl").to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(MoonInput::new("tun-copy-name").state(input).small()),
                        ),
                );
            }
        }
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
                                            if dlg.copy {
                                                t!(
                                                    "analytics.tuner.copy_title",
                                                    name = dlg.name.as_str()
                                                )
                                            } else {
                                                t!(
                                                    "analytics.tuner.save_title",
                                                    name = dlg.name.as_str()
                                                )
                                            }
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
    pub(super) fn strat_list_card(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // Resolved once for the whole list: this table is not virtualized, so a per-cell lookup
        // would clone the theme tokens twice for each of up to 300 rows × 7 columns.
        let scale = design::font_scale(cx);
        let (list, total, shown): (AnyElement, usize, usize) =
            match self.data.view(|d| d.strategies.is_empty()) {
                Ok(d) => {
                    let total = d.strategies.len();
                    let shown = total.min(MAX_ROWS);
                    let mut list = v_flex().w_full().gap_0();
                    for g in d.strategies.iter().take(MAX_ROWS) {
                        list = list.child(self.strategy_row(g, p, scale, cx));
                    }
                    (list.into_any_element(), total, shown)
                }
                Err(note) => (
                    super::note_el("an-strat-list-note", note, 18.0, p, cx),
                    0,
                    0,
                ),
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
                        t!("analytics.strat.mode_filter").to_string(),
                    ))
                    .child(mode_btn(
                        "sm-coins",
                        StratMode::Coins,
                        t!("analytics.strat.mode_coin").to_string(),
                    ))
                    .child(mode_btn(
                        "sm-time",
                        StratMode::Time,
                        t!("analytics.strat.mode_time").to_string(),
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
    fn strategy_row(
        &self,
        g: &GroupStat,
        p: MoonPalette,
        scale: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = self.sel_strategy.as_ref().is_some_and(|(k, _)| *k == g.key);
        let key = g.key.clone();
        // strategyid=0 = ручные ордера — подпись вместо голого «0» (и в
        // строке, и в выборе: заголовки тюнера/диалогов берут её же).
        let name = super::summary::strat_display(&g.name);
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
                    .child(div().flex_1().min_w_0().truncate().child(name.clone())),
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
                    .children(METRIC_COLS.iter().map(|c| metric_cell(c, g, p, scale))),
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
        // The selected-strategy detail and the all-strategies summary each keep
        // their own load note so either read failure remains visible.
        let scale = design::font_scale(cx);
        let (coins, scope): (Result<Vec<GroupStat>, super::Note>, String) = match &self.sel_strategy
        {
            Some((_, name)) => (
                self.detail
                    .view(|d| d.coins.is_empty())
                    .map(|d| d.coins.clone()),
                name.clone(),
            ),
            None => (
                self.data
                    .view(|d| d.coins.is_empty())
                    .map(|d| d.coins.clone()),
                t!("analytics.strat.scope_all").to_string(),
            ),
        };
        let head = h_flex()
            .w_full()
            .flex_none()
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .px(design::ui_px(cx, COIN_ROW_PAD_X))
            .gap(design::ui_px(cx, COIN_ROW_GAP))
            .items_center()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(div().flex_1().child(t!("analytics.col.coin").to_string()))
            .children(COIN_COLS.iter().map(|c| head_cell(c, scale)));

        let body: AnyElement = match coins {
            Err(note) => super::note_el("an-strat-coins-note", note, 10.0, p, cx),
            Ok(coins) => {
                let mut list = v_flex().w_full();
                for c in coins.iter().take(MAX_ROWS) {
                    list = list.child(
                        h_flex()
                            .w_full()
                            .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                            .px(design::ui_px(cx, COIN_ROW_PAD_X))
                            .gap(design::ui_px(cx, COIN_ROW_GAP))
                            .items_center()
                            .border_t_1()
                            .border_color(moon_alpha(p.border, 0.5))
                            .child(div().flex_1().min_w_0().truncate().child(c.name.clone()))
                            .children(COIN_COLS.iter().map(|col| metric_cell(col, c, p, scale))),
                    );
                }
                list.into_any_element()
            }
        };

        v_flex()
            .w(design::font_w_px(cx, COIN_PANEL_W))
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
    let scale = design::font_scale(cx);
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
                .children(METRIC_COLS.iter().map(|c| head_cell(c, scale))),
        )
}

/// Heading cell of a [`MetricCol`], right-aligned over its numbers.
///
/// `scale` is the caller's hoisted [`design::font_scale`]: resolving it per cell costs two
/// by-value theme-token clones each, and this table renders every row (it is not virtualized).
fn head_cell(col: &MetricCol, scale: f32) -> impl IntoElement {
    div()
        .w(px(col.w * scale))
        .flex_none()
        .text_right()
        .child(t!(col.key).to_string())
}

/// Body cell of a [`MetricCol`]: text and sign colour both derived from the same descriptor, so
/// a reordered column carries its colouring with it.
///
/// `scale` is the caller's hoisted font scale shared by every cell in the rendered table.
fn metric_cell(col: &MetricCol, g: &GroupStat, p: MoonPalette, scale: f32) -> impl IntoElement {
    let color = match col.signed {
        Some(value) => sign_color(p, value(g)),
        None => p.text_soft,
    };
    num_cell(scale, col.w, (col.text)(g), color)
}

/// Render one right-aligned numeric cell using a caller-hoisted font `scale`.
fn num_cell(scale: f32, w: f32, text: String, color: u32) -> impl IntoElement {
    div()
        .w(px(w * scale))
        .flex_none()
        .text_right()
        .text_color(moon(color))
        .child(text)
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests {
    use super::{
        COIN_COLS, COIN_PANEL_W, COIN_ROW_GAP, COIN_ROW_PAD_X, COL_PROFIT, COL_TRADES, COL_WINRATE,
        METRIC_COLS, MetricCol,
    };

    fn position(cols: &[MetricCol], key: &str) -> usize {
        cols.iter()
            .position(|c| c.key == key)
            .unwrap_or_else(|| panic!("column {key} missing"))
    }

    /// The tables are sorted by profit descending (SQL-side, `db::analytics`). The sort key has
    /// to lead the numbers: buried mid-row it leaves the ranking without an anchor, and winrate
    /// ahead of it presents a 100%-on-two-trades row as the top performer.
    #[test]
    fn profit_anchors_the_numeric_columns() {
        assert_eq!(METRIC_COLS[0].key, COL_TRADES.key, "trade count leads");
        assert_eq!(METRIC_COLS[1].key, COL_PROFIT.key, "profit follows it");
        assert!(
            position(METRIC_COLS, COL_PROFIT.key) < position(METRIC_COLS, COL_WINRATE.key),
            "profit must precede winrate"
        );
        // COIN_COLS inherits the ordering via `coin_columns_keep_the_strategy_order`.
    }

    /// Both tables sit on the same screen in "Монеты" mode, so a metric present in both has to
    /// keep the same relative position — otherwise the position cue learned on one misreads the
    /// other.
    #[test]
    fn coin_columns_keep_the_strategy_order() {
        let shared: Vec<&str> = METRIC_COLS
            .iter()
            .map(|c| c.key)
            .filter(|k| COIN_COLS.iter().any(|c| c.key == *k))
            .collect();
        let coin_order: Vec<&str> = COIN_COLS.iter().map(|c| c.key).collect();
        assert_eq!(shared, coin_order);
    }

    /// The coin panel is a fixed-width box and its name column is the residual, so widening a
    /// shared descriptor for the roomier strategy table silently eats the coin name here.
    #[test]
    fn coin_columns_leave_room_for_the_name() {
        let numbers: f32 = COIN_COLS.iter().map(|c| c.w).sum();
        let name_w =
            COIN_PANEL_W - COIN_ROW_PAD_X * 2.0 - COIN_ROW_GAP * COIN_COLS.len() as f32 - numbers;
        assert!(
            name_w >= 70.0,
            "coin name column squeezed to {name_w}px by the shared descriptors"
        );
    }
}

impl AnalyticsView {
    /// Фоновая детализация выбранной стратегии (перенесена из `analytics::mod` —
    /// страничная логика при своей странице). Скоуп — фильтры + выбранная строка.
    pub(super) fn reload_detail(&mut self, cx: &mut Context<Self>) {
        let Some((key, _)) = self.sel_strategy.clone() else {
            self.detail = LoadState::default();
            return;
        };
        // Ключ `strategyid@core_uid` — детализация по стратегии НА ЭТОМ ядре.
        let Some((id, core)) = parse_strat_key(&key) else {
            self.detail = LoadState::default();
            return;
        };
        self.detail_seq = self.detail_seq.wrapping_add(1);
        let req = self.detail_seq;
        let mut q = self.query();
        q.strat_core = core;
        self.detail.begin();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let detail = executor
                .spawn(async move { moon_core::db::analytics::strategy_detail(&q, id) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.detail_seq != req {
                        return;
                    }
                    this.detail.apply(detail);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Перечитать данные АКТИВНОЙ оси тюнинга (после записи в стратегию — освежить
    /// её чипы/KPI: у «По времени» — `reload_time`, иначе — `reload_tuner`).
    pub(super) fn reload_active_tuner(&mut self, cx: &mut Context<Self>) {
        match self.strat_mode {
            StratMode::Time => self.reload_time(cx),
            _ => self.reload_tuner(cx),
        }
    }
}
