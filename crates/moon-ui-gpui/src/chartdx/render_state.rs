//! Chart render state (`impl RenderState`): per-pane GPU-state composition, present pacing initialized
//! at 60 Hz and then driven by detected monitor refresh clamped to 30-360 Hz, with the renderer target
//! capped at 240 Hz, plus cursor, readout, and own-pass layer rendering. Owns the retained `RenderState` and userdata scratch buffers.

use super::*;

/// The userdata union and the archived pass's own buffers, reused across order syncs.
#[derive(Default)]
pub(in crate::chartdx) struct UdScratch {
    pub(in crate::chartdx) zones: Vec<moon_chart::layers::ZoneInstance>,
    pub(in crate::chartdx) hlines: Vec<moon_chart::layers::LineInstance>,
    pub(in crate::chartdx) segs: Vec<moon_chart::layers::SegInstance>,
    pub(in crate::chartdx) markers: Vec<moon_chart::layers::MarkerInstance>,
    pub(in crate::chartdx) arch_zones: Vec<moon_chart::layers::ZoneInstance>,
    pub(in crate::chartdx) arch_hlines: Vec<moon_chart::layers::LineInstance>,
    pub(in crate::chartdx) arch_segs: Vec<moon_chart::layers::SegInstance>,
    pub(in crate::chartdx) arch_markers: Vec<moon_chart::layers::MarkerInstance>,
}

impl UdScratch {
    /// Empty the live union; the archived buffers are cleared by the build that fills them.
    pub(in crate::chartdx) fn clear_live(&mut self) {
        self.zones.clear();
        self.hlines.clear();
        self.segs.clear();
        self.markers.clear();
    }
}

/// Render state for all panels shared with `gpu_canvas` callbacks through `Rc<RefCell>`.
///
/// The UI is single-threaded, and `prepare` never overlaps frame callbacks in time.
pub(in crate::chartdx) struct RenderState {
    /// App-wide order-book width in physical pixels.
    pub(in crate::chartdx) order_book_width_px: f32,
    pub(in crate::chartdx) panes: Vec<PaneRender>,
    /// Buffer for the previous frame's cursor params, reused by the market sync.
    pub(in crate::chartdx) cursor_params_scratch: Vec<CursorParams>,
    /// CPU-side dirty flag for `GpuCanvasDriver::frame`: `prepare()` updated resident state, so the
    /// next platform tick must present even without GPUI dirtiness.
    pub(in crate::chartdx) needs_present: bool,
    /// Scene pixels changed since the optional DX11 cursor-restore cache was built.
    /// Live-scroll draws directly and invalidates that cache; cursor-only frames may rebuild it once.
    pub(in crate::chartdx) base_dirty: bool,
    pub(in crate::chartdx) last_present_at: Option<Instant>,
    pub(in crate::chartdx) target_present_interval: Duration,
    pub(in crate::chartdx) camera_shift_window_start: Option<Instant>,
    pub(in crate::chartdx) camera_shift_count: u32,
    pub(in crate::chartdx) camera_shift_hz: f32,
    pub(in crate::chartdx) last_gpu_prepare_generation: u64,
    pub(in crate::chartdx) text_runs: Vec<GpuCanvasTextRun>,
    pub(in crate::chartdx) text_run_cursor: usize,
    /// Retained runs for the configured captions, addressed by
    /// `(pane * CHART_LABEL_ROWS + row) * ROW_RUN_STRIDE + part` rather than by a running cursor.
    ///
    /// A separate pool precisely BECAUSE the cursor above is shared across panes and label kinds:
    /// an index from it moves whenever anything earlier in the frame stops drawing, and a run
    /// handed a different string reshapes it. A caption that appears and disappears — the scale
    /// badge, the comparison delta — would otherwise reshape its neighbours for free.
    pub(in crate::chartdx) caption_runs: Vec<GpuCanvasTextRun>,
    /// Lines of the PROSE captions on the pane being drawn, wrapped once and then measured and
    /// drawn from here.
    ///
    /// The caption pass measures a line, then measures it again to centre it, then again to draw
    /// it — which is free for a figure and is not free for a sentence that has to be broken on
    /// word boundaries first. Cleared per pane; an `Item` holds its index.
    pub(in crate::chartdx) caption_wraps: Vec<Vec<(String, f32)>>,
    /// Verified fit and wrap results retained across consecutive text passes.
    pub(in crate::chartdx) caption_fit_memo: text::FitMemo,
    /// Effective caption configuration, mirrored from `ChartDataState` so the text pass can read it
    /// without borrowing the data state during a frame.
    ///
    /// Behind an `Rc` because the draw pass takes a handle to it on every presented frame, per
    /// pane: the configuration owns a name string per row, and cloning it by value would allocate
    /// sixteen strings in the frame loop for nothing.
    pub(in crate::chartdx) chart_labels: Rc<moon_core::config::ChartLabelsCfg>,
    /// The closed trade this engine was handed, for the captions that state one.
    ///
    /// `None` on every live chart, which is what makes those captions print nothing there: they
    /// describe A trade, and a chart that was not handed one has none to describe. Mirrored here
    /// like `chart_labels` because the text pass reads it every frame and must not borrow the data
    /// state to do so.
    pub(in crate::chartdx) trade_labels: Option<Rc<TradeLabels>>,
    /// The chart's own candle timeframe in milliseconds, mirrored from `ChartDataState` like the
    /// captions above and for the same reason: a countdown caption set to `Авто` resolves against
    /// it while the text pass is running, and must not borrow the data state to read it.
    pub(in crate::chartdx) chart_tf_ms: i64,
    /// The arbitrage roster the caption column is arranged by, mirrored like `chart_labels` and for
    /// the same reason: the text pass reads it per pane on every rebuild and must not borrow the
    /// data state. GLOBAL — one roster for every chart — so every pane shares this handle.
    pub(in crate::chartdx) arb_view: Rc<moon_core::config::ArbViewCfg>,
    pub(in crate::chartdx) firetest_text_labels: Vec<String>,
    pub(in crate::chartdx) firetest_text_runs: Vec<GpuCanvasTextRun>,
    pub(in crate::chartdx) firetest_text_layer: GpuCanvasRetainedTextLayer,
    pub(in crate::chartdx) firetest_text_revision: u64,
    pub(in crate::chartdx) firetest_force_present: bool,
    pub(in crate::chartdx) ui_palette: moon_ui::MoonPalette,
    /// Top-left chart-slot origin in the backbuffer. UI cursor coordinates are local slot device
    /// pixels, while own-pass renders in window coordinates.
    pub(in crate::chartdx) slot_origin: [f32; 2],
    pub(in crate::chartdx) cursor: Option<CursorState>,
    /// Ghost crosshair price in comparison mode. A panel WITHOUT a real cursor draws a horizontal
    /// line at this price using its own Y mapping, plus order-book volume and percentage through
    /// `text/runs.rs::draw_ghost_cursor_labels`.
    /// The hovered sibling writes it through `ChartGhostCursor`, bypassing GPUI notification like
    /// the real cursor.
    pub(in crate::chartdx) ghost_price: Option<f32>,
    /// Anchor Last price for the large "+0.12%" delta below the corner label in broom mode. The stack
    /// supplies it through `apply_compare` on each observation. `None` means no comparison or this
    /// chart is the anchor.
    pub(in crate::chartdx) compare_ref_price: Option<f32>,
    /// When this chart arrived in a stack slot, driving the accent border flash and steady stroke.
    /// Retained after the flash while `arrival_hold` is set; the final arrival-present stamp stops
    /// extra presents once the stroke settles. Without the hold the frame loop clears it at the
    /// pulse deadline. `None` means the arrival decoration is not armed.
    pub(in crate::chartdx) arrival_pulse: Option<Instant>,
    /// Accent colour for the arrival flash, handed over with the stamp so the palette stays the
    /// single source of truth and this layer never guesses a colour.
    pub(in crate::chartdx) arrival_pulse_color: [f32; 4],
    /// Whether the border stays on as a steady stroke after the three pulses. Off, the pulses end
    /// with a clear present and the arrival is forgotten; the tab's popup decides, not this layer.
    pub(in crate::chartdx) arrival_hold: bool,
    /// When the last arrival frame was presented, pacing the flash to `ARRIVAL_PULSE_TICK`
    /// independently of the 60 Hz present cap. A stamp past expiry stops arrival presents after
    /// the final steady stroke has been scheduled.
    pub(in crate::chartdx) last_arrival_present_at: Option<Instant>,
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
    /// from wall clock, just as it ends or settles [`Self::arrival_pulse`] at its deadline, so
    /// nothing has to be trusted to call the caption clear.
    pub(in crate::chartdx) shot_caption_until: Option<Instant>,
    /// How many completed text passes have drawn substituted captions since it was armed.
    ///
    /// The shot's proof, and the reason it is safe to capture at all. A COUNT rather than a flag,
    /// with a threshold above one, because `prepare_text` having run does NOT prove the frame
    /// reached the screen: the fork's renderer skips `draw` outright on the first frame after a
    /// DirectX device recovery and swallows a `can_present` refusal the same way, while the canvas
    /// text pass still runs. A single drawn pass could therefore be one the GPU discarded, and
    /// capturing on it would put the ACCOUNT NAME on the clipboard — the one outcome this exists
    /// to prevent.
    pub(in crate::chartdx) shot_caption_frames: u8,
    /// Device generation the proof has been counted against, to notice a recovery mid-shot.
    pub(in crate::chartdx) shot_caption_device_gen: u64,
    /// Bumped on every ARM, so a superseded shot can tell it has been replaced.
    ///
    /// Two presses in quick succession run two wait chains against this one engine. The second
    /// arming zeroes the frame count the first is still waiting on, and without a generation the
    /// first would sit out its budget and report a failure for a shot that was simply replaced. It
    /// never affected what gets CAPTURED — the count is zeroed before any later frame is tallied —
    /// only what gets reported.
    pub(in crate::chartdx) shot_caption_gen: u64,
    pub(in crate::chartdx) cursor_color: [f32; 4],
    pub(in crate::chartdx) cursor_thickness: f32,
    pub(in crate::chartdx) readout_bg: [f32; 4],
    pub(in crate::chartdx) readout_soft_bg: [f32; 4],
    pub(in crate::chartdx) readout_order_bg: [f32; 4],
    pub(in crate::chartdx) readout_border: [f32; 4],
    pub(in crate::chartdx) readout_border_px: f32,
    pub(in crate::chartdx) label_positive: u32,
    pub(in crate::chartdx) label_negative: u32,
    pub(in crate::chartdx) label_neutral: u32,
    pub(in crate::chartdx) axis_label: u32,
    pub(in crate::chartdx) caption_label: u32,
    pub(in crate::chartdx) readout_label: u32,
    /// Order-line and cursor label font-size adjustment in pixels from `ChartTheme.label_font_delta`.
    /// `text/runs.rs` applies it through label draw/measure helpers used by the line-label column and
    /// cursor readout in `text/prepare.rs`.
    pub(in crate::chartdx) label_font_delta: f32,
    /// Whether to show per-tab order-line labels from the ⚙ popup. Disabled hides the line-label
    /// column built by `text/prepare.rs::prepare_text`.
    pub(in crate::chartdx) line_labels: bool,
    /// Whether to show crosshair readout labels for time, price, percentage, volume, and size.
    /// Disabled hides cursor values prepared by `text/prepare.rs::prepare_text` and ghost labels
    /// drawn by `text/runs.rs::draw_ghost_cursor_labels`.
    pub(in crate::chartdx) cursor_labels: bool,
    /// Marker drawn beside the crosshair while a mode is active — today the Sells-to-zone
    /// drawing mode. `None` draws nothing. It rides the crosshair rather than the GPUI tree, so following
    /// the pointer costs no repaint of the view tree.
    ///
    /// A `&'static str` because a mode marker is a GLYPH, not a sentence: nothing to translate and
    /// nothing to allocate on the present path that redraws it.
    pub(in crate::chartdx) cursor_badge: Option<&'static str>,
    /// The percent ruler while a drag holds it (`ruler.rs`): its band rides the readout batch and
    /// its text the text pass, both redrawn per present, so moving it costs no layer rebuild.
    pub(in crate::chartdx) ruler: Option<ruler::RulerReadout>,
    pub(in crate::chartdx) pixel_scale: f32,
    /// Lazily created own-pass scissor rasterizer, recreated on device changes. It clips layers to
    /// the panel so price-positioned order books and orders cannot spill beyond the plot onto
    /// toolbars or scales.
    #[cfg(windows)]
    pub(in crate::chartdx) scissor_rs: Option<ID3D11RasterizerState>,
    #[cfg(windows)]
    pub(in crate::chartdx) scissor_generation: u64,
    /// Dark base of the chart, equal to `rgb4(theme.bg)` and updated in `prepare`. The base
    /// texture is CLEARED to it, which is both the fill and — see `base.rs`'s module doc — what
    /// keeps a bake/blit divergence off the screen. Do not clear that texture to zero.
    ///
    /// Within the blitted slot it is what covers GPUI or SwapChain's unpainted white background on
    /// the first frame. The branded empty-state logo is a GPUI SVG layer, not a native raster
    /// splash.
    #[cfg(windows)]
    pub(in crate::chartdx) window_bg_color: [f32; 4],
    #[cfg(windows)]
    pub(in crate::chartdx) base_cache: base::BaseCache,
}

const READOUT_FALLBACK_FONT_W: f32 = 8.5;
const READOUT_PAD_X: f32 = 5.0;
const READOUT_PAD_Y: f32 = 2.5;
const READOUT_INSET: f32 = 2.0;

#[cfg(windows)]
/// The clip for the order-line and trade-mark pass: the pane, less the horizontal-volume zone
/// when one is carved out beside the plot (`HvolStyleGpu::user_clip_left`). The zone sits at the
/// pane's left edge, so this only moves the left side.
fn userdata_clip(pane_clip: [f32; 4], hvol: &super::HvolStyleGpu) -> [f32; 4] {
    let Some(edge) = hvol.user_clip_left() else {
        return pane_clip;
    };
    let left = edge.ceil().clamp(pane_clip[0], pane_clip[2] - 1.0);
    [left, pane_clip[1], pane_clip[2], pane_clip[3]]
}

fn bounds_clip(bounds: [f32; 4], res: [f32; 2]) -> [f32; 4] {
    // IMPORTANT: `clamp(min, max)` panics when min exceeds max. With degenerate panel bounds such as
    // zero width or a panel touching the right or bottom edge, `l` can equal the resolution. Then
    // `l + 1.0 > res` previously killed the frame with e.g. f32 clamp min=1681, max=1680 during
    // resize or reconnect. Use `max(res, l+1)` as the upper bound.
    let l = bounds[0].floor().clamp(0.0, res[0].max(1.0));
    let t = bounds[1].floor().clamp(0.0, res[1].max(1.0));
    let r = (bounds[0] + bounds[2])
        .ceil()
        .clamp(l + 1.0, res[0].max(l + 1.0));
    let b = (bounds[1] + bounds[3])
        .ceil()
        .clamp(t + 1.0, res[1].max(t + 1.0));
    [l, t, r, b]
}

fn readout_text_width(label: &str, measured: f32) -> f32 {
    measured.max(label.chars().count() as f32 * READOUT_FALLBACK_FONT_W)
}

fn readout_rect_dst(
    anchor_x: f32,
    anchor_y: f32,
    text_w: f32,
    line_h: f32,
    ax: f32,
    ay: f32,
    scale: f32,
) -> [f32; 4] {
    let x = anchor_x - text_w * ax - READOUT_PAD_X;
    let y = anchor_y - line_h * ay - READOUT_PAD_Y;
    [
        x * scale,
        y * scale,
        (text_w + READOUT_PAD_X * 2.0) * scale,
        (line_h + READOUT_PAD_Y * 2.0) * scale,
    ]
}

fn clamp_anchor(value: f32, min: f32, max: f32) -> f32 {
    if min <= max {
        value.clamp(min, max)
    } else {
        (min + max) * 0.5
    }
}

/// How long a newly arrived chart pulses before its border ends or, with the hold, settles.
///
/// Lives here, next to the code that draws and paces it, rather than in `chart_tabs`: the flash
/// is an own-pass decoration now, and a duration split across two modules is exactly the drift this
/// change exists to remove.
const ARRIVAL_HIGHLIGHT: Duration = Duration::from_millis(2600);

/// Returns the three-pulse opacity, then — with `hold` — a calmer persistent stroke.
/// `None` means no arrival is armed, or the pulses ended and nothing asked the border to stay.
fn arrival_alpha(elapsed: Option<Duration>, hold: bool) -> Option<f32> {
    let elapsed = elapsed?;
    if elapsed < ARRIVAL_HIGHLIGHT {
        let delta = elapsed.as_secs_f32() / ARRIVAL_HIGHLIGHT.as_secs_f32();
        Some((delta * std::f32::consts::PI * 3.0).sin().abs())
    } else {
        hold.then_some(0.6)
    }
}

/// Requests paced pulse frames and exactly one settling frame, even after a hidden interval.
/// A last present at or beyond the pulse deadline means the steady stroke is already scheduled.
fn arrival_present_due(at: Instant, last: Option<Instant>, now: Instant) -> bool {
    if now.saturating_duration_since(at) >= ARRIVAL_HIGHLIGHT {
        last.is_none_or(|last| last.saturating_duration_since(at) < ARRIVAL_HIGHLIGHT)
    } else {
        last.is_none_or(|last| now.saturating_duration_since(last) >= ARRIVAL_PULSE_TICK)
    }
}

/// Present interval while the arrival flash runs.
///
/// Ten per second is not ten cheap frames: the fork ORs every canvas's present request together, so
/// each one re-runs `prepare_gpu`, `prepare_text` and `draw` for EVERY canvas in the window. Cheap
/// against a full GPUI view render, not cheap in absolute terms — do not raise this rate casually.
///
/// Shared with the News tint rather than redeclared:
/// both are decorations that do not need the vblank rate, and two 100 ms constants in two modules
/// is the drift this change exists to remove, not to re-create.
const ARRIVAL_PULSE_TICK: Duration = crate::pulse::PULSE_TICK;

/// Stroke width of the arrival border, in logical px before the DPI scale is applied.
const ARRIVAL_BORDER_PX: f32 = 2.0;

/// Whether the arrival border flash runs at all.
///
/// `MOON_ARRIVAL_FLASH=0` (also `false`/`no`/`off`) turns it off for the whole process, so a
/// measurement run can compare the same binary with and against without it. Its cost is not a
/// question code reading answers: the flash paces PRESENTS, and a present is a WINDOW present, so
/// every sibling canvas re-runs its own pass — which is exactly the kind of load only a live A/B
/// establishes.
///
/// Gated here rather than at the three `flash_arrival` call sites: this is the one place a flash
/// becomes state, so a caller added later is covered without knowing the switch exists. Read once —
/// a per-frame `var_os` on the chart path would itself distort what it is meant to measure.
pub(crate) fn arrival_flash_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("MOON_ARRIVAL_FLASH")
            .ok()
            .map(|value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(true)
    })
}

fn sync_readout_resolution(rects: &mut [ReadoutRect], res: [f32; 2]) {
    let w = res[0].max(1.0);
    let h = res[1].max(1.0);
    for rect in rects {
        rect.m[1] = w;
        rect.m[2] = h;
    }
}

/// Completed text passes that must have drawn the substituted caption before a shot may capture.
///
/// TWO, not one, and the second one is the whole point. A single pass proves only that the caption
/// was BUILT: the fork's renderer returns from `draw` without drawing on the first frame after a
/// DirectX device recovery and treats a `can_present` refusal the same way, while the canvas text
/// pass still runs. Capturing on that one pass photographs the PREVIOUS frame, which still carries
/// the user's account name.
///
/// A recovery raises the device generation, which resets the count, so reaching two again means two
/// passes drawn on the current device with no recovery between them.
const SHOT_CAPTION_MIN_FRAMES: u8 = 2;

/// How finely the measuring anchor follows the pointer, in milliseconds.
///
/// A second is under the width of one aggregate bucket, so nothing the history can resolve is lost
/// — and it is coarse enough that dragging the mouse across a chart asks for a handful of distinct
/// periods rather than one per pixel. Both caches downstream are keyed by this value.
const CURSOR_QUANTUM_MS: i64 = 1_000;

impl RenderState {
    pub(super) fn set_target_present_rate_hz(&mut self, hz: f32) {
        let hz = hz.clamp(1.0, 240.0);
        self.target_present_interval = Duration::from_secs_f64(1.0 / hz as f64);
    }

    pub(super) fn record_camera_shift(&mut self, now: Instant) {
        if self.camera_shift_window_start.is_none() {
            self.camera_shift_window_start = Some(now);
        }
        self.camera_shift_count = self.camera_shift_count.saturating_add(1);
        self.update_camera_shift_hz(now);
    }

    pub(super) fn camera_shift_hz(&mut self) -> f32 {
        self.update_camera_shift_hz(Instant::now());
        self.camera_shift_hz
    }

    pub(super) fn update_camera_shift_hz(&mut self, now: Instant) {
        let Some(start) = self.camera_shift_window_start else {
            self.camera_shift_window_start = Some(now);
            return;
        };
        let elapsed = now.duration_since(start);
        if elapsed < Duration::from_secs(1) {
            return;
        }
        self.camera_shift_hz = self.camera_shift_count as f32 / elapsed.as_secs_f32().max(1e-3);
        self.camera_shift_count = 0;
        self.camera_shift_window_start = Some(now);
    }

    pub(super) fn set_slot_origin(&mut self, x: f32, y: f32) {
        let next = [x, y];
        if self.slot_origin != next {
            self.slot_origin = next;
            self.base_dirty = true;
            self.needs_present = true;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    pub(super) fn set_cursor_style(&mut self, color: [f32; 4], thickness: f32) {
        let thickness = thickness.max(1.0);
        if self.cursor_color != color || self.cursor_thickness != thickness {
            self.cursor_color = color;
            self.cursor_thickness = thickness;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    pub(super) fn set_readout_style(
        &mut self,
        bg: [f32; 4],
        soft_bg: [f32; 4],
        order_bg: [f32; 4],
        border: [f32; 4],
        border_px: f32,
    ) {
        let border_px = border_px.max(0.0);
        if self.readout_bg != bg
            || self.readout_soft_bg != soft_bg
            || self.readout_order_bg != order_bg
            || self.readout_border != border
            || (self.readout_border_px - border_px).abs() > 0.001
        {
            self.readout_bg = bg;
            self.readout_soft_bg = soft_bg;
            self.readout_order_bg = order_bg;
            self.readout_border = border;
            self.readout_border_px = border_px;
            self.sync_readout_params();
            self.needs_present = true;
        }
    }

    pub(super) fn set_pixel_scale(&mut self, scale: f32) {
        let scale = scale.max(0.1);
        if (self.pixel_scale - scale).abs() > 0.001 {
            self.pixel_scale = scale;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    pub(super) fn set_cursor(&mut self, cursor: Option<CursorState>) -> bool {
        if self.cursor == cursor {
            return false;
        }
        self.cursor = cursor;
        self.sync_cursor_params();
        self.needs_present = true;
        true
    }

    /// Set the comparison-mode ghost crosshair price written by the hovered sibling tab.
    ///
    /// A price change requests present itself, so siblings do not need continuous presentation.
    pub(super) fn set_ghost_price(&mut self, price: Option<f32>) -> bool {
        if self.ghost_price.map(f32::to_bits) == price.map(f32::to_bits) {
            return false;
        }
        crate::diag::bump(&crate::diag::CHART_GHOST_UPDATE);
        self.ghost_price = price;
        self.sync_cursor_params();
        self.needs_present = true;
        true
    }

    /// Set the comparison anchor's Last for the large delta under the broom-mode corner label.
    ///
    /// A changed value requests present, keeping the delta current even when only the anchor ticks.
    pub(super) fn set_compare_ref_price(&mut self, price: Option<f32>) -> bool {
        if self.compare_ref_price.map(f32::to_bits) == price.map(f32::to_bits) {
            return false;
        }
        self.compare_ref_price = price;
        // The delta is FORMATTED on a sync, not on a frame, and the anchor's tick is not this
        // pane's revision: without re-resolving here a quiet follower would keep printing the
        // percentage it computed against an older anchor price while the anchor moved.
        for idx in 0..self.panes.len() {
            self.refresh_pane_labels(idx);
        }
        self.needs_present = true;
        true
    }

    /// Arm or clear the shot's caption substitution.
    ///
    /// Arming zeroes the drawn-frame count first, so a shot can never read a stale proof left by an
    /// earlier press and capture a frame that was drawn before the swap.
    ///
    /// `until` is a wall-clock deadline rather than a duration because `frame` is what expires it,
    /// and `frame` has no memory of when the caller armed it. Pass `None` to restore the core name
    /// immediately; the shot does that itself on every path, and the deadline is the watchdog
    /// behind it rather than the primary mechanism.
    ///
    /// Args:
    ///     until: Instant past which the caption returns to the core name, or `None` to restore it
    ///         now.
    ///
    /// Returns:
    ///     Whether anything changed, in the house convention meaning the caller should repaint.
    pub(super) fn arm_shot_caption(&mut self, until: Option<Instant>) -> bool {
        if self.shot_caption_until == until {
            return false;
        }
        self.shot_caption_until = until;
        self.shot_caption_frames = 0;
        if until.is_some() {
            // Only an ARM opens a new shot. A disarm ends one, and bumping there would make the
            // chain that is doing the disarming look superseded by itself.
            self.shot_caption_gen = self.shot_caption_gen.wrapping_add(1);
        }
        // Both directions present: arming has to reach a frame for the shot to have anything to
        // capture, and clearing has to reach one or the screen keeps the exchange on it.
        self.needs_present = true;
        true
    }

    /// Whether a shot's caption substitution is in force right now.
    ///
    /// Read by the label build and by the order sync, which is why it is a method rather than an
    /// inlined `is_some()` at each site: those two must never disagree about whether a shot is on.
    pub(super) fn shot_caption_active(&self) -> bool {
        self.shot_caption_until.is_some()
    }

    /// Whether the substituted caption has satisfied the renderer-side pre-capture proof.
    ///
    /// Returns:
    ///     `true` once enough completed `prepare_text` passes have drawn substituted captions.
    pub(super) fn shot_caption_drawn(&self) -> bool {
        self.shot_caption_frames >= SHOT_CAPTION_MIN_FRAMES
    }

    /// Which arming of the caption is currently in force.
    ///
    /// Returns:
    ///     A counter a waiting shot compares against to notice it has been superseded.
    pub(super) fn shot_caption_gen(&self) -> u64 {
        self.shot_caption_gen
    }

    /// Count one completed text pass towards the shot's proof.
    ///
    /// Called ONLY from the end of `prepare_text`, and only once every fallible draw in that pass
    /// has succeeded: the fork appends the canvas text frame only when `prepare_text` returns `Ok`,
    /// so committing at a draw site would count a pass whose text frame was then discarded — which
    /// is precisely the blind capture the proof exists to prevent.
    ///
    /// Args:
    ///     device_gen: Highest device generation across the panes drawn in this pass.
    pub(super) fn note_shot_caption_drawn(&mut self, device_gen: u64) {
        // A device recovery invalidates everything counted before it: that frame's `draw` is
        // skipped wholesale, so passes counted on the old generation say nothing about what is on
        // the screen now. Start again rather than letting them add up across the boundary.
        if self.shot_caption_device_gen != device_gen {
            self.shot_caption_device_gen = device_gen;
            self.shot_caption_frames = 0;
        }
        self.shot_caption_frames = self.shot_caption_frames.saturating_add(1);
    }

    /// Starts or clears the arrival's pulsing and steady border, returning whether it changed.
    /// The own-pass frame callback handles pacing without a timer or per-frame view notification.
    ///
    /// `hold` says whether the border outlives the pulses. Re-arming an old stamp with the hold on
    /// is one settling present: the stroke appears at once when the hold is switched on for charts
    /// that are already there. Without the hold an old stamp has nothing left to draw, so it arms
    /// nothing — the stack re-runs every chart's arrival on a look change, and a present that
    /// changes no pixel would be paid by every sibling canvas in the window.
    pub(super) fn set_arrival_pulse(
        &mut self,
        at: Option<Instant>,
        accent: [f32; 4],
        hold: bool,
    ) -> bool {
        // Switched off: every start becomes a clear, so a run with the flash disabled cannot be
        // left with one already in flight from before the call.
        let at = if arrival_flash_enabled() { at } else { None };
        let at = at.filter(|at| hold || at.elapsed() < ARRIVAL_HIGHLIGHT);
        // Refresh a changed colour or hold even after the pulse has stopped scheduling frames —
        // when something is armed to show it.
        let restyled = self.arrival_pulse_color != accent || self.arrival_hold != hold;
        self.arrival_pulse_color = accent;
        self.arrival_hold = hold;
        if self.arrival_pulse == at {
            let visible = restyled && at.is_some();
            if visible {
                self.sync_readout_params();
                self.needs_present = true;
            }
            return visible;
        }
        self.arrival_pulse = at;
        self.last_arrival_present_at = None;
        self.sync_readout_params();
        // Arming already schedules an overlay-only present through `arrival_present_due`.
        // Only clearing needs a generic present; preserve any independent dirty reason on arm.
        if at.is_none() {
            self.needs_present = true;
        }
        true
    }

    pub(super) fn set_firetest_force_present(&mut self, enabled: bool) -> bool {
        if self.firetest_force_present == enabled {
            return false;
        }
        self.firetest_force_present = enabled;
        if enabled {
            self.needs_present = true;
        }
        true
    }

    /// The moment the pointer is on, in unix milliseconds, QUANTIZED.
    ///
    /// The measuring anchor's whole input. Quantized here rather than where it is read because it
    /// becomes part of the caption cache key and of the readout cache key behind it: an unrounded
    /// value would make every pixel of mouse travel a fresh key, missing both caches and formatting
    /// the block again on each one.
    ///
    /// `None` when the pointer is not over THIS pane, or while the pane has no usable time
    /// mapping — a collapsed chart has a zero scale, and dividing by it would place the pointer at
    /// infinity.
    ///
    /// Args:
    ///     idx: Pane index.
    ///
    /// Returns:
    ///     The quantized moment under the pointer.
    pub(in crate::chartdx) fn pane_cursor_unix_ms(&self, idx: usize) -> Option<i64> {
        let cursor = self.cursor.filter(|c| c.pane == idx)?;
        let pane = self.panes.get(idx)?;
        let time_to_px = pane.view.time_to_px;
        if !(time_to_px > 0.0) {
            return None;
        }
        // TWO different origins meet here, and mixing them is a silent error worth stating: the
        // cursor is stored SLOT-relative (`(position - slot_origin) * scale_factor`), while a
        // view's bounds are WINDOW-global — `origin + chart_area`. Every other consumer of the pair
        // adds the slot origin back before comparing them, and so does this. Skipping it shifts the
        // measured moment by the whole left dock, which at normal zoom is minutes.
        let cursor_x = self.slot_origin[0] + cursor.local[0];
        let rel_ms = f64::from(pane.view.view_time0)
            + f64::from(cursor_x - pane.view.bounds[0]) / f64::from(time_to_px);
        let unix = pane.epoch_ms + rel_ms;
        if !unix.is_finite() {
            return None;
        }
        let unix = unix as i64;
        Some(unix.div_euclid(CURSOR_QUANTUM_MS) * CURSOR_QUANTUM_MS)
    }

    pub(super) fn sync_cursor_params(&mut self) {
        for (idx, pr) in self.panes.iter_mut().enumerate() {
            let right = (pr.orderbook_view.bounds[0] + pr.orderbook_view.bounds[2])
                .max(pr.view.bounds[0] + pr.view.bounds[2]);
            // The crosshair reaches across the horizontal-volume zone as it does across the book:
            // the volume readout prints on its line there.
            let zone = pr.hvol_style.zone;
            let left = if zone[2] >= 1.0 {
                pr.view.bounds[0].min(zone[0])
            } else {
                pr.view.bounds[0]
            };
            let bounds = [
                left,
                pr.view.bounds[1],
                (right - left).max(1.0),
                pr.view.bounds[3].max(1.0),
            ];
            let mut params = CursorParams {
                bounds,
                resolution: pr.view.resolution,
                color: self.cursor_color,
                thickness: self.cursor_thickness.max(1.0),
                ..CursorParams::default()
            };
            if pr.active {
                if let Some(cursor) = self.cursor.filter(|c| c.pane == idx) {
                    params.cursor = [
                        self.slot_origin[0] + cursor.local[0],
                        self.slot_origin[1] + cursor.local[1],
                    ];
                    params.enabled = 1.0;
                } else if let Some(price) = self.ghost_price {
                    // Comparison ghost: draw a horizontal at the sibling price using THIS panel's Y
                    // mapping. An out-of-bounds X makes cursor.hlsl suppress the vertical through its
                    // bounds check, while the horizontal has its own Y check and vanishes when price
                    // leaves the window. A broom-collapsed chart retains a valid order-book view with
                    // the same height and window.
                    let v = if pr.view.price_to_px > 0.0 {
                        &pr.view
                    } else {
                        &pr.orderbook_view
                    };
                    if v.price_to_px > 0.0 {
                        let bottom = v.bounds[1] + v.bounds[3];
                        let y = bottom - (price - v.view_price0) * v.price_to_px;
                        if y.is_finite() {
                            params.cursor = [-1.0e6, y];
                            params.enabled = 1.0;
                        }
                    }
                }
            }
            #[cfg(not(windows))]
            let changed = pr.cursor_params != params;
            pr.cursor_params = params;
            #[cfg(not(windows))]
            if changed {
                // Cursor uniforms/readout rects are uploaded from the draw callback on
                // Metal/wgpu. Treating cursor motion as prepare-dirty turns mouse-only
                // frames into full chart prepares and defeats the retained cursor path.
                self.needs_present = true;
            }
        }
        self.sync_readout_params();
    }

    /// Rebuilds the GPU readout rectangles from geometry published by text preparation.
    ///
    /// The corner-caption plate is consumed verbatim rather than re-derived here, preventing the
    /// background from drifting away when the order-book bounds or caption anchoring change.
    pub(super) fn sync_readout_params(&mut self) {
        let sf = self.pixel_scale.max(0.1);
        let bg = self.readout_bg;
        let border = self.readout_border;
        let border_px = self.readout_border_px;
        let m = [border_px, 1.0, 1.0, 0.0];
        let cursor = self.cursor;
        let slot_origin = self.slot_origin;
        // Sample once for every pane: three pulses, then a steady stroke when the hold is on.
        let arrival_alpha =
            arrival_alpha(self.arrival_pulse.map(|at| at.elapsed()), self.arrival_hold);
        let arrival_color = self.arrival_pulse_color;
        let ruler = self.ruler.as_ref().map(|r| r.span);

        for (idx, pr) in self.panes.iter_mut().enumerate() {
            pr.readout_rects.clear();
            if !pr.active {
                continue;
            }

            // The arrival flash is one more instance in the readout batch — no new layer, no new
            // draw call, no base-cache rebuild. Pushed FIRST so cursor plates and labels stay above
            // it. A transparent fill leaves only the stroke.
            if let Some(alpha) = arrival_alpha {
                // Flush with the pane edge, NOT inset: `readout.hlsl` strokes the border on the
                // inside of `dst`, so the rect is already the outer edge. The GPUI version needed a
                // 1 px inset only because `overflow_hidden` clipped it — there is nothing to clip
                // here, and an inset reads as the frame floating away from the sides.
                let w = ARRIVAL_BORDER_PX * sf;
                pr.readout_rects.push(ReadoutRect {
                    dst: pr.pane_bounds,
                    bg: [0.0, 0.0, 0.0, 0.0],
                    border: [
                        arrival_color[0],
                        arrival_color[1],
                        arrival_color[2],
                        arrival_color[3] * alpha,
                    ],
                    m: [w, 1.0, 1.0, 0.0],
                });
            }

            // The percent ruler's band, in the order book's own buy or sell colour by the move's
            // direction, translucent so the candles under it stay readable. Pushed right after the
            // arrival frame, so every plate and label below lands above it. In this batch rather
            // than the figure layer: a fill there re-bakes the base cache on every pointer move.
            if let Some(span) = ruler.filter(|span| span.pane == idx)
                && let Some(dst) = super::ruler::band_rect_px(&pr.view, pr.epoch_ms, &span)
            {
                let hue = if span.up() {
                    pr.book_style.bid
                } else {
                    pr.book_style.ask
                };
                pr.readout_rects.push(ReadoutRect {
                    dst,
                    bg: [hue[0], hue[1], hue[2], hue[3] * super::ruler::FILL_ALPHA],
                    border: [hue[0], hue[1], hue[2], hue[3] * super::ruler::BORDER_ALPHA],
                    m: [sf, 1.0, 1.0, 0.0],
                });
            }

            let pane_left = pr.pane_bounds[0] / sf;
            let pane_right = (pr.pane_bounds[0] + pr.pane_bounds[2]) / sf;
            let pane_bottom = (pr.pane_bounds[1] + pr.pane_bounds[3]) / sf;
            let plot_left = pr.view.bounds[0] / sf;
            let plot_top = pr.view.bounds[1] / sf;
            let plot_w = pr.view.bounds[2] / sf;
            let plot_h = pr.view.bounds[3] / sf;
            let plot_right = plot_left + plot_w;
            // Price-axis side: Hide omits the cursor-price plate because no axis or gutter exists;
            // Right places it at the panel's right edge beyond the order book. Keep this synchronized
            // with `text/prepare.rs::prepare_text`.
            use crate::persistence::chart_persist::PriceAxisPos;
            let axis_hidden = matches!(pr.price_axis_pos, PriceAxisPos::Hide);
            let axis_on_right = matches!(pr.price_axis_pos, PriceAxisPos::Right);

            // Translucent corner-label backing plate with alpha 0.2, or 80% transparency.
            //
            // The rectangle is NOT recomputed here: `text/prepare.rs::prepare_text` owns the
            // caption's geometry and publishes the finished plate, so the plate can no longer drift
            // away from the text. Pushed BEFORE the `plot_w<60` gate so order-book-only broom
            // followers keep a plate under their label while the chart is collapsed.
            for dst in pr.caption_plates {
                if dst[3] > 0.0 {
                    pr.readout_rects.push(ReadoutRect {
                        dst,
                        bg: self.readout_soft_bg,
                        border,
                        m,
                    });
                }
            }

            // Buy/sell proportion bars, published by the caption pass with the geometry it drew
            // the figures at. Two instances each — the track, then the filled part — in the SAME
            // batch as everything above: no new layer and no new draw call, which is what makes a
            // bar beside every volume caption affordable.
            //
            // The colours are the ORDER BOOK's own bid and ask: buying and selling already mean
            // those two colours everywhere else on this chart, and a third pair would make the
            // reader learn a second vocabulary for the same two facts.
            for bar in &pr.caption_bars {
                if bar.dst[2] <= 0.0 || bar.dst[3] <= 0.0 {
                    continue;
                }
                pr.readout_rects.push(ReadoutRect {
                    dst: bar.dst,
                    bg: self.readout_soft_bg,
                    border,
                    m,
                });
                let filled = bar.dst[2] * bar.fill.clamp(0.0, 1.0);
                if filled <= 0.0 {
                    continue;
                }
                let fill_color = match bar.sell {
                    true => pr.book_style.ask,
                    false => pr.book_style.bid,
                };
                pr.readout_rects.push(ReadoutRect {
                    dst: [bar.dst[0], bar.dst[1], filled, bar.dst[3]],
                    bg: fill_color,
                    // No border on the filled part: it sits INSIDE the track, whose own border is
                    // already drawn, and a second stroke on top of it reads as a second bar.
                    border: [0.0, 0.0, 0.0, 0.0],
                    m: [0.0, 1.0, 1.0, 0.0],
                });
            }

            // Backing plates for order and cursor labels laid out by `prepare_text`. Order labels use
            // a light alpha-0.2 plate like the market corner label; priority foreground cursor labels
            // use a dense alpha-0.96 plate. Build them BEFORE the cursor gate because order labels are
            // visible without a cursor, and BEFORE the collapsed-chart gate because a broom follower's
            // comparison ghost places volume and percentage here and needs a backing plate.
            let placed = std::mem::take(&mut pr.label_placed);
            for pl in &placed {
                let dst = readout_rect_dst(pl.x, pl.y, pl.w, pl.h, pl.ax, pl.ay, sf);
                // `solid` selects a dense cursor plate; otherwise use a semitransparent order plate
                // that lets a lower-priority label slide beneath a higher one during overlap.
                let pbg = if pl.solid { bg } else { self.readout_order_bg };
                pr.readout_rects.push(ReadoutRect {
                    dst,
                    bg: pbg,
                    border,
                    m,
                });
            }
            pr.label_placed = placed;

            // Remaining cursor plates and axes apply only to a normal, non-collapsed chart.
            if plot_w < 60.0 || plot_h < 60.0 || pr.view.price_to_px <= 0.0 {
                continue;
            }

            let plot_bottom = plot_top + plot_h;

            let Some(cursor) = cursor.filter(|c| c.pane == idx) else {
                continue;
            };
            let cx_log = (slot_origin[0] + cursor.local[0]) / sf;
            let cy_log = (slot_origin[1] + cursor.local[1]) / sf;

            let time_to_px = (pr.view.time_to_px / sf).max(moon_chart::view::MIN_PX_PER_MS);
            if cx_log >= plot_left && cx_log <= plot_right {
                let left_unix = pr.epoch_ms + pr.view.view_time0 as f64;
                let unix = left_unix + (cx_log - plot_left) as f64 / time_to_px as f64;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_millis() as f64);
                // A date other than today uses day-month plus time for large time frames or windows.
                let label = crate::chartdx::axes::format_clock_dated(unix, true, now_ms);
                let text_w = readout_text_width(&label, pr.readout_time_width);
                let line_h = pr.readout_time_line_h.max(1.0);
                let half_w = text_w * 0.5;
                let x = clamp_anchor(
                    cx_log,
                    plot_left + half_w + READOUT_PAD_X + READOUT_INSET,
                    plot_right - half_w - READOUT_PAD_X - READOUT_INSET,
                );
                let dst = readout_rect_dst(x, pane_bottom - 1.0, text_w, line_h, 0.5, 1.0, sf);
                pr.readout_rects.push(ReadoutRect { dst, bg, border, m });
            }

            if !axis_hidden && cy_log >= plot_top && cy_log <= plot_bottom {
                let price_to_px = pr.view.price_to_px / sf;
                let price_range = plot_h / price_to_px.max(1e-6);
                let y_min = pr.view.view_price0;
                let dec = moon_chart::axes::price_decimals(y_min + price_range * 0.5);
                let price = y_min + (plot_bottom - cy_log) / price_to_px.max(1e-6);
                let label = format!("{price:.dec$}");
                let text_w = readout_text_width(&label, pr.readout_price_width);
                let line_h = pr.readout_price_line_h.max(1.0);
                let x = if axis_on_right {
                    pane_right - 3.0
                } else {
                    (plot_left - 3.0).max(pane_left + READOUT_INSET + READOUT_PAD_X + text_w)
                };
                let dst = readout_rect_dst(x, cy_log, text_w, line_h, 1.0, 0.5, sf);
                pr.readout_rects.push(ReadoutRect { dst, bg, border, m });
            }
        }
    }

    /// Requests presents for changed chart state and bounded decorations; a steady border is idle.
    ///
    /// Monotonic decisions — arrival pacing, the present cap, shot-caption expiry, and the
    /// camera-shift window — read `info.now`. Camera positions stay on the wall clock.
    /// An arrival-only present does not advance the camera.
    ///
    /// Args:
    ///     info: Frame input from the GPU canvas. `info.now` drives scheduling; wall-clock
    ///         `now_unix_ms` still places the camera.
    ///
    /// Returns:
    ///     `RequestPresent` when the canvas changed, otherwise `Skip`.
    pub(super) fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        crate::diag::bump(&crate::diag::CHART_FRAME);
        if !info.presentable || info.bounds.is_empty() {
            crate::diag::bump(&crate::diag::CHART_FRAME_SKIP_NOT_PRESENTABLE);
            return GpuFrameDecision::Skip;
        }

        // Wall clock for camera positions. `info.now` is the monotonic clock for pacing.
        let now_ms = now_unix_ms();
        let now = info.now;
        let mut wants_present = std::mem::take(&mut self.needs_present);
        if self.firetest_force_present {
            wants_present = true;
        }
        // Shot caption: ENDED here, from the frame clock, with
        // no timer, no notify, and nobody to trust with the clear. This is the WATCHDOG, not the
        // normal path: the shot restores the caption itself as soon as it has its picture, and this
        // only fires when that chain never completed. Leaving it armed would keep the EXCHANGE on
        // the user's own screen, where the core name belongs.
        if let Some(deadline) = self.shot_caption_until
            && now >= deadline
        {
            self.shot_caption_until = None;
            self.shot_caption_frames = 0;
            wants_present = true;
        }
        // Preserve independent reasons to update the camera before adding an overlay-only present.
        let camera_present = wants_present;
        let mut arrival_present = false;
        if let Some(at) = self.arrival_pulse {
            if !self.arrival_hold && now.saturating_duration_since(at) >= ARRIVAL_HIGHLIGHT {
                // No hold: the pulses are over, so forget the arrival and present once without
                // the stroke. Clearing here is the load-bearing line — armed, this canvas would
                // keep answering `arrival_present_due` with one settling frame per reveal.
                self.arrival_pulse = None;
                self.last_arrival_present_at = None;
                self.sync_readout_params();
                arrival_present = true;
                wants_present = true;
            } else if arrival_present_due(at, self.last_arrival_present_at, now) {
                // Keep the colour armed, but stop requesting frames after the final steady stroke.
                self.last_arrival_present_at = Some(now);
                self.sync_readout_params();
                crate::diag::bump(&crate::diag::CHART_ARRIVAL_PULSE);
                arrival_present = true;
                wants_present = true;
            }
        }

        let cap_due = self
            .last_present_at
            .is_none_or(|last| now.duration_since(last) >= self.target_present_interval);
        let mut camera_moved = false;
        for pr in &mut self.panes {
            // A pulse-only tick reuses the base even when the camera cap is due. Normal platform
            // ticks still advance live scrolling, and real data/cursor dirtiness keeps its priority.
            if pr.active
                && (camera_present || (!arrival_present && cap_due))
                && pr.advance_camera(pr.live_clock.edge_ms(now_ms))
            {
                crate::diag::bump(&crate::diag::CHART_CAM_STEP);
                camera_moved = true;
                self.base_dirty = true;
                wants_present = true;
            }
        }
        if camera_moved {
            self.record_camera_shift(now);
        }

        if wants_present {
            self.last_present_at = Some(now);
            crate::diag::bump(&crate::diag::CHART_FRAME_REQUEST);
            GpuFrameDecision::RequestPresent
        } else {
            crate::diag::bump(&crate::diag::CHART_FRAME_SKIP_IDLE);
            GpuFrameDecision::Skip
        }
    }

    pub(super) fn prepare_gpu(&mut self, gpu: &RawGpuAccess) -> anyhow::Result<()> {
        let width = gpu.width();
        let height = gpu.height();
        if width == 0 || height == 0 {
            return Ok(());
        }

        let generation = gpu.device_generation();
        if self.last_gpu_prepare_generation != generation {
            self.last_gpu_prepare_generation = generation;
            self.base_dirty = true;
            for pr in &mut self.panes {
                pr.gpu_prepare_dirty = true;
            }
        }

        match gpu.backend() {
            #[cfg(windows)]
            GpuBackend::D3d11 => {
                let Some((device, context, _rtv)) = gpu::borrow_d3d(gpu) else {
                    anyhow::bail!("chart dx11 prepare received empty D3D11 raw gpu handles");
                };
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if !pr.active || !pr.gpu_prepare_dirty {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut orderbook_view = pr.orderbook_view;
                    view.resolution = res;
                    orderbook_view.resolution = res;
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_d3d(
                        &view,
                        &orderbook_view,
                        &pr.book_style,
                        &device,
                        &context,
                        gpu,
                    );
                    pr.finish_order_gpu_prepare(now_unix_ms());
                    pr.gpu_prepare_dirty = false;
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            GpuBackend::Wgpu => {
                let res = [width as f32, height as f32];
                let rebuild_base = self.base_dirty;
                for pr in &mut self.panes {
                    if !pr.active {
                        continue;
                    }
                    let needs_base = rebuild_base || pr.layers.needs_base_cache(gpu);
                    if !pr.gpu_prepare_dirty && !needs_base {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut background_params = pr.background_params;
                    let mut grid_params = pr.grid_params;
                    let mut cursor_params = pr.cursor_params;
                    let mut orderbook_view = pr.orderbook_view;
                    view.resolution = res;
                    background_params.resolution = res;
                    grid_params.resolution = res;
                    cursor_params.resolution = res;
                    orderbook_view.resolution = res;
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_wgpu(
                        &view,
                        &background_params,
                        &grid_params,
                        &cursor_params,
                        &orderbook_view,
                        &pr.book_style,
                        gpu,
                        needs_base,
                    )?;
                    pr.finish_order_gpu_prepare(now_unix_ms());
                    pr.gpu_prepare_dirty = false;
                }
                if rebuild_base {
                    self.base_dirty = false;
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            GpuBackend::Metal => {
                let res = [width as f32, height as f32];
                let rebuild_base = self.base_dirty;
                for pr in &mut self.panes {
                    if !pr.active {
                        continue;
                    }
                    let needs_base = rebuild_base || pr.layers.needs_base_cache(gpu);
                    if !pr.gpu_prepare_dirty && !needs_base {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut background_params = pr.background_params;
                    let mut grid_params = pr.grid_params;
                    let mut cursor_params = pr.cursor_params;
                    let mut orderbook_view = pr.orderbook_view;
                    view.resolution = res;
                    background_params.resolution = res;
                    grid_params.resolution = res;
                    cursor_params.resolution = res;
                    orderbook_view.resolution = res;
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_metal(
                        &view,
                        &background_params,
                        &grid_params,
                        &cursor_params,
                        &orderbook_view,
                        &pr.book_style,
                        gpu,
                        needs_base,
                    )?;
                    pr.finish_order_gpu_prepare(now_unix_ms());
                    pr.gpu_prepare_dirty = false;
                }
                if rebuild_base {
                    self.base_dirty = false;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    #[cfg(windows)]
    pub(super) fn render_chart_base_d3d(
        &mut self,
        res: [f32; 2],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        scissor_rs: &ID3D11RasterizerState,
    ) {
        for pr in &mut self.panes {
            if !pr.active {
                continue;
            }
            let mut view = pr.view;
            let mut background_params = pr.background_params;
            let mut grid_params = pr.grid_params;
            let mut orderbook_view = pr.orderbook_view;
            view.resolution = res;
            background_params.resolution = res;
            grid_params.resolution = res;
            orderbook_view.resolution = res;
            let panel_clip = [
                view.bounds[0],
                view.bounds[1],
                orderbook_view.bounds[0] + orderbook_view.bounds[2],
                view.bounds[1] + view.bounds[3],
            ];
            gpu::set_scissor(
                context,
                scissor_rs,
                panel_clip[0],
                panel_clip[1],
                panel_clip[2],
                panel_clip[3],
            );
            pr.layers.render_base_d3d(
                &view,
                &background_params,
                &grid_params,
                &orderbook_view,
                &pr.book_style,
                device,
                context,
                rtv,
                gpu,
                panel_clip,
            );
        }
    }

    pub(super) fn draw_gpu(&mut self, gpu: &RawGpuAccess) -> anyhow::Result<()> {
        let width = gpu.width();
        let height = gpu.height();
        if width == 0 || height == 0 {
            return Ok(());
        }

        crate::diag::bump(&crate::diag::CHART_PRESENT);
        let _present_us = crate::diag::scope_slow(
            &crate::diag::CHART_PRESENT_US,
            &crate::diag::CHART_PRESENT_SLOW,
            8_000,
        );
        let present_ms = now_unix_ms();

        match gpu.backend() {
            #[cfg(windows)]
            GpuBackend::D3d11 => {
                let Some((device, context, rtv)) = gpu::borrow_d3d(gpu) else {
                    anyhow::bail!("chart dx11 draw received empty D3D11 raw gpu handles");
                };

                let generation = gpu.device_generation();
                if self.scissor_rs.is_none() || self.scissor_generation != generation {
                    self.scissor_rs = Some(gpu::create_scissor_rasterizer(&device));
                    self.scissor_generation = generation;
                }
                let res = [width as f32, height as f32];
                let scissor_rs = self.scissor_rs.clone().unwrap();
                let prev_rs = unsafe { context.RSGetState().ok() };

                if self.base_dirty || self.base_cache.needs_rebuild(gpu) {
                    // The clear IS the background fill: it paints `window_bg_color` over the whole
                    // texture, and the dedicated background pass this used to run painted that same
                    // colour through a full pipeline pass — `opacity` was hardcoded to zero, so its
                    // shader reduced to `float4(bg.rgb, 1.0)`.
                    let base_rtv = self.base_cache.begin_rebuild(
                        &device,
                        &context,
                        gpu,
                        self.window_bg_color,
                    )?;
                    self.render_chart_base_d3d(res, &device, &context, &base_rtv, gpu, &scissor_rs);
                    self.base_dirty = false;
                }
                // Clip the blit to THIS chart's slot, the union of its active-panel bounds, rather
                // than the full backbuffer. With multiple `gpu_canvas` elements in one detached
                // window stack, a full-window blit would erase sibling charts. With no active
                // panels, skip the blit so the empty state remains the GPUI logo overlay.
                let mut blit_clip: Option<[f32; 4]> = None;
                for pr in &self.panes {
                    if !pr.active {
                        continue;
                    }
                    let c = bounds_clip(pr.pane_bounds, res);
                    blit_clip = Some(match blit_clip {
                        Some(u) => [
                            u[0].min(c[0]),
                            u[1].min(c[1]),
                            u[2].max(c[2]),
                            u[3].max(c[3]),
                        ],
                        None => c,
                    });
                }
                if let Some(clip) = blit_clip {
                    self.base_cache.blit_to(&context, &rtv, gpu, clip);
                }

                for pr in &mut self.panes {
                    if !pr.active {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut cursor_params = pr.cursor_params;
                    view.resolution = res;
                    cursor_params.resolution = res;
                    sync_readout_resolution(&mut pr.readout_rects, res);
                    let pane_clip = bounds_clip(pr.pane_bounds, res);
                    // Order lines and trade marks stop at the horizontal-volume zone: a mark whose
                    // time scrolled off the plot's left edge would otherwise draw over the zone's
                    // rows. The cursor pass below keeps the whole pane — the crosshair and the
                    // volume readout live in the zone.
                    let user_clip = userdata_clip(pane_clip, &pr.hvol_style);
                    gpu::set_scissor(
                        &context,
                        &scissor_rs,
                        user_clip[0],
                        user_clip[1],
                        user_clip[2],
                        user_clip[3],
                    );
                    pr.layers
                        .render_userdata_lines_d3d(&view, &context, &rtv, gpu);
                    gpu::set_scissor(
                        &context,
                        &scissor_rs,
                        pane_clip[0],
                        pane_clip[1],
                        pane_clip[2],
                        pane_clip[3],
                    );
                    pr.layers.render_cursor_d3d(
                        &cursor_params,
                        &pr.readout_rects,
                        &device,
                        &context,
                        &rtv,
                        gpu,
                    );
                    pr.finish_order_present(present_ms);
                }
                unsafe {
                    context.RSSetState(prev_rs.as_ref());
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            GpuBackend::Wgpu => {
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if pr.active {
                        let mut view = pr.view;
                        let mut background_params = pr.background_params;
                        let mut grid_params = pr.grid_params;
                        let mut cursor_params = pr.cursor_params;
                        let mut orderbook_view = pr.orderbook_view;
                        view.resolution = res;
                        background_params.resolution = res;
                        grid_params.resolution = res;
                        cursor_params.resolution = res;
                        orderbook_view.resolution = res;
                        sync_readout_resolution(&mut pr.readout_rects, res);
                        pr.layers.render_wgpu(
                            &view,
                            pr.pane_bounds,
                            &background_params,
                            &grid_params,
                            &cursor_params,
                            &pr.readout_rects,
                            &orderbook_view,
                            gpu,
                        )?;
                        pr.finish_order_present(present_ms);
                    }
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            GpuBackend::Metal => {
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if pr.active {
                        let mut view = pr.view;
                        let mut background_params = pr.background_params;
                        let mut grid_params = pr.grid_params;
                        let mut cursor_params = pr.cursor_params;
                        let mut orderbook_view = pr.orderbook_view;
                        view.resolution = res;
                        background_params.resolution = res;
                        grid_params.resolution = res;
                        cursor_params.resolution = res;
                        orderbook_view.resolution = res;
                        sync_readout_resolution(&mut pr.readout_rects, res);
                        pr.layers.render_metal(
                            &view,
                            pr.pane_bounds,
                            &background_params,
                            &grid_params,
                            &cursor_params,
                            &pr.readout_rects,
                            &orderbook_view,
                            gpu,
                        )?;
                        pr.finish_order_present(present_ms);
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
