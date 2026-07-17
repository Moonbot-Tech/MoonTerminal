//! Окно «Аналитика» — анализаторы отчётов поверх реплики `orders_rep`
//! (см. план analytics-panel-plan: сводка → сравнения → heatmap → календарь).
//!
//! Отдельное singleton ОС-окно (паттерн «Скринер»): геометрия персистится в
//! `layout.analytics_window`. Вкладки — полоса MoonButton (как в Настройках);
//! пока функциональна «Сводка», остальные — заглушки следующих этапов.
//! Данные считает `moon_core::db::analytics` на background executor (полная
//! выборка периода из SQLite — не на UI-потоке), перезапрашиваются ТОЛЬКО
//! действием пользователя: открытие окна, смена периода/фильтра, повторный
//! клик активного пресета периода (ручное обновление).

mod charts;
mod strategies;
mod summary;
mod tuner;
mod tuner_actions;
mod tuner_state;
mod tuner_hist;

use std::collections::HashSet;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCalendar,
    MoonCalendarEvent, MoonCalendarState, MoonDate, MoonDropdown, MoonMenuSize, MoonPalette,
    MoonPopover, MoonPopoverPlacement, MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use crate::design::{moon, moon_alpha};
use crate::{Backend, design};
use moon_core::db::SideFilter;
use moon_core::db::analytics::{Query, StrategyDetail, Summary};

const ANALYTICS_HEADER_H: f32 = 32.0;

/// Задержка показа оверлея занятости: быстрые пересчёты не мигают затемнением.
const BUSY_OVERLAY_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// Вкладки окна. Заглушки (Монеты/Heatmap/Календарь/Плечо) убраны — вернутся
/// по мере реализации этапов плана.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Summary,
    Strategies,
}

impl Tab {
    const ALL: [Tab; 2] = [Tab::Summary, Tab::Strategies];
    fn id(self) -> &'static str {
        match self {
            Tab::Summary => "an-summary",
            Tab::Strategies => "an-strategies",
        }
    }
    fn title(self) -> String {
        match self {
            Tab::Summary => t!("analytics.tab.summary"),
            Tab::Strategies => t!("analytics.tab.strategies"),
        }
        .to_string()
    }
}

/// Пресеты периода (UTC-сутки, как в панели «Отчёт»).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Period {
    Today,
    Yesterday,
    Week,
    /// Текущий календарный месяц (с 1-го числа, UTC).
    CurMonth,
    /// Скользящие 30 дней.
    Month,
    Year,
    All,
    /// Произвольный диапазон из полей «с»/«по»: `[from, to)` unix-секунды UTC;
    /// from = -1 — «с» не задано (вся история до «по»).
    Custom(i64, i64),
}

impl Period {
    const ALL: [Period; 7] = [
        Period::Today,
        Period::Yesterday,
        Period::Week,
        Period::CurMonth,
        Period::Month,
        Period::Year,
        Period::All,
    ];
    /// Пресет по его id (персист выбора в layout); None — незнакомый id.
    fn from_id(id: &str) -> Option<Period> {
        if let Some(rest) = id.strip_prefix("p-custom:") {
            let (f, t) = rest.split_once(':')?;
            return Some(Period::Custom(f.parse().ok()?, t.parse().ok()?));
        }
        Period::ALL.into_iter().find(|p| p.id() == id)
    }
    fn id(self) -> &'static str {
        match self {
            Period::Today => "p-today",
            Period::Yesterday => "p-yesterday",
            Period::Week => "p-week",
            Period::CurMonth => "p-cur-month",
            Period::Month => "p-month",
            Period::Year => "p-year",
            Period::All => "p-all",
            Period::Custom(..) => "p-custom",
        }
    }
    /// Строка персиста в layout: у Custom границы кодируются в id.
    fn persist_id(self) -> String {
        match self {
            Period::Custom(f, t) => format!("p-custom:{f}:{t}"),
            p => p.id().to_string(),
        }
    }
    fn title(self) -> String {
        match self {
            Period::Today => t!("analytics.period.today"),
            Period::Yesterday => t!("analytics.period.yesterday"),
            Period::Week => t!("analytics.period.week"),
            Period::CurMonth => t!("analytics.period.cur_month"),
            Period::Month => t!("analytics.period.month"),
            Period::Year => t!("analytics.period.year"),
            Period::All => t!("analytics.period.all"),
            Period::Custom(f, t) => {
                let a = if f < 0 { "—".to_string() } else { fmt_day(f) };
                return format!("{a} – {}", fmt_day((t - 86_400).max(f.max(0))));
            }
        }
        .to_string()
    }
    /// Границы `[from, to)` в unix-секундах UTC; from = -1 → вся история.
    fn range(self) -> (i64, i64) {
        let now = moon_core::util::now_unix_ms_i64() / 1000;
        let day0 = now.div_euclid(86_400) * 86_400;
        let tomorrow = day0 + 86_400;
        match self {
            Period::Today => (day0, tomorrow),
            Period::Yesterday => (day0 - 86_400, day0),
            Period::Week => (tomorrow - 7 * 86_400, tomorrow),
            Period::CurMonth => {
                // 1-е число текущего месяца: "YYYY-MM" из форматтера БД + "-01".
                let ym = moon_core::db::fmt_unix(now);
                let start = moon_core::db::parse_ymd(&format!("{}-01", &ym[..7.min(ym.len())]))
                    .unwrap_or(day0);
                (start, tomorrow)
            }
            Period::Month => (tomorrow - 30 * 86_400, tomorrow),
            Period::Year => (tomorrow - 365 * 86_400, tomorrow),
            Period::All => (-1, tomorrow),
            Period::Custom(f, t) => (f, t),
        }
    }
}

/// unix-секунды → дата UTC (для календарей «с»/«по»).
fn day_of_secs(secs: i64) -> Option<chrono::NaiveDate> {
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.date_naive())
}

/// Дата UTC → unix-секунды полуночи этих суток.
fn secs_of_day(d: chrono::NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc().timestamp()).unwrap_or(0)
}

/// «дд.мм.гг» для подписей полей диапазона.
fn fmt_day(secs: i64) -> String {
    day_of_secs(secs).map(|d| d.format("%d.%m.%y").to_string()).unwrap_or_default()
}

/// Состояние окна «Аналитика».
pub struct AnalyticsView {
    backend: Entity<Backend>,
    tab: Tab,
    period: Period,
    /// Ядра из реплики (для комбобокса) + мультивыбор (пусто = все) — те же
    /// контролы, что в «Ордерах»/«Отчёте».
    cores: Vec<(u64, String)>,
    sel_cores: HashSet<u64>,
    side: SideFilter,
    /// None — все, Some(false) — реальные, Some(true) — эмуляторные.
    emu: Option<bool>,
    /// Данные сводки (фоновый расчёт); None — ещё не загружены.
    pub(super) data: Option<Arc<Summary>>,
    inflight: bool,
    /// Счётчик фоновых пересчётов (сводка/тюнер/гистограмма/подбор): >0 —
    /// блокирующий оверлей «Загрузка…» поверх окна. Длинные сканы большой БД
    /// иначе никак не видны, а клики по фильтрам/стратегиям копились в очередь.
    busy_ops: usize,
    /// Начало текущей серии пересчётов: оверлей показываем только спустя
    /// BUSY_OVERLAY_DELAY — быстрые пересчёты не мигают затемнением.
    busy_since: Option<std::time::Instant>,
    /// Номер запроса — устаревшие результаты отбрасываются.
    seq: u64,
    /// Сводка: верхний левый чарт в режиме «по ядрам» (галка, деф. ВКЛ).
    pub(super) sum_by_core: bool,
    /// Ховер-ведро левого чарта по ядрам (индекс в `days`) — попап значений.
    pub(super) hover_core_bucket: Option<usize>,
    /// Ховер-ведро правого чарта «Дневная прибыль» — свой попап (иначе один
    /// стейт рисовал бы попапы на обоих чартах разом).
    pub(super) hover_daily_bucket: Option<usize>,
    /// Вкладка «Стратегии»: выбранная группа `(strategyid текстом, имя)`
    /// + её детализация.
    pub(super) sel_strategy: Option<(String, String)>,
    pub(super) detail: Option<Arc<StrategyDetail>>,
    detail_seq: u64,
    /// Режим вкладки «Стратегии» (Обзор / Фильтры / Монеты). Приватность —
    /// модульная: субмодули вкладок видят поля родителя без pub(super).
    strat_mode: strategies::StratMode,
    /// Тюнер порогов (режим «Фильтры») — состояние в своём модуле.
    tuner: tuner::TunerState,
    /// Календари произвольного диапазона «с»/«по» (moonui MoonCalendar в
    /// попапах); выбор даты переключает период в Period::Custom.
    cal_from: Entity<MoonCalendarState>,
    cal_to: Entity<MoonCalendarState>,
    cal_from_open: bool,
    cal_to_open: bool,
    _cal_subs: Vec<Subscription>,
    focus: FocusHandle,
}

impl AnalyticsView {
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Геометрия окна — в layout (как Скринер/Стратегии).
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
                return;
            };
            this.backend.update(cx, |b, _| {
                if b.layout.analytics_window.map(|g| (g.x, g.y, g.w, g.h)) != Some((x, y, w, h)) {
                    b.layout.analytics_window =
                        Some(moon_core::config::layout::GeomRect { x, y, w, h });
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        // Автоперечитки по новым отчётам НЕТ намеренно: пересчёт (полные сканы
        // периода + группировки) запускается только действием пользователя —
        // открытие окна, смена вкладки-периода-фильтра, повторный клик пресета.

        // Период: прошлый выбор из layout, дефолт — текущий календарный месяц.
        let saved_period = backend
            .read(cx)
            .layout
            .analytics_period
            .as_deref()
            .and_then(Period::from_id);

        // Календари «с»/«по»: выбор дня закрывает попап и переключает период.
        let cal_from = cx.new(|cx| MoonCalendarState::new(window, cx));
        let cal_to = cx.new(|cx| MoonCalendarState::new(window, cx));
        if let Some(Period::Custom(f, t)) = saved_period {
            if let Some(d) = (f >= 0).then(|| day_of_secs(f)).flatten() {
                cal_from.update(cx, |s, cx| s.set_date(d, window, cx));
            }
            if let Some(d) = day_of_secs(t - 86_400) {
                cal_to.update(cx, |s, cx| s.set_date(d, window, cx));
            }
        }
        let cal_subs = vec![
            cx.subscribe_in(&cal_from, window, |this, _, ev, window, cx| {
                let MoonCalendarEvent::Selected(_) = ev;
                this.cal_from_open = false;
                this.apply_custom_range(window, cx);
            }),
            cx.subscribe_in(&cal_to, window, |this, _, ev, window, cx| {
                let MoonCalendarEvent::Selected(_) = ev;
                this.cal_to_open = false;
                this.apply_custom_range(window, cx);
            }),
        ];
        let mut this = Self {
            backend,
            tab: Tab::Summary,
            period: saved_period.unwrap_or(Period::CurMonth),
            cores: Vec::new(),
            sel_cores: HashSet::new(),
            side: SideFilter::All,
            // Дефолт «Реальные» — как в Отчёте (эмуляторные шумят статистику).
            emu: Some(false),
            data: None,
            inflight: false,
            busy_ops: 0,
            busy_since: None,
            seq: 0,
            sum_by_core: true,
            hover_core_bucket: None,
            hover_daily_bucket: None,
            sel_strategy: None,
            detail: None,
            detail_seq: 0,
            strat_mode: strategies::StratMode::Filters,
            tuner: tuner::TunerState::load(),
            cal_from,
            cal_to,
            cal_from_open: false,
            cal_to_open: false,
            _cal_subs: cal_subs,
            focus: cx.focus_handle(),
        };
        this.reload(cx);
        this
    }

    /// Текущие фильтры одной структурой (общая для всех вкладок).
    fn query(&self) -> Query {
        let (from, to) = self.period.range();
        Query {
            from,
            to,
            cores: self.cores_selected(),
            side: self.side,
            emulator: self.emu,
            strategy: None,
        }
    }

    /// Выбранные ядра для запроса: пусто или все = без фильтра.
    fn cores_selected(&self) -> Vec<u64> {
        if self.sel_cores.is_empty() || self.sel_cores.len() == self.cores.len() {
            Vec::new()
        } else {
            self.sel_cores.iter().copied().collect()
        }
    }

    /// Старт/финиш фоновой операции: каждый spawn обязан декрементить ровно
    /// один раз (ДО seq-проверки — устаревшие завершения тоже считаются).
    pub(super) fn op_started(&mut self) {
        self.busy_ops += 1;
        if self.busy_since.is_none() {
            self.busy_since = Some(std::time::Instant::now());
        }
    }
    pub(super) fn op_finished(&mut self, cx: &mut Context<Self>) {
        self.busy_ops = self.busy_ops.saturating_sub(1);
        if self.busy_ops == 0 {
            self.busy_since = None;
        }
        cx.notify();
    }

    /// Показывать ли оверлей занятости; если серия ещё моложе задержки —
    /// взводит таймер на перерисовку в момент её истечения.
    fn busy_overlay_due(&self, cx: &mut Context<Self>) -> bool {
        let Some(since) = self.busy_since else {
            return false;
        };
        let waited = since.elapsed();
        if waited >= BUSY_OVERLAY_DELAY {
            return true;
        }
        let left = BUSY_OVERLAY_DELAY - waited;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(left).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |_, cx| cx.notify());
            });
        })
        .detach();
        false
    }

    /// Фоновый расчёт сводки за текущий период/фильтры.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.inflight = true;
        self.op_started();
        self.seq = self.seq.wrapping_add(1);
        let req = self.seq;
        // Тюнер зависит от тех же фильтров: сбрасываем; активному режиму —
        // пересчёт сразу, иначе — при следующем входе в режим «Фильтры».
        self.tuner.invalidate();
        if self.tab == Tab::Strategies && self.strat_mode == strategies::StratMode::Filters {
            self.reload_tuner(cx);
            self.reload_hist(cx);
        }
        let q = self.query();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let data = executor
                .spawn(async move { moon_core::db::analytics::summary(&q) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.seq != req {
                        return; // период/фильтры уже сменили
                    }
                    this.inflight = false;
                    if let Some(d) = &data {
                        this.cores = d.cores.clone();
                    }
                    this.data = data.map(Arc::new);
                    // Детализация стратегии зависит от фильтров — перечитать.
                    if this.sel_strategy.is_some() {
                        this.reload_detail(cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Фоновая детализация выбранной стратегии (вкладка «Стратегии»).
    pub(super) fn reload_detail(&mut self, cx: &mut Context<Self>) {
        let Some((key, _)) = self.sel_strategy.clone() else {
            self.detail = None;
            return;
        };
        let id: i64 = key.parse().unwrap_or(0);
        self.detail_seq = self.detail_seq.wrapping_add(1);
        let req = self.detail_seq;
        let q = self.query();
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
                    this.detail = detail.map(Arc::new);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn set_period(&mut self, p: Period, window: &mut Window, cx: &mut Context<Self>) {
        // Повторный клик по активному пресету = ручное обновление данных
        // (автоперечитки по новым отчётам нет).
        self.period = p;
        // Пресет побеждает произвольный диапазон: поля «с»/«по» очищаются.
        if !matches!(p, Period::Custom(..)) {
            for cal in [&self.cal_from, &self.cal_to] {
                cal.update(cx, |s, cx| s.set_date(MoonDate::Single(None), window, cx));
            }
        }
        // Выбор персистится — окно (и следующий запуск) откроется с ним.
        self.backend.update(cx, |b, _| {
            let id = Some(p.persist_id());
            if b.layout.analytics_period != id {
                b.layout.analytics_period = id;
                b.layout_dirty = true;
            }
        });
        self.reload(cx);
        cx.notify();
    }

    /// Пересчёт периода из календарей «с»/«по». Пустое «с» — вся история;
    /// пустое «по» — до завтра; «по» раньше «с» — границы меняются местами.
    fn apply_custom_range(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut f = self.cal_from.read(cx).date().start();
        let mut t = self.cal_to.read(cx).date().start();
        if let (Some(a), Some(b)) = (f, t) {
            if b < a {
                (f, t) = (Some(b), Some(a));
                self.cal_from.update(cx, |s, cx| s.set_date(b, window, cx));
                self.cal_to.update(cx, |s, cx| s.set_date(a, window, cx));
            }
        }
        if f.is_none() && t.is_none() {
            return;
        }
        let now = moon_core::util::now_unix_ms_i64() / 1000;
        let tomorrow = now.div_euclid(86_400) * 86_400 + 86_400;
        let from = f.map(secs_of_day).unwrap_or(-1);
        // «по» — включительно: конец диапазона = следующая полночь.
        let to = t.map(|d| secs_of_day(d) + 86_400).unwrap_or(tomorrow);
        self.set_period(Period::Custom(from, to), window, cx);
    }

    /// Тогл ядра в мультивыборе; `None` — тумблер «Все» (пусто ↔ все).
    fn toggle_core(&mut self, core: Option<u64>, cx: &mut Context<Self>) {
        match core {
            None => {
                if self.sel_cores.is_empty() {
                    self.sel_cores = self.cores.iter().map(|(c, _)| *c).collect();
                } else {
                    self.sel_cores.clear();
                }
            }
            Some(c) => {
                if !self.sel_cores.remove(&c) {
                    self.sel_cores.insert(c);
                }
            }
        }
        self.reload(cx);
        cx.notify();
    }

    fn set_side(&mut self, side: SideFilter, cx: &mut Context<Self>) {
        if self.side != side {
            self.side = side;
            self.reload(cx);
            cx.notify();
        }
    }

    fn set_emu(&mut self, emu: Option<bool>, cx: &mut Context<Self>) {
        if self.emu != emu {
            self.emu = emu;
            self.reload(cx);
            cx.notify();
        }
    }

    /// Полоса вкладок (как таб-бар Настроек).
    fn tabs_bar(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let mut row = h_flex()
            .flex_none()
            .w_full()
            .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
            .gap(design::ui_px(cx, 6.0))
            .px(design::ui_px(cx, 8.0))
            .items_center()
            .bg(moon(p.shell_high))
            .border_b_1()
            .border_color(moon(p.border));
        for t in Tab::ALL {
            let on = self.tab == t;
            row = row.child(
                MoonButton::new(t.id())
                    .variant(if on {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .size(MoonButtonSize::Custom {
                        height: 24.0,
                        radius: design::R_BUTTON_BASE,
                        font_size: 10.5,
                        line_height: 13.0,
                        gap: 5.0,
                    })
                    .width(112.0)
                    .selected(on)
                    .label(t.title())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.tab != t {
                            this.tab = t;
                            if t == Tab::Strategies
                                && this.strat_mode == strategies::StratMode::Filters
                                && this.tuner.needs_reload()
                            {
                                this.reload_tuner(cx);
                                this.reload_hist(cx);
                            }
                            cx.notify();
                        }
                    }))
                    .render(),
            );
        }
        // Фильтры — прижаты вправо (те же контролы, что в Ордерах/Отчёте).
        row.child(div().flex_1())
            .child(self.core_combo(cx))
            .child(self.side_combo(cx))
            .child(self.kind_combo(cx))
    }

    /// Комбобокс ядер — мультивыбор (общий виджет, как в Ордерах/Отчёте).
    fn core_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        crate::controls::core_combo(
            cx,
            "an-core",
            &self.cores,
            &self.sel_cores,
            t!("report.all_cores").to_string(),
            |n| t!("report.cores_n", n = n).to_string(),
            180.0,
            move |uid, app| {
                view.update(app, |t, c| t.toggle_core(uid, c));
            },
        )
    }

    /// Комбобокс стороны (Все/Лонг/Шорт) — как в Отчёте.
    fn side_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.side {
            SideFilter::All => t!("report.filter.all").to_string(),
            SideFilter::Long => t!("report.side.long").to_string(),
            SideFilter::Short => t!("report.side.short").to_string(),
        };
        let view = cx.entity();
        let items = crate::panels::radio_items(
            [
                (SideFilter::All, "as-all".into(), t!("report.filter.all").to_string().into()),
                (SideFilter::Long, "as-long".into(), t!("report.side.long").to_string().into()),
                (SideFilter::Short, "as-short".into(), t!("report.side.short").to_string().into()),
            ],
            self.side,
            crate::panels::RadioMark::Highlight,
            move |app, side| {
                view.update(app, |t, c| t.set_side(side, c));
            },
        );
        MoonDropdown::new("an-side")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::font_w(cx, 69.0))
            .menu_width(design::font_w(cx, 120.0))
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Комбобокс типа ордеров (Все / Реальные / Эмуляторные) — как в Отчёте.
    fn kind_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.emu {
            None => t!("report.kind.all"),
            Some(false) => t!("report.kind.real"),
            Some(true) => t!("report.kind.emu"),
        };
        let view = cx.entity();
        let items = crate::panels::radio_items(
            [
                (None, "ak-all".into(), t!("report.kind.all").to_string().into()),
                (Some(false), "ak-real".into(), t!("report.kind.real").to_string().into()),
                (Some(true), "ak-emu".into(), t!("report.kind.emu").to_string().into()),
            ],
            self.emu,
            crate::panels::RadioMark::Check,
            move |app, k| {
                view.update(app, |t, c| t.set_emu(k, c));
            },
        );
        MoonDropdown::new("an-kind")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::font_w(cx, 102.0))
            .menu_width(design::font_w(cx, 138.0))
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Поле произвольной границы периода: кнопка «с/по дд.мм.гг» + попап с
    /// moonui-календарём (готовый MoonCalendar; MoonDatePicker не ужимается до
    /// высоты Micro-чипсов — Sizable не в фасаде moon_ui).
    fn date_field(&self, is_to: bool, _p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let (cal, open) = if is_to {
            (&self.cal_to, self.cal_to_open)
        } else {
            (&self.cal_from, self.cal_from_open)
        };
        let date_txt = cal
            .read(cx)
            .date()
            .format("%d.%m.%y")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".to_string());
        let lbl = if is_to {
            t!("analytics.period.to_lbl")
        } else {
            t!("analytics.period.from_lbl")
        };
        let set = cal.read(cx).date().is_some();
        let custom_on = matches!(self.period, Period::Custom(..)) && set;
        let view = cx.entity();
        MoonPopover::new(if is_to { "an-date-to" } else { "an-date-from" })
            .placement(MoonPopoverPlacement::BottomStart)
            .width(264.0 + design::POPOVER_PAD_W)
            .open(open)
            .on_open_change(move |o, _, app| {
                view.update(app, |t, cx| {
                    if is_to {
                        t.cal_to_open = o;
                    } else {
                        t.cal_from_open = o;
                    }
                    cx.notify();
                });
            })
            .trigger(
                MoonButton::new(if is_to { "an-date-to-btn" } else { "an-date-from-btn" })
                    .variant(if custom_on {
                        MoonButtonVariant::Amber
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .size(MoonButtonSize::Micro)
                    .selected(custom_on)
                    .label(format!("{lbl} {date_txt}"))
                    .render(),
            )
            .content(MoonCalendar::new(cal))
    }

    /// Полоса пресетов периода + счётчик закрытых сделок справа.
    fn period_bar(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let mut seg = h_flex().gap(design::ui_px(cx, 4.0)).items_center();
        for per in Period::ALL {
            let on = self.period == per;
            seg = seg.child(
                MoonButton::new(per.id())
                    .variant(if on {
                        MoonButtonVariant::Amber
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .size(MoonButtonSize::Micro)
                    .selected(on)
                    .label(per.title())
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.set_period(per, window, cx)),
                    )
                    .render(),
            );
        }
        // Произвольный диапазон: два поля-попапа «с»/«по» с MoonCalendar.
        seg = seg
            .child(self.date_field(false, p, cx))
            .child(self.date_field(true, p, cx));
        let counter = match (&self.data, self.inflight) {
            (_, true) => "…".to_string(),
            (Some(d), _) => t!("analytics.trades_count", n = d.cur.n).to_string(),
            (None, _) => String::new(),
        };
        h_flex()
            .flex_none()
            .w_full()
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .child(seg)
            .child(div().flex_1())
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(moon(p.text_muted))
                    .child(counter),
            )
    }
}

impl EventEmitter<()> for AnalyticsView {}
impl Focusable for AnalyticsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AnalyticsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let chrome_width = match window.window_bounds() {
            WindowBounds::Windowed(b)
            | WindowBounds::Maximized(b)
            | WindowBounds::Fullscreen(b) => f32::from(b.size.width),
        };
        let body = match self.tab {
            Tab::Summary => self.summary_tab(p, cx),
            Tab::Strategies => self.strategies_tab(p, window, cx),
        };
        // Обе вкладки делят высоту сами (нижние плашки прибиты к низу окна,
        // содержимое скроллится внутри) — внешнего скролла нет.
        let body_scrolls = false;
        let busy_overlay = self.busy_overlay_due(cx);
        v_flex()
            .size_full()
            .relative()
            .bg(moon(p.shell))
            .text_color(moon(p.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .track_focus(&self.focus)
            .child(analytics_header(p, cx))
            .child(self.tabs_bar(p, cx))
            .child(self.period_bar(p, cx))
            .child(
                div()
                    .id("analytics-body")
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .when(body_scrolls, |el| el.overflow_y_scroll())
                    .child(body),
            )
            // Фоновый пересчёт дольше задержки: приглушаем окно и глушим
            // клики (occlude) — иначе долгие сканы невидимы, а клики копятся.
            .when(busy_overlay, |el| {
                el.child(
                    div()
                        .id("an-busy-overlay")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(moon_alpha(p.shell, 0.45))
                        .child(
                            h_flex()
                                .px(design::ui_px(cx, 14.0))
                                .py(design::ui_px(cx, 7.0))
                                .rounded(design::ui_px(cx, 6.0))
                                .bg(moon(p.panel_high))
                                .border_1()
                                .border_color(moon(p.border))
                                .text_size(design::t_body(cx))
                                .text_color(moon(p.text_soft))
                                .child(t!("analytics.loading").to_string()),
                        ),
                )
            })
            .child(
                MoonWindowFrame::tool("analytics-window-frame-hit", chrome_width)
                    .header_height(ANALYTICS_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

fn analytics_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("analytics-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, ANALYTICS_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(p.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("analytics-titlebar-title", 0.0)
                .title_cluster(t!("analytics.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("analytics-window-frame-visual", 0.0)
                    .header_height(ANALYTICS_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Открыть окно «Аналитика» (tool-окно, singleton). Дедуп/фокус — в `Backend`.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).analytics_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let saved = backend.read(cx).layout.analytics_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(120.0), px(90.0)),
            size: size(px(1240.0), px(800.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    let display_id = crate::windowing::saved_or_owner_display_id(
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::windowing::tool_window_options(
        t!("analytics.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(860.0), px(520.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| AnalyticsView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.analytics_window = Some(handle));
        crate::windowing::activate_new_window(handle.into(), cx);
    }
}
