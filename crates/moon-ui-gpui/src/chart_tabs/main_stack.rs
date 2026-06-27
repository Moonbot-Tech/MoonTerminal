//! Main-вкладка чартов: один рынок = один отдельный `ChartPanel`/`gpu_canvas`, активный —
//! fullscreen, ПКМ по области панели графика разворачивает весь stack. Вынесено из `chart_tabs` как
//! самостоятельная вью-модель; общий рендер стека — в [`super::stack`].

use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::{MoonPalette, MoonVirtualListScrollHandle};

use super::stack::{
    ChartStackEntry, CompareRole, apply_compare, chart_stack_card, handle_compare_broom_requests,
    handle_compare_lock_requests, render_chart_stack, resolve_layout, retain_nonempty_panels,
    set_panels_action_btn_pos, set_panels_auto_pin, set_panels_orderbook_enabled, set_panels_scale,
    set_panels_show_zone,
};
use crate::Backend;
use crate::chart_persist::{ChartBtnPos, StackLayoutMode, StackOrientation};
use crate::panels::ChartPanel;
use moon_core::config::ChartTheme;
use moon_core::session::CoreId;

/// Main-вкладка: один рынок = один отдельный `ChartPanel`/`gpu_canvas`.
/// Обычный клик по рынку в таблицах открывает/фокусирует его fullscreen. ПКМ по области
/// текущей панели, включая стакан/glass, переключает fullscreen ↔ весь stack, не возвращая
/// несколько рынков внутрь одного `ChartEngine`.
pub(crate) struct MainChartStack {
    backend: Entity<Backend>,
    group: String,
    epoch: f64,
    theme: ChartTheme,
    charts: Vec<ChartStackEntry>,
    active: Option<usize>,
    show_stack: bool,
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
    /// Позиции кнопок Cancel Buy / Panic Sell в зоне чарта (per-окно). None = дефолт (Right).
    cancel_buy_pos: Option<ChartBtnPos>,
    panic_sell_pos: Option<ChartBtnPos>,
    /// Якорь сравнения `(core, market)` — ведущий по цене (замок горит, стоит слева). None = выкл.
    compare_anchor: Option<(CoreId, String)>,
    /// Общее Y-окно сравнения, следует за последней изменённой панелью.
    compare_y: Option<(f32, f32)>,
    /// Режим метлы: соседи якоря показывают «только стакан».
    compare_orderbook_only: bool,
    /// Армирован ли one-shot таймер авто-закрытия по неактивности (config `main_idle_close_secs`).
    /// Тикает ~1 Гц, пока фича включена и есть графики; сам пере-армится. См. `arm_idle_timer`.
    idle_timer_armed: bool,
    scroll: MoonVirtualListScrollHandle,
}

impl MainChartStack {
    pub(super) fn new(
        backend: Entity<Backend>,
        group: String,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            backend,
            group,
            epoch,
            theme,
            charts: Vec::new(),
            active: None,
            show_stack: false,
            scale: None,
            layout_mode: None,
            layout_height_fit: None,
            layout_height_scroll: None,
            orderbook_enabled: None,
            show_zone: None,
            auto_pin: None,
            layout_orientation: None,
            cancel_buy_pos: None,
            panic_sell_pos: None,
            compare_anchor: None,
            compare_y: None,
            compare_orderbook_only: false,
            idle_timer_armed: false,
            scroll: MoonVirtualListScrollHandle::new(),
        };
        if let Some((core, market)) = focus_open {
            this.open_or_focus(core, market, cx);
        }
        this
    }

    fn create_panel(
        &self,
        core: CoreId,
        market: &str,
        cx: &mut Context<Self>,
    ) -> Entity<ChartPanel> {
        let backend = self.backend.clone();
        let epoch = self.epoch;
        let theme = self.theme.clone();
        let market = market.to_string();
        let panel =
            cx.new(|cx| ChartPanel::new_main(backend, Some((core, market)), epoch, theme, cx));
        cx.observe(&panel, |this, _, cx| {
            let mut dirty = this.prune_empty(cx);
            if dirty {
                this.sync_visibility(cx);
                this.sync_backend_active(cx);
            }
            dirty |= this.sync_compare(cx);
            if dirty {
                cx.notify();
            }
        })
        .detach();
        if self.scale.is_some() {
            panel.update(cx, |panel, pcx| panel.set_scale(self.scale, pcx));
        }
        if let Some(en) = self.orderbook_enabled {
            panel.update(cx, |panel, pcx| panel.set_orderbook_enabled(en, pcx));
        }
        if let Some(sz) = self.show_zone {
            panel.update(cx, |panel, pcx| panel.set_show_zone(sz, pcx));
        }
        if let Some(ap) = self.auto_pin {
            panel.update(cx, |panel, pcx| panel.set_auto_pin(ap, pcx));
        }
        panel.update(cx, |panel, pcx| {
            panel.set_action_btn_pos(
                self.cancel_buy_pos.unwrap_or_default(),
                self.panic_sell_pos.unwrap_or_default(),
                pcx,
            )
        });
        panel
    }

    /// Синхронизировать режим сравнения (как в `AddChartStack`): забрать клики замка, навязать
    /// общее Y-окно/флаги. Возвращает true, если якорь/порядок изменились (нужен notify стека).
    fn sync_compare(&mut self, cx: &mut Context<Self>) -> bool {
        let horizontal = self
            .layout_orientation
            .unwrap_or(StackOrientation::Vertical)
            .is_horizontal();
        if !horizontal {
            self.compare_anchor = None;
        }
        let mut changed =
            handle_compare_lock_requests(&mut self.charts, &mut self.compare_anchor, cx);
        changed |=
            handle_compare_broom_requests(&self.charts, &mut self.compare_orderbook_only, cx);
        if self.compare_anchor.is_none() {
            self.compare_orderbook_only = false;
        }
        apply_compare(
            &self.charts,
            &self.compare_anchor,
            &mut self.compare_y,
            horizontal,
            self.compare_orderbook_only,
            cx,
        );
        changed
    }

    /// Роль слота для размеров метлы: Normal (метла выкл), Anchor (с замком) или Follower (стакан).
    fn compare_role(&self, ix: usize) -> CompareRole {
        if !self.compare_orderbook_only {
            return CompareRole::Normal;
        }
        match self.charts.get(ix) {
            Some(e) => {
                let is_anchor = self
                    .compare_anchor
                    .as_ref()
                    .is_some_and(|k| k.0 == e.core && k.1 == e.market);
                if is_anchor {
                    CompareRole::Anchor
                } else {
                    CompareRole::Follower
                }
            }
            None => CompareRole::Normal,
        }
    }

    pub(super) fn open_or_focus(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        if let Some(ix) = self
            .charts
            .iter()
            .position(|entry| entry.core == core && entry.market == market)
        {
            self.active = Some(ix);
            self.show_stack = false;
            self.sync_visibility(cx);
            self.sync_backend_active(cx);
            cx.notify();
            return;
        }

        let panel = self.create_panel(core, &market, cx);
        self.charts.push(ChartStackEntry::new(core, market, panel));
        self.active = Some(self.charts.len() - 1);
        self.show_stack = false;
        self.sync_visibility(cx);
        self.sync_backend_active(cx);
        // Новый чарт: в режиме сравнения сразу получает eligible + общее Y-окно.
        self.sync_compare(cx);
        cx.notify();
    }

    fn prune_empty(&mut self, cx: &App) -> bool {
        let active_key = self
            .active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| (entry.core, entry.market.clone()));
        let changed = retain_nonempty_panels(&mut self.charts, cx);
        if self.charts.is_empty() {
            self.active = None;
            self.show_stack = false;
        } else {
            self.active = active_key
                .and_then(|(core, market)| {
                    self.charts
                        .iter()
                        .position(|entry| entry.core == core && entry.market == market)
                })
                .or_else(|| Some(self.active.unwrap_or(0).min(self.charts.len() - 1)));
        }
        changed
    }

    /// Армировать (если ещё нет) one-shot таймер авто-закрытия по неактивности. Тикает ~1 Гц,
    /// пока фича включена (config `main_idle_close_secs` > 0) и есть графики; сам пере-армится
    /// в колбэке. Зовётся из render — поэтому стартует и при включении фичи на лету.
    fn arm_idle_timer(&mut self, cx: &mut Context<Self>) {
        if self.idle_timer_armed
            || self.charts.is_empty()
            || self.backend.read(cx).main_idle_close_secs() == 0
        {
            return;
        }
        self.idle_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(Duration::from_secs(1)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.idle_timer_armed = false;
                    this.prune_idle(cx);
                    this.arm_idle_timer(cx);
                })
                .is_ok()
            });
        })
        .detach();
    }

    /// Авто-закрытие графиков по неактивности окна (config `main_idle_close_secs`, сек). Дедлайн
    /// графика = max(последний активный ввод окна, время его прихода) + N → новейший закрывается
    /// последним; ровно N сек после начала неактивности для уже открытых. Если закрылся активный
    /// фулскрин-график — выходим из фулскрина (оставшиеся показываем стеком). Закрытие сразу
    /// отписывает стаканы (через `close_all_panes` панели). Возвращает, было ли изменение.
    fn prune_idle(&mut self, cx: &mut Context<Self>) -> bool {
        let secs = self.backend.read(cx).main_idle_close_secs();
        if secs == 0 || self.charts.is_empty() {
            return false;
        }
        // Не закрывать графики Main, пока в фокусе любое выносное ОКНО ГРАФИКА этой группы:
        // его активность не даёт mouse-move в Main-окне (это другое ОС-окно), поэтому таймер
        // иначе досчитал бы до закрытия, пока пользователь работает с откреплённым чартом.
        // Освежаем отметку активности → после расфокуса окна Main получает полный TTL, а не
        // закрывается мгновенно.
        let group = self.group.clone();
        let chart_handles: Vec<_> = self
            .backend
            .read(cx)
            .detached_chart_windows
            .iter()
            .filter(|(g, _)| *g == group)
            .map(|(_, h)| *h)
            .collect();
        let chart_focused = chart_handles
            .into_iter()
            .any(|h| h.is_active(cx).unwrap_or(false));
        if chart_focused {
            self.backend.update(cx, |b, _| b.note_main_input(&group));
            return false;
        }
        let ttl = Duration::from_secs(secs as u64);
        let last_input = self.backend.read(cx).main_input_at(&self.group);
        let now = Instant::now();
        let expired: Vec<usize> = self
            .charts
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let base = match last_input {
                    Some(t) => t.max(e.arrived_at),
                    None => e.arrived_at,
                };
                now.duration_since(base) >= ttl
            })
            .map(|(ix, _)| ix)
            .collect();
        if expired.is_empty() {
            return false;
        }
        let active_closed = self.active.is_some_and(|a| expired.contains(&a));
        // С конца, чтобы индексы не съезжали; перед удалением закрываем панель (отписка стаканов).
        for &ix in expired.iter().rev() {
            let entry = self.charts.remove(ix);
            entry.panel.update(cx, |p, pcx| p.close_all_panes(pcx));
        }
        if self.charts.is_empty() {
            self.active = None;
            self.show_stack = false;
        } else {
            // Активный закрылся в фулскрине → выходим из фулскрина, показываем оставшиеся стеком.
            if active_closed && !self.show_stack {
                self.show_stack = true;
            }
            self.active = Some(self.active.unwrap_or(0).min(self.charts.len() - 1));
        }
        self.sync_visibility(cx);
        self.sync_backend_active(cx);
        cx.notify();
        true
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
        cx.notify();
    }

    pub(crate) fn orderbook_enabled(&self) -> Option<bool> {
        self.orderbook_enabled
    }

    /// Вкл/выкл стакан для всех графиков стека (per-окно).
    pub(crate) fn set_orderbook_enabled(&mut self, enabled: Option<bool>, cx: &mut Context<Self>) {
        if self.orderbook_enabled == enabled {
            return;
        }
        self.orderbook_enabled = enabled;
        set_panels_orderbook_enabled(&self.charts, enabled.unwrap_or(true), cx);
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

    pub(crate) fn action_btn_pos(&self) -> (Option<ChartBtnPos>, Option<ChartBtnPos>) {
        (self.cancel_buy_pos, self.panic_sell_pos)
    }

    /// Позиции кнопок Cancel Buy / Panic Sell для всех графиков стека (per-окно).
    pub(crate) fn set_action_btn_pos(
        &mut self,
        cancel: Option<ChartBtnPos>,
        panic: Option<ChartBtnPos>,
        cx: &mut Context<Self>,
    ) {
        if self.cancel_buy_pos == cancel && self.panic_sell_pos == panic {
            return;
        }
        self.cancel_buy_pos = cancel;
        self.panic_sell_pos = panic;
        set_panels_action_btn_pos(
            &self.charts,
            cancel.unwrap_or_default(),
            panic.unwrap_or_default(),
            cx,
        );
        cx.notify();
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
    pub(crate) fn set_orientation(
        &mut self,
        orientation: Option<StackOrientation>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_orientation == orientation {
            return;
        }
        self.layout_orientation = orientation;
        self.sync_compare(cx);
        cx.notify();
    }

    pub(crate) fn active_target(&self, cx: &App) -> Option<(CoreId, String)> {
        self.active
            .and_then(|ix| self.charts.get(ix))
            .and_then(|entry| entry.panel.read(cx).active_target())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_data_handle(&self, cx: &App) -> Option<crate::chartdx::ChartDataHandle> {
        self.active
            .and_then(|ix| self.charts.get(ix))
            .map(|entry| entry.panel.read(cx).debug_data_handle())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_fill_history_to_capacity(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.active else {
            log::warn!("debug fill main chart: no active main chart");
            return false;
        };
        let Some(entry) = self.charts.get(ix) else {
            log::warn!("debug fill main chart: active main chart index is stale");
            return false;
        };
        entry
            .panel
            .update(cx, |panel, pcx| panel.debug_fill_history_to_capacity(pcx))
    }

    pub(super) fn set_scene_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible {
            self.sync_visibility(cx);
        } else {
            for entry in &self.charts {
                entry.panel.update(cx, |panel, _| {
                    panel.set_main_stack_scroll(false);
                    panel.set_scene_visible(false);
                });
            }
        }
    }

    fn sync_visibility(&mut self, cx: &mut Context<Self>) {
        for (ix, entry) in self.charts.iter().enumerate() {
            // Fullscreen: ровно активный график видим. Stack: конкретные видимые tiles
            // сами выставят visible=true в `ChartPanel::render`; offscreen элементы
            // виртуального списка остаются false и не гоняют prepare.
            let visible = !self.show_stack && Some(ix) == self.active;
            let stack_scroll = self.show_stack;
            entry.panel.update(cx, |panel, _| {
                panel.set_main_stack_scroll(stack_scroll);
                panel.set_scene_visible(visible);
            });
        }
    }

    fn sync_backend_active(&self, cx: &mut Context<Self>) {
        let target = self.active_target(cx);
        // Все монеты стека Main (без пустых держащихся слотов) — «Ордера» подсветят по одной строке.
        let open: Vec<(CoreId, String)> = self
            .charts
            .iter()
            .filter(|e| !e.vacated)
            .map(|e| (e.core, e.market.clone()))
            .collect();
        self.backend.update(cx, |b, _| {
            b.set_main_chart_target(&self.group, target);
            b.set_main_open_markets(&self.group, open);
        });
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            if let Some(handle) = self.debug_data_handle(cx) {
                self.backend.update(cx, |b, _| {
                    b.register_debug_main_chart(self.group.clone(), handle);
                });
            }
        }
    }

    fn toggle_from_chart(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.charts.len() {
            return;
        }
        self.active = Some(ix);
        self.show_stack = !self.show_stack;
        self.sync_visibility(cx);
        self.sync_backend_active(cx);
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn render_tile(
        &self,
        ix: usize,
        panel: Entity<ChartPanel>,
        size: Option<f32>,
        flex: bool,
        min_w: Option<f32>,
        horizontal: bool,
        border: Rgba,
        entity: Entity<Self>,
        palette: MoonPalette,
        stack_card: bool,
    ) -> Stateful<Div> {
        let panel_for_event = panel.clone();
        let label = self
            .charts
            .get(ix)
            .map(|e| e.market.clone())
            .unwrap_or_else(|| "Chart".to_string());
        let mut tile = if stack_card {
            chart_stack_card(
                SharedString::from(format!("main-chart-stack-tile-{ix}")),
                label,
                panel,
                palette,
                border,
            )
        } else {
            div()
                .id(("main-chart-stack-tile", ix))
                .w_full()
                .relative()
                .overflow_hidden()
                .border_1()
                .border_color(border)
                .child(div().size_full().relative().overflow_hidden().child(panel))
        }
        .on_mouse_up(
            MouseButton::Right,
            move |event: &MouseUpEvent, _window, app| {
                // Возврат из фулскрина — короткий ПКМ по области панели, включая стакан.
                // RMB-drag цены остаётся зумом/scale-жестом и не переключает stack.
                let panel = panel_for_event.read(app);
                if panel.window_pos_allows_main_stack_toggle(event.position)
                    && !panel.rmb_was_moved()
                {
                    entity.update(app, |this, cx| this.toggle_from_chart(ix, cx));
                    app.stop_propagation();
                }
            },
        );
        // Поперёк оси — на всю ширину/высоту; вдоль оси — flex+cap (COMPRESS до size, сжатие),
        // фикс (size без flex) или растяжение (FIT). Гор: ось = X (ширина), верт: ось = Y.
        tile = if horizontal {
            tile.h_full()
        } else {
            tile.w_full()
        };
        if flex {
            tile = tile.flex_1();
            let m = min_w.unwrap_or(0.0);
            tile = if horizontal {
                tile.min_w(px(m))
            } else {
                tile.min_h(px(m))
            };
            if let Some(v) = size {
                tile = if horizontal {
                    tile.max_w(px(v))
                } else {
                    tile.max_h(px(v))
                };
            }
        } else if let Some(v) = size {
            // Фикс. БЕЗ сжатия (min=max=v): в SCROLL тайлы переполняют контейнер → есть скролл.
            tile = if horizontal {
                tile.w(px(v)).min_w(px(v))
            } else {
                tile.h(px(v)).min_h(px(v))
            };
        }
        tile
    }
}

impl Render for MainChartStack {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Запустить (если надо) таймер авто-закрытия по неактивности — идемпотентно, дёшево.
        self.arm_idle_timer(cx);
        let palette = moon_ui::MoonPalette::active(cx);
        if self.charts.is_empty() {
            return div()
                .relative()
                .size_full()
                .bg(rgb(palette.chart_bg))
                .flex()
                .items_center()
                .justify_center()
                .child(crate::design::logo_glow_sized(cx, 220.0))
                .into_any_element();
        }

        let active = self.active.unwrap_or(0).min(self.charts.len() - 1);
        if !self.show_stack {
            let panel = self.charts[active].panel.clone();
            let entity = cx.entity();
            return self
                .render_tile(
                    active,
                    panel,
                    None,
                    false,
                    None,
                    false,
                    rgb(palette.border),
                    entity,
                    palette,
                    false,
                )
                .size_full()
                .border_0()
                .into_any_element();
        }

        // Stack: per-tab раскладка (FIT/SCROLL/COMPRESS + высота), иначе глобальный дефолт.
        let (scroll, compress, cfg_h) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        let count = self.charts.len();
        let border = rgb(palette.border);
        let base_id = format!("main-chart-stack-{}", self.group);
        let horizontal = self
            .layout_orientation
            .unwrap_or(StackOrientation::Vertical)
            .is_horizontal();
        let entity = cx.entity();
        let p = palette;
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
            |s, ix| s.charts.get(ix).map(|e| e.panel.clone()),
            move |s, ix, panel, size, flex, min_w, horizontal, border, ent| {
                s.render_tile(
                    ix, panel, size, flex, min_w, horizontal, border, ent, p, true,
                )
                .into_any_element()
            },
            |s, ix| s.compare_role(ix),
        )
    }
}
