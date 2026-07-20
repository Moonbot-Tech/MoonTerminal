//! Окно «Аналитика» — анализаторы отчётов поверх реплики `orders_rep`
//! (см. план analytics-panel-plan: сводка → сравнения → heatmap → календарь).
//!
//! Отдельное singleton ОС-окно (паттерн «Скринер»): геометрия персистится в
//! `layout.analytics_window`. Вкладки — полоса MoonButton (как в Настройках):
//! «Сводка», «Стратегии», «Календарь» (тепловые карты). Плечо/Монеты — заглушки
//! следующих этапов.
//! Данные считает `moon_core::db::analytics` на background executor (полная
//! выборка периода из SQLite — не на UI-потоке), перезапрашиваются ТОЛЬКО
//! действием пользователя: открытие окна, смена периода/фильтра, повторный
//! клик активного пресета периода (ручное обновление).

mod calendar;
/// Period presets, window tabs and date helpers — the time axis shared by every page.
mod period;
mod summary;
/// The window's top chrome: tabs, filter combos, date fields, period bar.
mod toolbar;
/// Страница «Тюнинг стратегий» целиком (список + тюнеры «По фильтру»/«По времени»
/// + общая оболочка). Раньше плоский набор `strategies`/`tuner*`/`strat_time`/
/// `time_tuner` в корне — теперь папка `tuner/`.
mod tuner;

// Pages reach these through the familiar `super::…`, unaware of the `period` module.
pub(in crate::analytics) use period::{Period, Tab, day_of_secs, fmt_day, secs_of_day};

use std::collections::HashSet;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAlert, MoonBackgroundPolicy, MoonCalendarEvent, MoonCalendarState, MoonDate,
    MoonInputState, MoonPalette, MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use crate::design::{moon, moon_alpha};
use crate::{Backend, design};
use moon_core::db::SideFilter;
use moon_core::db::analytics::{DayCell, Query, StrategyDetail, Summary};

use crate::load_state::{LoadState, Note, note_el};

const ANALYTICS_HEADER_H: f32 = 32.0;

/// Задержка показа оверлея занятости: быстрые пересчёты не мигают затемнением.
const BUSY_OVERLAY_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// Состояние окна «Аналитика».
pub struct AnalyticsView {
    backend: Entity<Backend>,
    tab: Tab,
    /// Период вкладки «Сводка» (пресеты/«с»–«по»).
    period: Period,
    /// Период вкладки «Тюнинг стратегий» — НЕЗАВИСИМЫЙ от «Сводки»: у каждой
    /// вкладки своё окно времени, период-бар редактирует активную (`active_period`).
    strat_period: Period,
    /// Период, которым сейчас посчитан `data` (сводка/список стратегий). При
    /// входе на вкладку с другим окном времени — перечитываем.
    data_period: Period,
    /// Ядра из реплики (для комбобокса) + мультивыбор (пусто = все) — те же
    /// контролы, что в «Ордерах»/«Отчёте».
    cores: Vec<(u64, String)>,
    sel_cores: HashSet<u64>,
    side: SideFilter,
    /// None — все, Some(false) — реальные, Some(true) — эмуляторные.
    emu: Option<bool>,
    /// Background summary state with distinct loading, unavailable, ready, and
    /// failed outcomes so only a successful empty read appears empty.
    pub(super) data: LoadState<Summary>,
    /// Счётчик фоновых пересчётов (сводка/тюнер/гистограмма/подбор): >0 —
    /// блокирующий оверлей «Загрузка…» поверх окна. Длинные сканы большой БД
    /// иначе никак не видны, а клики по фильтрам/стратегиям копились в очередь.
    busy_ops: usize,
    /// Начало текущей серии пересчётов: оверлей показываем только спустя
    /// BUSY_OVERLAY_DELAY — быстрые пересчёты не мигают затемнением.
    busy_since: Option<std::time::Instant>,
    /// Номер запроса — устаревшие результаты отбрасываются.
    seq: u64,
    /// Summary: the top-left chart in "by cores" mode (checkbox, default OFF).
    pub(super) sum_by_core: bool,
    /// Ховер-ведро левого чарта по ядрам (индекс в `days`) — попап значений.
    pub(super) hover_core_bucket: Option<usize>,
    /// Ховер-ведро правого чарта «Дневная прибыль» — свой попап (иначе один
    /// стейт рисовал бы попапы на обоих чартах разом).
    pub(super) hover_daily_bucket: Option<usize>,
    /// Вкладка «Стратегии»: выбранная группа `(strategyid текстом, имя)`
    /// + её детализация.
    pub(super) sel_strategy: Option<(String, String)>,
    /// Multi-select (Ctrl): the EXTRA selected rows beyond the anchor (`sel_strategy`).
    /// The anchor drives scope/suggest/detail; these are bulk-write addressees only,
    /// stored as `(key, name)`. Empty = single selection.
    pub(super) sel_extra: Vec<(String, String)>,
    /// Strategy-list filter bar (see tuner::list): name search text, kind filter (None = all),
    /// and "active only" (default on — hides strategies no longer present in any core).
    pub(super) strat_search: String,
    pub(super) strat_type: Option<String>,
    pub(super) strat_active_only: bool,
    /// Lazily-created search input backing `strat_search`.
    pub(super) strat_search_input: Option<Entity<MoonInputState>>,
    /// List sort: `(column key, descending)`. None → the default profit-descending order.
    pub(super) strat_sort: Option<(String, bool)>,
    /// Visible-column bitmask for the strategy list (kind, core, then the metric columns).
    pub(super) strat_cols: u16,
    pub(super) detail: LoadState<StrategyDetail>,
    detail_seq: u64,
    /// Вкладка «Календарь»: посуточные ячейки (PnL+сделки+wins) за период.
    pub(super) cal_days: Option<Arc<Vec<DayCell>>>,
    cal_seq: u64,
    /// Серия устарела относительно текущих фильтров — перечитать при входе.
    cal_dirty: bool,
    cal_mode: calendar::CalMode,
    /// Показанный месяц календаря `(год, месяц 1..12)` — СВОЯ навигация вкладки
    /// (Назад/Вперёд); период-бар окна на «Календаре» не действует.
    pub(super) cal_ym: (i32, u32),
    /// Выбранный день (start суток) для режима «День».
    pub(super) cal_day: i64,
    /// Агрегат ПРЕДЫДУЩЕГО месяца `(profit, trades, wins)` — для дельт KPI
    /// «к пред. периоду» (сравниваем месяц с месяцем, не 30 дней).
    pub(super) cal_prev: Option<(f64, i64, i64)>,
    /// День под курсором в календаре (start суток) — подсветка ячейки.
    pub(super) cal_hover: Option<i64>,
    /// Режим вкладки «Стратегии» (Обзор / Фильтры / Монеты). Приватность —
    /// модульная: субмодули вкладок видят поля родителя без pub(super).
    strat_mode: tuner::StratMode,
    /// Тюнер порогов (режим «Фильтры») — состояние в своём модуле.
    tuner: tuner::TunerState,
    /// Режим «По времени»: профили «час дня» по столбцам-периодам
    /// (текущий/неделя/месяц/90д). None до первого расчёта / при ошибке чтения.
    pub(super) time_profiles: Option<Arc<Vec<[moon_core::db::analytics::HourStat; 24]>>>,
    /// Профили среднего профита для раскраски ползунков «По времени»
    /// (неделя×час / час суток / минута в часе). None до расчёта / при ошибке.
    pub(super) time_slider: Option<Arc<moon_core::db::tuner::SliderProfiles>>,
    /// KPI «Факт vs варианты» режима «По времени»: столбцы Факт / v1 / v2 по
    /// недельному расписанию из сетки — универсальная матрица в правом углу.
    pub(super) time_stats: LoadState<Vec<moon_core::db::tuner::VarStats>>,
    /// Сетка недельного расписания (7 дней × от/до для v1/v2) режима «По времени».
    time_tuner: tuner::TimeTunerState,
    time_seq: u64,
    /// Профиль устарел относительно текущих фильтров/периода — пересчитать при входе.
    time_dirty: bool,
    /// Bounds дорожек трёх ползунков «По времени» (неделя/сутки/час) — захват через
    /// `canvas` для перевода координаты мыши в значение при drag.
    slider_track: [Option<gpui::Bounds<gpui::Pixels>>; 3],
    /// Активный drag ползунка: `(поле 0..2, тянем ли левый хендл `от`)`.
    slider_drag: Option<(usize, bool)>,
    /// Календари произвольного диапазона «с»/«по» (moonui MoonCalendar в
    /// попапах); выбор даты переключает период в Period::Custom.
    cal_from: Entity<MoonCalendarState>,
    cal_to: Entity<MoonCalendarState>,
    cal_from_open: bool,
    cal_to_open: bool,
    /// Whether the single delayed integrity-status poll is armed.
    integrity_poll_armed: bool,
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
        // Период «Тюнинга» персистится отдельным ключом (независим от «Сводки»).
        let saved_strat_period = backend
            .read(cx)
            .layout
            .analytics_strat_period
            .as_deref()
            .and_then(Period::from_id);
        // Visible strategy-list columns from the previous run (default — all of them).
        let saved_strat_cols = backend
            .read(cx)
            .layout
            .analytics_strat_cols
            .unwrap_or(tuner::STRAT_COLS_ALL);
        // Режим календаря из прошлого запуска (дефолт — «Месяц»).
        let saved_mode = backend
            .read(cx)
            .layout
            .analytics_heat_mode
            .as_deref()
            .and_then(calendar::CalMode::from_id)
            .unwrap_or(calendar::CalMode::Month);

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
            strat_period: saved_strat_period.unwrap_or(Period::CurMonth),
            data_period: saved_period.unwrap_or(Period::CurMonth),
            cores: Vec::new(),
            sel_cores: HashSet::new(),
            side: SideFilter::All,
            // Дефолт «Реальные» — как в Отчёте (эмуляторные шумят статистику).
            emu: Some(false),
            data: LoadState::default(),
            busy_ops: 0,
            busy_since: None,
            seq: 0,
            sum_by_core: false,
            hover_core_bucket: None,
            hover_daily_bucket: None,
            sel_strategy: None,
            sel_extra: Vec::new(),
            strat_search: String::new(),
            strat_type: None,
            strat_active_only: true,
            strat_search_input: None,
            strat_sort: Some(("analytics.col.profit".to_string(), true)),
            strat_cols: saved_strat_cols,
            detail: LoadState::default(),
            detail_seq: 0,
            cal_days: None,
            cal_seq: 0,
            cal_dirty: true,
            cal_mode: saved_mode,
            cal_ym: {
                use chrono::Datelike;
                let d = day_of_secs(moon_core::util::now_unix_ms_i64() / 1000).unwrap_or_default();
                (d.year(), d.month())
            },
            cal_day: (moon_core::util::now_unix_ms_i64() / 1000).div_euclid(86_400) * 86_400,
            cal_prev: None,
            cal_hover: None,
            strat_mode: tuner::StratMode::Filters,
            tuner: tuner::TunerState::load(),
            time_profiles: None,
            time_slider: None,
            time_stats: LoadState::default(),
            time_tuner: tuner::TimeTunerState::load(),
            time_seq: 0,
            time_dirty: true,
            slider_track: Default::default(),
            slider_drag: None,
            cal_from,
            cal_to,
            cal_from_open: false,
            cal_to_open: false,
            integrity_poll_armed: false,
            _cal_subs: cal_subs,
            focus: cx.focus_handle(),
        };
        this.reload(cx);
        this
    }

    /// Период активной вкладки: «Тюнинг» ведёт СВОЁ окно времени, отдельное от
    /// «Сводки». «Календарь» период-баром не пользуется (у него своя навигация),
    /// но его reload_calendar строит запрос сам — сюда он не заходит.
    fn active_period(&self) -> Period {
        match self.tab {
            Tab::Strategies => self.strat_period,
            _ => self.period,
        }
    }

    /// Текущие фильтры одной структурой (общая для вкладок «Сводка»/«Тюнинг»);
    /// период — активной вкладки (`active_period`).
    fn query(&self) -> Query {
        let (from, to) = self.active_period().range();
        Query {
            from,
            to,
            cores: self.cores_selected(),
            side: self.side,
            emulator: self.emu,
            strategy: None,
            strat_core: None,
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
        // Mark the request at its start so an error from another period cannot
        // remain under the current period label.
        self.data.begin();
        self.op_started();
        self.seq = self.seq.wrapping_add(1);
        let req = self.seq;
        // `data` считается за окно времени АКТИВНОЙ вкладки — фиксируем его.
        self.data_period = self.active_period();
        // Тюнер зависит от тех же фильтров: сбрасываем; активному режиму —
        // пересчёт сразу, иначе — при следующем входе в режим «Фильтры».
        self.tuner.invalidate();
        // Ось «По времени» — тот же скоуп: отбросить её автоподбор в полёте, иначе
        // стейл-результат от СТАРОГО запроса дописался бы в v1 (и стал бы Saveable).
        self.time_tuner.invalidate_suggest();
        if self.tab == Tab::Strategies && self.strat_mode == tuner::StratMode::Filters {
            self.reload_tuner(cx);
            self.reload_hist(cx);
        }
        // Профиль «По времени» зависит от тех же фильтров/периода: помечаем
        // устаревшим; активному режиму — пересчёт сразу, иначе при входе.
        self.time_dirty = true;
        if self.tab == Tab::Strategies && self.strat_mode == tuner::StratMode::Time {
            self.reload_time(cx);
        }
        // Календарь зависит от тех же фильтров: помечаем устаревшим;
        // активной вкладке — пересчёт сразу, иначе при входе на неё.
        self.cal_dirty = true;
        if self.tab == Tab::Calendar {
            self.reload_calendar(cx);
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
                    // Keep the last known core list when a read produces no
                    // summary: an empty `cores` makes `cores_selected()` read as
                    // "no filter", which renders as if every core were selected.
                    if let Ok(d) = &data {
                        this.cores = d.cores.clone();
                    }
                    this.data.apply(data);
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

    // reload_detail → tuner/mod.rs; cal_query/cal_query_prev/reload_calendar →
    // calendar/mod.rs (страничные пересчёты живут при своих страницах).

    fn set_period(&mut self, p: Period, window: &mut Window, cx: &mut Context<Self>) {
        // Повторный клик по активному пресету = ручное обновление данных
        // (автоперечитки по новым отчётам нет). Период-бар редактирует окно
        // времени АКТИВНОЙ вкладки: «Сводка» и «Тюнинг» независимы.
        let strat = self.tab == Tab::Strategies;
        if strat {
            self.strat_period = p;
        } else {
            self.period = p;
        }
        // Пресет побеждает произвольный диапазон: поля «с»/«по» очищаются.
        if !matches!(p, Period::Custom(..)) {
            for cal in [&self.cal_from, &self.cal_to] {
                cal.update(cx, |s, cx| s.set_date(MoonDate::Single(None), window, cx));
            }
        }
        // Выбор персистится в СВОЙ ключ — окно (и следующий запуск) откроется с ним.
        let id = Some(p.persist_id());
        self.backend.update(cx, |b, _| {
            let slot = if strat {
                &mut b.layout.analytics_strat_period
            } else {
                &mut b.layout.analytics_period
            };
            if *slot != id {
                *slot = id;
                b.layout_dirty = true;
            }
        });
        self.reload(cx);
        cx.notify();
    }

    /// Синхронизировать поля «с»/«по» (общие MoonCalendarState) с периодом
    /// активной вкладки — при переключении вкладок период-бар должен показывать
    /// СВОЙ диапазон вкладки, а не оставшийся от предыдущей.
    fn sync_period_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (from_date, to_date) = match self.active_period() {
            Period::Custom(f, t) => (
                if f >= 0 { day_of_secs(f) } else { None },
                day_of_secs(t - 86_400),
            ),
            _ => (None, None),
        };
        self.cal_from.update(cx, |s, cx| {
            s.set_date(MoonDate::Single(from_date), window, cx)
        });
        self.cal_to.update(cx, |s, cx| {
            s.set_date(MoonDate::Single(to_date), window, cx)
        });
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
            Tab::Calendar => self.calendar_tab(p, cx),
        };
        // Вкладки делят высоту сами (нижние плашки прибиты к низу окна,
        // содержимое скроллится внутри) — внешнего скролла нет.
        let body_scrolls = false;
        let integrity = self.integrity_note(cx);
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
            // «Календарь» ведёт СВОЮ навигацию по месяцам — период-бар (с/по)
            // на нём скрыт (у него своя строка Назад/месяц/Вперёд в теле).
            .when(self.tab != Tab::Calendar, |el| {
                el.child(self.period_bar(p, cx))
            })
            // Баннер целостности — на ЛЮБОЙ вкладке: повреждённая реплика важна
            // и на «Календаре», который читает ту же базу.
            .when_some(integrity, |el, (title, detail)| {
                el.child(
                    // Do not use `.banner()`: MoonAlert renders the title only in the
                    // non-banner form (alert.rs `when(!self.banner, ..title..)`),
                    // so the banner variant would drop the localized heading and
                    // show the bare SQLite diagnostic line.
                    div()
                        .px(design::ui_px(cx, 10.0))
                        .pb(design::ui_px(cx, 6.0))
                        .child(MoonAlert::warning("an-integrity-banner", detail).title(title)),
                )
            })
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
                                .child(t!("common.loading").to_string()),
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
