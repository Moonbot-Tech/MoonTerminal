//! Chart view state — a port of the Moonbot/WebGame interactions:
//!   X (time): wheel zoom around the cursor, LMB/Shift-wheel pan. Live/latest is
//!             determined spatially: when the right edge is within 5% of the window
//!             from now, it re-anchors to now. Panning also starts a 3-second manual
//!             hold, while the toolbar's manual mode remains persistent.
//!   Y (price): auto/fixed-percent scaling and manual Y-pan/RMB-zoom are independent
//!              of X-follow, so browsing history horizontally does not freeze the
//!              price scale.

/// Rectangle in pixels (top-left origin).
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

// --- Behavior constants ---
/// Auto-scale convergence rate toward the visible range (fraction per 60 Hz reference frame).
const AUTO_LERP: f32 = 0.15;
/// Centering dead zone: do not move while the price is within ±BUFFER*range of the center
/// (otherwise it would jitter every frame). The buffer is a fraction of the current scale.
const CENTER_BUFFER: f32 = 0.10;
/// Rate at which the center follows the price after it leaves the buffer (per 60 Hz reference frame).
const TICK_LERP: f32 = 0.10;
/// Pixels of vertical RMB drag needed to double or halve the Y range.
const YSCALE_PX_PER_2X: f32 = 150.0;
/// Y-range hysteresis: keep render_range until the smooth target moves beyond ±15%, then
/// snap to the target. Between snaps, the Y scale stays constant so cached base rendering
/// remains valid across frames.
const RANGE_HYST: f32 = 1.15;
/// Y-center movement threshold in pixels: the center remains fixed until the price moves farther.
const CENTER_SNAP_PX: f32 = 8.0;
/// Treat the view as live again when the right live anchor is this close to now.
const LIVE_REJOIN_FRAC: f32 = 0.05;
/// Maximum visible time window in ms. It used to be 6 hours (Delphi MaxTimeRange=360 min)
/// for a tick chart; candles (deep history from the core, with timeframes up to one day) need
/// a MUCH larger window: 365 days (about 365 daily candles). Trades do not become more
/// expensive because only the latest K candles are read, and candles are capped by layer capacity.
const MAX_WINDOW_MS: f32 = 31_536_000_000.0;
/// Lower px_per_ms guard for division (ONLY against zero/garbage). Shared by every geometry
/// path (view, own-pass camera, and time axis): historical `.max(1e-6)` safeguards were ABOVE
/// the actual minimum zoom (area/MAX_WINDOW ≈ 4e-8 for a 365-day window) and triggered
/// differently in different places, causing the camera, data, and labels to diverge and the
/// chart to drift left of the order book at deep zoom-out. The guard must be below the minimum ppm.
pub const MIN_PX_PER_MS: f32 = 1e-9;
/// Default visible window used to select a pixel-smooth live scale.
const DEFAULT_WINDOW_MS: f32 = 60_000.0;
/// Minimum visible window at maximum zoom-in (previously limited to the default 60 s).
const MIN_WINDOW_MS: f32 = 30_000.0;
/// How long to keep manual X mode after the last pan before automatically returning to live.
const MANUAL_HOLD_MS: f64 = 3000.0;

#[derive(Clone)]
pub struct ChartView {
    /// Fixed time origin (unix ms), set at startup.
    pub epoch_ms: f64,
    /// X zoom: pixels per millisecond.
    pub px_per_ms: f32,
    /// Time (unix ms) at the area's right "now" anchor.
    pub right_time_ms: f64,
    /// Automatic following of the right time edge (live).
    pub follow: bool,
    /// Fraction of the window width reserved on the right as "future" (like xRange*0.9).
    pub right_margin_frac: f32,

    /// Price at the center of the area.
    pub center_price: f32,
    /// Visible price range (price units from top to bottom of the area).
    pub price_range: f32,
    /// Automatic fitting of the price range (the "Auto" button).
    pub auto_price: bool,
    /// Manual Y view after vertical drag/RMB zoom. Reset by scale buttons but not by the Live
    /// button: Live controls only X/latest.
    pub manual_price: bool,
    /// Last fixed percentage (range = center*percent), used for drift mode.
    pub scale_percent: f32,
    /// Derived pixels per price unit (cached for panning/hit testing).
    pub px_per_price: f32,

    /// Snapped (piecewise-constant) Y parameters used for ACTUAL rendering.
    /// The live smooth target is center_price/price_range; rendering uses
    /// render_center/render_range and keeps them stable between infrequent jumps,
    /// so the Y mapping remains unchanged on most frames for effective caching.
    pub render_center: f32,
    pub render_range: f32,

    /// Marker half-size in pixels.
    pub marker_half_px: f32,

    /// Whether the time scale is in default phase mode and may be recalculated on
    /// resize/present-rate changes. The first manual zoom leaves this mode.
    x_default_scale: bool,
    /// Whether zoom has not yet been fitted to the default window (done once using the
    /// actual chart-area width on the first frame).
    x_init_pending: bool,
    last_phase_area_w: f32,
    last_phase_present_hz: f32,
    phase_default_px_per_ms: f32,
    /// Time of the previous `update_y` (unix ms), used to normalize Y smoothing by real dt
    /// rather than per frame (otherwise auto-Y speed would depend on preparation frequency).
    last_update_ms: f64,
    /// Keep manual X mode after a pan until this time (unix ms); once it expires,
    /// `tick_auto_live` automatically returns to live. 0 means no pending return (or already live).
    manual_until: f64,
}

impl ChartView {
    pub fn new(epoch_ms: f64) -> Self {
        Self {
            epoch_ms,
            px_per_ms: 0.05, // Approximately 20 seconds visible across 1000 px.
            right_time_ms: epoch_ms,
            follow: true,
            right_margin_frac: 0.10, // "Future" area on the right, as in moonweb (xRange*0.9).
            center_price: 0.0,
            price_range: 1.0,
            auto_price: true,
            manual_price: false,
            scale_percent: 0.10,
            px_per_price: 0.5,
            render_center: 0.0,
            render_range: 1.0,
            marker_half_px: 3.5, // 7 px cross (Moonbot NormalX).
            x_default_scale: true,
            x_init_pending: true,
            last_phase_area_w: f32::NAN,
            last_phase_present_hz: f32::NAN,
            phase_default_px_per_ms: 0.0,
            last_update_ms: 0.0,
            manual_until: 0.0,
        }
    }

    fn phase_clean_default_px_per_ms(area_w: f32, present_hz: f32) -> f32 {
        let dt_ms = 1000.0 / present_hz.max(1.0);
        let s0 = area_w.max(1.0) * dt_ms / DEFAULT_WINDOW_MS;
        let shift_px = if s0 >= 1.0 {
            s0.round().max(1.0)
        } else {
            let n = (1.0 / s0.max(1e-9)).round().max(1.0);
            1.0 / n
        };
        (shift_px / dt_ms).max(1e-9)
    }

    /// Fits the default time window to the nearest phase-clean point around 60 s:
    /// an integer number of px/frame or 1 px every N frames. Recalculated only while
    /// the scale remains default/reset-to-live, on the first frame, resize, or present change.
    /// `default_ppm` is the saved user X scale ([Shift+MMB] sync): a new chart starts
    /// with it instead of the phase-clean 60-second default.
    pub fn ensure_default_window(
        &mut self,
        area_w: f32,
        present_hz: f32,
        default_ppm: Option<f32>,
    ) {
        if area_w < 1.0 {
            return;
        }
        let present_hz = present_hz.max(1.0);
        let default_px_per_ms = Self::phase_clean_default_px_per_ms(area_w, present_hz);
        let area_changed = (area_w - self.last_phase_area_w).abs() >= 0.5;
        let present_changed = (present_hz - self.last_phase_present_hz).abs() >= 0.5;
        let phase_changed = area_changed || present_changed;
        if phase_changed || self.x_init_pending {
            self.phase_default_px_per_ms = default_px_per_ms;
            self.last_phase_area_w = area_w;
            self.last_phase_present_hz = present_hz;
        }
        // Apply the saved user scale ONLY during initialization. It then behaves as a manual
        // scale and is not recalculated on resize.
        if self.x_init_pending {
            if let Some(ppm) = default_ppm.filter(|p| p.is_finite() && *p > 0.0) {
                self.px_per_ms = ppm.clamp(area_w / MAX_WINDOW_MS, 100.0);
                self.x_default_scale = false;
                self.x_init_pending = false;
                return;
            }
        }
        // Recalculate the visible scale only during initialization or when WIDTH changes
        // (resize), NOT when present_hz alone changes. At startup, the detected rate converges
        // from 60 to the actual value (120 Hz on ProMotion); recalculating px_per_ms at every
        // convergence step made the time axis (crosses/lines) jitter because the own-pass camera
        // (advance_camera) and sync used different scales on transitional frames. The jitter
        // disappeared after the first zoom (x_default_scale=false). Integer camera rounding keeps
        // the phase clean at 120 Hz, so the "60 Hz default" scale is visually acceptable.
        if !self.x_init_pending && !(self.x_default_scale && area_changed) {
            return;
        }
        self.px_per_ms = default_px_per_ms;
        self.x_default_scale = true;
        self.x_init_pending = false;
    }

    /// Applies an external X scale (px/ms) for [Shift+MMB] synchronization across charts.
    /// Returns `true` on an actual change. The scale then behaves as manual (see zoom_x_at),
    /// while the live anchor is preserved.
    pub fn set_px_per_ms_sync(&mut self, ppm: f32, now_ms: f64) -> bool {
        if !(ppm.is_finite() && ppm > 0.0) {
            return false;
        }
        let ppm = ppm.clamp(MIN_PX_PER_MS, 100.0);
        if (self.px_per_ms - ppm).abs() <= self.px_per_ms * 1e-6 {
            return false;
        }
        self.px_per_ms = ppm;
        self.x_default_scale = false;
        self.x_init_pending = false;
        if self.follow {
            self.right_time_ms = now_ms;
        }
        true
    }

    /// Whether the view is currently live.
    pub fn is_live(&self, now_ms: f64) -> bool {
        let _ = now_ms;
        self.follow
    }

    /// Anchors the right edge to `edge_ms` while live, quantized to whole-pixel time steps.
    pub fn follow_edge(&mut self, edge_ms: f64, now_ms: f64) {
        if self.is_live(now_ms) {
            self.right_time_ms = self.quantize_edge_ms(edge_ms);
        }
    }

    /// Snaps the right edge to a WHOLE pixel (like Moonbot `NowPhase`): only an integer
    /// number of pixels changes between frames, so thin elements (trade crosses, order lines,
    /// last/mark) do not jitter at subpixel positions. Counterintuitively, discrete movement
    /// looks smooth in crisp 2D (at 60+ Hz, a 1 px step is imperceptible). The own-pass callback
    /// uses the SAME formula when moving the camera on every present (see chartdx).
    pub fn quantize_edge_ms(&self, edge_ms: f64) -> f64 {
        let ppm = self.px_per_ms.max(MIN_PX_PER_MS) as f64;
        let rel = edge_ms - self.epoch_ms;
        (rel * ppm).round() / ppm + self.epoch_ms
    }

    /// Immediately returns to live (the Live button), anchored to now.
    pub fn resume_live(&mut self, now_ms: f64) {
        self.follow = true;
        self.right_time_ms = now_ms;
        self.manual_until = 0.0;
    }

    /// Explicitly and persistently disables live from the toolbar, with no automatic return.
    pub fn set_manual_persistent(&mut self) {
        self.follow = false;
        self.manual_until = 0.0;
    }

    /// Returns to live when the manual hold after a pan expires (Item 9). Driven by a timer
    /// because the own-pass moves the camera but prepare does not tick while idle
    /// (see moon-ui-gpui/src/panels/chart/mod.rs).
    /// Returns true if live was resumed.
    pub fn tick_auto_live(&mut self, now_ms: f64) -> bool {
        if !self.follow && self.manual_until > 0.0 && now_ms >= self.manual_until {
            self.resume_live(now_ms);
            true
        } else {
            false
        }
    }

    /// Next automatic-return deadline (unix ms), if pending, used to arm the timer.
    pub fn auto_live_deadline_ms(&self) -> Option<f64> {
        if !self.follow && self.manual_until > 0.0 {
            Some(self.manual_until)
        } else {
            None
        }
    }

    pub fn reset_default_window_on_next_prepare(&mut self) {
        self.x_default_scale = true;
        self.x_init_pending = true;
    }

    pub fn snap_to_live_if_near(&mut self, now_ms: f64, area_w: f32) -> bool {
        if self.follow {
            return false;
        }
        let tolerance_ms =
            (area_w.max(1.0) * LIVE_REJOIN_FRAC) as f64 / self.px_per_ms.max(MIN_PX_PER_MS) as f64;
        if now_ms - self.right_time_ms <= tolerance_ms {
            self.resume_live(now_ms);
            true
        } else {
            false
        }
    }

    /// Visible X window: (time at the left edge, window width in ms).
    /// Single source of X geometry for uniforms and visible-tick culling.
    pub fn visible_x(&self, area_w: f32) -> (f32, f32) {
        let window_ms = area_w / self.px_per_ms.max(MIN_PX_PER_MS);
        let right_rel =
            (self.right_time_ms - self.epoch_ms) as f32 + window_ms * self.right_margin_frac;
        (right_rel - window_ms, window_ms)
    }

    // ── Y scale (toolbar buttons) ─────────────────────────────────────────────

    /// The "Auto" button: dynamically fits the visible range.
    pub fn set_auto(&mut self) {
        self.auto_price = true;
        self.manual_price = false;
    }

    /// Current visible Y window `(center, range)` for comparison mode (anchor lock).
    pub fn y_window(&self) -> (f32, f32) {
        (self.render_center, self.render_range)
    }

    /// Applies a Y window for comparison mode: set center+range and freeze auto-fit (`manual_price`).
    /// Idempotent: returns `false` if the values already match, preventing a broadcast loop.
    pub fn set_y_window(&mut self, center: f32, range: f32) -> bool {
        let range = range.max(1e-6);
        let same = self.manual_price
            && (self.center_price - center).abs() < 1e-6
            && (self.price_range - range).abs() < 1e-6;
        if same {
            return false;
        }
        self.auto_price = false;
        self.manual_price = true;
        self.center_price = center;
        self.price_range = range;
        self.render_center = center;
        self.render_range = range;
        true
    }

    /// Fixed percentage: visible range = price*percent (like the moonweb ZoomBar).
    pub fn set_scale_percent(&mut self, percent: f32) {
        self.auto_price = false;
        self.manual_price = false;
        self.scale_percent = percent;
        let base = if self.center_price.abs() > 1e-6 {
            self.center_price.abs()
        } else {
            self.price_range
        };
        self.price_range = (base * percent).max(1e-6);
        self.render_range = self.price_range;
        self.render_center = self.center_price;
    }

    // ── Mouse pan/zoom ───────────────────────────────────────────────────────────
    // X drag detaches the view from live immediately; re-anchoring is checked separately on mouse-up.

    /// Pans X by dx pixels (LMB drag/Shift-wheel).
    pub fn pan_x_px(&mut self, dx: f32, now_ms: f64, area_w: f32) {
        let dt_ms = dx as f64 / self.px_per_ms.max(MIN_PX_PER_MS) as f64;
        self.right_time_ms = (self.right_time_ms - dt_ms).min(now_ms);
        self.follow = false;
        // Item 9: panning does not disable live permanently. Start a manual hold window,
        // after which `tick_auto_live` re-anchors to now. Every pan frame advances the deadline,
        // so the return occurs about 3 s AFTER release.
        self.manual_until = now_ms + MANUAL_HOLD_MS;
        let _ = area_w;
    }

    /// Pans Y by dy pixels (LMB drag).
    pub fn pan_y_px(&mut self, dy: f32, now_ms: f64) {
        let _ = now_ms;
        self.center_price += dy / self.px_per_price.max(1e-6);
        self.manual_price = true;
        self.render_center = self.center_price;
    }

    /// Zooms X. In live mode, preserves the live anchor (as in WebGame/Moonbot); in manual
    /// X view, preserves the time under the cursor and may re-anchor to live after a discrete step.
    pub fn zoom_x_at(&mut self, factor: f32, area_w: f32, cursor_x: f32, now_ms: f64) {
        let was_follow = self.follow;
        let old_px = self.px_per_ms.max(MIN_PX_PER_MS);
        let cursor_x = cursor_x.clamp(0.0, area_w.max(1.0));
        let (old_left, _) = self.visible_x(area_w);
        let cursor_time = self.epoch_ms + old_left as f64 + cursor_x as f64 / old_px as f64;
        let next = self.px_per_ms * factor;
        let lo = if area_w >= 1.0 {
            area_w / MAX_WINDOW_MS
        } else {
            0.0005
        };
        // Allow zoom-in beyond the default (60 s) down to MIN_WINDOW_MS (30 s): use the
        // phase-clean default multiplied by DEFAULT/MIN = 2×. Otherwise the maximum was 1 min (Item 7).
        let hi = (if self.phase_default_px_per_ms > 0.0 {
            self.phase_default_px_per_ms
        } else {
            Self::phase_clean_default_px_per_ms(area_w, 60.0)
        } * (DEFAULT_WINDOW_MS / MIN_WINDOW_MS))
            .max(lo);
        self.px_per_ms = next.clamp(lo, hi);
        self.x_default_scale = (self.px_per_ms - self.phase_default_px_per_ms).abs() <= 1e-9;
        if was_follow {
            self.right_time_ms = now_ms;
            self.follow = true;
            return;
        }
        let new_window = area_w / self.px_per_ms.max(MIN_PX_PER_MS);
        let left = cursor_time - self.epoch_ms - cursor_x as f64 / self.px_per_ms as f64;
        self.right_time_ms =
            (self.epoch_ms + left + new_window as f64 * (1.0 - self.right_margin_frac as f64))
                .min(now_ms);
        self.snap_to_live_if_near(now_ms, area_w);
    }

    /// Zooms Y by RMB drag from the press-time snapshot. Up=zoom out, down=zoom in.
    pub fn rmb_zoom(&mut self, start_center: f32, start_range: f32, cum_dy: f32, now_ms: f64) {
        let factor = 2f32.powf(-cum_dy / YSCALE_PX_PER_2X);
        let r = (start_range * factor).clamp(start_range * 0.25, start_range * 4.0);
        self.center_price = start_center;
        self.price_range = r.max(1e-6);
        self.manual_price = true;
        self.render_center = self.center_price;
        self.render_range = self.price_range;
        let _ = now_ms;
    }

    // ── Per-frame price-scale update ──────────────────────────────────────────────

    /// Fits the price center/range. X-follow affects only target selection: live mode centers
    /// on the last price, while manual X centers on the visible range. Manual Y-pan/RMB-zoom
    /// (`manual_price`) freezes Y until a scale is selected.
    pub fn update_y(
        &mut self,
        now_ms: f64,
        area_h: f32,
        visible: Option<(f32, f32)>,
        last_price: Option<f32>,
    ) {
        let live = self.is_live(now_ms);
        // Smooth Y using REAL dt rather than per frame: prepare now ticks at a variable rate
        // (the own-pass moves the camera on vblank, while data updates less often), so a constant
        // per-frame fraction would produce different auto-zoom/centering speeds across machines.
        // Convert per-frame coefficients to the actual interval: f = 1 - (1-base)^(dt/16.67ms).
        let dt_ms = if self.last_update_ms > 0.0 {
            (now_ms - self.last_update_ms).clamp(1.0, 250.0)
        } else {
            1000.0 / 60.0
        };
        self.last_update_ms = now_ms;
        let frame_ref = 1000.0 / 60.0;
        let auto_lerp = (1.0 - (1.0 - AUTO_LERP as f64).powf(dt_ms / frame_ref)) as f32;
        let tick_lerp = (1.0 - (1.0 - TICK_LERP as f64).powf(dt_ms / frame_ref)) as f32;
        if !self.manual_price {
            let visible_mid = visible.map(|(lo, hi)| (lo + hi) * 0.5);
            let target_center = if live {
                last_price.or(visible_mid)
            } else {
                visible_mid.or(last_price)
            };
            let target_range = match (self.auto_price, visible, target_center) {
                (true, Some((lo, hi)), Some(c)) if live => {
                    // In live mode, keep the last price centered and expand the range symmetrically
                    // so neither tick tails nor order lines are clipped. ×1.20 leaves about 10%
                    // padding at each edge, keeping lines away from the top and bottom.
                    let half = (c - lo).max(hi - c).max(c.abs() * 0.0005 + 1e-6);
                    Some(half * 2.0 * 1.20)
                }
                (true, Some((lo, hi)), Some(c)) => {
                    Some((hi - lo).abs().max(c.abs() * 0.0005 + 1e-6) * 1.20)
                }
                (true, None, Some(c)) => Some((c.abs() * 0.001).max(1e-6)),
                (false, _, Some(c)) => Some((c.abs() * self.scale_percent).max(1e-6)),
                _ => None,
            };

            if let Some(r) = target_range {
                if live && self.auto_price && self.center_price != 0.0 && self.price_range > 0.0 {
                    // Asymmetry (Item 5): EXPAND the range immediately; otherwise visible ticks/order
                    // lines are clipped for dozens of frames while the slow lerp catches up (symptom:
                    // only the buy line and order book are visible, with ticks off-screen). SHRINK
                    // smoothly to avoid jitter during brief price spikes.
                    if r > self.price_range {
                        self.price_range = r;
                    } else {
                        self.price_range += (r - self.price_range) * auto_lerp;
                    }
                } else {
                    self.price_range = r;
                }
            }

            if let Some(c) = target_center {
                if self.center_price == 0.0 || !live {
                    self.center_price = c;
                } else if self.price_range > 1e-9 {
                    let drift = (c - self.center_price).abs() / self.price_range;
                    if drift > CENTER_BUFFER {
                        self.center_price += (c - self.center_price) * tick_lerp;
                    }
                }
            }
        }
        if !(self.price_range > 1e-9) {
            self.price_range = self.center_price.abs() * 0.10 + 1.0;
        }
        // Snap Y. In live mode, render_* is piecewise constant: keep the range until the target
        // moves beyond ±RANGE_HYST, and keep the center until the price moves by more than
        // CENTER_SNAP_PX px. In manual mode, follow input exactly for responsive dragging;
        // the render cache is rebuilt transiently during interaction.
        if live && !self.manual_price {
            let target = self.price_range.max(1e-9);
            if !self.auto_price
                || !(self.render_range > 1e-9)
                || target > self.render_range * RANGE_HYST
                || target < self.render_range / RANGE_HYST
            {
                self.render_range = target;
            }
            let ppp = (area_h / self.render_range.max(1e-9)).max(1e-6);
            if (self.center_price - self.render_center).abs() * ppp > CENTER_SNAP_PX {
                self.render_center = self.center_price;
            }
        } else {
            self.render_range = self.price_range.max(1e-9);
            self.render_center = self.center_price;
        }
        self.px_per_price = (area_h / self.render_range.max(1e-9)).max(1e-6);
    }
}

#[cfg(test)]
mod tests;
