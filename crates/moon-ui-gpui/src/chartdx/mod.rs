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
#[cfg(any(windows, target_os = "macos"))]
mod candle_window;
#[cfg(windows)]
pub mod candles;
#[cfg(windows)]
pub mod combo;
#[cfg(windows)]
pub mod cursor;
mod data_state;
mod engine;
mod figure_snap;
mod filter_headers;
mod handle;
mod helpers;
mod hits;
pub mod input;
mod pane_layout;
mod pane_render;
use filter_headers::{ColumnBand, FilterHeaderHit};
mod archived_lines;
mod figures_sync;
mod news_sync;
pub(crate) mod trade_history_sync;
mod warn_sync;
use data_state::ChartDataState;
pub(crate) use data_state::TradeLabels;
pub use engine::ChartGhostCursor;
pub(crate) use figures_sync::FigureVisual;
pub use handle::{ChartDataHandle, OrderRenderProbe};
use helpers::{
    BOOK_FOCUS_HALF_FRAC, chart_market_diag, chart_market_diag_due, chart_market_diag_enabled,
    mix_sig, scale_badge_pct, str_sig, time_scale_secs, union_range,
};
use hits::{ActionPlacement, CursorState};
pub(super) use hits::{
    ArbHit, GRID_N_HORIZ, GRID_N_VERT, ORDER_LABEL_NEUTRAL, OrderBookLabel, OrderLabel, PRIO_BUY,
    PRIO_SELL_PCT, PRIO_SELL_SIZE, PRIO_STOP_PCT, PlacedLabel, VolumeHit,
};
pub use hits::{ChartActionButton, MarketActionState, WantedActions};
pub(crate) use pane_layout::{BookLayout, PaneAreas, pane_layout};
use pane_render::PaneRender;
use render_state::{RenderState, UdScratch};
#[cfg(windows)]
pub mod gpu;
#[cfg(windows)]
pub mod grid;
#[cfg(windows)]
pub mod hvol;
#[cfg(target_os = "macos")]
mod metal_backend;
#[cfg(windows)]
pub mod orderbook;
pub mod pane;
#[cfg(windows)]
pub mod readout;
mod render_state;
mod ruler;
pub use ruler::RulerSpan;
#[cfg(windows)]
pub mod side_volume;
pub(crate) use render_state::arrival_flash_enabled;
mod text;
/// The caption editor formats its sample line with the chart's OWN formatter, never a second
/// spelling of it.
pub(crate) use text::{notch_steps, preview_row};
#[cfg(any(windows, target_os = "macos"))]
mod price_ring;
#[cfg(test)]
mod tests;
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
    GridParams, HvolRowGpu, HvolStyleGpu, PriceStyleGpu, ReadoutRect, SideVolumeGpu, TickStyleGpu,
    VolumeStyleGpu, cover_uv, extend_candle_upload, extend_price_upload, fill_candle_upload,
    fill_cross_upload, fill_hvol_upload, fill_liq_upload, fill_price_upload,
    fill_side_volume_upload, rgb4, rgba3,
};

const CHART_PHOTO_BACKGROUND_ENABLED: bool = false;

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
