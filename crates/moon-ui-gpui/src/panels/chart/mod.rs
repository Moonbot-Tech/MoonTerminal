//! Панель чарта (center DockArea): НАШ own-pass DX11 рендер (через generic-хук gpui) +
//! ввод. Как Dock-панель — отцепляется в окно. Монета — из
//! focus и `Backend.open_request`.
//!
//! Рендер: `ChartEngine.canvas()` отдаёт GPUI `gpu_canvas` ПОД сценой (рисует combo/слои в
//! backbuffer GPUI без readback), `prepare` обновляет вид и заливает новые тики.
//! Текст осей/readout — retained gpu_canvas text; линии перекрестия — native chartdx cursor layer.
//!
//! По функционалу разнесено: состояние/жизненный цикл/конструкторы/трейты — здесь;
//! геометрия и хит-тест — [`geom`]; рефкаунт рынков и таймеры TTL/auto-live — [`refs`];
//! ручная торговля (ордера/drag) — [`trade`]; `impl Render` — [`render`].

mod geom;
mod refs;
mod render;
mod trade;

use std::collections::HashSet;
use std::time::Instant;

use gpui::*;
use moon_ui::{MoonBackgroundPolicy, Panel, PanelEvent};

use rust_i18n::t;

use crate::chartdx::ChartEngine;
use crate::{Backend, input};
use moon_chart::container::ContainerKind;
use moon_chart::paint::now_unix_ms;
use moon_core::config::{ChartBucket, ChartTheme, OrdersStyleSet};
use moon_core::session::CoreId;

use trade::{OrderDrag, OrderHoverKey};

#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(windows)]
fn monitor_refresh_hz() -> u32 {
    unsafe {
        let mut mode = DEVMODEW::default();
        mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        if EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut mode).as_bool()
            && mode.dmDisplayFrequency > 1
        {
            mode.dmDisplayFrequency
        } else {
            60
        }
    }
}

#[cfg(not(windows))]
fn monitor_refresh_hz() -> u32 {
    60
}

fn chart_bootstrap_present_rate_hz() -> f32 {
    let refresh = monitor_refresh_hz().clamp(30, 360);
    refresh as f32
}

const DEBUG_HISTORY_FILL_SPAN_MS: i64 = 3_600_000;

#[derive(Clone, PartialEq)]
struct ChartSettingsSig {
    theme: ChartTheme,
    orders: OrdersStyleSet,
    follow: bool,
}

fn chart_settings_sig(backend: &Backend) -> ChartSettingsSig {
    let effective = backend.preview.as_ref().unwrap_or(&backend.config);
    ChartSettingsSig {
        theme: effective.theme.clone(),
        orders: effective.orders.clone(),
        follow: backend.follow,
    }
}

pub struct ChartPanel {
    backend: Entity<Backend>,
    chart: ChartEngine,
    input: input::ChartInput,
    market: Option<String>,
    /// Масштаб цены ЭТОЙ вкладки (None = Авто). Теперь ПО-ВКЛАДОЧНЫЙ (не глобальный): правится
    /// своим регулятором (тулбар активной вкладки / шапка выносного окна), применяется в render.
    scale: Option<f32>,
    /// Показывать ли стакан на графиках этой панели (per-окно, из настроек вкладки). Применяется
    /// в render (`set_orderbook_enabled` движка). Дефолт — вкл.
    orderbook_enabled: bool,
    /// Показывать ли тусклую заливку зоны управления при раздельных зонах и СКРЫТОМ стакане
    /// (per-окно/вкладка, из настроек попапа ⚙). Применяется в render. Дефолт — вкл.
    show_zone: bool,
    /// Авто-пин графика при выставлении ордера лонг/шорт (per-окно/вкладка). Дефолт — выкл.
    auto_pin: bool,
    /// Позиции кнопок рыночных действий в зоне чарта (per-окно/вкладка, из попапа ⚙). Дефолт — Right.
    cancel_buy_pos: crate::chart_persist::ChartBtnPos,
    panic_sell_pos: crate::chart_persist::ChartBtnPos,
    /// Положение оси цен (Left/Right/Hide) этой панели (per-окно/вкладка, из попапа ⚙). Применяется
    /// в render (`set_price_axis_pos` движка) и в раскладке/хит-тесте. Дефолт — Left.
    price_axis_pos: crate::chart_persist::PriceAxisPos,
    /// Видна ли ось времени (per-окно/вкладка, из попапа ⚙). Применяется в render
    /// (`set_time_axis_visible` движка) и в раскладке/хит-тесте (высота плота). Дефолт — вкл.
    time_axis_visible: bool,
    /// Показывать подписи у линий ордеров (per-окно/вкладка, попап ⚙). Дефолт — вкл.
    line_labels: bool,
    /// Показывать подписи у перекрестия (курсорный ридаут). Дефолт — вкл.
    cursor_labels: bool,
    /// Номер AddToChart-вкладки (None = Main).
    num: Option<u32>,
    /// Рынки, владельцем которых является именно эта chart panel. Backend держит refcount
    /// по всем панелям и строит `desired` из него.
    registered_markets: HashSet<(CoreId, String)>,
    /// Рынки, по которым эта панель держит orderbook-ref в backend (= registered_markets, когда
    /// стакан включён; пусто, когда выключен). Backend по ним строит `desired_orderbook`.
    registered_orderbook: HashSet<(CoreId, String)>,
    /// Поколение backend registry, в котором были взяты `registered_markets`.
    /// Structural rebuild сбрасывает registry целиком и bump-ит epoch; старые панели после
    /// этого не должны release-ить refs свежих панелей.
    market_ref_epoch: u64,
    /// Сигнатура рыночных данных прошлого кадра — нотифаим только при реальном приходе данных.
    data_sig: u64,
    /// UI-настройки, которые применяются в render. После включения cached dock-панелей
    /// top-down Shell render больше не будит ChartPanel, поэтому изменения должны нотифаить
    /// саму панель.
    settings_sig: ChartSettingsSig,
    /// FastChart: true → плавный кадр по vsync (фокусный чарт); false → адаптивно
    /// (по приходу данных через observe). Main=true, AddToChart=false.
    fast: bool,
    /// Панель реально присутствует в GPUI scene этого окна. Скрытые вкладки не должны гонять
    /// CPU prepare по data observe: их `gpu_canvas` всё равно не будет опрошен/нарисован.
    scene_visible: bool,
    /// Панель сейчас является плиткой Main stack. В этом режиме wheel принадлежит внешнему
    /// ScrollBox-у; fullscreen и AddToChart сохраняют обычный chart zoom.
    main_stack_scroll: bool,
    last_axis_notify_data_sig: u64,
    /// Режим сравнения доступен (вкладка горизонтальная) → показываем кнопку-замок.
    compare_eligible: bool,
    /// Этот чарт — якорь сравнения (замок горит, цена ведущая).
    is_compare_anchor: bool,
    /// Пользователь кликнул замок — стек заберёт запрос в своём observe (как пин, но наружу).
    compare_lock_pending: bool,
    /// Навязанное Y-окно `(center, range)` от якоря (lock сравнения). None = свободный Y.
    /// Применяется в render через `set_locked_y` движка.
    locked_y: Option<(f32, f32)>,
    /// Режим «только стакан» (кнопка-метла у соседей якоря): чарт+ось цен скрыты, виден стакан.
    orderbook_only: bool,
    /// Кликнули метлу — стек заберёт запрос в observe (переключает режим для соседей).
    compare_broom_pending: bool,
    /// Режим метлы включён на вкладке (для подсветки кнопки-метлы на якоре). Ставит стек.
    compare_broom_on: bool,
    view_dirty: bool,
    last_adaptive_notify_ms: f64,
    /// Последний scale_factor окна (ставится в render). Нужен data prepare path, у которого
    /// нет window — DPI меняется редко, между сменами берём запомненный.
    last_ppp: f32,
    /// One-shot timer до ближайшего истечения AddToChart TTL. Это time-based dirty,
    /// поэтому он не должен зависеть от backend data observe.
    ttl_timer_armed: bool,
    /// One-shot timer авто-возврата в live после пана (П.9). Тоже time-based: prepare в
    /// покое не тикает (камеру двигает own-pass), поэтому возврат нужен по таймеру.
    auto_live_timer_armed: bool,
    order_drag: Option<OrderDrag>,
    order_hover: Option<OrderHoverKey>,
    /// Момент последнего закрытия pane крестиком (×). Быстрое закрытие нескольких графиков
    /// подряд создаёт у GPUI двойной клик на том же экранном месте; после того как график
    /// уезжает, второй клик попадал бы на стакан и засчитывался как дабл-клик → ордер. Гасим
    /// постановку ордера на ~600мс после закрытия. См. `try_place_order_click`.
    last_pane_close: Option<Instant>,
    focus: FocusHandle,
}

impl ChartPanel {
    pub fn new(
        backend: Entity<Backend>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_main(backend, focus_open, epoch, theme, cx)
    }

    pub fn new_main(
        backend: Entity<Backend>,
        focus_open: Option<(CoreId, String)>,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut chart = ChartEngine::new(epoch, theme);
        chart.set_market_source(Some(backend.read(cx).session.market_source()));
        let market_ref_epoch = backend.read(cx).chart_market_refs_epoch;
        let mut market = None;
        let mut registered_markets = HashSet::new();
        let mut registered_orderbook = HashSet::new();
        if let Some((core, m)) = focus_open {
            chart.open(core, &m);
            market = Some(m.clone());
            registered_markets.insert((core, m.clone()));
            // Стакан по умолчанию вкл → сразу держим и его ref (Stage 2 подписка).
            registered_orderbook.insert((core, m.clone()));
            backend.update(cx, |b, _| {
                b.retain_chart_market(core, &m);
                b.retain_chart_orderbook(core, &m);
            });
        }
        let settings_sig = {
            let b = backend.read(cx);
            chart_settings_sig(&b)
        };
        // UI notify при изменении настроек/редкого текста осей. Частые рыночные данные не
        // идут через notify: gpu_canvas.frame() подтягивает MarketDataSource напрямую.
        // Time-based TTL панелей обслуживает локальный one-shot timer, не backend data observe.
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::CHART_OBS_FIRE);
            let now = now_unix_ms();
            let (sig, settings_sig) = {
                let b = backend.read(cx);
                (
                    this.chart.notify_signature(&b.session),
                    chart_settings_sig(&b),
                )
            };
            if settings_sig != this.settings_sig {
                this.settings_sig = settings_sig;
                this.view_dirty = true;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            this.data_sig = sig;
            // Троттл notify. Данные gpu_canvas рисует сам по present (форк), notify нужен лишь
            // для GPUI-оверлея осей, а он идёт top-down → дёргает Orders. Поэтому ≤4 Гц для
            // fast (≥250мс) и ≤1 Гц для addto. Частые GPU data/state обновляет
            // gpu_canvas.frame() без GPUI dirty; notify здесь только для редкого текста осей.
            let floor = if this.fast { 250.0 } else { 1000.0 };
            if sig != this.last_axis_notify_data_sig && now - this.last_adaptive_notify_ms >= floor
            {
                this.last_axis_notify_data_sig = sig;
                this.last_adaptive_notify_ms = now;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();
        let chart_handle = chart.data_handle();
        backend.update(cx, |b, _| b.register_chart_consumer(chart_handle));
        cx.on_release(|this, cx| {
            this.release_all_market_refs(cx);
        })
        .detach();
        Self {
            backend,
            chart,
            input: input::ChartInput::default(),
            market,
            scale: None,
            orderbook_enabled: true,
            show_zone: true,
            auto_pin: false,
            cancel_buy_pos: Default::default(),
            panic_sell_pos: Default::default(),
            price_axis_pos: Default::default(),
            time_axis_visible: true,
            line_labels: true,
            cursor_labels: true,
            num: None,
            registered_markets,
            registered_orderbook,
            market_ref_epoch,
            data_sig: 0,
            settings_sig,
            fast: true,
            scene_visible: false,
            main_stack_scroll: false,
            last_axis_notify_data_sig: u64::MAX,
            compare_eligible: false,
            is_compare_anchor: false,
            compare_lock_pending: false,
            locked_y: None,
            orderbook_only: false,
            compare_broom_pending: false,
            compare_broom_on: false,
            view_dirty: true,
            last_adaptive_notify_ms: 0.0,
            last_ppp: 1.0,
            ttl_timer_armed: false,
            auto_live_timer_armed: false,
            order_drag: None,
            order_hover: None,
            last_pane_close: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn active_target(&self) -> Option<(CoreId, String)> {
        self.chart.active_target()
    }

    /// AddToChart-вкладка №`num` (наполняется детектами через add_coin). Без `window`: панель
    /// строится из данных, окно ей не нужно (важно для отложенного восстановления откреп-окон).
    pub fn new_addto(
        backend: Entity<Backend>,
        num: u32,
        bucket: ChartBucket,
        epoch: f64,
        theme: ChartTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut chart = ChartEngine::new_kind(epoch, theme, ContainerKind::Chart { num, bucket });
        chart.set_market_source(Some(backend.read(cx).session.market_source()));
        let market_ref_epoch = backend.read(cx).chart_market_refs_epoch;
        let settings_sig = {
            let b = backend.read(cx);
            chart_settings_sig(&b)
        };
        cx.observe(&backend, |this, backend, cx| {
            let now = now_unix_ms();
            let (sig, settings_sig) = {
                let b = backend.read(cx);
                (
                    this.chart.notify_signature(&b.session),
                    chart_settings_sig(&b),
                )
            };
            if settings_sig != this.settings_sig {
                this.settings_sig = settings_sig;
                this.view_dirty = true;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
            this.data_sig = sig;
            // AddToChart — фоновый график: notify (а с ним top-down перерисовка Orders)
            // ≤1 Гц. Частые GPU data/state обновляет gpu_canvas.frame() без notify;
            // time-based prune делает локальный TTL timer.
            if sig != this.last_axis_notify_data_sig && now - this.last_adaptive_notify_ms >= 1000.0
            {
                this.last_axis_notify_data_sig = sig;
                this.last_adaptive_notify_ms = now;
                crate::diag::bump(&crate::diag::CHART_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();
        let chart_handle = chart.data_handle();
        backend.update(cx, |b, _| b.register_chart_consumer(chart_handle));
        cx.on_release(|this, cx| {
            this.release_all_market_refs(cx);
        })
        .detach();
        Self {
            backend,
            chart,
            input: input::ChartInput::default(),
            market: None,
            scale: None,
            orderbook_enabled: true,
            show_zone: true,
            auto_pin: false,
            cancel_buy_pos: Default::default(),
            panic_sell_pos: Default::default(),
            price_axis_pos: Default::default(),
            time_axis_visible: true,
            line_labels: true,
            cursor_labels: true,
            num: Some(num),
            registered_markets: HashSet::new(),
            registered_orderbook: HashSet::new(),
            market_ref_epoch,
            data_sig: 0,
            settings_sig,
            fast: false,
            scene_visible: false,
            main_stack_scroll: false,
            last_axis_notify_data_sig: u64::MAX,
            compare_eligible: false,
            is_compare_anchor: false,
            compare_lock_pending: false,
            locked_y: None,
            orderbook_only: false,
            compare_broom_pending: false,
            compare_broom_on: false,
            view_dirty: true,
            last_adaptive_notify_ms: 0.0,
            last_ppp: 1.0,
            ttl_timer_armed: false,
            auto_live_timer_armed: false,
            order_drag: None,
            order_hover: None,
            last_pane_close: None,
            focus: cx.focus_handle(),
        }
    }

    /// Число открытых панелей чарта (для бейджа-счётчика на вкладке).
    pub fn pane_count(&self) -> usize {
        self.chart.pane_count()
    }

    /// Закреплён ли хоть один график панели (●). Стек сортирует запиненные наверх; пин также
    /// защищает график от TTL (`prune_ttl` пропускает pinned).
    pub fn is_pinned(&self) -> bool {
        (0..self.chart.pane_count()).any(|i| self.chart.pane_pinned(i))
    }

    /// Идемпотентно закрепить все панели чарта (кастомная вкладка: чарты сразу запинены).
    /// Пин отменяет авто-закрытие по TTL → пере-арм таймера дедлайнов.
    pub fn ensure_pinned(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for i in 0..self.chart.pane_count() {
            if !self.chart.pane_pinned(i) && self.chart.toggle_pane_pin(i) {
                changed = true;
            }
        }
        if changed {
            self.view_dirty = true;
            self.arm_ttl_timer(cx);
            cx.notify();
        }
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub fn debug_data_handle(&self) -> crate::chartdx::ChartDataHandle {
        self.chart.data_handle()
    }

    /// Mark whether this panel is part of the currently rendered GPUI scene. This only gates
    /// CPU-side data prepare; the `gpu_canvas` element lifetime is still owned by GPUI scene replay.
    pub fn set_scene_visible(&mut self, visible: bool) {
        self.scene_visible = visible;
        self.chart.set_scene_visible(visible);
    }

    /// Main stack mode scrolls the list of chart tiles. The chart itself must not consume wheel
    /// events there, otherwise the outer ScrollBox cannot move.
    pub fn set_main_stack_scroll(&mut self, enabled: bool) {
        self.main_stack_scroll = enabled;
    }

    pub(crate) fn sync_orders_if_visible(&mut self, cx: &mut Context<Self>, force: bool) {
        if !self.scene_visible {
            return;
        }
        let b = self.backend.read(cx);
        self.data_sig = self.chart.notify_signature(&b.session);
        self.chart.sync_orders_if_visible(&b.session, force);
    }

    /// Поставить масштаб ЭТОЙ вкладки (None=Авто). Применяется в render через `set_scale` движка.
    pub fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        if self.scale != pct {
            self.scale = pct;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Вкл/выкл стакан (per-окно). Применяется в render через `set_orderbook_enabled` движка;
    /// плюс синхронизирует orderbook-ref backend (Stage 2: подписка по спросу).
    pub fn set_orderbook_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.orderbook_enabled != enabled {
            self.orderbook_enabled = enabled;
            self.view_dirty = true;
            self.sync_orderbook_refs(cx);
            cx.notify();
        }
    }

    /// Показывать ли заливку зоны управления при скрытом стакане (per-окно). Только UI-оверлей,
    /// движок не трогаем.
    pub fn set_show_zone(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_zone != show {
            self.show_zone = show;
            cx.notify();
        }
    }

    /// Авто-пин графика при выставлении ордера (per-окно). Только флаг; пин делается в
    /// `try_place_order_click` при успешном ордере.
    pub fn set_auto_pin(&mut self, on: bool, _cx: &mut Context<Self>) {
        self.auto_pin = on;
    }

    /// Позиции кнопок рыночных действий (Cancel Buy / Panic Sell) в зоне чарта (per-окно).
    pub fn set_action_btn_pos(
        &mut self,
        cancel_buy: crate::chart_persist::ChartBtnPos,
        panic_sell: crate::chart_persist::ChartBtnPos,
        cx: &mut Context<Self>,
    ) {
        if self.cancel_buy_pos != cancel_buy || self.panic_sell_pos != panic_sell {
            self.cancel_buy_pos = cancel_buy;
            self.panic_sell_pos = panic_sell;
            cx.notify();
        }
    }

    /// Доступность режима сравнения (вкладка горизонтальная) — показывать ли кнопку-замок.
    pub fn set_compare_eligible(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.compare_eligible != on {
            self.compare_eligible = on;
            cx.notify();
        }
    }

    /// Пометить этот чарт якорем сравнения (замок горит). Управляет стек.
    pub fn set_compare_anchor(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.is_compare_anchor != on {
            self.is_compare_anchor = on;
            cx.notify();
        }
    }

    /// Клик по замку → выставить запрос и уведомить (стек заберёт его в observe).
    fn request_compare_lock(&mut self, cx: &mut Context<Self>) {
        self.compare_lock_pending = true;
        cx.notify();
    }

    /// Забрать флаг «кликнули замок» (стек дёргает в своём observe). Сбрасывает его.
    pub fn take_compare_lock_request(&mut self) -> bool {
        std::mem::take(&mut self.compare_lock_pending)
    }

    /// Клик по метле → выставить запрос (стек заберёт в observe, переключит режим у соседей).
    fn request_compare_broom(&mut self, cx: &mut Context<Self>) {
        self.compare_broom_pending = true;
        cx.notify();
    }

    /// Забрать флаг «кликнули метлу». Сбрасывает его.
    pub fn take_compare_broom_request(&mut self) -> bool {
        std::mem::take(&mut self.compare_broom_pending)
    }

    /// Режим «только стакан» (метла): чарт+ось цен скрыты, стакан на всю ширину. Применяется в render.
    pub fn set_orderbook_only(&mut self, only: bool, cx: &mut Context<Self>) {
        if self.orderbook_only != only {
            self.orderbook_only = only;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Положение оси цен (Left/Right/Hide) этой панели (per-окно/вкладка). Применяется в render
    /// через `set_price_axis_pos` движка; влияет и на раскладку плот/стакан/жёлоб, и на хит-тест.
    pub fn set_price_axis_pos(
        &mut self,
        pos: crate::chart_persist::PriceAxisPos,
        cx: &mut Context<Self>,
    ) {
        if self.price_axis_pos != pos {
            self.price_axis_pos = pos;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Видимость оси времени этой панели (per-окно/вкладка). Применяется в render через
    /// `set_time_axis_visible` движка; влияет и на высоту плота (раскладка/хит-тест).
    pub fn set_time_axis_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.time_axis_visible != visible {
            self.time_axis_visible = visible;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Показывать подписи у линий ордеров (per-окно/вкладка). Применяется в render через
    /// `set_line_labels` движка. На раскладку не влияет (только видимость текста).
    pub fn set_line_labels(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.line_labels != show {
            self.line_labels = show;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Показывать подписи у перекрестия (курсорный ридаут). Применяется в render через
    /// `set_cursor_labels` движка.
    pub fn set_cursor_labels(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.cursor_labels != show {
            self.cursor_labels = show;
            self.view_dirty = true;
            cx.notify();
        }
    }

    /// Подсветка кнопки-метлы на якоре (режим метлы включён на вкладке). Ставит стек.
    pub fn set_compare_broom_on(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.compare_broom_on != on {
            self.compare_broom_on = on;
            cx.notify();
        }
    }

    /// Текущее Y-окно `(center, range)` (для стека — окно якоря). None если нет панелей.
    pub fn y_window(&self) -> Option<(f32, f32)> {
        self.chart.y_window()
    }

    /// Навязать/снять lock Y-окна от якоря сравнения. Установка применяется в render каждый кадр;
    /// снятие (None) одноразово возвращает Y-режим вкладки (масштаб/авто).
    pub fn set_locked_y(&mut self, window: Option<(f32, f32)>, cx: &mut Context<Self>) {
        if self.locked_y == window {
            return;
        }
        let exiting = window.is_none();
        self.locked_y = window;
        if exiting {
            // Выходим из сравнения → вернуть масштаб вкладки (или авто), минуя кэш движка.
            self.chart.reapply_scale(self.scale);
        }
        self.view_dirty = true;
        cx.notify();
    }

    /// AddToChart: открыть/продлить монету в этой панели с TTL.
    pub fn add_coin(&mut self, core: CoreId, market: &str, ttl_ms: f64, cx: &mut Context<Self>) {
        self.release_market_refs_except(Some((core, market)), cx);
        self.chart.push_auto(core, market, ttl_ms, now_unix_ms());
        self.retain_market_ref(core, market, cx);
        self.view_dirty = true;
        self.arm_ttl_timer(cx);
        // ВАЖНО: notify самой панели. Для вкладки в стрипе её перерисовывает render ChartTabs,
        // но ОТКРЕПЛЁННАЯ панель живёт в своём окне — без notify оно не перерисуется и новая
        // монета не появится (баг «детект пришёл, а графика в откреп-окне нет»).
        cx.notify();
    }

    /// Закрыть панель-монету крестиком: убрать график + отписаться от стакана
    /// (убрать (core, market) из `desired`, если ни одна оставшаяся панель этого чарта его не
    /// держит — трейды биржи идут оптом, снимаем именно стакан через `set_open`-дифф).
    fn remove_pane(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some((core, market)) = self.chart.remove_pane(idx) else {
            return;
        };
        // Гасим постановку ордера сразу после закрытия (защита от дабл-клика по стакану при
        // быстром закрытии нескольких графиков подряд — кнопка × уезжает, второй клик попал бы
        // на стакан).
        self.last_pane_close = Some(Instant::now());
        self.view_dirty = true;
        if !self.chart.uses_market(core, &market) {
            self.release_market_ref(core, &market, cx);
        }
        cx.notify();
    }

    /// П.2: приколоть/открепить панель idx. Пин отменяет авто-закрытие по TTL; открепление
    /// возвращает TTL (панель закроется, если срок уже истёк) → пере-арм таймера дедлайнов.
    fn toggle_pin(&mut self, idx: usize, cx: &mut Context<Self>) {
        if self.chart.toggle_pane_pin(idx) {
            self.view_dirty = true;
            self.arm_ttl_timer(cx);
            cx.notify();
        }
    }

    /// Закрыть ВСЕ монеты этого чарта (кнопка «закрыть все графики» в выносном окне) +
    /// отписаться от их стаканов.
    pub fn close_all_panes(&mut self, cx: &mut Context<Self>) {
        let removed = self.chart.clear_panes();
        if removed.is_empty() {
            return;
        }
        self.view_dirty = true;
        for (core, market) in removed {
            self.release_market_ref(core, &market, cx);
        }
        cx.notify();
    }

    fn mark_input_changed(&mut self, cx: &mut Context<Self>) {
        self.chart.sync_follow_from_views();
        let follow = self.chart.follow();
        self.backend.update(cx, |b, bcx| {
            if b.follow != follow {
                b.follow = follow;
                bcx.notify();
            }
        });
        self.view_dirty = true;
        // Если пан перевёл панель в ручной режим — заводим таймер авто-возврата (П.9).
        self.arm_auto_live_timer(cx);
    }

    /// Подпись вкладки: для AddToChart — «N · рынок» (П.4: вместо безликого «Чарт N»),
    /// иначе рынок открытой монеты, затем «Main». Группа/ядро тут недоступны (их знают
    /// ChartTabs/DetachedChartHost) — используем номер + активный рынок.
    pub fn title_text(&self) -> String {
        let market = self
            .chart
            .active_market()
            .filter(|m| !m.is_empty())
            .or_else(|| self.market.clone());
        if let Some(n) = self.num {
            return match market {
                Some(m) => format!("{n} · {m}"),
                None => t!("chartwin.tab_title", n = n).to_string(),
            };
        }
        market.unwrap_or_else(|| "Main".into())
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub fn debug_fill_history_to_capacity(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((core, market)) = self.chart.active_target() else {
            log::warn!("debug history fill: current chart has no active market");
            return false;
        };
        let now_ms = now_unix_ms();
        log::info!(
            "debug history fill: requesting core={core} market={market} span_ms={DEBUG_HISTORY_FILL_SPAN_MS}"
        );
        let filled = self
            .backend
            .read(cx)
            .session
            .diag_fill_market_history_to_capacity(
                core,
                &market,
                now_ms.round() as i64,
                DEBUG_HISTORY_FILL_SPAN_MS,
            );
        if !filled {
            log::warn!("debug history fill: failed core={core} market={market}");
            return false;
        }
        self.chart.force_history_reupload();
        self.view_dirty = true;
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
        log::info!("debug history fill: force reupload core={core} market={market}");
        true
    }
}

impl EventEmitter<PanelEvent> for ChartPanel {}
impl Focusable for ChartPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ChartPanel {
    fn panel_name(&self) -> &'static str {
        "Chart"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.title_text())
    }
    fn background_policy(&self, _cx: &App) -> MoonBackgroundPolicy {
        MoonBackgroundPolicy::NoFill
    }
}
