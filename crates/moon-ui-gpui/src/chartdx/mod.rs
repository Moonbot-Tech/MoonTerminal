//! Native `gpu_canvas` chart renderer replacing wgpu offscreen rendering and readback. Own-pass
//! layers follow data semantics: Combo for market history, OrderBook for a snapshot, UserData for
//! mutable user state, chrome for Grid and Background, plus native cursor and readout. GPUI renders
//! static axis text.
//!
//! Chart domain behavior lives HERE in the terminal; the GPUI fork exposes only the generic
//! `RawGpuAccess` hook. Each layer has its own file. This module contains the `ChartEngine`
//! orchestrator: per-pane data preparation WITHOUT rendering plus the `gpu_canvas` element that
//! draws inside the GPUI frame.

pub mod axes;
mod backend;
#[cfg(windows)]
pub mod background;
#[cfg(windows)]
mod base;
#[cfg(windows)]
pub mod candles;
pub mod input;
// Engine orchestration extracted from this file into impl blocks; structures remain declared below.
// Child modules can access ancestor-private fields, so only code location changed, not behavior.
#[cfg(windows)]
pub mod combo;
#[cfg(windows)]
pub mod cursor;
mod data_state;
mod engine;
mod figures_sync;
mod news_sync;
pub(crate) mod trade_history_sync;
mod warn_sync;
pub use engine::ChartGhostCursor;
pub(crate) use figures_sync::FigureVisual;
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
pub(crate) use render_state::arrival_flash_enabled;
mod text;
/// The caption editor formats its sample line with the chart's OWN formatter, never a second
/// spelling of it.
pub(crate) use text::preview_row;
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
use moon_core::market::{ChartHistoryBuffers, ChartHistoryCursor, MarketDataSource, MarketLabel};
use moon_core::session::order_lines::LineKind;
use moon_core::session::{CoreId, SessionManager};
use moon_core::symbol::Exchange;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11RasterizerState, ID3D11RenderTargetView,
};

use backend::PlatformLayers;
use pane::{Container, ContainerKind};
use types::{
    BackgroundParams, BookStyle, CandleGpu, CandleStyleGpu, ChartCross, ChartViewGpu, CursorParams,
    GridParams, PriceStyleGpu, ReadoutRect, VolumeStyleGpu, cover_uv, fill_candle_upload,
    fill_cross_upload, fill_liq_upload, fill_price_upload, rgb4, rgba3,
};

const CHART_PHOTO_BACKGROUND_ENABLED: bool = false;

/// Minimum half-width of the visible auto-focus band around the order-book midpoint when there are
/// no trades, expressed as a price fraction. The band always includes best bid and ask but is never
/// narrower than +/-0.5%, preventing absurd zoom into a tight spread while showing both sides of a
/// wide HIP-3 spread. Once trades arrive, ticks drive the range.
const BOOK_FOCUS_HALF_FRAC: f32 = 0.005;

fn union_range(a: Option<(f32, f32)>, b: Option<(f32, f32)>) -> Option<(f32, f32)> {
    match (a, b) {
        (Some((alo, ahi)), Some((blo, bhi))) => Some((alo.min(blo), ahi.max(bhi))),
        (Some(r), None) | (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

/// Return the whole-number percentage of the current visible Y range relative to price for the
/// scale badge beside the corner label, or `None` to hide it.
///
/// Auto always shows the badge. Manual Y from drag, right-click zoom, or comparison lock shows it
/// only when the whole percentage differs from the selected step. An untouched fixed percentage
/// matches the selected step by construction and stays hidden.
fn scale_badge_pct(view: &moon_chart::view::ChartView) -> Option<i32> {
    // Measured against the instrument's price, not the centre of the viewport: dragging the chart
    // vertically moves that centre without touching the zoom, and reporting a changed scale for a
    // scale that did not change is what this badge is least allowed to do.
    let cur = view.visible_scale_percent()?.round() as i32;
    if view.auto_price {
        return Some(cur);
    }
    if !view.manual_price {
        return None;
    }
    let selected = (view.scale_percent * 100.0).round() as i32;
    (cur != selected).then_some(cur)
}

/// Whether the market channel is on (`channels.markets` in `cfg/diagnostics.toml`, or
/// `MOON_MARKET_DIAG`/`MOON_RENDER_DIAG`). Live, so it follows an edit without a restart.
fn chart_market_diag_enabled() -> bool {
    moon_core::diagnostics::markets()
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
        // The configured floor, not a literal: this throttle and `market::source`'s serve the same
        // channel, and a second copy of the number would ignore `limits.market_trace_min_interval_ms`.
        Some(prev)
            if now.duration_since(prev) < moon_core::diagnostics::market_trace_min_interval() =>
        {
            false
        }
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

/// A placed label after overlap avoidance stores logical position, alignment, and width so
/// `sync_readout_params` can build a translucent backing plate. `solid` selects a dense foreground
/// plate for cursor numbers instead of the light plate used by the market corner label.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PlacedLabel {
    pub x: f32,
    pub y: f32,
    pub ax: f32,
    pub ay: f32,
    pub w: f32,
    pub h: f32,
    pub solid: bool,
}

/// One arbitrage venue name as it was drawn, and which venue it names.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ArbHit {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Protocol platform code, and the DEX name for a deployer.
    pub code: u8,
    pub dex: String,
    /// Whether a core is connected to this venue — whether the click has anywhere to go.
    pub reachable: bool,
}

impl ArbHit {
    /// Whether a point in the pane's own logical pixels lands on this name.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

pub(super) const ORDER_LABEL_NEUTRAL: u32 = u32::MAX;

// STATIC grid density uses fixed width and height divisions. Like Moonbot, the grid stays still and
// only labels move; both time verticals and price horizontals use fixed screen fractions.
pub(super) const GRID_N_VERT: f32 = 60.0;
pub(super) const GRID_N_HORIZ: f32 = 6.0;

// Order-line label overlap priorities: higher values place first and win overlaps. SELL and STOP,
// which show current position PnL or stop percentages, take precedence over BUY entry and size.
pub(super) const PRIO_BUY: u8 = 10;
pub(super) const PRIO_SELL_SIZE: u8 = 20;
pub(super) const PRIO_SELL_PCT: u8 = 30;
pub(super) const PRIO_STOP_PCT: u8 = 40;

/// Prepared order-line label (reference category E): text, line price on Y, placement above or below
/// the line, and line color. It is built while orders synchronize in `sync_orders_from_session`,
/// where `session` is available, and drawn by `prepare_text`.
#[derive(Clone)]
pub(super) struct OrderLabel {
    /// Line price converted to Y through `view` each frame.
    pub price: f32,
    pub text: String,
    /// `true` places the label above the line; `false` places it below.
    pub above: bool,
    /// Line color as `0xRRGGBB`, also used for the label.
    pub color: u32,
    /// Draw-order priority at intersections: lower values draw first and higher values on top.
    /// A true Moonbot-style Y bucket for secondary captions still requires a separate pass.
    pub priority: u8,
    /// Whether a DRAG label must render on top without overlap suppression. Hover does not set it —
    /// hovering feeds `order_highlight`, and [`OrderLabel::highlighted`] is that half.
    pub force: bool,
    /// Whether this label belongs to the order under the pointer.
    ///
    /// Only the pinned column reads it, and only to keep the caption: several exits pinned to one
    /// edge are thinned down to the nearest one's captions, and the painter puts the HIGHLIGHTED
    /// order's line on top of that pile — thinning its caption away would leave the highlighted
    /// line labelled with a stranger's numbers.
    pub highlighted: bool,
    /// Whether the label follows a line that is PINNED to the plot's edge when its price leaves the
    /// visible band, and must therefore be clamped the same way instead of dropped off screen.
    ///
    /// The text is unaffected: it states the order's real price, percentage and size wherever the
    /// line ended up, which is the whole point of pinning the drawing and nothing else.
    pub pinned: bool,
}

#[derive(Clone)]
pub(super) struct OrderBookLabel {
    /// Sell-line price; the label is drawn in the orderbook zone at this Y.
    pub price: f32,
    pub short: bool,
    /// Cached whole-book notional for this sell-line depth label, recomputed when the order
    /// labels or the book revision change; text frames only format and draw it. `None` means the
    /// figure was never measured — no book for this market yet, or the book is switched off — and
    /// the label is not drawn at all, because a drawn `0` claims "no glass to clear".
    pub notional: Option<f32>,
}

/// GPU state for one panel's `gpu_canvas` callbacks, separate from `Container` logic and
/// synchronized in `prepare` by index plus `(core, market)` identity.
struct PaneRender {
    core: Option<CoreId>,
    market: String,
    /// Core name for the chart corner label, resolved from `SessionManager` during order sync.
    core_name: String,
    /// Ticker for that same caption (`BEAT-USDT`), resolved from the core's catalog in
    /// `sync_from_market_source` and cached here.
    ///
    /// Cached rather than derived while drawing: the caption is drawn every frame, and resolving
    /// it takes the market-source lock and reads a snapshot. Deriving it from `market` alone
    /// cannot name a Hyperliquid spot index (`@156`) or tell two COIN-M expiries apart.
    ticker: String,
    /// Provider+generation+meta key the cached `ticker` was resolved at; see the retry in
    /// `sync_from_market_source`.
    ticker_catalog_key: u64,
    /// Whether `ticker` has been resolved at all. An empty string cannot say this: a market with
    /// no label resolves to one, and the pane would take the source lock again every sync.
    ticker_resolved: bool,
    /// Current Y-scale badge to the left of the corner label, as a whole percentage of visible range
    /// relative to price. `None` hides it when fixed percentage matches the selected step. Computed
    /// by `sync_from_market_source` from the panel's logical `ChartView`.
    scale_badge: Option<i32>,
    /// Finished translucent plate under the corner caption, in DEVICE pixels `[x, y, w, h]`.
    ///
    /// Computed by `prepare_text`, which owns the caption's geometry, and drawn verbatim by
    /// `sync_readout_params`. A zero height means no caption is drawn this frame. This is the ONE
    /// caption's plates, one per column: the coin's and, when the two lines split around the order
    /// book's left edge, the core name's. Measuring each drawn row separately is what let the plate
    /// drift away from the text it sits under; a single plate spanning both columns would instead
    /// darken the candles lying between them.
    caption_plates: [[f32; 4]; text::CAPTION_PLATES],
    /// Captions this pane resolved from the configuration, and the inputs they were built from.
    labels: text::LabelState,
    /// Venue label for the pane's core, resolved during order sync beside `core_name`.
    venue: String,
    /// Quote currency of this pane's market, resolved with the ticker from the same label.
    quote: String,
    /// Whether [`Self::labels`] was last built with a shot's caption substitution in force.
    ///
    /// The shot's proof is about what a FRAME drew, and `refresh_pane_labels` runs on the sync
    /// paths rather than the frame path, so a presented frame can still be showing captions built
    /// before the substitution landed. This flag is what lets `prepare_text` tell those two frames
    /// apart; without it the proof would count a frame that still names the user's own core.
    labels_shot_substituted: bool,
    /// Strategy name of the newest open order on this market, from the same sync.
    label_strategy: String,
    /// Strategy and line of the newest detect this pane's core fired on this market, from the same
    /// sync. Replaced only by the next detect on that market — see `LabelInputs::detect_strategy`.
    label_detect_strategy: String,
    label_detect_msg: String,
    /// Open-position figures per basis, from the same sync.
    label_basis: [text::BasisStats; 3],
    /// Signed one-hour and 24-hour changes, refreshed with the market snapshot.
    delta_1h: Option<f64>,
    delta_24h: Option<f64>,
    /// Exchange and BTC background movement plus funding, refreshed with the same snapshot and
    /// only while a caption asks for any of it.
    label_context: Option<moon_core::market::MarketContextReadout>,
    /// Quote side, venue caps, coin tags and the exchange's own position, on the same terms as
    /// [`Self::label_context`]: refreshed with the market snapshot, and only while a caption asks.
    label_figures: Option<moon_core::market::MarketFiguresReadout>,
    /// Retained-history movement and volume per window, gated separately because it costs more.
    label_windows: Option<moon_core::market::MarketWindowsReadout>,
    /// Venues this terminal is connected to, refreshed with the session sync that fills the order
    /// figures beside it. Empty until a caption asks for the column.
    label_arb_reachable: Vec<(u8, String)>,
    /// Where each arbitrage venue NAME was drawn, in the pane's own logical pixels.
    ///
    /// Rebuilt by the caption pass on every presented frame and reused in place, because a click
    /// has to hit what the LAST frame actually drew: the column moves with the pane, and a stale
    /// rectangle would open the wrong exchange.
    pub(super) arb_hits: Vec<ArbHit>,
    /// Arbitrage quotes for this pane's market, refreshed on a throttle rather than per revision:
    /// the protocol only hands them over one venue at a time, each behind the market lock.
    label_arb: Vec<moon_core::market::ArbQuote>,
    /// When they were last read, in Unix milliseconds. Zero means never.
    label_arb_read_ms: i64,
    /// Market those quotes were read for.
    ///
    /// Carried because the THROTTLE outlives a retarget: a pane switched to another coin would
    /// otherwise keep printing the previous one's arbitrage prices until the quarter-second was up.
    /// Every other readout here follows the new market on the revision that changed it.
    label_arb_market: String,
    /// Wall clock the funding countdown is measured against, QUANTIZED TO THE MINUTE.
    ///
    /// Quantized because it is part of the caption cache key: the raw clock would differ on every
    /// revision and re-format a countdown that prints the same minute either way. Zero while no
    /// countdown is configured, so an unused clock cannot wake anything.
    label_now_ms: i64,
    view: ChartViewGpu,
    layers: PlatformLayers,
    background_params: BackgroundParams,
    grid_params: GridParams,
    cursor_params: CursorParams,
    readout_rects: Vec<ReadoutRect>,
    readout_time_width: f32,
    readout_time_line_h: f32,
    readout_price_width: f32,
    readout_price_line_h: f32,
    history_cursor: ChartHistoryCursor,
    history_buffers: ChartHistoryBuffers,
    /// Last source slice signature used to decide if retained chart history must be read.
    source_history_sig: u64,
    /// Last provider generation seen by this pane. Changed generation means source replacement.
    source_generation: u64,
    /// Last chart-archive revision seen by this pane.
    ///
    /// A change means the core's archive was merged into the retained rings, prepending rows OLDER
    /// than this pane's cursors. Those rows are unreachable by an incremental drain, so the pane
    /// answers with a full history reset rather than the usual wake.
    source_archive: u64,
    cross_upload: Vec<ChartCross>,
    /// LIQUIDATION trade-cross upload buffer using `side=2` in the same combo ring.
    liq_upload: Vec<ChartCross>,
    last_line_upload: Vec<PriceLinePoint>,
    mark_line_upload: Vec<PriceLinePoint>,
    /// Reusable candle-layer upload buffer.
    candle_upload: Vec<CandleGpu>,
    /// Last candle-series revision delivered to the GPU; `u64::MAX` means never delivered.
    last_candle_rev: u64,
    /// Applied candle-view config, stored already reduced to `CandleViewCfg::history_inputs` — the
    /// fields the history read consumes: time frame, mode, the trade-candle boundary and price-line
    /// visibility. A change in any of them resets history. Style-only fields are neutralized there
    /// and reach the separately cached GPU style, which also carries theme colors and fill alpha.
    applied_candle_cfg: moon_core::market::CandleViewCfg,
    /// Current-time time-frame bucket at the previous sync; movement shifts the trade zone and resets.
    last_zone_bucket: i64,
    /// Last candle style sent to the layer, compared before `set_candle_style`.
    candle_style: CandleStyleGpu,
    /// Last price-line style sent to the layer, compared before `set_price_style`.
    price_style: PriceStyleGpu,
    /// Last bottom-volume style sent to the layer, compared before `set_volume_style`.
    volume_style: VolumeStyleGpu,
    /// Retained per-candle volume samples for the visible-range max/average.
    ///
    /// A COPY on purpose: `history_buffers.candles` is cleared at the start of every read and
    /// refilled only when the series revision moved, so during a plain pan it is empty while
    /// the uploaded candle layer is still resident. Scaling the band from it would blank the
    /// band on exactly the gesture that should rescale it.
    volume_samples: Vec<moon_chart::VolumeSample>,
    /// Visible-range volume max and average behind the band, kept as SEMANTIC values.
    ///
    /// The numeric labels read these rather than inverting `VolumeStyleGpu.m`, whose fields are
    /// normalisation reciprocals and are deliberately quantized for cache stability.
    volume_stats: Option<moon_chart::VolumeStats>,
    combo_cross_capacity: usize,
    combo_price_line_capacity: usize,
    orderbook_view: ChartViewGpu,
    pane_bounds: [f32; 4],
    book_style: BookStyle,
    resident_left_rel: f32,
    /// Relative time of the OLDEST trade cross actually resident in the combo ring.
    ///
    /// `NaN` while none are. Distinct from `resident_left_rel`, which records what the read ASKED
    /// for: the hide-candles zone needs what the ring actually HAS, so it never blanks a bucket
    /// with no crosses to draw in its place.
    combo_left_rel: f32,
    /// Camera position, in pixels, at this pane's last history reset.
    ///
    /// A pan is covered by the prefetch the last read already fetched, so the next reset is owed
    /// only once the camera has travelled further than that; see the use site. `i64::MIN` means
    /// "never reset", which makes the distance overflow into a reset on the first pass.
    pan_reset_cam_px: i64,
    /// Last observed combo device generation; device loss requires history reupload.
    last_device_gen: u64,
    /// Last order-book build: data revision plus visible price window.
    last_book_rev: u64,
    last_book_lo: f32,
    last_book_hi: f32,
    /// Book revision the sell-line depth labels were measured against; `u64::MAX` means
    /// unmeasured, which is also how `sync_orders_from_session` asks for a re-measure after
    /// rebuilding them. Separate from `last_book_rev` because that one also tracks the visible
    /// window: the labels' figure spans price to the line and does not depend on the camera, so
    /// panning must not re-sum the book.
    last_label_book_rev: u64,
    /// Last order revision uploaded into the userdata buffer.
    last_order_lines_rev: u64,
    /// Last order-zone signature. Zones live in the base cache, drawn over the grid and under the
    /// candles, while lines and traces render as an overlay. Zone changes must invalidate base;
    /// line hover and drag must not.
    last_order_zone_sig: u64,
    /// Local time when the userdata buffer was rebuilt from `order_lines_rev`.
    last_order_lines_sync_ms: f64,
    /// Order-userdata revision waiting for the next GPU prepare.
    pending_order_gpu_rev: Option<u64>,
    /// Last order revision that reached GPU prepare.
    last_order_gpu_rev: u64,
    /// Local time of the GPU prepare associated with `last_order_gpu_rev`.
    last_order_gpu_ms: f64,
    /// Last order revision actually rendered by the own-pass draw.
    last_order_present_rev: u64,
    /// Local time of the first draw for `last_order_present_rev`.
    last_order_present_ms: f64,
    /// Last order UID highlighted while building userdata.
    last_order_highlight_uid: Option<u64>,
    /// Last drag preview encoded into userdata.
    last_order_drag_preview: Option<(u64, LineKind, u32)>,
    /// Figure signature from store and interaction encoded into userdata; `u64::MAX` means dirty.
    last_figures_sig: u64,
    /// News-mark signature encoded into userdata; `u64::MAX` means dirty.
    last_news_sig: u64,
    /// Durable closed-trade marker signature encoded into userdata.
    last_trade_history_sig: u64,
    /// What the currently uploaded trade arrows were built FROM: the clusters, and the map from
    /// their members back to the panel's own record list.
    ///
    /// Retained rather than recomputed because the signature above quantizes the view scale: within
    /// one bucket the view keeps moving while the buffers do not, so re-clustering from the live
    /// scale would answer about a picture that is not on screen. Hit-testing reads this.
    trade_geometry: trade_history_sync::TradeGeometry,
    /// Warning-badge signature encoded into userdata; `u64::MAX` means dirty.
    last_warn_sig: u64,
    /// Prepared order-line labels for size, percentage, and quantity, rebuilt when orders change.
    /// `prepare_text` draws them and maps Y through `view` each frame.
    order_labels: Vec<OrderLabel>,
    /// Figure readouts, rebuilt with figure userdata and drawn by `prepare_text`.
    ///
    /// Most tools fill this only for the figure under the cursor and the one being drawn, so an
    /// idle chart pays nothing. A ratio scale (Fibonacci) is the exception and always names its
    /// levels — a level whose price appears only under the cursor cannot be read at a glance.
    figure_labels: Vec<moon_chart::figures::FigureLabel>,
    /// Stable priority order for `order_labels`, rebuilt together with order labels.
    /// Cursor-only text frames must not allocate/sort it again.
    order_label_order: Vec<usize>,
    /// Order-book volume labels on sell lines, matching Moonbot `LastSellOrderPriceVol`: the order
    /// provides the target and the current CPU order-book copy provides actual volume.
    orderbook_labels: Vec<OrderBookLabel>,
    /// Prospective selected F1-F6 order size in USD rendered at the cursor crosshair. `None` means no
    /// active size or rate. `ChartPanel::render`, which has Backend access, computes and copies it here.
    prospective_usd: Option<f64>,
    /// Placed order and cursor labels for this frame. `prepare_text` lays them out with overlap
    /// avoidance, and `sync_readout_params` builds their backing plates.
    label_placed: Vec<PlacedLabel>,
    /// CPU copy of visible order-book levels for quantity labels under the cursor and on sell lines.
    /// Filled during order-book upload in `prepare`; empty while the order book is disabled.
    orderbook_levels: Vec<moon_core::data::BookDepthPoint>,
    /// Live best `(bid, ask)` book prices defining the three-color order-book zone background.
    book_best: Option<(f32, f32)>,
    /// Own-pass X camera: time epoch, right-side future fraction, follow flag, and last QUANTIZED
    /// right-edge pixel position. The callback advances the camera from these fields on every
    /// whole-pixel vblank present, providing live scrolling without a separate timer.
    epoch_ms: f64,
    right_margin_frac: f32,
    follow: bool,
    last_edge_px: i64,
    /// Cache of the expensive auto-Y scan over visible tick minima and maxima plus the camera pixel
    /// position for which it is valid. Rescan only on pixel crossings; see the prepare switch.
    scan_cam_px: i64,
    cached_tick_price: Option<(f32, f32)>,
    cached_last_price: Option<f32>,
    /// Whether this pane has ever had price data of its OWN in the window — trades, candles or an
    /// order line — as opposed to the last price and the order-book band, which are only kept on
    /// screen. Decides whether the price fit may fall back to those references; see `fit_band`.
    /// Monotone until the pane's market changes.
    saw_window_data: bool,
    /// Last live-order range for auto-Y. Full session sync updates it; market-only frame sync reads
    /// this cache without touching CoreStore from `frame()`.
    cached_order_price: Option<(f32, f32)>,
    /// Whether this pane is visible and rendered this frame, set by `prepare`.
    active: bool,
    /// Whether this panel enables its per-window order book; disabled hides the book and corner label.
    orderbook_enabled: bool,
    /// Whether this panel draws per-window liquidation trades. Disabled omits liquidation crosses
    /// from combo. Changing the flag resets combo for reupload with or without liquidations.
    liquidations_enabled: bool,
    /// This panel's order-book-only mode, hiding chart and price axis and using the full width.
    orderbook_only: bool,
    /// Price-axis position (`Left`, `Right`, or `Hide`), controlling label side and reserved gutter.
    /// Applied to every engine panel.
    price_axis_pos: crate::persistence::chart_persist::PriceAxisPos,
    /// Whether the time axis and its bottom-label gutter are visible. Disabled lets the plot fill
    /// slot height. Applied to every engine panel.
    time_axis_visible: bool,
    /// CPU/base inputs changed and D3D prepare must upload/bake resident resources before draw.
    /// Cursor-only presents leave this false.
    gpu_prepare_dirty: bool,
}

impl PaneRender {
    /// Creates a pane with no retained caption geometry or uploaded market state.
    ///
    /// The caption plate starts empty because `prepare_text` is its sole publisher; seeding a
    /// guessed rectangle here would briefly draw stale backing geometry before the first prepare.
    fn new() -> Self {
        Self {
            core: None,
            market: String::new(),
            core_name: String::new(),
            ticker: String::new(),
            ticker_catalog_key: 0,
            ticker_resolved: false,
            scale_badge: None,
            caption_plates: [[0.0; 4]; text::CAPTION_PLATES],
            labels: text::LabelState::default(),
            venue: String::new(),
            quote: String::new(),
            labels_shot_substituted: false,
            label_strategy: String::new(),
            label_basis: [text::BasisStats::default(); 3],
            delta_1h: None,
            delta_24h: None,
            label_detect_strategy: String::new(),
            label_detect_msg: String::new(),
            label_context: None,
            label_figures: None,
            label_windows: None,
            arb_hits: Vec::new(),
            label_arb_reachable: Vec::new(),
            label_arb: Vec::new(),
            label_arb_read_ms: 0,
            label_arb_market: String::new(),
            label_now_ms: 0,
            view: ChartViewGpu::default(),
            layers: PlatformLayers::new(),
            background_params: BackgroundParams::default(),
            grid_params: GridParams::default(),
            cursor_params: CursorParams::default(),
            readout_rects: Vec::new(),
            readout_time_width: 0.0,
            readout_time_line_h: 0.0,
            readout_price_width: 0.0,
            readout_price_line_h: 0.0,
            history_cursor: ChartHistoryCursor::default(),
            history_buffers: ChartHistoryBuffers::default(),
            source_history_sig: u64::MAX,
            source_generation: u64::MAX,
            source_archive: u64::MAX,
            cross_upload: Vec::new(),
            liq_upload: Vec::new(),
            last_line_upload: Vec::new(),
            mark_line_upload: Vec::new(),
            candle_upload: Vec::new(),
            last_candle_rev: u64::MAX,
            applied_candle_cfg: moon_core::market::CandleViewCfg::default().history_inputs(),
            last_zone_bucket: i64::MIN,
            candle_style: CandleStyleGpu::default(),
            price_style: PriceStyleGpu::default(),
            volume_style: VolumeStyleGpu::default(),
            volume_samples: Vec::new(),
            volume_stats: None,
            combo_cross_capacity: 0,
            combo_price_line_capacity: 0,
            orderbook_view: ChartViewGpu::default(),
            pane_bounds: [0.0, 0.0, 1.0, 1.0],
            book_style: BookStyle::default(),
            resident_left_rel: f32::NAN,
            combo_left_rel: f32::NAN,
            pan_reset_cam_px: i64::MIN,
            last_device_gen: 0,
            last_book_rev: u64::MAX,
            last_label_book_rev: u64::MAX,
            last_book_lo: f32::NAN,
            last_book_hi: f32::NAN,
            last_order_lines_rev: u64::MAX,
            last_order_zone_sig: 0,
            last_order_lines_sync_ms: 0.0,
            pending_order_gpu_rev: None,
            last_order_gpu_rev: u64::MAX,
            last_order_gpu_ms: 0.0,
            last_order_present_rev: u64::MAX,
            last_order_present_ms: 0.0,
            last_order_highlight_uid: None,
            last_order_drag_preview: None,
            last_figures_sig: u64::MAX,
            last_news_sig: u64::MAX,
            last_trade_history_sig: u64::MAX,
            trade_geometry: trade_history_sync::TradeGeometry::default(),
            last_warn_sig: u64::MAX,
            order_labels: Vec::new(),
            figure_labels: Vec::new(),
            order_label_order: Vec::new(),
            orderbook_labels: Vec::new(),
            prospective_usd: None,
            label_placed: Vec::new(),
            orderbook_levels: Vec::new(),
            book_best: None,
            epoch_ms: 0.0,
            right_margin_frac: 0.10,
            follow: false,
            last_edge_px: i64::MIN,
            scan_cam_px: i64::MIN,
            cached_tick_price: None,
            cached_last_price: None,
            saw_window_data: false,
            cached_order_price: None,
            active: false,
            orderbook_enabled: true,
            liquidations_enabled: true,
            orderbook_only: false,
            price_axis_pos: crate::persistence::chart_persist::PriceAxisPos::Left,
            time_axis_visible: true,
            gpu_prepare_dirty: true,
        }
    }

    /// Drops everything derived from a book this pane no longer has: the order book was switched
    /// off for the window, or the market view went away with its core.
    ///
    /// Both are figures about a live book, so neither may outlive it — a frozen bid/ask would keep
    /// answering the cursor's percentage, and a stale sell-line volume would keep describing glass
    /// that is no longer drawn. `u64::MAX` also asks the book path to re-measure once one returns.
    fn forget_book_figures(&mut self) {
        if self.book_best.is_none() && self.last_label_book_rev == u64::MAX {
            return;
        }
        self.book_best = None;
        self.last_label_book_rev = u64::MAX;
        crate::chartdx::data_state::orders::clear_orderbook_label_notionals(
            &mut self.orderbook_labels,
        );
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

    /// Advance the X-follow camera only when `now_ms` moves by at least one WHOLE pixel, matching
    /// Moonbot `round(Now/FdtScale)`. Between pixel crossings the frame is pixel-identical, so present
    /// can reuse it without work. Whole-pixel steps remove subpixel jitter, while calling on every
    /// present keeps vblank motion smooth. Returns `true` when the camera actually moved for the
    /// productive-frame counter.
    fn advance_camera(&mut self, now_ms: f64) -> bool {
        if !self.follow || !(self.view.time_to_px > 0.0) {
            return false;
        }
        // Use ONE ppm guard for forward and inverse conversion. Previously `target_px` used raw ppm
        // while `inv_ppm` used a 1e-6 floor; at deep zoom-out below 1e-6 for a 365-day window,
        // `right_rel` collapsed near zero and shifted the chart left of the order book.
        let ppm = self.view.time_to_px.max(moon_chart::view::MIN_PX_PER_MS);
        let target_px = ((now_ms - self.epoch_ms) * ppm as f64).round() as i64;
        if target_px == self.last_edge_px {
            return false;
        }
        self.last_edge_px = target_px;
        let inv_ppm = 1.0 / ppm;
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

/// Render state for all panels shared with `gpu_canvas` callbacks through `Rc<RefCell>`.
///
/// The UI is single-threaded, and `prepare` never overlaps frame callbacks in time.
struct RenderState {
    panes: Vec<PaneRender>,
    /// CPU-side dirty flag for `GpuCanvasDriver::frame`: `prepare()` updated resident state, so the
    /// next platform tick must present even without GPUI dirtiness.
    needs_present: bool,
    /// Scene pixels changed since the optional DX11 cursor-restore cache was built.
    /// Live-scroll draws directly and invalidates that cache; cursor-only frames may rebuild it once.
    base_dirty: bool,
    last_present_at: Option<Instant>,
    target_present_interval: Duration,
    camera_shift_window_start: Option<Instant>,
    camera_shift_count: u32,
    camera_shift_hz: f32,
    last_gpu_prepare_generation: u64,
    text_runs: Vec<GpuCanvasTextRun>,
    text_run_cursor: usize,
    /// Retained runs for the configured captions, addressed by
    /// `(pane * CHART_LABEL_ROWS + row) * ROW_RUN_STRIDE + part` rather than by a running cursor.
    ///
    /// A separate pool precisely BECAUSE the cursor above is shared across panes and label kinds:
    /// an index from it moves whenever anything earlier in the frame stops drawing, and a run
    /// handed a different string reshapes it. A caption that appears and disappears — the scale
    /// badge, the comparison delta — would otherwise reshape its neighbours for free.
    caption_runs: Vec<GpuCanvasTextRun>,
    /// Lines of the PROSE captions on the pane being drawn, wrapped once and then measured and
    /// drawn from here.
    ///
    /// The caption pass measures a line, then measures it again to centre it, then again to draw
    /// it — which is free for a figure and is not free for a sentence that has to be broken on
    /// word boundaries first. Cleared per pane; an `Item` holds its index.
    caption_wraps: Vec<Vec<(String, f32)>>,
    /// Effective caption configuration, mirrored from `ChartDataState` so the text pass can read it
    /// without borrowing the data state during a frame.
    ///
    /// Behind an `Rc` because the draw pass takes a handle to it on every presented frame, per
    /// pane: the configuration owns a name string per row, and cloning it by value would allocate
    /// sixteen strings in the frame loop for nothing.
    chart_labels: Rc<moon_core::config::ChartLabelsCfg>,
    /// The arbitrage roster the caption column is arranged by, mirrored like `chart_labels` and for
    /// the same reason: the text pass reads it per pane on every rebuild and must not borrow the
    /// data state. GLOBAL — one roster for every chart — so every pane shares this handle.
    arb_view: Rc<moon_core::config::ArbViewCfg>,
    firetest_text_labels: Vec<String>,
    firetest_text_runs: Vec<GpuCanvasTextRun>,
    firetest_text_layer: GpuCanvasRetainedTextLayer,
    firetest_text_revision: u64,
    firetest_force_present: bool,
    ui_palette: moon_ui::MoonPalette,
    /// Top-left chart-slot origin in the backbuffer. UI cursor coordinates are local slot device
    /// pixels, while own-pass renders in window coordinates.
    slot_origin: [f32; 2],
    cursor: Option<CursorState>,
    /// Ghost crosshair price in comparison mode. A panel WITHOUT a real cursor draws a horizontal
    /// line at this price using its own Y mapping, plus order-book volume and percentage through
    /// `text/runs.rs::draw_ghost_cursor_labels`.
    /// The hovered sibling writes it through `ChartGhostCursor`, bypassing GPUI notification like
    /// the real cursor.
    ghost_price: Option<f32>,
    /// Anchor Last price for the large "+0.12%" delta below the corner label in broom mode. The stack
    /// supplies it through `apply_compare` on each observation. `None` means no comparison or this
    /// chart is the anchor.
    compare_ref_price: Option<f32>,
    /// When this chart arrived in a stack slot, driving the accent border flash. `None` once the
    /// flash is over — clearing it is what STOPS the extra presents, so it is the load-bearing
    /// half of this feature, not bookkeeping.
    arrival_pulse: Option<Instant>,
    /// Accent colour for the arrival flash, handed over with the stamp so the palette stays the
    /// single source of truth and this layer never guesses a colour.
    arrival_pulse_color: [f32; 4],
    /// When the last arrival-flash frame was presented, pacing it to `ARRIVAL_PULSE_TICK`
    /// independently of the 60 Hz present cap.
    last_arrival_present_at: Option<Instant>,
    /// Deadline until which every pane's core-name caption names the EXCHANGE instead, for a shot.
    ///
    /// Named for what it HOLDS — a wall-clock deadline — not for what it selects, so that
    /// `shot_caption_until = None` reads as "stop substituting" rather than as clearing a string.
    ///
    /// ONE flag for the whole engine rather than one per pane: a picture that named the exchange in
    /// one pane and the account in another would be worse than either.
    ///
    /// It carries a DEADLINE rather than a plain `bool` because the value is a privacy control. The
    /// screen must not be left naming the exchange if the shot's callback chain never completes — a
    /// closed window, a panel re-parented between windows, a stalled machine. `frame` expires it
    /// from wall clock exactly as it does [`Self::arrival_pulse`], so nothing has to be trusted to
    /// call the clear.
    shot_caption_until: Option<Instant>,
    /// How many completed text passes have drawn substituted captions since it was armed.
    ///
    /// The shot's proof, and the reason it is safe to capture at all. A COUNT rather than a flag,
    /// with a threshold above one, because `prepare_text` having run does NOT prove the frame
    /// reached the screen: the fork's renderer skips `draw` outright on the first frame after a
    /// DirectX device recovery and swallows a `can_present` refusal the same way, while the canvas
    /// text pass still runs. A single drawn pass could therefore be one the GPU discarded, and
    /// capturing on it would put the ACCOUNT NAME on the clipboard — the one outcome this exists
    /// to prevent.
    shot_caption_frames: u8,
    /// Device generation the proof has been counted against, to notice a recovery mid-shot.
    shot_caption_device_gen: u64,
    /// Bumped on every ARM, so a superseded shot can tell it has been replaced.
    ///
    /// Two presses in quick succession run two wait chains against this one engine. The second
    /// arming zeroes the frame count the first is still waiting on, and without a generation the
    /// first would sit out its budget and report a failure for a shot that was simply replaced. It
    /// never affected what gets CAPTURED — the count is zeroed before any later frame is tallied —
    /// only what gets reported.
    shot_caption_gen: u64,
    cursor_color: [f32; 4],
    cursor_thickness: f32,
    readout_bg: [f32; 4],
    readout_soft_bg: [f32; 4],
    readout_order_bg: [f32; 4],
    readout_border: [f32; 4],
    readout_border_px: f32,
    label_positive: u32,
    label_negative: u32,
    label_neutral: u32,
    axis_label: u32,
    caption_label: u32,
    readout_label: u32,
    /// Order-line and cursor label font-size adjustment in pixels from `ChartTheme.label_font_delta`.
    /// `text/runs.rs` applies it through label draw/measure helpers used by the line-label column and
    /// cursor readout in `text/prepare.rs`.
    label_font_delta: f32,
    /// Whether to show per-tab order-line labels from the ⚙ popup. Disabled hides the line-label
    /// column built by `text/prepare.rs::prepare_text`.
    line_labels: bool,
    /// Whether to show crosshair readout labels for time, price, percentage, volume, and size.
    /// Disabled hides cursor values prepared by `text/prepare.rs::prepare_text` and ghost labels
    /// drawn by `text/runs.rs::draw_ghost_cursor_labels`.
    cursor_labels: bool,
    /// Marker drawn beside the crosshair while a mode is active — today the Sells-to-zone
    /// drawing mode. `None` draws nothing. It rides the crosshair rather than the GPUI tree, so following
    /// the pointer costs no repaint of the view tree.
    ///
    /// A `&'static str` because a mode marker is a GLYPH, not a sentence: nothing to translate and
    /// nothing to allocate on the present path that redraws it.
    cursor_badge: Option<&'static str>,
    pixel_scale: f32,
    /// Lazily created own-pass scissor rasterizer, recreated on device changes. It clips layers to
    /// the panel so price-positioned order books and orders cannot spill beyond the plot onto
    /// toolbars or scales.
    #[cfg(windows)]
    scissor_rs: Option<ID3D11RasterizerState>,
    #[cfg(windows)]
    scissor_generation: u64,
    /// Full-window dark base drawn as the FIRST own-pass layer across the ENTIRE backbuffer without
    /// scissoring, covering GPUI or SwapChain's unpainted white background on the first frame. The
    /// branded empty-state logo is a GPUI SVG layer, not a native raster splash.
    #[cfg(windows)]
    window_bg: background::BackgroundLayer,
    /// Dark-base color equal to `rgb4(theme.bg)`, updated in `prepare` and filling the ENTIRE window.
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

    /// Start or clear the arrival border flash on this chart for a measurement stage.
    ///
    /// The same state a real arrival sets, reached without one: a live detect is not something a
    /// run can schedule, so measuring the flash's cost by waiting for one measures the market's
    /// mood instead. `accent` is the palette token, exactly as `ChartPanel::set_arrival_pulse`
    /// passes it, so the measured flash is the one the user sees and not a stand-in.
    ///
    /// Returns whether the chart is still alive and took the stamp.
    pub fn set_firetest_arrival_flash(&self, at: Option<Instant>, accent: u32) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        let render = inner.borrow().render.clone();
        render
            .borrow_mut()
            .set_arrival_pulse(at, types::accent_rgb4(accent));
        true
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
        Some(render.borrow_mut().camera_shift_hz())
    }
}

/// Resolve the horizontal plot and order-book widths shared by data preparation and navigation.
///
/// Args:
///     rect_w: Full pane width in device pixels.
///     orderbook_only: Whether the plot collapses behind the order book.
///     orderbook_enabled: Whether the normal order-book zone is visible.
///     price_axis_pos: Configured per-tab price-axis position.
///     pixel_scale: Device pixels per logical pixel.
///
/// Returns:
///     Effective axis position, axis width, order-book width, and plot width.
fn horizontal_chart_layout(
    rect_w: f32,
    orderbook_only: bool,
    orderbook_enabled: bool,
    price_axis_pos: crate::persistence::chart_persist::PriceAxisPos,
    pixel_scale: f32,
) -> (
    crate::persistence::chart_persist::PriceAxisPos,
    f32,
    f32,
    f32,
) {
    let axis_pos = if orderbook_only {
        crate::persistence::chart_persist::PriceAxisPos::Hide
    } else {
        price_axis_pos
    };
    let price_axis_w = if matches!(
        axis_pos,
        crate::persistence::chart_persist::PriceAxisPos::Hide
    ) {
        0.0
    } else {
        moon_chart::PRICE_AXIS_W * pixel_scale
    };
    let glass_cap = rect_w * 0.5;
    let glass_base = moon_chart::GLASS_ZONE_PX.min(glass_cap);
    let chart_w_base = rect_w - price_axis_w - glass_base;
    let glass_w = if orderbook_only {
        (rect_w - price_axis_w).max(1.0)
    } else if !orderbook_enabled {
        0.0
    } else if chart_w_base < glass_base * 2.0 {
        (moon_chart::GLASS_ZONE_PX * 0.8).min(glass_cap)
    } else {
        glass_base
    };
    let chart_w = (rect_w - price_axis_w - glass_w).max(1.0);
    (axis_pos, price_axis_w, glass_w, chart_w)
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
    /// Whether to show the per-window or panel order book. Disabled sets `glass_w=0`, skips level
    /// construction, and hides the label. Applied to every panel in this engine.
    orderbook_enabled: bool,
    /// Whether to draw per-window or panel liquidation trades. Disabled omits liquidation crosses
    /// from combo. Applied to every panel in this engine.
    liquidations_enabled: bool,
    /// Order-book-only mode from the comparison broom button: hide chart and price axis and use the
    /// full width for the order book. Applied to every panel in follower engines.
    orderbook_only: bool,
    /// Per-window price-axis position (`Left`, `Right`, or `Hide`) controlling gutter layout and
    /// label side. Defaults to Left, the historical left gutter.
    price_axis_pos: crate::persistence::chart_persist::PriceAxisPos,
    /// Whether the per-window time axis, bottom labels, and gutter are visible. Disabled lets the
    /// plot fill the full height. Enabled by default.
    time_axis_visible: bool,
    /// Effective candle and trade rendering settings for time frame, mode, and zone, applied to all
    /// engine panels. They may be a per-tab override or the `layout.candle_view` fallback.
    candle_view: moon_core::market::CandleViewCfg,
    /// Effective chart graphics settings: trade-history arrow size, connector thickness, and which
    /// order lines are drawn. Like `candle_view` above, a per-tab override or the
    /// `layout.chart_graphics` fallback.
    chart_graphics: moon_core::config::ChartGraphicsCfg,
    /// Effective chart caption configuration: which figures print beside the plot, in which corner
    /// and style. A per-tab override or the `layout.chart_labels` fallback, like the two above.
    /// Shared with the render mirror through an `Rc`; see the field there.
    chart_labels: Rc<moon_core::config::ChartLabelsCfg>,
    /// The GLOBAL arbitrage roster, shared with the render mirror the same way. Not a per-tab
    /// override: which venues matter and what colour they are is one answer for the whole terminal.
    arb_view: Rc<moon_core::config::ArbViewCfg>,
    /// Saved X scale in pixels per millisecond from Shift+middle-click sync. NEW panels start with it
    /// instead of the built-in time-window default; `None` uses that default.
    default_x_ppm: Option<f32>,
    /// Prospective selected F1-F6 manual order size in USD for the cursor crosshair label.
    /// `ChartPanel::render`, which has Backend access, sets it. `None` means no size or rate.
    prospective_usd: Option<f64>,
    /// Interactive order-line hover or drag highlight. It does not change market data and only
    /// triggers an infrequent userdata rebuild when the UID changes.
    order_highlight: Option<(CoreId, u64)>,
    /// Local line-price preview during drag; the command reaches the core only on mouse-up.
    order_drag_preview: Option<(CoreId, u64, LineKind, f32)>,
    /// Shared user-figure store from Backend `Rc`; see `figures_sync`.
    figures: Option<std::rc::Rc<std::cell::RefCell<moon_core::figures::FigureStore>>>,
    /// This panel's figure interaction state for drawing preview, hover, and selection plus its revision.
    figure_visual: figures_sync::FigureVisual,
    figure_visual_rev: u64,
    /// This panel's news marks (tag-coloured gems on the plot's bottom edge) plus their revision;
    /// see `news_sync`. Shared with the panel, which hit-tests the same list.
    news_marks: std::rc::Rc<Vec<moon_chart::news_marks::NewsMark>>,
    /// Index of the mark under the cursor, drawn grown from the axis.
    news_hovered: Option<usize>,
    /// Durable closed trades for this exact Main chart target.
    trade_history: std::rc::Rc<Vec<moon_core::db::ChartTradeRecord>>,
    /// Revision incremented whenever the durable history set changes.
    trade_history_revision: u64,
    /// The trade arrow under the cursor as `(pane, mark index in that pane, buy)`. It is drawn
    /// grown and fully opaque.
    ///
    /// Qualified by PANE because every pane draws only its own core's trades. Identified by an
    /// ACTION — mark plus direction — rather than by cluster, because clusters are renumbered by
    /// every rebuild and a bare mark names a whole trade rather than one of its two ends.
    trade_hovered: Option<(usize, usize, bool)>,
    /// This panel's warning badges (amber gems on the plot's bottom edge); see `warn_sync`. Shared
    /// with the panel, which hit-tests the same list.
    warn_marks: std::rc::Rc<Vec<moon_chart::news_marks::NewsMark>>,
    /// Index of the warning badge under the cursor.
    warn_hovered: Option<usize>,
    market_source: Option<MarketDataSource>,
    /// Frozen market history this engine draws INSTEAD of the live source, when it has one.
    ///
    /// Set only by the trade window, which owns its own engine. While it is `Some`, the history
    /// read below is answered from these rows and the live source is never consulted — so a replay
    /// cannot reach the user's main chart even by mistake: that engine's field is `None` and there
    /// is no shared key either could collide on. Contrast `moon_core::fixture`, whose bench state
    /// is process-wide by design.
    trade_replay: Option<Rc<moon_core::market::trade_replay::TradeReplaySeries>>,
    last_frame_tick_at: Option<Instant>,
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
