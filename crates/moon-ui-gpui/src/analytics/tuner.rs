//! Тюнер порогов по рыночным полям отчёта (приём из «Аналитики V3») — режим
//! «Фильтры» вкладки «Стратегии»: матрица KPI «Факт vs варианты», конструктор
//! диапазонов от/до по полям и гистограмма профита по квантильным вёдрам
//! выбранного поля. Скоуп — выбранная в списке стратегия (или все). Границы
//! персистятся в layout (строками, как ввёл юзер).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonInputEvent, MoonInputState,
    MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use super::AnalyticsView;
use super::summary::{fmt_signed, sign_color};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::config::layout::{TunerBoundCfg, TunerVariantCfg};
use moon_core::db::tuner::{Bound, FIELDS, HistBucket, Suggestion, VarStats, Variant};

/// Вариантов помимо «Факта» (V3 держит 8 — начинаем с двух).
pub(super) const N_VAR: usize = 2;
/// Вёдер гистограммы.
const HIST_BUCKETS: usize = 14;

/// Состояние тюнера внутри `AnalyticsView`.
pub(super) struct TunerState {
    /// Границы вариантов текстом: `[вариант][индекс поля] = (от, до)`.
    pub(super) bounds: Vec<Vec<(String, String)>>,
    /// Кэш инпутов границ (ленивое создание в render).
    inputs: HashMap<String, Entity<MoonInputState>>,
    /// Поле гистограммы (индекс в `FIELDS`).
    pub(super) sel_field: usize,
    pub(super) stats: Option<Arc<Vec<VarStats>>>,
    pub(super) hist: Option<Arc<Vec<HistBucket>>>,
    /// Пороговые параметры выбранной стратегии по полям (чипы у имён полей).
    strat_bounds: Arc<HashMap<&'static str, (Option<f64>, Option<f64>)>>,
    /// Результаты автоподбора (кнопка «Подобрать»).
    sugg: Option<Arc<Vec<Suggestion>>>,
    sugg_busy: bool,
    seq: u64,
    hist_seq: u64,
    sugg_seq: u64,
}

impl TunerState {
    /// Загрузка границ из layout (незнакомые поля молча пропускаются).
    pub(super) fn load(cfg: &[TunerVariantCfg]) -> Self {
        let mut bounds = vec![vec![(String::new(), String::new()); FIELDS.len()]; N_VAR];
        for (vi, v) in cfg.iter().take(N_VAR).enumerate() {
            for b in &v.bounds {
                if let Some(fi) = FIELDS.iter().position(|(c, _)| *c == b.field) {
                    bounds[vi][fi] = (b.from.clone(), b.to.clone());
                }
            }
        }
        Self {
            bounds,
            inputs: HashMap::new(),
            sel_field: 0,
            stats: None,
            hist: None,
            strat_bounds: Arc::new(HashMap::new()),
            sugg: None,
            sugg_busy: false,
            seq: 0,
            hist_seq: 0,
            sugg_seq: 0,
        }
    }

    /// Сброс всех расчётов (смена скоупа/фильтров/периода) — пересчёт при
    /// следующем входе в режим «Фильтры» или явным reload_*.
    pub(super) fn invalidate(&mut self) {
        self.stats = None;
        self.hist = None;
        self.sugg = None;
    }

    /// Варианты для запроса: [пустой «Факт», v1..vN].
    fn variants(&self) -> Vec<Variant> {
        let mut out = vec![Variant::default()];
        for v in &self.bounds {
            let bounds = v
                .iter()
                .enumerate()
                .filter_map(|(fi, (from, to))| {
                    let from = parse_num(from);
                    let to = parse_num(to);
                    (from.is_some() || to.is_some()).then(|| Bound {
                        field: FIELDS[fi].0.to_string(),
                        from,
                        to,
                    })
                })
                .collect();
            out.push(Variant { bounds });
        }
        out
    }
}

/// Число из поля ввода: запятая как точка, суффиксы k/M/B/T (и кириллические
/// к/м), пусто/мусор = None.
fn parse_num(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let (mut t, mut mult) = (s.as_str(), 1.0f64);
    if let Some(c) = s.chars().last() {
        let m = match c {
            'k' | 'K' | 'к' | 'К' => 1e3,
            'm' | 'M' | 'м' | 'М' => 1e6,
            'b' | 'B' => 1e9,
            't' | 'T' => 1e12,
            _ => 1.0,
        };
        if m != 1.0 {
            mult = m;
            t = &s[..s.len() - c.len_utf8()];
        }
    }
    t.trim()
        .parse::<f64>()
        .ok()
        .map(|v| v * mult)
        .filter(|v| v.is_finite())
}

impl AnalyticsView {
    /// Запрос тюнера: общие фильтры + скоуп выбранной стратегии.
    fn tuner_query(&self) -> moon_core::db::analytics::Query {
        let mut q = self.query();
        q.strategy = self
            .sel_strategy
            .as_ref()
            .and_then(|(k, _)| k.parse::<i64>().ok());
        q
    }

    /// Фоновый пересчёт матрицы KPI по вариантам (+ пороги выбранной стратегии).
    pub(super) fn reload_tuner(&mut self, cx: &mut Context<Self>) {
        self.tuner.seq = self.tuner.seq.wrapping_add(1);
        let req = self.tuner.seq;
        let q = self.tuner_query();
        let sid = q.strategy;
        let variants = self.tuner.variants();
        // Дефолты схемы ядра (числовые поля): чипы прячут значения, равные
        // дефолту — «фильтр не настроен» порогом не является.
        let defaults: HashMap<String, f64> = {
            let b = self.backend.read(cx);
            let store = b.session.store();
            let mut out = HashMap::new();
            for (_, cd) in store.cores() {
                let Some(sch) = cd.schema.as_ref() else { continue };
                for k in &sch.kinds {
                    for s in &k.sections {
                        for f in &s.fields {
                            let Some(d) = f.default.as_ref() else { continue };
                            if let Ok(v) = d
                                .trim()
                                .trim_end_matches('%')
                                .replace(',', ".")
                                .parse::<f64>()
                            {
                                out.entry(f.name.to_ascii_lowercase()).or_insert(v);
                            }
                        }
                    }
                }
                break; // схемы ядер совпадают — одной достаточно
            }
            out
        };
        // Автоподбор НЕ сбрасываем: правка границ не меняет распределение
        // факта, по которому он считался (сброс — в invalidate()).
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let (stats, strat_bounds) = executor
                .spawn(async move {
                    let stats = moon_core::db::tuner::variant_stats(&q, &variants);
                    let sb = sid
                        .map(|sid| moon_core::db::tuner::strategy_bounds(sid, &defaults))
                        .unwrap_or_default();
                    (stats, sb)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.tuner.seq != req {
                        return;
                    }
                    this.tuner.stats = stats.map(Arc::new);
                    this.tuner.strat_bounds = Arc::new(strat_bounds);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Фоновый автоподбор порогов (перебор пар квантильных краёв всех полей).
    fn reload_suggest(&mut self, cx: &mut Context<Self>) {
        self.tuner.sugg_seq = self.tuner.sugg_seq.wrapping_add(1);
        let req = self.tuner.sugg_seq;
        self.tuner.sugg_busy = true;
        let q = self.tuner_query();
        // Не позволяем «оптимизации» схлопнуться в пару счастливых сделок:
        // держим минимум 1/5 фактических сделок (но не меньше 30).
        let min_n = self
            .tuner
            .stats
            .as_ref()
            .and_then(|s| s.first().map(|f| f.n / 5))
            .unwrap_or(0)
            .max(30);
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let sugg = executor
                .spawn(async move { moon_core::db::tuner::auto_suggest(&q, min_n) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.tuner.sugg_seq != req {
                        return;
                    }
                    this.tuner.sugg_busy = false;
                    this.tuner.sugg = sugg.map(Arc::new);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Фоновая гистограмма выбранного поля.
    pub(super) fn reload_hist(&mut self, cx: &mut Context<Self>) {
        self.tuner.hist_seq = self.tuner.hist_seq.wrapping_add(1);
        let req = self.tuner.hist_seq;
        let q = self.tuner_query();
        let field = FIELDS[self.tuner.sel_field].0.to_string();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let hist = executor
                .spawn(async move { moon_core::db::tuner::histogram(&q, &field, HIST_BUCKETS) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.tuner.hist_seq != req {
                        return;
                    }
                    this.tuner.hist = hist.map(Arc::new);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Коммит границы (Blur/Enter инпута): состояние → layout → пересчёт.
    fn commit_bound(&mut self, vi: usize, fi: usize, is_to: bool, value: String, cx: &mut Context<Self>) {
        let slot = &mut self.tuner.bounds[vi][fi];
        let cur = if is_to { &mut slot.1 } else { &mut slot.0 };
        if *cur == value {
            return;
        }
        *cur = value;
        self.persist_tuner(cx);
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Программная установка ОБЕИХ границ поля (чип стратегии / очистка /
    /// автоподбор): состояние + тихая синхронизация инпутов + пересчёт.
    fn apply_bounds(&mut self, vi: usize, fi: usize, from: String, to: String, cx: &mut Context<Self>) {
        if self.tuner.bounds[vi][fi] == (from.clone(), to.clone()) {
            return;
        }
        self.tuner.bounds[vi][fi] = (from.clone(), to.clone());
        for (is_to, value) in [(false, from), (true, to)] {
            let id = format!("tv{vi}f{fi}{}", if is_to { "b" } else { "a" });
            if let Some(state) = self.tuner.inputs.get(&id) {
                // sync_value не эмитит Change/Blur — не зацикливает коммит.
                state.update(cx, |s, cx| s.sync_value(value, cx));
            }
        }
        self.persist_tuner(cx);
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Границы вариантов → layout (персист как ввёл пользователь).
    fn persist_tuner(&mut self, cx: &mut Context<Self>) {
        let cfg: Vec<TunerVariantCfg> = self
            .tuner
            .bounds
            .iter()
            .map(|v| TunerVariantCfg {
                bounds: v
                    .iter()
                    .enumerate()
                    .filter(|(_, (f, t))| !f.trim().is_empty() || !t.trim().is_empty())
                    .map(|(fi, (f, t))| TunerBoundCfg {
                        field: FIELDS[fi].0.to_string(),
                        from: f.clone(),
                        to: t.clone(),
                    })
                    .collect(),
            })
            .collect();
        self.backend.update(cx, |b, _| {
            if b.layout.analytics_tuner != cfg {
                b.layout.analytics_tuner = cfg;
                b.layout_dirty = true;
            }
        });
    }

    /// Инпут границы с ленивым кэшем (паттерн field_input_state Стратегий).
    fn bound_input(
        &mut self,
        vi: usize,
        fi: usize,
        is_to: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = format!("tv{vi}f{fi}{}", if is_to { "b" } else { "a" });
        if let Some(state) = self.tuner.inputs.get(&id) {
            return state.clone();
        }
        let slot = &self.tuner.bounds[vi][fi];
        let value = if is_to { slot.1.clone() } else { slot.0.clone() };
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                let value = state.read(cx).value().to_string();
                this.commit_bound(vi, fi, is_to, value, cx);
            }
        })
        .detach();
        self.tuner.inputs.insert(id, state.clone());
        state
    }

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

    /// Конструктор диапазонов: строки-поля, клик по имени — гистограмма поля.
    pub(super) fn fields_grid(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let in_w = 56.0;
        let mut head = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(
                div()
                    .w(design::font_w_px(cx, 58.0))
                    .flex_none()
                    .child(t!("analytics.tuner.field").to_string()),
            )
            // Колонка чипа: пороги выбранной стратегии (клик — в v1).
            .child(div().flex_1().child(t!("analytics.tuner.strat_chip").to_string()));
        for vi in 0..N_VAR {
            head = head
                .child(
                    div()
                        .w(design::font_w_px(cx, in_w))
                        .flex_none()
                        .text_right()
                        .child(format!("v{} {}", vi + 1, t!("analytics.tuner.from"))),
                )
                .child(
                    div()
                        .w(design::font_w_px(cx, in_w))
                        .flex_none()
                        .text_right()
                        .child(t!("analytics.tuner.to").to_string()),
                )
                // Спейсер под кнопку очистки строки.
                .child(div().w(design::ui_px(cx, 12.0)).flex_none());
        }

        let strat_bounds = self.tuner.strat_bounds.clone();
        let mut grid = v_flex().w_full().child(head);
        for fi in 0..FIELDS.len() {
            let selected = self.tuner.sel_field == fi;
            let mut row = h_flex()
                .id(SharedString::from(format!("tun-field-{fi}")))
                .w_full()
                .px(design::ui_px(cx, 8.0))
                .py(design::ui_px(cx, 2.0))
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .border_t_1()
                .border_color(moon_alpha(p.border, 0.5))
                .child(
                    div()
                        .w(design::font_w_px(cx, 58.0))
                        .flex_none()
                        .truncate()
                        .cursor_pointer()
                        .text_color(moon(if selected { p.amber } else { p.text }))
                        .child(FIELDS[fi].1.to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.tuner.sel_field != fi {
                        this.tuner.sel_field = fi;
                        this.tuner.hist = None;
                        this.reload_hist(cx);
                        cx.notify();
                    }
                }));
            if selected {
                row = row.bg(moon_alpha(p.amber, 0.08));
            }
            // Чип: НЕдефолтные пороги выбранной стратегии по этому полю
            // (справочно, некликабелен).
            let chip = strat_bounds.get(FIELDS[fi].0).copied();
            row = row.child(match chip {
                Some((lo, hi)) => {
                    let text = [
                        lo.map(|v| format!("min({})", fmt_bound(v))),
                        hi.map(|v| format!("max({})", fmt_bound(v))),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.amber))
                        .child(text)
                        .into_any_element()
                }
                None => div().flex_1().into_any_element(),
            });
            for vi in 0..N_VAR {
                for is_to in [false, true] {
                    let input = self.bound_input(vi, fi, is_to, window, cx);
                    row = row.child(
                        div().w(design::font_w_px(cx, in_w)).flex_none().child(
                            MoonInput::new(SharedString::from(format!("tun-in-{vi}-{fi}-{is_to}")))
                                .state(&input)
                                .small(),
                        ),
                    );
                }
                // Очистка обеих границ варианта в этой строке.
                row = row.child(
                    div()
                        .id(SharedString::from(format!("tun-clr-{vi}-{fi}")))
                        .flex_none()
                        .px(design::ui_px(cx, 2.0))
                        .cursor_pointer()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .hover(move |s| s.text_color(moon(p.orange)))
                        .child("✕")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.apply_bounds(vi, fi, String::new(), String::new(), cx);
                        })),
                );
            }
            grid = grid.child(row);
        }
        card(
            t!("analytics.tuner.fields_title").to_string(),
            t!("analytics.tuner.fields_sub").to_string(),
            grid.into_any_element(),
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
                                div().w_full().h(half).flex().items_end().justify_center().child(
                                    div()
                                        .w(relative(0.62))
                                        .h(relative(up.max(if b.wsum > 0.0 { 0.02 } else { 0.0 })))
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
                                            .h(relative(dn.max(if b.lsum > 0.0 { 0.02 } else { 0.0 })))
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

    /// Карточка автоподбора: кнопка «Подобрать» + топ диапазонов по профиту.
    pub(super) fn suggest_card(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let fact_profit = self
            .tuner
            .stats
            .as_ref()
            .and_then(|s| s.first().map(|f| f.profit))
            .unwrap_or(0.0);
        let body: AnyElement = if self.tuner.sugg_busy {
            div()
                .p(design::ui_px(cx, 8.0))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.loading").to_string())
                .into_any_element()
        } else {
            match self.tuner.sugg.clone() {
                None => div()
                    .p(design::ui_px(cx, 8.0))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.tuner.suggest_hint").to_string())
                    .into_any_element(),
                Some(sugg) => {
                    let mut list = v_flex().w_full();
                    for (i, s) in sugg.iter().take(8).enumerate() {
                        let range = format!(
                            "{} {}  {} {}",
                            t!("analytics.tuner.from"),
                            s.from.map(fmt_bound).unwrap_or_else(|| "—".into()),
                            t!("analytics.tuner.to"),
                            s.to.map(fmt_bound).unwrap_or_else(|| "—".into()),
                        );
                        let delta = s.profit - fact_profit;
                        let (from_s, to_s) = (
                            s.from.map(fmt_bound).unwrap_or_default(),
                            s.to.map(fmt_bound).unwrap_or_default(),
                        );
                        let field = s.field;
                        let mut row = h_flex()
                            .w_full()
                            .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                            .px(design::ui_px(cx, 8.0))
                            .gap(design::ui_px(cx, 6.0))
                            .items_center()
                            .border_t_1()
                            .border_color(moon_alpha(p.border, 0.5))
                            .child(
                                div()
                                    .w(design::font_w_px(cx, 58.0))
                                    .flex_none()
                                    .truncate()
                                    .child(s.label.to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(p.text_soft))
                                    .child(range),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(moon(sign_color(p, delta)))
                                    .child(format!("{} ({})", fmt_signed(delta), s.n)),
                            );
                        for vi in 0..N_VAR {
                            let (from_s, to_s) = (from_s.clone(), to_s.clone());
                            row = row.child(
                                MoonButton::new(SharedString::from(format!("sug-{i}-v{vi}")))
                                    .variant(MoonButtonVariant::Soft)
                                    .size(MoonButtonSize::Micro)
                                    .label(format!("→v{}", vi + 1))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let Some(fi) =
                                            FIELDS.iter().position(|(c, _)| *c == field)
                                        else {
                                            return;
                                        };
                                        this.apply_bounds(
                                            vi,
                                            fi,
                                            from_s.clone(),
                                            to_s.clone(),
                                            cx,
                                        );
                                    }))
                                    .render(),
                            );
                        }
                        list = list.child(row);
                    }
                    list.into_any_element()
                }
            }
        };
        // card() с кастомной шапкой: заголовок + кнопка запуска.
        v_flex()
            .w_full()
            .flex_none()
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
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        div()
                            .text_size(design::t_title(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("analytics.tuner.suggest_title").to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        MoonButton::new("tun-suggest-run")
                            .variant(MoonButtonVariant::Blue)
                            .size(MoonButtonSize::Micro)
                            .label(t!("analytics.tuner.suggest_run").to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.tuner.sugg_busy {
                                    this.reload_suggest(cx);
                                    cx.notify();
                                }
                            }))
                            .render(),
                    ),
            )
            .child(body)
            .into_any_element()
    }
}

/// Формат числа для границ/чипов: крупные — с суффиксом k/M/B/T (обратно
/// понимается `parse_num`), прочие — до 4 знаков без хвостовых нулей.
fn fmt_bound(v: f64) -> String {
    let a = v.abs();
    let (div, suf) = if a >= 1e12 {
        (1e12, "T")
    } else if a >= 1e9 {
        (1e9, "B")
    } else if a >= 1e6 {
        (1e6, "M")
    } else if a >= 1e3 {
        (1e3, "k")
    } else {
        (1.0, "")
    };
    let x = v / div;
    let mut s = if suf.is_empty() {
        format!("{x:.4}")
    } else {
        format!("{x:.2}")
    };
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s.push_str(suf);
    s
}

/// Карточка с заголовком и подзаголовком (общий вид карточек Аналитики).
fn card(
    title: String,
    sub: String,
    body: AnyElement,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let mut head = h_flex()
        .w_full()
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 8.0))
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        );
    if !sub.is_empty() {
        head = head.child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(sub),
        );
    }
    v_flex()
        .w_full()
        // В скролл-колонке карточки не должны ужиматься под высоту вьюпорта.
        .flex_none()
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .overflow_hidden()
        .child(head)
        .child(body)
        .into_any_element()
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
