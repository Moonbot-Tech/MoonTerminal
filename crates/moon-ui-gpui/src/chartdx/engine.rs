//! Public `ChartEngine` handle for opening, scaling, following, pruning, pinning, layout,
//! presentation control, and panel synchronization. This implementation block was extracted from
//! `mod.rs`, where the `ChartEngine` structure remains declared and visible to this child module.

use super::*;

mod controls;
mod geometry;
mod ghost;
mod hits;
mod overlays;
mod panes;
mod scale;

pub use ghost::ChartGhostCursor;
use ghost::{hex3, initial_palette_from_theme};

impl ChartEngine {
    pub fn new(epoch: f64, theme: ChartTheme) -> Self {
        Self::new_kind(epoch, theme, ContainerKind::Main)
    }

    pub fn new_kind(epoch: f64, theme: ChartTheme, kind: ContainerKind) -> Self {
        let container = Rc::new(RefCell::new(Container::new(kind)));
        let state = Rc::new(RefCell::new(RenderState {
            order_book_width_px: moon_core::config::book_width::DEFAULT,
            panes: Vec::new(),
            cursor_params_scratch: Vec::new(),
            needs_present: true,
            base_dirty: true,
            last_present_at: None,
            target_present_interval: Duration::from_secs_f64(1.0 / 60.0),
            camera_shift_window_start: None,
            camera_shift_count: 0,
            camera_shift_hz: 0.0,
            last_gpu_prepare_generation: 0,
            text_runs: Vec::new(),
            text_run_cursor: 0,
            caption_runs: Vec::new(),
            caption_wraps: Vec::new(),
            caption_fit_memo: text::FitMemo::default(),
            chart_labels: std::rc::Rc::new(moon_core::config::ChartLabelsCfg::default()),
            trade_labels: None,
            // The same timeframe `ChartDataState` starts on, so an `Авто` countdown is right from
            // the first frame rather than from the first `set_candle_view`.
            chart_tf_ms: moon_core::market::CandleViewCfg::default().tf_ms(),
            arb_view: std::rc::Rc::new(moon_core::config::ArbViewCfg::default()),
            firetest_text_labels: Vec::new(),
            firetest_text_runs: Vec::new(),
            firetest_text_layer: GpuCanvasRetainedTextLayer::default(),
            firetest_text_revision: 0,
            firetest_force_present: false,
            ui_palette: initial_palette_from_theme(&theme),
            slot_origin: [0.0, 0.0],
            cursor: None,
            ghost_price: None,
            compare_ref_price: None,
            arrival_pulse: None,
            arrival_pulse_color: [0.0; 4],
            arrival_hold: false,
            last_arrival_present_at: None,
            shot_caption_until: None,
            shot_caption_frames: 0,
            shot_caption_device_gen: 0,
            shot_caption_gen: 0,
            cursor_color: {
                let mut c = rgb4(theme.cross);
                c[3] = theme.cross_alpha;
                c
            },
            cursor_thickness: theme.cross_thickness.max(1.0),
            readout_bg: rgba3(theme.bg, theme.readout_bg_alpha),
            readout_soft_bg: rgba3(theme.bg, theme.readout_soft_bg_alpha),
            readout_order_bg: rgba3(theme.bg, theme.line_label_bg_alpha),
            readout_border: rgba3(theme.bg, theme.readout_border_alpha),
            readout_border_px: theme.readout_border_px.max(0.0),
            label_positive: hex3(theme.label_positive),
            label_negative: hex3(theme.label_negative),
            label_neutral: hex3(theme.label_neutral),
            axis_label: hex3(theme.axis_label),
            caption_label: hex3(theme.caption_label),
            readout_label: hex3(theme.readout_label),
            label_font_delta: theme.label_font_delta,
            line_labels: true,
            cursor_labels: true,
            cursor_badge: None,
            ruler: None,
            pixel_scale: 1.0,
            #[cfg(windows)]
            scissor_rs: None,
            #[cfg(windows)]
            scissor_generation: 0,
            #[cfg(windows)]
            window_bg_color: rgb4(theme.bg),
            #[cfg(windows)]
            base_cache: base::BaseCache::new(),
        }));
        let data = Rc::new(RefCell::new(ChartDataState::new(
            container.clone(),
            state.clone(),
            theme.clone(),
        )));
        let canvas = GpuCanvasHandle::new(ChartCanvasDriver {
            state: state.clone(),
            data: Rc::downgrade(&data),
        });
        Self {
            container,
            state,
            data,
            canvas,
            epoch,
            theme,
            orders: OrdersStyle::default(),
            scale: None,
            follow: true,
            present_rate_hz: 60.0,
        }
    }
}

#[cfg(test)]
mod tests;
