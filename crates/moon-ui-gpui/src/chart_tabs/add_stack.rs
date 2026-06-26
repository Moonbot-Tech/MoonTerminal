//! AddToChart-вкладка: визуально один список графиков, архитектурно — отдельный `ChartPanel`
//! на каждый график. Вынесено из `chart_tabs` как самостоятельная вью-модель; общий рендер
//! стека — в [`super::stack`]. Используется и полоской вкладок, и выносными окнами ([`super::windows`]).

use gpui::*;
use moon_ui::MoonVirtualListScrollHandle;

use super::stack::{
    ChartStackEntry, HIGHLIGHT, render_chart_stack, resolve_layout, set_panels_orderbook_enabled,
    set_panels_auto_pin, set_panels_scale, set_panels_show_zone,
};
use crate::Backend;
use crate::chart_persist::{StackLayoutMode, StackOrientation};
use crate::panels::ChartPanel;
use moon_core::config::{ChartBucket, ChartTheme};
use moon_core::session::CoreId;

/// AddToChart-вкладка: визуально это один список графиков, но архитектурно каждый график —
/// отдельный `ChartPanel`/`gpu_canvas`/dirty entity. Не возвращаемся к старой модели
/// `ChartPanel -> Container.panes`, где mousemove одного графика перерисовывал overlay всех.
pub(crate) struct AddChartStack {
    backend: Entity<Backend>,
    num: u32,
    bucket: ChartBucket,
    epoch: f64,
    theme: ChartTheme,
    charts: Vec<ChartStackEntry>,
    scale: Option<f32>,
    /// Per-tab режим раскладки (Fit/Scroll; None = дефолт Fit).
    layout_mode: Option<StackLayoutMode>,
    /// Высота слота для Fit: 0 = растяжение, ≥20 = compress. None = дефолт.
    layout_height_fit: Option<u16>,
    /// Высота слота для Scroll. None = дефолт.
    layout_height_scroll: Option<u16>,
    /// Показывать ли стакан на графиках вкладки (per-окно). None = дефолт (вкл).
    orderbook_enabled: Option<bool>,
    /// Показывать ли заливку зоны управления (per-окно). None = дефолт (вкл).
    show_zone: Option<bool>,
    /// Авто-пин графика при выставлении ордера (per-окно). None = дефолт (выкл).
    auto_pin: Option<bool>,
    /// Ориентация стека (per-окно). None = дефолт (Vertical).
    layout_orientation: Option<StackOrientation>,
    /// Подписки на стаканы временно приостановлены (вкладка не в фокусе > 5с). Эффективный
    /// стакан = `orderbook_enabled ∧ !suspended` — не затирает пользовательскую галку «Стакан».
    /// Откреплённые в окно вкладки никогда не suspend (окно само держит спрос).
    orderbook_suspended: bool,
    /// Скролл-хэндл вертикального MoonVirtualList (scroll-режим стека).
    scroll: MoonVirtualListScrollHandle,
}

impl AddChartStack {
    pub(super) fn new(
        backend: Entity<Backend>,
        num: u32,
        bucket: ChartBucket,
        epoch: f64,
        theme: ChartTheme,
    ) -> Self {
        Self {
            backend,
            num,
            bucket,
            epoch,
            theme,
            charts: Vec::new(),
            scale: None,
            layout_mode: None,
            layout_height_fit: None,
            layout_height_scroll: None,
            orderbook_enabled: None,
            show_zone: None,
            auto_pin: None,
            layout_orientation: None,
            orderbook_suspended: false,
            scroll: MoonVirtualListScrollHandle::new(),
        }
    }

    pub(super) fn add_coin(
        &mut self,
        core: CoreId,
        market: &str,
        ttl_ms: f64,
        cx: &mut Context<Self>,
    ) {
        let (_, compress, _) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );

        // Уже есть такой график → продлить TTL.
        if let Some(i) = self
            .charts
            .iter()
            .position(|e| e.core == core && e.market == market)
        {
            if self.charts[i].vacated {
                self.charts[i].vacated = false;
                self.charts[i].arrived_at = std::time::Instant::now();
            }
            let panel = self.charts[i].panel.clone();
            panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
            cx.notify();
            return;
        }

        // COMPRESS: новый занимает ПЕРВЫЙ пустой держащийся слот (без сдвига/смены размера соседей).
        if compress {
            if let Some(i) = self.charts.iter().position(|e| e.vacated) {
                self.charts[i].core = core;
                self.charts[i].market = market.to_string();
                self.charts[i].arrived_at = std::time::Instant::now();
                self.charts[i].vacated = false;
                let panel = self.charts[i].panel.clone();
                panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
                cx.notify();
                return;
            }
        }

        // Новый график — в конец (в FIT-stretch запиненные всплывут при сортировке в render).
        let backend = self.backend.clone();
        let num = self.num;
        let bucket = self.bucket.clone();
        let epoch = self.epoch;
        let theme = self.theme.clone();
        let scale = self.scale;
        let panel = cx.new(|cx| ChartPanel::new_addto(backend, num, bucket, epoch, theme, cx));
        // Любое изменение панели (вкл. переключение пина ●/○) → перерисовать стек: prune пустых +
        // пере-сортировка запиненных наверх происходит в render.
        cx.observe(&panel, |this, _, cx| {
            this.prune_or_hold(cx);
            cx.notify();
        })
        .detach();
        if scale.is_some() {
            panel.update(cx, |panel, pcx| panel.set_scale(scale, pcx));
        }
        // Эффективный стакан (учитывая suspend-гейт): новый чарт не должен подписываться, если
        // вкладка сейчас приостановлена (или галка «Стакан» снята).
        panel.update(cx, |panel, pcx| {
            panel.set_orderbook_enabled(self.effective_orderbook(), pcx)
        });
        if let Some(sz) = self.show_zone {
            panel.update(cx, |panel, pcx| panel.set_show_zone(sz, pcx));
        }
        if let Some(ap) = self.auto_pin {
            panel.update(cx, |panel, pcx| panel.set_auto_pin(ap, pcx));
        }
        panel.update(cx, |panel, pcx| panel.add_coin(core, market, ttl_ms, pcx));
        self.charts
            .push(ChartStackEntry::new(core, market.to_string(), panel));
        cx.notify();
    }

    /// Реакция на выбытие графиков (TTL истёк → пустая панель).
    /// - **FIT-stretch / Scroll**: удаляем пустые сразу (стабильность даёт пин — сортировка в render).
    /// - **COMPRESS (Fit+пиксели)**: слот НЕ удаляем — помечаем `vacated` (держит позицию и размер
    ///   соседей). Сброс ВСЕХ слотов — только когда пустыми стали все (→ вернётся дефолтная высота).
    fn prune_or_hold(&mut self, cx: &App) -> bool {
        let (_, compress, _) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        if !compress {
            let before = self.charts.len();
            self.charts.retain(|e| e.panel.read(cx).pane_count() > 0);
            return self.charts.len() != before;
        }
        let mut changed = false;
        for e in self.charts.iter_mut() {
            let empty = e.panel.read(cx).pane_count() == 0;
            if empty != e.vacated {
                e.vacated = empty;
                changed = true;
            }
        }
        if !self.charts.is_empty() && self.charts.iter().all(|e| e.vacated) {
            self.charts.clear();
            changed = true;
        }
        changed
    }

    pub(crate) fn pane_count(&self, cx: &App) -> usize {
        self.charts
            .iter()
            .filter(|entry| entry.panel.read(cx).pane_count() > 0)
            .count()
    }

    pub(crate) fn scale(&self) -> Option<f32> {
        self.scale
    }

    pub(crate) fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        if self.scale == pct {
            return;
        }
        self.scale = pct;
        set_panels_scale(&self.charts, pct, cx);
        cx.notify();
    }

    pub(crate) fn orderbook_enabled(&self) -> Option<bool> {
        self.orderbook_enabled
    }

    /// Эффективный стакан = пользовательская галка (None→вкл) И не приостановлен по фокусу.
    fn effective_orderbook(&self) -> bool {
        self.orderbook_enabled.unwrap_or(true) && !self.orderbook_suspended
    }

    /// Вкл/выкл стакан для всех графиков стека (per-окно). Применяется с учётом suspend-гейта.
    pub(crate) fn set_orderbook_enabled(&mut self, enabled: Option<bool>, cx: &mut Context<Self>) {
        if self.orderbook_enabled == enabled {
            return;
        }
        self.orderbook_enabled = enabled;
        set_panels_orderbook_enabled(&self.charts, self.effective_orderbook(), cx);
        cx.notify();
    }

    /// Приостановить/возобновить подписки на стаканы по фокусу вкладки (гейтинг кастомных
    /// вкладок: ушли > 5с → suspend=true → отписка; вернулись → resume). Не трогает галку «Стакан».
    pub(crate) fn set_orderbook_suspended(&mut self, suspended: bool, cx: &mut Context<Self>) {
        if self.orderbook_suspended == suspended {
            return;
        }
        self.orderbook_suspended = suspended;
        set_panels_orderbook_enabled(&self.charts, self.effective_orderbook(), cx);
        self.backend.update(cx, |b, _| b.rebuild_orderbook_wanted());
        cx.notify();
    }

    pub(crate) fn show_zone(&self) -> Option<bool> {
        self.show_zone
    }

    /// Вкл/выкл заливку зоны управления для всех графиков стека (per-окно).
    pub(crate) fn set_show_zone(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        if self.show_zone == show {
            return;
        }
        self.show_zone = show;
        set_panels_show_zone(&self.charts, show.unwrap_or(true), cx);
        cx.notify();
    }

    pub(crate) fn auto_pin(&self) -> Option<bool> {
        self.auto_pin
    }

    /// Вкл/выкл авто-пин при ордере для всех графиков стека (per-окно).
    pub(crate) fn set_auto_pin(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        if self.auto_pin == on {
            return;
        }
        self.auto_pin = on;
        set_panels_auto_pin(&self.charts, on.unwrap_or(false), cx);
        cx.notify();
    }

    pub(crate) fn layout_orientation(&self) -> Option<StackOrientation> {
        self.layout_orientation
    }

    /// Сменить ориентацию стека (per-окно). Перестраивает текущее отображение.
    pub(crate) fn set_orientation(&mut self, orientation: Option<StackOrientation>, cx: &mut Context<Self>) {
        if self.layout_orientation == orientation {
            return;
        }
        self.layout_orientation = orientation;
        cx.notify();
    }

    pub(crate) fn layout_mode(&self) -> Option<StackLayoutMode> {
        self.layout_mode
    }

    pub(crate) fn layout_height_fit(&self) -> Option<u16> {
        self.layout_height_fit
    }

    pub(crate) fn layout_height_scroll(&self) -> Option<u16> {
        self.layout_height_scroll
    }

    /// Применить per-tab раскладку (режим + раздельные высоты Fit/Scroll) к этому стеку.
    pub(crate) fn set_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_mode == mode
            && self.layout_height_fit == height_fit
            && self.layout_height_scroll == height_scroll
        {
            return;
        }
        self.layout_mode = mode;
        self.layout_height_fit = height_fit;
        self.layout_height_scroll = height_scroll;
        // Слоты держатся только в COMPRESS. При переключении в другой режим пустые слоты убираем,
        // чтобы FIT-stretch/Scroll не показывали пустые плашки.
        let (_, compress, _) = resolve_layout(mode, height_fit, height_scroll);
        if !compress {
            self.charts.retain(|e| !e.vacated);
        }
        cx.notify();
    }

    /// Текущий список тикеров стека `(core, market)` — для персиста кастомной вкладки.
    /// Пустые/освобождённые слоты пропускаем.
    pub(crate) fn coins(&self, cx: &App) -> Vec<(CoreId, String)> {
        self.charts
            .iter()
            .filter(|e| !e.vacated && e.panel.read(cx).pane_count() > 0)
            .map(|e| (e.core, e.market.clone()))
            .collect()
    }

    /// Закрепить (pin) все графики стека — для кастомной вкладки (чарты сразу запинены).
    pub(crate) fn pin_all(&mut self, cx: &mut Context<Self>) {
        for e in &self.charts {
            e.panel.update(cx, |p, pcx| p.ensure_pinned(pcx));
        }
    }

    pub(super) fn set_scene_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        for entry in &self.charts {
            entry
                .panel
                .update(cx, |panel, _| panel.set_scene_visible(visible));
        }
    }

    pub(crate) fn close_all_panes(&mut self, cx: &mut Context<Self>) {
        for entry in &self.charts {
            entry
                .panel
                .update(cx, |panel, pcx| panel.close_all_panes(pcx));
        }
        self.charts.clear();
        cx.notify();
    }
}

impl Render for AddChartStack {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = moon_ui::MoonPalette::active(cx);
        if self.charts.is_empty() {
            // Непрозрачный фон: в выносном окне Root=NoFill и own-pass нет → без фона
            // сквозь логотип просвечивает белая подложка окна.
            return div()
                .size_full()
                .bg(rgb(palette.chart_bg))
                .flex()
                .items_center()
                .justify_center()
                .child(crate::design::logo_glow_sized(220.0))
                .into_any_element();
        }

        // Stack: per-tab раскладка (FIT/SCROLL/COMPRESS + высота), иначе глобальный дефолт.
        // ВАЖНО: чарт-слоты ПРОЗРАЧНЫЕ. own-pass (combo/стакан) — слой GpuCanvasLayer::UnderScene
        // (под сценой); любой непрозрачный `.bg()` над слотом его перекрывает. Разделитель — рамка.
        let (scroll, compress, cfg_h) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        // Запиненные наверх кластером — ТОЛЬКО НЕ в COMPRESS (там слоты позиционно стабильны,
        // сортировка их бы двигала). В FIT-stretch/Scroll пин поднимает график к запиненным.
        if !compress {
            self.charts.sort_by_key(|e| !e.panel.read(cx).is_pinned());
        }
        let count = self.charts.len();
        let border = rgb(palette.border);
        let accent = rgb(palette.blue);
        let base_id = format!("add-chart-stack-{}", self.num);
        let horizontal = self
            .layout_orientation
            .unwrap_or(StackOrientation::Vertical)
            .is_horizontal();
        let entity = cx.entity();
        render_chart_stack(
            &base_id,
            self,
            entity,
            count,
            scroll,
            compress,
            horizontal,
            cfg_h,
            &self.scroll,
            border,
            // Пустой (держащийся) COMPRESS-слот → None: render покажет прозрачную плашку.
            |s, ix| {
                s.charts
                    .get(ix)
                    .filter(|e| !e.vacated)
                    .map(|e| e.panel.clone())
            },
            move |s, ix, panel, size, flex, horizontal, border, _ent| {
                let (id, fresh) = match s.charts.get(ix) {
                    Some(e) => (
                        format!("add-chart-stack-tile-{}-{}-{}", s.num, e.core, e.market),
                        e.arrived_at.elapsed() < HIGHLIGHT,
                    ),
                    None => (format!("add-chart-stack-tile-{}-{ix}", s.num), false),
                };
                let mut tile = div()
                    .id(SharedString::from(id.clone()))
                    .relative()
                    .overflow_hidden()
                    .border_1()
                    .border_color(border);
                // Поперёк оси — на всю ширину/высоту; вдоль оси — flex+cap (COMPRESS до size, сжатие),
                // фикс (size без flex) или растяжение (FIT). Гор: ось = X (ширина), верт: ось = Y.
                tile = if horizontal { tile.h_full() } else { tile.w_full() };
                if flex {
                    tile = tile.flex_1();
                    tile = if horizontal { tile.min_w(px(0.0)) } else { tile.min_h(px(0.0)) };
                    if let Some(v) = size {
                        tile = if horizontal { tile.max_w(px(v)) } else { tile.max_h(px(v)) };
                    }
                } else if let Some(v) = size {
                    tile = if horizontal {
                        tile.w(px(v)).min_w(px(0.0))
                    } else {
                        tile.h(px(v)).min_h(px(0.0))
                    };
                }
                // Подсветка только что появившегося графика: яркая акцентная рамка поверх, пульс
                // (3 мигания за HIGHLIGHT). Сдвинута внутрь на 1px, чтобы overflow_hidden её не
                // срезал; opacity не падает в 0 на пике, чтобы было хорошо видно. gpui гонит кадры.
                let highlight = fresh.then(|| {
                    div()
                        .absolute()
                        .top(px(1.0))
                        .left(px(1.0))
                        .right(px(1.0))
                        .bottom(px(1.0))
                        .border_2()
                        .border_color(accent)
                        .rounded(px(2.0))
                        .with_animation(
                            SharedString::from(format!("{id}-arrive")),
                            Animation::new(HIGHLIGHT),
                            |el, delta| {
                                // 3 чётких мигания: 0 → 1 → 0, повторённые.
                                let pulse = (delta * std::f32::consts::PI * 3.0).sin().abs();
                                el.opacity(pulse)
                            },
                        )
                });
                tile.child(div().size_full().relative().overflow_hidden().child(panel))
                    .children(highlight)
                    .into_any_element()
            },
        )
    }
}
