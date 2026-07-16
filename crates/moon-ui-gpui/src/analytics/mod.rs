//! Окно «Аналитика» — анализаторы отчётов поверх реплики `orders_rep`
//! (см. план analytics-panel-plan: сводка → сравнения → heatmap → календарь).
//!
//! Отдельное singleton ОС-окно (паттерн «Скринер»): геометрия персистится в
//! `layout.analytics_window`. Вкладки — полоса MoonButton (как в Настройках);
//! пока функциональна «Сводка», остальные — заглушки следующих этапов.
//! Данные считает `moon_core::db::analytics` на background executor (полная
//! выборка периода из SQLite — не на UI-потоке), перезапрашиваются по смене
//! периода и по поколению writer'а отчётов.

mod charts;
mod summary;

use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette,
    MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use crate::design::{moon, moon_alpha};
use crate::{Backend, design};
use moon_core::db::analytics::Summary;

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
    Month,
    Year,
    All,
}

impl Period {
    const ALL: [Period; 6] = [
        Period::Today,
        Period::Yesterday,
        Period::Week,
        Period::Month,
        Period::Year,
        Period::All,
    ];
    fn id(self) -> &'static str {
        match self {
            Period::Today => "p-today",
            Period::Yesterday => "p-yesterday",
            Period::Week => "p-week",
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
    /// Данные сводки (фоновый расчёт); None — ещё не загружены.
    pub(super) data: Option<Arc<Summary>>,
    inflight: bool,
    /// Номер запроса — устаревшие результаты отбрасываются.
    seq: u64,
    /// Поколение writer'а отчётов на момент последней загрузки — новые записи
    /// в БД перезапускают расчёт (не чаще гейта наблюдателя).
    last_gen: u64,
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

        // Новые записи отчётов → перерасчёт (поколение writer'а, гейт по факту
        // смены — сам observe дёргается часто, сравнение дёшево).
        cx.observe(&backend, |this, backend, cx| {
            let g = backend
                .read(cx)
                .reports
                .as_ref()
                .map(|h| h.generation.load(std::sync::atomic::Ordering::Relaxed))
                .unwrap_or(0);
            if g != this.last_gen && !this.inflight {
                this.reload(cx);
            }
        })
        .detach();

        let mut this = Self {
            backend,
            tab: Tab::Summary,
            period: Period::Month,
            data: None,
            inflight: false,
            seq: 0,
            last_gen: 0,
            focus: cx.focus_handle(),
        };
        this.reload(cx);
        this
    }

    /// Фоновый расчёт сводки за текущий период.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.inflight = true;
        self.seq = self.seq.wrapping_add(1);
        let req = self.seq;
        self.last_gen = self
            .backend
            .read(cx)
            .reports
            .as_ref()
            .map(|h| h.generation.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(0);
        let (from, to) = self.period.range();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let data = executor
                .spawn(async move { moon_core::db::analytics::summary(from, to) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.seq != req {
                        return; // период уже сменили
                    }
                    this.inflight = false;
                    this.data = data.map(Arc::new);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn set_period(&mut self, p: Period, cx: &mut Context<Self>) {
        if self.period != p {
            self.period = p;
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
                            cx.notify();
                        }
                    }))
                    .render(),
            );
        }
        row
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
            .gap(design::ui_px(cx, 10.0))
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
            _ => self.stub(p, cx),
        };
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
                    .overflow_y_scroll()
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
