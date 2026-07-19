//! Сетка режима «По времени»: ТРИ строки-поля, каждое — одно «от-до».
//!   - «Недельно» (`WorkingWeekTime`): непрерывный спан по МИНУТЕ НЕДЕЛИ, шаг 1 мин,
//!     ввод/вывод `день.чч:мм` (день 1..7); напр. `1.23:44-6.22:22`.
//!   - «Сутки» (`WorkingTime`, режим времени суток): окно `чч:мм-чч:мм`.
//!   - «В часе» (`WorkingTime`, режим минут-в-часе): окно `N-M` (0..59).
//! «Сутки» и «В часе» — ДВА вида ОДНОГО поля WorkingTime → ВЗАИМОИСКЛЮЧАЮЩИЕ:
//! заполнил/подобрал одно — другое чистится (тумблера режима нет).
//!
//! Текущее значение полей стратегии просто ПОКАЗЫВАЕМ сырой строкой (парсинг
//! ненадёжен) — янтарным «в стратегии». v1/v2 — свежий ввод/автоподбор; Save пишет
//! непустые изменившиеся поля. Оболочка (тулбар + строка подбора) — ОБЩАЯ с фильтром.

use std::collections::HashMap;

use gpui::*;
use moon_ui::{MoonInput, MoonInputEvent, MoonInputState, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::state::TunerKind;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::{TimeWindow, Variant, format_week_span, format_working_time};

/// Вариантов помимо «Факта» (v1, v2) — как N_VAR тюнера фильтров.
pub(super) const TIME_N_VAR: usize = 2;
/// Полей: 0 = Недельно (WorkingWeekTime), 1 = Сутки, 2 = В часе (оба — WorkingTime).
const N_FIELD: usize = 3;
/// Минут в неделе (7 дней × 1440) — максимум минуты недели + 1.
pub(super) const WEEK_MIN: u16 = 7 * 1440;

/// Состояние тюнера «По времени» внутри `AnalyticsView`.
pub(in crate::analytics) struct TimeTunerState {
    /// `[вариант 0..TIME_N_VAR][поле 0..N_FIELD] = (от, до)` строками.
    pub(super) bounds: Vec<[(String, String); N_FIELD]>,
    /// СЫРЫЕ текущие значения полей стратегии `[WorkingWeekTime, WorkingTime]`.
    pub(super) current_raw: [String; 2],
    /// Текущий `IgnoreTime` стратегии (расписание игнорируется, если YES).
    pub(super) ignore_cur: bool,
    /// Текущий `IgnoreFilters` (ОБЩИЙ игнор — тоже гейтит время; должен быть NO).
    pub(super) ign_filters_cur: bool,
    /// Ручной тумблер `IgnoreTime` (None = следовать авто-логике).
    pub(super) ignore_staged: Option<bool>,
    /// Ленивый кэш инпутов (создаются в render, как в тюнере фильтров).
    pub(super) inputs: HashMap<String, Entity<MoonInputState>>,
    // Настройки общей оболочки — параллельно `TunerState` (у времени часть скрыта).
    pub(super) iters: String,
    pub(super) min_trades: String,
    pub(super) edges: usize,
    pub(super) round_results: bool,
    pub(super) sugg_busy: bool,
    /// Поколение автоподбора — устаревший результат отбрасывается.
    pub(super) sugg_seq: u64,
}

impl TimeTunerState {
    /// Свежий тюнер: все поля пусты.
    pub(in crate::analytics) fn load() -> Self {
        Self {
            bounds: vec![std::array::from_fn(|_| (String::new(), String::new())); TIME_N_VAR],
            current_raw: Default::default(),
            ignore_cur: false,
            ign_filters_cur: false,
            ignore_staged: None,
            inputs: HashMap::new(),
            iters: "20".to_string(),
            min_trades: String::new(),
            edges: 64,
            round_results: true,
            sugg_busy: false,
            sugg_seq: 0,
        }
    }

    /// Спан варианта `vi` по минуте недели (поле 0). Пусто → None; пропуск края →
    /// начало (0) / конец (WEEK_MIN-1) недели.
    pub(super) fn week_span(&self, vi: usize) -> Option<(u16, u16)> {
        let (from, to) = &self.bounds[vi][0];
        if from.trim().is_empty() && to.trim().is_empty() {
            return None;
        }
        Some((
            parse_week_ep(from, false).unwrap_or(0),
            parse_week_ep(to, true).unwrap_or(WEEK_MIN - 1),
        ))
    }

    /// Окно WorkingTime варианта `vi`: из строки «Сутки» (поле 1 → `Day`) ЛИБО
    /// «В часе» (поле 2 → `Hour`). Заполнены оба (не должно быть) → приоритет «Сутки».
    pub(super) fn tod(&self, vi: usize) -> Option<TimeWindow> {
        let (df, dt) = &self.bounds[vi][1];
        if !df.trim().is_empty() || !dt.trim().is_empty() {
            let w = (parse_time(df).unwrap_or(0), parse_time(dt).unwrap_or(1439));
            // Полные сутки (00:00–23:59) = без ограничения (симметрично полной неделе).
            return (w != (0, 1439)).then_some(TimeWindow::Day(w.0, w.1));
        }
        let (hf, ht) = &self.bounds[vi][2];
        if !hf.trim().is_empty() || !ht.trim().is_empty() {
            let w = (parse_moh(hf).unwrap_or(0), parse_moh(ht).unwrap_or(59));
            // Весь час (0–59) = без ограничения.
            return (w != (0, 59)).then_some(TimeWindow::Hour(w.0, w.1));
        }
        None
    }

    /// Какое поле WorkingTime задано в v1 (для затемнения неиспользуемой строки/
    /// полосы): `Some(1)`=Сутки, `Some(2)`=В часе, `None`=обе пусты.
    pub(super) fn active_wt(&self) -> Option<usize> {
        let d = &self.bounds[0][1];
        if !d.0.trim().is_empty() || !d.1.trim().is_empty() {
            return Some(1);
        }
        let h = &self.bounds[0][2];
        if !h.0.trim().is_empty() || !h.1.trim().is_empty() {
            return Some(2);
        }
        None
    }

    /// Строка поля стратегии из варианта `vi`: `which` 0 = WorkingWeekTime (спан
    /// недели; полная неделя = «без ограничения» → ""), 1 = WorkingTime. Пусто → "".
    pub(super) fn field_value(&self, vi: usize, which: usize) -> String {
        if which == 0 {
            self.week_span(vi)
                .filter(|&s| s != (0, WEEK_MIN - 1))
                .map(format_week_span)
                .unwrap_or_default()
        } else {
            self.tod(vi).map(format_working_time).unwrap_or_default()
        }
    }

    /// Варианты для KPI: `[Факт, v1, v2]`. Каждый несёт week_span И tod.
    pub(super) fn variants(&self) -> Vec<Variant> {
        let mut out = vec![Variant::default()];
        for vi in 0..self.bounds.len() {
            out.push(Variant {
                // Полная неделя == без ограничения (== «Факт»).
                week_span: self.week_span(vi).filter(|&s| s != (0, WEEK_MIN - 1)),
                tod: self.tod(vi),
                ..Default::default()
            });
        }
        out
    }

    /// Меняем ли расписание: хоть одно поле v1 непусто И отличается от текущего.
    pub(super) fn has_schedule_change(&self) -> bool {
        [0usize, 1].iter().any(|&which| {
            let v = self.field_value(0, which);
            !v.is_empty() && v != self.current_raw[which].trim()
        })
    }

    /// Эффективное значение `IgnoreTime` для показа/записи: ручной тумблер, иначе
    /// авто — при заданном расписании NO (иначе MoonBot игнорит окна), без него — текущее.
    pub(super) fn ignore_effective(&self) -> bool {
        self.ignore_staged.unwrap_or(if self.has_schedule_change() {
            false
        } else {
            self.ignore_cur
        })
    }

    /// Есть ли что записывать (Save горит): расписание, `IgnoreTime` или общий игнор.
    pub(super) fn is_dirty(&self) -> bool {
        !self.changes().is_empty()
    }

    /// Правки в стратегию: изменившиеся `WorkingWeekTime`/`WorkingTime` + флаги игнора,
    /// которые надо погасить, чтобы расписание применилось (`IgnoreTime`, а также
    /// ОБЩИЙ `IgnoreFilters` — при записи расписания, если он сейчас YES). Пишем только
    /// то, что реально меняется.
    pub(super) fn changes(&self) -> Vec<(String, String)> {
        const NAMES: [&str; 2] = ["WorkingWeekTime", "WorkingTime"];
        let yesno = |b: bool| if b { "YES" } else { "NO" }.to_string();
        let mut out: Vec<(String, String)> = (0..2)
            .filter_map(|which| {
                let v = self.field_value(0, which);
                (!v.is_empty() && v != self.current_raw[which].trim())
                    .then(|| (NAMES[which].to_string(), v))
            })
            .collect();
        let schedule = !out.is_empty();
        // IgnoreTime — тумблер/авто; пишем, если отличается от текущего.
        let ig_time = self.ignore_effective();
        if ig_time != self.ignore_cur {
            out.push(("IgnoreTime".to_string(), yesno(ig_time)));
        }
        // Общий IgnoreFilters тоже гейтит время — при записи расписания гасим, если YES.
        if schedule && self.ign_filters_cur {
            out.push(("IgnoreFilters".to_string(), "NO".to_string()));
        }
        out
    }

    /// Сбросить сетку при смене стратегии (v1/v2 + инпуты + текущее + тумблер игнора),
    /// отбросить автоподбор в полёте.
    pub(super) fn reset_grid(&mut self) {
        self.bounds = vec![std::array::from_fn(|_| (String::new(), String::new())); TIME_N_VAR];
        self.current_raw = Default::default();
        self.ignore_staged = None;
        self.inputs.clear();
        self.sugg_seq = self.sugg_seq.wrapping_add(1);
        self.sugg_busy = false;
    }

    /// Отбросить автоподбор В ПОЛЁТЕ при смене ЗАПРОСА (период/фильтр) — как
    /// `TunerState::invalidate` у фильтра: бампим поколение и гасим busy, НЕ трогая
    /// v1. Иначе стейл-результат от старого запроса дописался бы в v1 (и стал бы Saveable).
    pub(in crate::analytics) fn invalidate_suggest(&mut self) {
        self.sugg_seq = self.sugg_seq.wrapping_add(1);
        self.sugg_busy = false;
    }
}

/// Минуты дня → "чч:мм".
pub(super) fn fmt_min(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Край спана недели `(от, до)` строкой инпута «день.чч:мм» (или «день» на границе
/// суток): начало недели/суток для «от» и конец — коротко без времени.
pub(super) fn fmt_week_ep(wm: u16, is_to: bool) -> String {
    let (day, tod) = (wm / 1440 % 7 + 1, wm % 1440);
    if (!is_to && tod == 0) || (is_to && tod == 1439) {
        day.to_string()
    } else {
        format!("{day}.{}", fmt_min(tod))
    }
}

/// Разобрать край спана недели из поля: «день» или «день.чч:мм» (день 1..7) →
/// минута недели 0..WEEK_MIN-1. Без времени: «от» → начало дня, «до» → конец дня.
/// Пусто/мусор/вне 1..7 → None.
fn parse_week_ep(s: &str, is_to: bool) -> Option<u16> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (day_s, tod) = match s.split_once('.') {
        Some((d, t)) => (d, parse_time(t)?),
        None => (s, if is_to { 1439 } else { 0 }),
    };
    let day: u16 = day_s.trim().parse().ok()?;
    if !(1..=7).contains(&day) {
        return None;
    }
    Some((day - 1) * 1440 + tod)
}

/// Минута в часе 0..59 из поля. Пусто/мусор → None; больше 59 зажимаем к 59.
pub(super) fn parse_moh(s: &str) -> Option<u8> {
    s.trim().parse::<u8>().ok().map(|v| v.min(59))
}

/// Время суток из поля: "чч:мм" или минуты дня; пусто/мусор = None.
pub(super) fn parse_time(s: &str) -> Option<u16> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((h, m)) = s.split_once(':') {
        let h: u16 = h.trim().parse().ok()?;
        let m: u16 = m.trim().parse().ok()?;
        if h > 23 || m > 59 {
            return None;
        }
        Some(h * 60 + m)
    } else {
        s.parse::<u16>().ok().map(|v| v.min(1439))
    }
}

impl AnalyticsView {
    /// Коммит границы поля (Blur/Enter): состояние → взаимоисключение WT → KPI.
    fn commit_time(
        &mut self,
        vi: usize,
        field: usize,
        is_to: bool,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let slot = &mut self.time_tuner.bounds[vi][field];
        let cur = if is_to { &mut slot.1 } else { &mut slot.0 };
        if *cur == value {
            return;
        }
        *cur = value;
        // «Сутки» и «В часе» — одно поле WorkingTime: заполнил одно → чистим другое.
        if field == 1 {
            self.clear_field(vi, 2);
        } else if field == 2 {
            self.clear_field(vi, 1);
        }
        self.reload_time(cx);
        cx.notify();
    }

    /// Очистить обе границы поля `field` варианта `vi` (без пересчёта — служебное).
    pub(super) fn clear_field(&mut self, vi: usize, field: usize) {
        self.time_tuner.bounds[vi][field] = (String::new(), String::new());
        self.time_tuner.inputs.remove(&format!("tt{vi}f{field}a"));
        self.time_tuner.inputs.remove(&format!("tt{vi}f{field}b"));
    }

    /// Очистить поле `field` варианта `vi` (крестик ячейки) + пересчёт.
    fn time_clear_cell(&mut self, vi: usize, field: usize, cx: &mut Context<Self>) {
        self.clear_field(vi, field);
        self.reload_time(cx);
        cx.notify();
    }

    /// Копировать поле `field` v1 → v2 (стрелка между вариантами).
    fn time_copy_row(&mut self, field: usize, cx: &mut Context<Self>) {
        let v = self.time_tuner.bounds[0][field].clone();
        if self.time_tuner.bounds[1][field] != v {
            let non_empty = !v.0.trim().is_empty() || !v.1.trim().is_empty();
            self.time_tuner.bounds[1][field] = v;
            self.time_tuner.inputs.remove(&format!("tt1f{field}a"));
            self.time_tuner.inputs.remove(&format!("tt1f{field}b"));
            // WT-строки взаимоисключаемы и в v2: копия непустой одной чистит другую.
            if non_empty && field == 1 {
                self.clear_field(1, 2);
            }
            if non_empty && field == 2 {
                self.clear_field(1, 1);
            }
            self.reload_time(cx);
        }
        cx.notify();
    }

    /// Автоподбор в v1 (фон): окно недели (WorkingWeekTime) + окно WorkingTime,
    /// причём режим (Сутки vs В часе) подбор выбирает САМ — заполняет ОДНУ из строк
    /// «Сутки»/«В часе», другую чистит. Скоуп — `tuner_query`. Нет улучшения → пусто.
    pub(super) fn time_suggest(&mut self, cx: &mut Context<Self>) {
        if self.time_tuner.sugg_busy {
            return;
        }
        self.time_tuner.sugg_seq = self.time_tuner.sugg_seq.wrapping_add(1);
        let req = self.time_tuner.sugg_seq;
        self.time_tuner.sugg_busy = true;
        let q = self.tuner_query();
        let min_n = self
            .time_tuner
            .min_trades
            .trim()
            .parse::<i64>()
            .unwrap_or(5)
            .max(1);
        let edges = 512usize;
        // Время: точные минуты. round_bound (3-значное окр.) для минут суток/недели
        // пересекал бы границы суток/часа (напр. 23:59→24:00) — потому НЕ округляем.
        let round = false;
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let sugg = executor
                .spawn(async move { moon_core::db::tuner::suggest_time(&q, min_n, edges, round) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.time_tuner.sugg_seq != req {
                        return; // устаревший подбор (стратегия/скоуп сменились)
                    }
                    this.time_tuner.sugg_busy = false;
                    match sugg {
                        Ok(s) => {
                            // Поле 0: окно недели в «день.чч:мм».
                            let (w0, w1) = s
                                .week_span
                                .map(|(f, t)| (fmt_week_ep(f, false), fmt_week_ep(t, true)))
                                .unwrap_or_default();
                            this.set_v1_cell(0, w0, w1);
                            // WorkingTime: подбор сам выбрал режим → заполняем ЕГО строку,
                            // другую чистим (взаимоисключение).
                            match s.tod {
                                Some(TimeWindow::Day(f, t)) => {
                                    this.set_v1_cell(1, fmt_min(f), fmt_min(t));
                                    this.clear_field(0, 2);
                                }
                                Some(TimeWindow::Hour(f, t)) => {
                                    this.set_v1_cell(2, f.to_string(), t.to_string());
                                    this.clear_field(0, 1);
                                }
                                None => {
                                    this.clear_field(0, 1);
                                    this.clear_field(0, 2);
                                }
                            }
                            this.reload_time(cx);
                        }
                        Err(e) => log::warn!("аналитика: автоподбор времени не выполнен — {e}"),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Записать `(от, до)` в v1 поля `field` и сбросить его кэш-инпуты.
    pub(super) fn set_v1_cell(&mut self, field: usize, from: String, to: String) {
        self.time_tuner.bounds[0][field] = (from, to);
        self.time_tuner.inputs.remove(&format!("tt0f{field}a"));
        self.time_tuner.inputs.remove(&format!("tt0f{field}b"));
    }

    /// Очистить ВСЮ колонку варианта (✕ в шапке) — все поля от/до этого vi.
    fn time_clear_variant(&mut self, vi: usize, cx: &mut Context<Self>) {
        for field in 0..N_FIELD {
            self.clear_field(vi, field);
        }
        self.reload_time(cx);
        cx.notify();
    }

    /// Копировать ВСЮ колонку v1 → v2 (→ в шапке) — все поля.
    fn time_copy_all(&mut self, cx: &mut Context<Self>) {
        for field in 0..N_FIELD {
            self.time_tuner.bounds[1][field] = self.time_tuner.bounds[0][field].clone();
            self.time_tuner.inputs.remove(&format!("tt1f{field}a"));
            self.time_tuner.inputs.remove(&format!("tt1f{field}b"));
        }
        self.reload_time(cx);
        cx.notify();
    }

    /// Тумблер `IgnoreTime` (YES/NO). Клик переключает ручной стейдж; если вернулись
    /// к текущему значению — стейдж снимаем (авто-логика). Влияет на Save (не KPI).
    fn toggle_ignore_time(&mut self, cx: &mut Context<Self>) {
        let cur = self.time_tuner.ignore_cur;
        let next = !self.time_tuner.ignore_effective();
        self.time_tuner.ignore_staged = (next != cur).then_some(next);
        cx.notify();
    }

    /// Строка флагов игнора: тумблер `IgnoreTime` + наблюдаемый ОБЩИЙ `IgnoreFilters`
    /// (при заданном расписании YES → будет погашен в NO). Чтобы расписание работало,
    /// оба должны быть NO.
    fn time_ignore_row(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let ig_time = self.time_tuner.ignore_effective();
        let ig_filters = self.time_tuner.ign_filters_cur;
        // Стиль как подзаголовки «По фильтру»: table_head-фон, зелёный = работает
        // (ignore=NO), серый = игнорируется (ignore=YES). Клик по IgnoreTime переключает.
        let col = |ignored: bool| {
            if ignored {
                moon_alpha(p.text_muted, 0.7)
            } else {
                moon(p.green)
            }
        };
        let mut row = h_flex()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 3.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .bg(moon_alpha(p.table_head, 0.6))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.7))
            .text_size(design::t_caption(cx))
            .child(div().text_color(moon(p.text_soft)).child("IgnoreTime"))
            .child(
                div()
                    .id("tun-ign-IgnoreTime")
                    .cursor_pointer()
                    .text_color(col(ig_time))
                    .child(if ig_time { "ignore=YES" } else { "ignore=NO" })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_ignore_time(cx))),
            );
        // Общий IgnoreFilters — тоже гейтит время; наблюдаемый (при расписании YES → NO).
        if ig_filters {
            row = row
                .child(div().text_color(moon(p.text_soft)).child("IgnoreFilters"))
                .child(div().text_color(col(true)).child("ignore=YES"));
        }
        row.into_any_element()
    }

    /// Инпут границы поля с ленивым кэшем (паттерн `bound_input` тюнера).
    fn time_input(
        &mut self,
        vi: usize,
        field: usize,
        is_to: bool,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = format!("tt{vi}f{field}{}", if is_to { "b" } else { "a" });
        if let Some(state) = self.time_tuner.inputs.get(&id) {
            return state.clone();
        }
        let slot = &self.time_tuner.bounds[vi][field];
        let value = if is_to {
            slot.1.clone()
        } else {
            slot.0.clone()
        };
        let ph = placeholder.to_string();
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(value)
                .placeholder(ph)
        });
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                let value = state.read(cx).value().to_string();
                this.commit_time(vi, field, is_to, value, cx);
            }
        })
        .detach();
        self.time_tuner.inputs.insert(id, state.clone());
        state
    }

    /// Нижняя правая плашка «По времени»: ОБЩАЯ оболочка (тулбар + строка подбора)
    /// + 3 строки-поля. Шапка: Поле | в стратегии | v1 от|до|✕ | v2 от|→|до|✕.
    pub(super) fn time_grid(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let in_w = 66.0; // шире — вмещает «д.чч:мм» недельного поля
        let header = self.shell_toolbar(
            TunerKind::Time,
            t!("analytics.time.autopick_title").to_string(),
            p,
            cx,
        );
        let cfg_row = self.shell_config_row(TunerKind::Time, p, window, cx);
        let ignore_row = self.time_ignore_row(p, cx);

        let head_cell = |label: String| {
            div()
                .w(design::font_w_px(cx, in_w))
                .flex_none()
                .text_center()
                .child(label)
        };
        let head = h_flex()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 8.0))
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(
                div()
                    .w(design::font_w_px(cx, 60.0))
                    .flex_none()
                    .child(t!("analytics.time.col_field").to_string()),
            )
            .child(div().flex_1().child(t!("analytics.time.cur").to_string()))
            .child(head_cell(format!("v1 {}", t!("analytics.tuner.from"))))
            .child(head_cell(t!("analytics.tuner.to").to_string()))
            .child(
                div()
                    .id("tt-clr-col-0")
                    .w(design::ui_px(cx, 12.0))
                    .flex_none()
                    .cursor_pointer()
                    .text_color(moon(p.text_muted))
                    .hover(move |s| s.text_color(moon(p.orange)))
                    .child("✕")
                    .on_click(cx.listener(|this, _, _, cx| this.time_clear_variant(0, cx))),
            )
            .child(head_cell(format!("v2 {}", t!("analytics.tuner.from"))))
            .child(
                div()
                    .id("tt-cp-col")
                    .w(design::ui_px(cx, 12.0))
                    .flex_none()
                    .cursor_pointer()
                    .text_color(moon(p.text_muted))
                    .hover(move |s| s.text_color(moon(p.amber)))
                    .child("→")
                    .on_click(cx.listener(|this, _, _, cx| this.time_copy_all(cx))),
            )
            .child(head_cell(t!("analytics.tuner.to").to_string()))
            .child(
                div()
                    .id("tt-clr-col-1")
                    .w(design::ui_px(cx, 12.0))
                    .flex_none()
                    .cursor_pointer()
                    .text_color(moon(p.text_muted))
                    .hover(move |s| s.text_color(moon(p.orange)))
                    .child("✕")
                    .on_click(cx.listener(|this, _, _, cx| this.time_clear_variant(1, cx))),
            );

        let mut grid = v_flex().w_full().child(head);
        for field in 0..N_FIELD {
            grid = grid.child(self.time_row(field, in_w, p, window, cx));
        }
        // Три heatmap-ползунка под строками (Недельно / Сутки / В часе).
        let sliders = self.time_sliders(p, cx);

        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(header)
            .child(cfg_row)
            .child(ignore_row)
            .child(grid)
            // Распорка: ползунки прижаты к НИЗУ контейнера (тянется к нижней карте).
            .child(div().flex_1())
            .child(sliders)
            .into_any_element()
    }

    /// Одна строка-поле (0 Недельно · 1 Сутки · 2 В часе). Плейсхолдер по полю.
    fn time_row(
        &mut self,
        field: usize,
        in_w: f32,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (label_key, ph, cur_idx) = match field {
            0 => ("analytics.time.field_week", "д.чч:мм", 0usize),
            1 => ("analytics.time.field_day", "чч:мм", 1usize),
            _ => ("analytics.time.field_hour", "0..59", 1usize),
        };
        let cur = self.time_tuner.current_raw[cur_idx].trim().to_string();
        // Неиспользуемую из пары Сутки/Час затемняем (одно поле WorkingTime).
        let dim = matches!(
            (field, self.time_tuner.active_wt()),
            (1, Some(2)) | (2, Some(1))
        );

        let mut row = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 3.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.5))
            .opacity(if dim { 0.45 } else { 1.0 })
            .child(
                div()
                    .w(design::font_w_px(cx, 60.0))
                    .flex_none()
                    .text_color(moon(p.text))
                    .child(t!(label_key).to_string()),
            )
            // «в стратегии» — сырое текущее значение поля (янтарным).
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(if cur.is_empty() {
                        p.text_muted
                    } else {
                        p.amber
                    }))
                    .child(if cur.is_empty() {
                        "—".to_string()
                    } else {
                        cur
                    }),
            );
        for vi in [0usize, 1usize] {
            for is_to in [false, true] {
                if vi == 1 && is_to {
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("tt-cp-{field}")))
                            .w(design::ui_px(cx, 12.0))
                            .flex_none()
                            .cursor_pointer()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .hover(move |s| s.text_color(moon(p.amber)))
                            .child("→")
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.time_copy_row(field, cx)),
                            ),
                    );
                }
                let input = self.time_input(vi, field, is_to, ph, window, cx);
                // Поле СО значением обводим акцентной рамкой — чтобы явно отличать от
                // пустого (где виден лишь плейсхолдер). Пустое → прозрачная рамка (без сдвига).
                let filled = {
                    let slot = &self.time_tuner.bounds[vi][field];
                    let v = if is_to { &slot.1 } else { &slot.0 };
                    !v.trim().is_empty()
                };
                row = row.child(
                    div()
                        .w(design::font_w_px(cx, in_w))
                        .flex_none()
                        .rounded(design::ui_px(cx, 4.0))
                        .border_1()
                        .border_color(if filled {
                            moon(p.accent)
                        } else {
                            moon_alpha(p.border, 0.0)
                        })
                        .child(
                            MoonInput::new(SharedString::from(format!(
                                "tt-in-{vi}-{field}-{is_to}"
                            )))
                            .state(&input)
                            .small(),
                        ),
                );
            }
            row = row.child(
                div()
                    .id(SharedString::from(format!("tt-clr-{vi}-{field}")))
                    .w(design::ui_px(cx, 12.0))
                    .flex_none()
                    .cursor_pointer()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .hover(move |s| s.text_color(moon(p.orange)))
                    .child("✕")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.time_clear_cell(vi, field, cx)),
                    ),
            );
        }
        row.into_any_element()
    }
}
