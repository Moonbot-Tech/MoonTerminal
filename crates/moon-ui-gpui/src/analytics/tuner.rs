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
use moon_core::db::tuner::{Bound, FIELDS, FieldClass, HistBucket, StratFilters, VarStats, Variant};

/// Вариантов помимо «Факта» (V3 держит 8 — начинаем с двух).
pub(super) const N_VAR: usize = 2;
/// Вёдер гистограммы.
const HIST_BUCKETS: usize = 14;

/// Состояние тюнера внутри `AnalyticsView`.
pub(super) struct TunerState {
    /// Границы вариантов текстом: `[вариант][индекс поля] = (от, до)`.
    pub(super) bounds: Vec<Vec<(String, String)>>,
    /// Кэш инпутов границ (ленивое создание в render).
    pub(super) inputs: HashMap<String, Entity<MoonInputState>>,
    /// Поле гистограммы (индекс в `FIELDS`).
    pub(super) sel_field: usize,
    pub(super) stats: Option<Arc<Vec<VarStats>>>,
    pub(super) hist: Option<Arc<Vec<HistBucket>>>,
    /// Фильтровая карточка выбранной стратегии (Ignore-флаги + пороги).
    pub(super) strat: Arc<StratFilters>,
    /// Автоподбор (кнопки «Подобрать») выполняется в фоне.
    pub(super) sugg_busy: bool,
    /// Кнопка «В стратегию» ждёт второго клика-подтверждения.
    pub(super) save_confirm: bool,
    seq: u64,
    hist_seq: u64,
    pub(super) sugg_seq: u64,
}

impl TunerState {
    /// Загрузка границ из layout (незнакомые поля молча пропускаются).
    pub(super) fn load(cfg: &[TunerVariantCfg]) -> Self {
        let mut bounds = vec![vec![(String::new(), String::new()); FIELDS.len()]; N_VAR];
        for (vi, v) in cfg.iter().take(N_VAR).enumerate() {
            for b in &v.bounds {
                if let Some(fi) = FIELDS.iter().position(|(c, _, _)| *c == b.field) {
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
            strat: Arc::new(StratFilters::default()),
            sugg_busy: false,
            save_confirm: false,
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
        self.save_confirm = false;
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
pub(super) fn parse_num(s: &str) -> Option<f64> {
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
    pub(super) fn tuner_query(&self) -> moon_core::db::analytics::Query {
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
            let (stats, strat) = executor
                .spawn(async move {
                    let stats = moon_core::db::tuner::variant_stats(&q, &variants);
                    let sf = sid
                        .map(|sid| moon_core::db::tuner::strategy_filters(sid, &defaults))
                        .unwrap_or_default();
                    (stats, sf)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.tuner.seq != req {
                        return;
                    }
                    this.tuner.stats = stats.map(Arc::new);
                    this.tuner.strat = Arc::new(strat);
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
    pub(super) fn apply_bounds(&mut self, vi: usize, fi: usize, from: String, to: String, cx: &mut Context<Self>) {
        if self.tuner.bounds[vi][fi] == (from.clone(), to.clone()) {
            return;
        }
        self.tuner.bounds[vi][fi] = (from, to);
        // Пересоздаём инпуты (сброс кэша): свежий default_value рисуется с
        // НАЧАЛА строки; sync_value оставлял курсор в конце — длинное значение
        // «уезжало» вправо и виден был только хвост.
        self.tuner.inputs.remove(&format!("tv{vi}f{fi}a"));
        self.tuner.inputs.remove(&format!("tv{vi}f{fi}b"));
        self.persist_tuner(cx);
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Границы вариантов → layout (персист как ввёл пользователь).
    pub(super) fn persist_tuner(&mut self, cx: &mut Context<Self>) {
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
        let in_w = 60.0;
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

        let strat = self.tuner.strat.clone();
        let mut grid = v_flex().w_full().child(head);
        let mut last_class: Option<FieldClass> = None;
        for fi in 0..FIELDS.len() {
            let class = FIELDS[fi].2;
            // Подзаголовок группы (класс игноров); у игнорируемой — метка.
            if last_class != Some(class) {
                last_class = Some(class);
                let (label, ignored) = match class {
                    FieldClass::Filter => (
                        t!("analytics.tuner.grp_filter").to_string(),
                        strat.found && strat.ignore_filters,
                    ),
                    FieldClass::DeltaSlot => (
                        t!("analytics.tuner.grp_slot").to_string(),
                        strat.found && strat.class_ignored(FieldClass::DeltaSlot),
                    ),
                    FieldClass::Delta => (
                        t!("analytics.tuner.grp_delta").to_string(),
                        strat.found && strat.class_ignored(FieldClass::Delta),
                    ),
                    FieldClass::Volume => (
                        t!("analytics.tuner.grp_volume").to_string(),
                        strat.found && strat.class_ignored(FieldClass::Volume),
                    ),
                };
                let mut hdr = h_flex()
                    .w_full()
                    .px(design::ui_px(cx, 8.0))
                    .py(design::ui_px(cx, 2.0))
                    .gap(design::ui_px(cx, 6.0))
                    .items_center()
                    .bg(moon_alpha(p.table_head, 0.6))
                    .border_t_1()
                    .border_color(moon_alpha(p.border, 0.7))
                    .text_size(design::t_caption(cx))
                    .child(div().text_color(moon(p.text_soft)).child(label));
                if ignored {
                    hdr = hdr.child(
                        div()
                            .text_color(moon_alpha(p.text_muted, 0.7))
                            .child("ignore"),
                    );
                }
                grid = grid.child(hdr);
            }
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
            // Чип: НЕдефолтные пороги выбранной стратегии (справочно). Поля-
            // слоты показывают назначение «Δ2/Δ3» + пороги слота. Если класс
            // игнорируется флагами — значений НЕ показываем (метка «ignore»
            // стоит на подзаголовке группы).
            let chip: Option<(Option<u8>, Option<f64>, Option<f64>)> =
                if strat.found && !strat.class_ignored(class) {
                    if class == FieldClass::DeltaSlot {
                        strat
                            .slot_of(FIELDS[fi].0)
                            .map(|(n, lo, hi)| (Some(n), lo, hi))
                    } else {
                        strat
                            .bounds
                            .get(FIELDS[fi].0)
                            .copied()
                            .map(|(lo, hi)| (None, lo, hi))
                    }
                } else {
                    None
                };
            row = row.child(match chip {
                Some((slot, lo, hi)) => {
                    let text = [
                        slot.map(|n| format!("Δ{n}")),
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
        // Карточка со своей шапкой: заголовок + кнопки «Подобрать» (выбранное
        // поле → v1), «Подобрать всё» (все поля → v1) и «В стратегию»
        // (запись v1 в параметры выбранной стратегии, с подтверждением).
        let busy = self.tuner.sugg_busy;
        let mut header = h_flex()
            .w_full()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 8.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(
                div()
                    .text_size(design::t_title(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("analytics.tuner.fields_title").to_string()),
            )
            .child(div().flex_1())
            .child(
                MoonButton::new("tun-suggest-one")
                    .variant(MoonButtonVariant::Soft)
                    .size(MoonButtonSize::Micro)
                    .label(if busy {
                        "…".to_string()
                    } else {
                        t!("analytics.tuner.suggest_one").to_string()
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.tuner.sugg_busy {
                            this.suggest_one_into_v1(cx);
                            cx.notify();
                        }
                    }))
                    .render(),
            )
            .child(
                MoonButton::new("tun-suggest-run")
                    .variant(MoonButtonVariant::Blue)
                    .size(MoonButtonSize::Micro)
                    .label(if busy {
                        "…".to_string()
                    } else {
                        t!("analytics.tuner.suggest_run").to_string()
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.tuner.sugg_busy {
                            this.suggest_into_v1(cx);
                            cx.notify();
                        }
                    }))
                    .render(),
            );
        // «В стратегию» — только при выбранной стратегии; двухкликовое
        // подтверждение (первый клик — «Подтвердить?»).
        if self.sel_strategy.is_some() {
            let confirm = self.tuner.save_confirm;
            header = header.child(
                MoonButton::new("tun-save-strat")
                    .variant(if confirm {
                        MoonButtonVariant::Amber
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .size(MoonButtonSize::Micro)
                    .label(if confirm {
                        t!("analytics.tuner.save_confirm").to_string()
                    } else {
                        t!("analytics.tuner.save_btn").to_string()
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.tuner.save_confirm {
                            this.tuner.save_confirm = false;
                            this.save_v1_to_strategy(cx);
                        } else {
                            this.tuner.save_confirm = true;
                        }
                        cx.notify();
                    }))
                    .render(),
            );
        }
        v_flex()
            .w_full()
            .flex_none()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(header)
            .child(grid)
            .into_any_element()
    }

}

/// Формат числа для границ/чипов: крупные — с суффиксом k/M/B/T (обратно
/// понимается `parse_num`), прочие — до 4 знаков без хвостовых нулей.
pub(super) fn fmt_bound(v: f64) -> String {
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
pub(super) fn card(
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
