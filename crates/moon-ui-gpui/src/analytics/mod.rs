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

use std::collections::HashSet;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown,
    MoonMenuSize, MoonPalette, MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use crate::design::{moon, moon_alpha};
use crate::{Backend, design};
use moon_core::db::SideFilter;
use moon_core::db::analytics::{Query, StrategyDetail, Summary};

const ANALYTICS_HEADER_H: f32 = 32.0;

/// Вкладки окна (реализована «Сводка»; остальные — этапы плана).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Summary,
    Strategies,
    Coins,
    Heatmap,
    Calendar,
    Leverage,
}

impl Tab {
    const ALL: [Tab; 6] = [
        Tab::Summary,
        Tab::Strategies,
        Tab::Coins,
        Tab::Heatmap,
        Tab::Calendar,
        Tab::Leverage,
    ];
    fn id(self) -> &'static str {
        match self {
            Tab::Summary => "an-summary",
            Tab::Strategies => "an-strategies",
            Tab::Coins => "an-coins",
            Tab::Heatmap => "an-heatmap",
            Tab::Calendar => "an-calendar",
            Tab::Leverage => "an-leverage",
        }
    }
    fn title(self) -> String {
        match self {
            Tab::Summary => t!("analytics.tab.summary"),
            Tab::Strategies => t!("analytics.tab.strategies"),
            Tab::Coins => t!("analytics.tab.coins"),
            Tab::Heatmap => t!("analytics.tab.heatmap"),
            Tab::Calendar => t!("analytics.tab.calendar"),
            Tab::Leverage => t!("analytics.tab.leverage"),
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
    fn id(self) -> &'static str {
        match self {
            Period::Today => "p-today",
            Period::Yesterday => "p-yesterday",
            Period::Week => "p-week",
            Period::CurMonth => "p-cur-month",
            Period::Month => "p-month",
            Period::Year => "p-year",
            Period::All => "p-all",
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
        }
    }
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
    /// Номер запроса — устаревшие результаты отбрасываются.
    seq: u64,
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

        let backend_tuner_cfg = backend.read(cx).layout.analytics_tuner.clone();
        let mut this = Self {
            backend,
            tab: Tab::Summary,
            period: Period::Month,
            cores: Vec::new(),
            sel_cores: HashSet::new(),
            side: SideFilter::All,
            // Дефолт «Реальные» — как в Отчёте (эмуляторные шумят статистику).
            emu: Some(false),
            data: None,
            inflight: false,
            seq: 0,
            sel_strategy: None,
            detail: None,
            detail_seq: 0,
            strat_mode: strategies::StratMode::Overview,
            tuner: tuner::TunerState::load(&backend_tuner_cfg),
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

    /// Фоновый расчёт сводки за текущий период/фильтры.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.inflight = true;
        self.seq = self.seq.wrapping_add(1);
        let req = self.seq;
        // Тюнер зависит от тех же фильтров: сбрасываем; активному режиму —
        // пересчёт сразу, иначе — при следующем входе в режим «Фильтры».
        self.tuner.stats = None;
        self.tuner.hist = None;
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
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let detail = executor
                .spawn(async move { moon_core::db::analytics::strategy_detail(&q, id) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
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

    fn set_period(&mut self, p: Period, cx: &mut Context<Self>) {
        // Повторный клик по активному пресету = ручное обновление данных
        // (автоперечитки по новым отчётам нет).
        self.period = p;
        self.reload(cx);
        cx.notify();
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
                                && this.tuner.stats.is_none()
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
                    .on_click(cx.listener(move |this, _, _, cx| this.set_period(per, cx)))
                    .render(),
            );
        }
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

    /// Заглушка нереализованной вкладки.
    fn stub(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        div()
            .w_full()
            .p(design::ui_px(cx, 18.0))
            .text_color(moon(p.text_muted))
            .child(t!("analytics.stub").to_string())
            .into_any_element()
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
            _ => self.stub(p, cx),
        };
        // «Стратегии» делят высоту сами (нижняя плашка прибита к низу экрана,
        // список скроллится внутри) — внешний скролл только прочим вкладкам.
        let body_scrolls = self.tab != Tab::Strategies;
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
