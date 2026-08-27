//! Chart render state (`impl RenderState`): per-pane GPU-state composition, present pacing initialized
//! at 60 Hz and then driven by detected monitor refresh clamped to 30-360 Hz, with the renderer target
//! capped at 240 Hz, plus cursor, readout, and own-pass layer rendering. Extracted from `mod.rs`, where
//! the `RenderState` structure remains declared.

use super::*;

const READOUT_FALLBACK_FONT_W: f32 = 8.5;
const READOUT_PAD_X: f32 = 5.0;
const READOUT_PAD_Y: f32 = 2.5;
const READOUT_INSET: f32 = 2.0;

#[cfg(windows)]
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

/// How long a newly arrived chart carries its accent border flash.
///
/// Lives here, next to the code that draws and expires it, rather than in `chart_tabs`: the flash
/// is an own-pass decoration now, and a duration split across two modules is exactly the drift this
/// change exists to remove.
const ARRIVAL_HIGHLIGHT: Duration = Duration::from_millis(2600);

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

    /// Start (or clear) the arrival border flash for this chart.
    ///
    /// The stamp is all the own-pass needs: `frame` paces the flash and expires it from wall clock,
    /// so the caller neither notifies nor keeps a timer. That is the whole point — a GPUI repaint
    /// of the owning stack re-renders every chart panel in the tab.
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

    pub(super) fn set_arrival_pulse(&mut self, at: Option<Instant>, accent: [f32; 4]) -> bool {
        // Switched off: every start becomes a clear, so a run with the flash disabled cannot be
        // left with one already in flight from before the call.
        let at = if arrival_flash_enabled() { at } else { None };
        // The colour is refreshed even when the stamp is unchanged: a theme switch mid-flash must
        // not leave the border in the old accent for the rest of its 2.6 s.
        let recolored = self.arrival_pulse_color != accent;
        self.arrival_pulse_color = accent;
        if self.arrival_pulse == at {
            return recolored;
        }
        self.arrival_pulse = at;
        self.last_arrival_present_at = None;
        self.sync_readout_params();
        self.needs_present = true;
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
            let bounds = [
                pr.view.bounds[0],
                pr.view.bounds[1],
                (right - pr.view.bounds[0]).max(1.0),
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
        // Arrival flash phase, sampled once for every pane. Three flashes over `ARRIVAL_HIGHLIGHT`,
        // the same curve the GPUI element used before this moved into the own-pass.
        let arrival_alpha = self.arrival_pulse.and_then(|at| {
            let elapsed = at.elapsed();
            (elapsed < ARRIVAL_HIGHLIGHT).then(|| {
                let delta = elapsed.as_secs_f32() / ARRIVAL_HIGHLIGHT.as_secs_f32();
                (delta * std::f32::consts::PI * 3.0).sin().abs()
            })
        });
        let arrival_color = self.arrival_pulse_color;

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

    pub(super) fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        crate::diag::bump(&crate::diag::CHART_FRAME);
        if !info.presentable || info.bounds.is_empty() {
            crate::diag::bump(&crate::diag::CHART_FRAME_SKIP_NOT_PRESENTABLE);
            return GpuFrameDecision::Skip;
        }

        let now_ms = now_unix_ms();
        let now = Instant::now();
        let mut wants_present = std::mem::take(&mut self.needs_present);
        if self.firetest_force_present {
            wants_present = true;
        }
        // Arrival flash: paced and ENDED here, from wall clock, with no timer and no notify.
        //
        // Clearing `arrival_pulse` on expiry is the load-bearing line: leave it set and this canvas
        // asks for a present ten times a second forever, which reads as a mysterious idle floor and
        // no test would catch it. The final tick after expiry still rebuilds the rects, and that is
        // the frame which erases the border.
        // Shot caption: ENDED here, from wall clock, for the same reason the arrival flash is —
        // no timer, no notify, and nobody to trust with the clear. This is the WATCHDOG, not the
        // normal path: the shot restores the caption itself as soon as it has its picture, and this
        // only fires when that chain never completed. Leaving it armed would keep the EXCHANGE on
        // the user's own screen, where the core name belongs.
        if let Some(deadline) = self.shot_caption_until {
            if now >= deadline {
                self.shot_caption_until = None;
                self.shot_caption_frames = 0;
                wants_present = true;
            }
        }
        if let Some(at) = self.arrival_pulse {
            if at.elapsed() >= ARRIVAL_HIGHLIGHT {
                self.arrival_pulse = None;
                self.last_arrival_present_at = None;
                self.sync_readout_params();
                wants_present = true;
            } else if self
                .last_arrival_present_at
                .is_none_or(|last| now.duration_since(last) >= ARRIVAL_PULSE_TICK)
            {
                self.last_arrival_present_at = Some(now);
                self.sync_readout_params();
                crate::diag::bump(&crate::diag::CHART_ARRIVAL_PULSE);
                wants_present = true;
            }
        }

        let cap_due = self
            .last_present_at
            .is_none_or(|last| now.duration_since(last) >= self.target_present_interval);
        let mut camera_moved = false;
        for pr in &mut self.panes {
            if pr.active && (wants_present || cap_due) && pr.advance_camera(now_ms) {
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
        let _present_us = crate::diag::scope(&crate::diag::CHART_PRESENT_US);
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
                    gpu::set_scissor(
                        &context,
                        &scissor_rs,
                        pane_clip[0],
                        pane_clip[1],
                        pane_clip[2],
                        pane_clip[3],
                    );
                    pr.layers
                        .render_userdata_lines_d3d(&view, &context, &rtv, gpu);
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
