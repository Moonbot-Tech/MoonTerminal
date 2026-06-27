//! Native `gpu_canvas` рендер чарта (замена wgpu-offscreen+readback). Слои по природе данных
//! chartdx own-pass renderer: Combo (рыночная история) / OrderBook (срез) /
//! UserData (мутирующее юзерское) + хром (Grid/Background) + native cursor/readout;
//! статичный текст осей — в GPUI.
//!
//! Доменная специфика чарта живёт ЗДЕСЬ (в терминале); форк gpui отдаёт только generic-хук
//! `RawGpuAccess`. Файл на слой; здесь — оркестратор `ChartEngine`: prepare данных per pane
//! (БЕЗ рисования) + `gpu_canvas` element, который и рисует в кадре GPUI.

mod backend;
#[cfg(windows)]
pub mod background;
#[cfg(windows)]
mod base;
// Оркестратор движка, вынесенный из этого файла (impl-блоки; структуры объявлены ниже).
// Дочерние модули видят приватные поля структур-предка — логика не менялась, только переезд.
#[cfg(windows)]
pub mod combo;
#[cfg(windows)]
pub mod cursor;
mod data_state;
mod engine;
#[cfg(windows)]
pub mod gpu;
#[cfg(windows)]
pub mod grid;
#[cfg(target_os = "macos")]
mod metal_backend;
#[cfg(windows)]
pub mod orderbook;
pub mod pane;
#[cfg(windows)]
pub mod readout;
mod render_state;
mod text;
pub mod types;
#[cfg(windows)]
pub mod userdata;
pub mod view;
#[cfg(target_os = "linux")]
mod wgpu_backend;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    Bounds, GpuBackend, GpuCanvasDriver, GpuCanvasHandle, GpuCanvasRetainedTextLayer,
    GpuCanvasTextContext, GpuCanvasTextRun, GpuCanvasTextTransform, GpuFrameDecision, GpuFrameInfo,
    Pixels, RawGpuAccess,
};
use moon_chart::axes::AxisSnapshot;
use moon_chart::paint::now_unix_ms;
use moon_chart::view::Rect;
use moon_core::config::{ChartTheme, OrdersStyle};
use moon_core::data::PriceLinePoint;
use moon_core::market::{ChartHistoryBuffers, ChartHistoryCursor, MarketDataSource};
use moon_core::session::order_lines::LineKind;
use moon_core::session::{CoreId, SessionManager};
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11RasterizerState, ID3D11RenderTargetView,
};

use backend::PlatformLayers;
use pane::{Container, ContainerKind};
use types::{
    BackgroundParams, BookStyle, ChartCross, ChartViewGpu, CursorParams, GridParams, ReadoutRect,
    cover_uv, fill_cross_upload, fill_price_upload, rgb4,
};

const CHART_PHOTO_BACKGROUND_ENABLED: bool = false;

/// Минимальная полу-ширина видимой полосы авто-фокуса вокруг середины стакана, когда
/// трейдов нет (доля от цены). Полоса всегда включает лучшие bid/ask, но не уже ±0.5%
/// → на узком спреде не зумит абсурдно в спред, на широком (HIP-3) показывает обе
/// стороны. Как только пойдут трейды — диапазон ведут тики.
const BOOK_FOCUS_HALF_FRAC: f32 = 0.005;

fn union_range(a: Option<(f32, f32)>, b: Option<(f32, f32)>) -> Option<(f32, f32)> {
    match (a, b) {
        (Some((alo, ahi)), Some((blo, bhi))) => Some((alo.min(blo), ahi.max(bhi))),
        (Some(r), None) | (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

fn chart_market_diag_enabled() -> bool {
    std::env::var_os("MOON_MARKET_DIAG").is_some() || std::env::var_os("MOON_RENDER_DIAG").is_some()
}

fn chart_market_diag_due(key: impl Into<String>) -> bool {
    if !chart_market_diag_enabled() {
        return false;
    }
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let key = key.into();
    let now = Instant::now();
    let mut last = LAST
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("chart market diag lock poisoned");
    match last.get(&key).copied() {
        Some(prev) if now.duration_since(prev) < Duration::from_millis(1000) => false,
        _ => {
            last.insert(key, now);
            true
        }
    }
}

fn chart_market_diag(msg: impl std::fmt::Display) {
    if chart_market_diag_enabled() {
        log::info!("[chart_market_diag] {msg}");
    }
}

fn mix_sig(mut sig: u64, value: u64) -> u64 {
    sig ^= value;
    sig = sig.wrapping_mul(0x100000001b3);
    sig
}

fn str_sig(s: &str) -> u64 {
    let mut sig = 0xcbf29ce484222325;
    for b in s.bytes() {
        sig = mix_sig(sig, b as u64);
    }
    sig
}

#[derive(Clone, Copy, PartialEq)]
struct CursorState {
    pane: usize,
    local: [f32; 2],
}

/// GPU-состояние одной панели для `gpu_canvas` callbacks — отделено от логики `Container`,
/// синхронизируется по индексу + идентичности (core, market) в `prepare`.
struct PaneRender {
    core: Option<CoreId>,
    market: String,
    /// Имя ядра для угловой подписи чарта (резолв из `SessionManager` при синке ордеров).
    /// Тикер подписи выводим из `market` на лету (`symbol::display_pair`), его не храним.
    core_name: String,
    /// Изменённая ширина (лог. px) самой широкой строки угловой подписи — `prepare_text` её
    /// замеряет, `sync_readout_params` строит по ней прозрачную плашку-подложку. 0 = подписи нет.
    caption_w: f32,
    view: ChartViewGpu,
    layers: PlatformLayers,
    background_params: BackgroundParams,
    grid_params: GridParams,
    cursor_params: CursorParams,
    readout_rects: Vec<ReadoutRect>,
    readout_time_width: f32,
    readout_price_width: f32,
    history_cursor: ChartHistoryCursor,
    history_buffers: ChartHistoryBuffers,
    /// Last source slice signature used to decide if retained chart history must be read.
    source_history_sig: u64,
    /// Last provider generation seen by this pane. Changed generation means source replacement.
    source_generation: u64,
    cross_upload: Vec<ChartCross>,
    last_line_upload: Vec<PriceLinePoint>,
    mark_line_upload: Vec<PriceLinePoint>,
    combo_cross_capacity: usize,
    combo_price_line_capacity: usize,
    orderbook_view: ChartViewGpu,
    pane_bounds: [f32; 4],
    book_style: BookStyle,
    resident_left_rel: f32,
    /// Последнее виденное поколение device combo: сменилось (device-lost) → перезалить историю.
    last_device_gen: u64,
    /// Последняя сборка стакана: ревизия данных + видимое ценовое окно.
    last_book_rev: u64,
    last_book_lo: f32,
    last_book_hi: f32,
    /// Последняя ревизия ордеров, по которой залит userdata-буфер.
    last_order_lines_rev: u64,
    /// Локальное время, когда userdata-буфер был пересобран из `order_lines_rev`.
    last_order_lines_sync_ms: f64,
    /// Ревизия order userdata, которая ждёт ближайшего GPU prepare.
    pending_order_gpu_rev: Option<u64>,
    /// Последняя order revision, дошедшая до GPU prepare.
    last_order_gpu_rev: u64,
    /// Локальное время ближайшего GPU prepare для `last_order_gpu_rev`.
    last_order_gpu_ms: f64,
    /// Последняя order revision, реально попавшая в own-pass draw.
    last_order_present_rev: u64,
    /// Локальное время первого draw для `last_order_present_rev`.
    last_order_present_ms: f64,
    /// Последний uid ордера, который был подсвечен при сборке userdata.
    last_order_highlight_uid: Option<u64>,
    /// Последний preview drag, который был зашит в userdata.
    last_order_drag_preview: Option<(u64, LineKind, u32)>,
    /// Камера X для own-pass: эпоха времени, поле справа (доля «будущего»), флаг follow и
    /// последняя КВАНТОВАННАЯ пиксель-позиция правого края. Callback двигает камеру по этим
    /// полям на каждый present (vblank, целопиксельно) — живой скролл без отдельного таймера.
    epoch_ms: f64,
    right_margin_frac: f32,
    follow: bool,
    last_edge_px: i64,
    /// Кэш дорогого авто-Y скана (min/max видимых тиков) + пиксель-позиция камеры, при которой
    /// он валиден. Пересканируем лишь на пиксель-кроссе (рубильник, см. prepare).
    scan_cam_px: i64,
    cached_tick_price: Option<(f32, f32)>,
    cached_last_price: Option<f32>,
    /// Последний диапазон live-ордеров для auto-Y. Обновляется полным session-sync;
    /// market-only frame-sync использует этот кэш, не трогая CoreStore из frame().
    cached_order_price: Option<(f32, f32)>,
    /// Видима в этом кадре (рисуем) — ставится в `prepare`.
    active: bool,
    /// Стакан включён на этой панели (per-окно). Выкл → не рисуем стекло и угловую подпись.
    orderbook_enabled: bool,
    /// Режим «только стакан» этой панели (чарт+ось цен скрыты, стакан на всю ширину).
    orderbook_only: bool,
    /// Положение оси цен (Left/Right/Hide) — определяет, с какой стороны рисуются подписи оси
    /// и где резервируется жёлоб под неё. Применяется ко всем панелям движка.
    price_axis_pos: crate::chart_persist::PriceAxisPos,
    /// Видна ли ось времени (нижние подписи + жёлоб под них). Выкл → подписи времени не
    /// рисуются, плот занимает всю высоту слота. Применяется ко всем панелям движка.
    time_axis_visible: bool,
    /// CPU/base inputs changed and D3D prepare must upload/bake resident resources before draw.
    /// Cursor-only presents leave this false.
    gpu_prepare_dirty: bool,
}

impl PaneRender {
    fn new() -> Self {
        Self {
            core: None,
            market: String::new(),
            core_name: String::new(),
            caption_w: 0.0,
            view: ChartViewGpu::default(),
            layers: PlatformLayers::new(),
            background_params: BackgroundParams::default(),
            grid_params: GridParams::default(),
            cursor_params: CursorParams::default(),
            readout_rects: Vec::new(),
            readout_time_width: 0.0,
            readout_price_width: 0.0,
            history_cursor: ChartHistoryCursor::default(),
            history_buffers: ChartHistoryBuffers::default(),
            source_history_sig: u64::MAX,
            source_generation: u64::MAX,
            cross_upload: Vec::new(),
            last_line_upload: Vec::new(),
            mark_line_upload: Vec::new(),
            combo_cross_capacity: 0,
            combo_price_line_capacity: 0,
            orderbook_view: ChartViewGpu::default(),
            pane_bounds: [0.0, 0.0, 1.0, 1.0],
            book_style: BookStyle::default(),
            resident_left_rel: f32::NAN,
            last_device_gen: 0,
            last_book_rev: u64::MAX,
            last_book_lo: f32::NAN,
            last_book_hi: f32::NAN,
            last_order_lines_rev: u64::MAX,
            last_order_lines_sync_ms: 0.0,
            pending_order_gpu_rev: None,
            last_order_gpu_rev: u64::MAX,
            last_order_gpu_ms: 0.0,
            last_order_present_rev: u64::MAX,
            last_order_present_ms: 0.0,
            last_order_highlight_uid: None,
            last_order_drag_preview: None,
            epoch_ms: 0.0,
            right_margin_frac: 0.10,
            follow: false,
            last_edge_px: i64::MIN,
            scan_cam_px: i64::MIN,
            cached_tick_price: None,
            cached_last_price: None,
            cached_order_price: None,
            active: false,
            orderbook_enabled: true,
            orderbook_only: false,
            price_axis_pos: crate::chart_persist::PriceAxisPos::Left,
            time_axis_visible: true,
            gpu_prepare_dirty: true,
        }
    }

    fn finish_order_gpu_prepare(&mut self, now_ms: f64) {
        if let Some(rev) = self.pending_order_gpu_rev.take() {
            self.last_order_gpu_rev = rev;
            self.last_order_gpu_ms = now_ms;
        }
    }

    fn finish_order_present(&mut self, now_ms: f64) {
        if self.last_order_present_rev != self.last_order_gpu_rev {
            self.last_order_present_rev = self.last_order_gpu_rev;
            self.last_order_present_ms = now_ms;
        }
    }

    /// Пиксельный рубильник камеры (follow по X). Двигаем правый край по `now_ms` ТОЛЬКО
    /// когда «сейчас» уехало на ≥1 ЦЕЛЫЙ пиксель (MoonBot `round(Now/FdtScale)`): между
    /// пикселями кадр попиксельно идентичен → present переказывает его без работы. Целый
    /// шаг убирает субпиксельное дрожание; вызов на каждый present даёт гладкость на vblank.
    /// True — камера реально сдвинулась (для счётчика «рабочих» кадров).
    fn advance_camera(&mut self, now_ms: f64) -> bool {
        if !self.follow || !(self.view.time_to_px > 0.0) {
            return false;
        }
        let ppm = self.view.time_to_px;
        let target_px = ((now_ms - self.epoch_ms) * ppm as f64).round() as i64;
        if target_px == self.last_edge_px {
            return false;
        }
        self.last_edge_px = target_px;
        let inv_ppm = 1.0 / ppm.max(1e-6);
        let area_w = self.view.bounds[2];
        let glass_w = self.orderbook_view.bounds[2];
        let window_ms = area_w * inv_ppm;
        let right_rel = target_px as f32 * inv_ppm;
        self.view.view_time0 = right_rel + window_ms * self.right_margin_frac - window_ms;
        self.view.pad = self.view.view_time0 + (area_w + glass_w) * inv_ppm;
        self.gpu_prepare_dirty = true;
        true
    }
}

/// Состояние рендера всех панелей — шарится с `gpu_canvas` callbacks (`Rc<RefCell>`,
/// единственный поток UI: `prepare` и callbacks кадра не пересекаются по времени).
struct RenderState {
    panes: Vec<PaneRender>,
    /// CPU-side dirty flag для `GpuCanvasDriver::frame`: `prepare()` обновил resident state,
    /// значит следующий platform tick должен презентить кадр даже без GPUI dirty.
    needs_present: bool,
    /// Scene pixels changed since the optional DX11 cursor-restore cache was built.
    /// Live-scroll draws directly and invalidates that cache; cursor-only frames may rebuild it once.
    base_dirty: bool,
    last_present_ms: f64,
    target_present_interval_ms: f64,
    camera_shift_window_start_ms: f64,
    camera_shift_count: u32,
    camera_shift_hz: f32,
    last_gpu_prepare_generation: u64,
    text_runs: Vec<GpuCanvasTextRun>,
    text_run_cursor: usize,
    firetest_text_labels: Vec<String>,
    firetest_text_runs: Vec<GpuCanvasTextRun>,
    firetest_text_layer: GpuCanvasRetainedTextLayer,
    firetest_text_revision: u64,
    firetest_force_present: bool,
    ui_palette: moon_ui::MoonPalette,
    /// Левый верхний угол chart slot в backbuffer. Cursor приходит из UI в локальных
    /// device-px слота, а own-pass рисует в координатах окна.
    slot_origin: [f32; 2],
    cursor: Option<CursorState>,
    cursor_color: [f32; 4],
    cursor_thickness: f32,
    pixel_scale: f32,
    /// Scissor-растеризатор own-pass (lazy, пересоздаётся на смене device): клипует слои к
    /// зоне панели, чтобы стакан/ордера (позиционируются по ЦЕНЕ) не лезли за плот на тулбар/шкалы.
    #[cfg(windows)]
    scissor_rs: Option<ID3D11RasterizerState>,
    #[cfg(windows)]
    scissor_generation: u64,
    /// Полно-оконная тёмная база: рисуется ПЕРВЫМ слоем own-pass на ВЕСЬ backbuffer
    /// (без scissor), чтобы закрыть белый незакрашенный фон GPUI/SwapChain на первом кадре.
    /// Брендовый empty-state логотип рисуется SVG-слоем GPUI, не растровым native splash.
    #[cfg(windows)]
    window_bg: background::BackgroundLayer,
    /// Цвет тёмной базы (= `rgb4(theme.bg)`), обновляется в `prepare`. Заливает ВСЁ окно.
    #[cfg(windows)]
    window_bg_color: [f32; 4],
    #[cfg(windows)]
    base_cache: base::BaseCache,
}

#[derive(Clone)]
pub struct ChartDataHandle {
    inner: Weak<RefCell<ChartDataState>>,
}

#[derive(Clone, Copy, Debug)]
pub struct OrderRenderProbe {
    pub order_lines_rev: u64,
    pub order_lines_sync_ms: f64,
    pub gpu_rev: u64,
    pub gpu_ms: f64,
    pub present_rev: u64,
    pub present_ms: f64,
}

impl PartialEq for ChartDataHandle {
    fn eq(&self, other: &Self) -> bool {
        self.inner.ptr_eq(&other.inner)
    }
}

impl ChartDataHandle {
    pub fn is_alive(&self) -> bool {
        self.inner.strong_count() > 0
    }

    pub fn sync_orders_if_visible(&self, session: &SessionManager, force: bool) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        inner.borrow_mut().sync_orders_if_visible(session, force)
    }

    pub fn set_firetest_text_labels(&self, count: usize) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        let mut data = inner.borrow_mut();
        let render = data.render.clone();
        let changed = render.borrow_mut().set_firetest_text_labels(count);
        if changed {
            data.mark_view_dirty();
        }
        changed
    }

    pub fn set_firetest_force_present(&self, enabled: bool) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        let render = inner.borrow().render.clone();
        render.borrow_mut().set_firetest_force_present(enabled)
    }

    pub fn order_render_probe(&self, core: CoreId, market: &str) -> Option<OrderRenderProbe> {
        let inner = self.inner.upgrade()?;
        let render = inner.borrow().render.clone();
        render
            .borrow()
            .panes
            .iter()
            .find(|pane| pane.core == Some(core) && pane.market == market)
            .map(|pane| OrderRenderProbe {
                order_lines_rev: pane.last_order_lines_rev,
                order_lines_sync_ms: pane.last_order_lines_sync_ms,
                gpu_rev: pane.last_order_gpu_rev,
                gpu_ms: pane.last_order_gpu_ms,
                present_rev: pane.last_order_present_rev,
                present_ms: pane.last_order_present_ms,
            })
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub fn camera_shift_hz(&self) -> Option<f32> {
        let inner = self.inner.upgrade()?;
        let render = inner.borrow().render.clone();
        Some(render.borrow_mut().camera_shift_hz(now_unix_ms()))
    }
}

struct ChartDataState {
    container: Rc<RefCell<Container>>,
    render: Rc<RefCell<RenderState>>,
    theme: ChartTheme,
    orders: OrdersStyle,
    follow: bool,
    present_rate_hz: f32,
    w: u32,
    h: u32,
    origin: (f32, f32),
    scene_visible: bool,
    /// Показывать ли стакан (per-окно/панель). Выкл → glass_w=0, уровни не строятся, подпись не
    /// рисуется. Применяется ко всем панелям этого движка.
    orderbook_enabled: bool,
    /// Режим «только стакан» (кнопка-метла в сравнении): чарт и ось цен скрыты, стакан на всю
    /// ширину. Применяется ко всем панелям этого движка (у соседей якоря).
    orderbook_only: bool,
    /// Положение оси цен (Left/Right/Hide), per-окно. Управляет раскладкой жёлоба оси и стороной
    /// рендера подписей. Дефолт — Left (жёлоб слева, исторический вид).
    price_axis_pos: crate::chart_persist::PriceAxisPos,
    /// Видна ли ось времени (нижние подписи + жёлоб под них), per-окно. Выкл → подписи времени
    /// не рисуются, плот занимает всю высоту. Дефолт — вкл.
    time_axis_visible: bool,
    /// Интерактивная подсветка линии ордера (hover/drag). Это не меняет рыночные данные:
    /// только заставляет редкую пересборку userdata при смене uid.
    order_highlight: Option<(CoreId, u64)>,
    /// Локальная preview-цена линии при drag. Ядру команда уходит только на mouse-up.
    order_drag_preview: Option<(CoreId, u64, LineKind, f32)>,
    market_source: Option<MarketDataSource>,
    last_frame_tick_ms: f64,
    present_rate_candidate_hz: f32,
    present_rate_candidate_hits: u8,
    last_ppp: f32,
    slot_bounds: Option<Bounds<Pixels>>,
    last_order_sig: u64,
    last_prepared_market_sig: u64,
    last_source_market_sig: u64,
    view_dirty: bool,
}

#[derive(Clone)]
struct ChartCanvasDriver {
    state: Rc<RefCell<RenderState>>,
    data: Weak<RefCell<ChartDataState>>,
}

impl GpuCanvasDriver for ChartCanvasDriver {
    fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        if let Some(data) = self.data.upgrade() {
            data.borrow_mut().frame(info)
        } else {
            self.state.borrow_mut().frame(info)
        }
    }

    fn prepare_gpu(&mut self, ctx: &mut gpui::GpuCanvasPrepareContext<'_>) -> anyhow::Result<()> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.state.borrow_mut().prepare_gpu(&ctx.gpu)
        }));
        match result {
            Ok(result) => result,
            Err(e) => {
                let msg = e
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| e.downcast_ref::<String>().map(|s| s.as_str()))
                    .unwrap_or("<non-string panic>");
                log::error!("chart gpu_canvas prepare PANIC (кадр пропущен): {msg}");
                moon_core::detect_diag::line(&format!("[gpu_canvas] prepare PANIC: {msg}"));
                Ok(())
            }
        }
    }

    fn prepare_text(&mut self, ctx: &mut GpuCanvasTextContext<'_>) -> anyhow::Result<()> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.state.borrow_mut().prepare_text(ctx)
        }));
        match result {
            Ok(result) => result,
            Err(e) => {
                let msg = e
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| e.downcast_ref::<String>().map(|s| s.as_str()))
                    .unwrap_or("<non-string panic>");
                log::error!("chart gpu_canvas text PANIC (text skipped): {msg}");
                moon_core::detect_diag::line(&format!("[gpu_canvas] text PANIC: {msg}"));
                Ok(())
            }
        }
    }

    fn draw(&mut self, ctx: &mut gpui::GpuCanvasDrawContext<'_>) -> anyhow::Result<()> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.state.borrow_mut().draw_gpu(&ctx.gpu)
        }));
        match result {
            Ok(result) => result,
            Err(e) => {
                let msg = e
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| e.downcast_ref::<String>().map(|s| s.as_str()))
                    .unwrap_or("<non-string panic>");
                log::error!("chart gpu_canvas PANIC (кадр пропущен): {msg}");
                moon_core::detect_diag::line(&format!("[gpu_canvas] PANIC: {msg}"));
                Ok(())
            }
        }
    }
}

#[derive(Clone)]
pub struct ChartEngine {
    container: Rc<RefCell<Container>>,
    state: Rc<RefCell<RenderState>>,
    data: Rc<RefCell<ChartDataState>>,
    canvas: GpuCanvasHandle,
    epoch: f64,
    theme: ChartTheme,
    orders: OrdersStyle,
    scale: Option<f32>,
    follow: bool,
    present_rate_hz: f32,
}
