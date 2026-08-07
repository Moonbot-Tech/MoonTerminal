//! Chart view state — a port of the Moonbot/WebGame interactions:
//!   X (time): wheel zoom around the cursor, LMB/Shift-wheel pan. Live/latest is
//!             determined spatially: when the right edge is within 5% of the window
//!             from now, it re-anchors to now. Panning into HISTORY starts a 3-second
//!             manual hold, while the toolbar's manual mode remains persistent.
//!             Panning the other way walks past the live edge into empty future chart,
//!             as far as putting `now` on the left edge of the plot — a place to draw a
//!             plan, so while the view is AHEAD there is no hold timer at all. Panning
//!             back onto the data restores the ordinary 3-second hold. The live edge is
//!             never allowed off screen: `clamp_future_anchor` owns that ceiling.
//!   Y (price): auto/fixed-percent scaling and manual Y-pan/RMB-zoom are independent
//!              of X-follow, so browsing history horizontally does not freeze the
//!              price scale. What the auto-fit is given to cover is decided OUTSIDE this
//!              type, by `fit_band` — the view fits what it is handed and does not know
//!              where the window is pointing.

/// What the price auto-fit should cover, from what the window actually CONTAINS and what merely has
/// to stay on screen.
///
/// The two are different things and the fit cannot tell them apart once they are unioned. Trades,
/// candles and order lines are IN the window; the last price and the order-book band are folded in
/// so that a market which has not traded yet still has something to show. Off the live edge — parked
/// in empty future, or in a gap of history — the window contains none of the first kind, and the
/// second kind is not on screen either: fitting to it snapped a settled 2.4% window to 1.2% against
/// the book band, and to 0.06% against the bare last price, in a single frame, because the manual-X
/// branch of [`ChartView::update_y`] applies its result with neither lerp nor hysteresis.
///
/// So the rule is: with data in the window, fit everything; with none of it, fall back to the
/// references only when the view has no scale worth keeping, and otherwise nothing at all — `None`
/// tells the view to hold the scale the user is reading rather than invent one.
///
/// `use_reference` is that "no scale worth keeping" test, and it is two things, not one: a view that
/// FOLLOWS the live edge is about the live price by definition, and a pane that has never had window
/// data at all has nothing to hold — a chart opened while the toolbar's Live is off, or a market
/// with an order book and no trades, would otherwise sit forever on the constructor's zero centre.
/// The second half must be a fact about the DATA and not about the view's current scale: the first
/// fit gives the view a scale, so testing the scale would switch the fallback off after one frame
/// and latch the pane onto whatever that first frame produced.
///
/// One honest limit: `window_data` is what the CALLER believes is in the window, and prepare scans
/// ticks over the window plus a 20% prefetch margin. Parked at the very ceiling, the newest trades
/// just left of the plot therefore still count — the scale stays anchored to the recent price band
/// instead of freezing completely, which is the harmless direction to be wrong in.
///
/// This lives here, beside the fit it feeds, so it can be tested: its caller is in a binary crate.
///
/// Args:
///     window_data: Price span of what is actually drawn inside the window, if anything.
///     reference: Price span that must remain visible regardless (last price, order-book band).
///     use_reference: Whether the reference may stand in for missing window data.
///
/// Returns:
///     The span to fit, or `None` to leave the price scale untouched.
pub fn fit_band(
    window_data: Option<(f32, f32)>,
    reference: Option<(f32, f32)>,
    use_reference: bool,
) -> Option<(f32, f32)> {
    match (window_data, reference, use_reference) {
        (Some((dlo, dhi)), Some((rlo, rhi)), _) => Some((dlo.min(rlo), dhi.max(rhi))),
        (Some(data), None, _) => Some(data),
        (None, reference, true) => reference,
        (None, _, false) => None,
    }
}

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
/// Smallest visible price window, as a fraction of the price itself.
///
/// RELATIVE on purpose. An absolute floor is a floor for one order of magnitude only: `1e-6` is a
/// rounding error on a $60k coin and eighty times the whole price of one trading at 1.2e-8, where
/// it pinned the window open and made every scale read thousands of percent. Same lesson the order
/// book's span guard learned.
const MIN_RANGE_FRAC: f32 = 1e-6;

/// Live history visible left of the current timestamp in the built-in chart default.
const DEFAULT_HISTORY_MS: f32 = 6.0 * 60.0 * 60.0 * 1000.0;
/// Fraction of the default viewport reserved to the right of the current timestamp.
const DEFAULT_RIGHT_MARGIN_FRAC: f32 = 0.10;
/// Full default viewport required to retain the history after reserving the future margin.
const DEFAULT_WINDOW_MS: f32 = DEFAULT_HISTORY_MS / (1.0 - DEFAULT_RIGHT_MARGIN_FRAC);
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
    /// Last fixed percentage (range = price*percent), used for drift mode.
    pub scale_percent: f32,
    /// The price a scale percentage is measured against: the rendered centre, FROZEN while the user
    /// holds a manual Y view.
    ///
    /// Not the live centre, because a vertical drag moves that without changing any zoom, and the
    /// same window would then restate itself — a 1% chart dragged far enough would report 2%. Not
    /// the last trade either: panned into history, the window belongs to the prices on screen, and
    /// dividing it by today's price would call a 12% window "0.12%". Taken from `render_center`
    /// rather than `center_price` so it inherits the same snapping: an unsnapped divisor would make
    /// the reported figure flicker between two integers on ordinary ticks.
    scale_anchor: f32,
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
    /// Whether following was turned off EXPLICITLY — [`Self::set_manual_persistent`], which is what
    /// the toolbar's Live button reaches — so that no later pan may schedule a return that was not
    /// asked for.
    ///
    /// A separate flag rather than `manual_until == 0`, which a pan ahead of the live edge produces
    /// too. Those two states are identical to the timer and mean opposite things to the user, and
    /// reading the timer for both made one excursion into the future switch a pane's three-second
    /// return off for the rest of its life.
    manual_persistent: bool,
}

impl ChartView {
    /// Construct a live chart view whose X scale is fitted on its first prepared frame.
    ///
    /// Args:
    ///     epoch_ms: Fixed Unix-millisecond time origin for chart geometry.
    ///
    /// Returns:
    ///     A view with built-in scale defaults and live following enabled.
    pub fn new(epoch_ms: f64) -> Self {
        Self {
            epoch_ms,
            px_per_ms: 0.05, // Approximately 20 seconds visible across 1000 px.
            right_time_ms: epoch_ms,
            follow: true,
            // "Future" area on the right, as in moonweb (xRange*0.9).
            right_margin_frac: DEFAULT_RIGHT_MARGIN_FRAC,
            center_price: 0.0,
            price_range: 1.0,
            auto_price: true,
            manual_price: false,
            scale_percent: 0.10,
            scale_anchor: 0.0,
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
            manual_persistent: false,
        }
    }

    /// Return a phase-clean default X scale whose visible window is never shorter than the target.
    ///
    /// Args:
    ///     area_w: Plot width in logical pixels.
    ///     present_hz: Current display refresh estimate.
    ///
    /// Returns:
    ///     Pixels per millisecond, rounded outward to whole pixels per frame or frames per pixel.
    fn phase_clean_default_px_per_ms(area_w: f32, present_hz: f32) -> f32 {
        let area_w = area_w.max(1.0);
        let dt_ms = 1000.0 / present_hz.max(1.0);
        let s0 = area_w * dt_ms / DEFAULT_WINDOW_MS;
        let shift_px = if s0 >= 1.0 {
            s0.floor().max(1.0)
        } else {
            let n = (1.0 / s0.max(1e-9)).ceil().max(1.0);
            1.0 / n
        };
        let target_scale = area_w / DEFAULT_WINDOW_MS;
        let mut scale = (shift_px / dt_ms).max(1e-9).min(target_scale);
        // Division can round the reconstructed f32 window a few milliseconds below the target.
        // Move one positive finite scale ULP outward when that happens.
        if area_w / scale < DEFAULT_WINDOW_MS {
            scale = f32::from_bits(scale.to_bits().saturating_sub(1));
        }
        scale
    }

    /// Fits the default time window to a phase-clean point with at least six hours of live history:
    /// an integer number of px/frame or 1 px every N frames. Recalculated only while
    /// the scale remains default/reset-to-live, on the first frame, resize, or present change.
    /// `default_ppm` is the saved user X scale ([Shift+MMB] sync): a new chart starts
    /// with it instead of the built-in six-hour-history default.
    ///
    /// Args:
    ///     area_w: Plot width in logical pixels.
    ///     present_hz: Current display refresh estimate.
    ///     default_ppm: Optional saved X scale that overrides the built-in default on initialization.
    ///
    /// Returns:
    ///     Nothing; the view is updated only while its default initialization is pending.
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
        let area_delta = area_w - self.last_phase_area_w;
        // Every shrink matters for the six-hour lower bound; sub-pixel growth can safely retain
        // the previous scale because it only increases the visible history.
        let area_changed = area_delta < 0.0 || area_delta >= 0.5;
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

    /// Latest time the right anchor may hold, which is how far ahead of the live edge the user may
    /// pan in order to draw on empty chart.
    ///
    /// The limit is not a new number: it is the point at which `now` reaches the LEFT edge of the
    /// plot. [`Self::visible_x`] places the left edge at `right + window*margin - window`, so
    /// anchoring at `now + window*(1-margin)` puts the live edge exactly at x=0 and every visible
    /// pixel in the future. Panning further would take the price off screen entirely with nothing
    /// left to navigate by.
    ///
    /// The window comes from [`Self::visible_x`] itself, so the invariant is exact rather than
    /// nearly, and is capped at [`MAX_WINDOW_MS`]: the ppm guard below it is only a division floor,
    /// and [`Self::set_px_per_ms_sync`] accepts any scale down to that floor, so a scale synced from
    /// a much narrower pane would otherwise put the ceiling decades ahead.
    fn max_right_time_ms(&self, now_ms: f64, area_w: f32) -> f64 {
        let window_ms = self.visible_x(area_w).1.min(MAX_WINDOW_MS) as f64;
        now_ms + window_ms * (1.0 - self.right_margin_frac as f64)
    }

    /// Re-applies the future ceiling to the current scale and plot width, returning whether the
    /// anchor moved. The single owner of that invariant: every other site delegates here.
    ///
    /// The ceiling relates three things that move independently — the anchor, the zoom and the
    /// width — so enforcing it only where the user pans leaves the other two able to break it.
    /// Measured: a pane parked at the ceiling and then narrowed from 1000 to 500 px put the live
    /// edge 450 px LEFT of the plot, and a Shift+MMB scale sync put it 13 500 px off, each leaving a
    /// wholly empty chart with no data to navigate back by. Prepare calls this every frame, so the
    /// invariant survives whichever of the three moved.
    pub fn clamp_future_anchor(&mut self, now_ms: f64, area_w: f32) -> bool {
        // The ceiling is never earlier than `now`, so anything at or behind the live edge is already
        // inside it — the common manual case (browsing history) leaves without dividing anything.
        //
        // A pane with no width to speak of is left ALONE rather than clamped to `now`: its window is
        // zero, so the ceiling would be `now` and the clamp would silently delete a pan the user
        // made at full size. That is not hypothetical — prepare passes `chart_w = 1.0` in
        // order-book-only mode, and a pane mid-layout can be narrower still.
        if self.follow || self.right_time_ms <= now_ms || area_w < 1.0 {
            return false;
        }
        let ceiling = self.max_right_time_ms(now_ms, area_w);
        if self.right_time_ms <= ceiling {
            return false;
        }
        self.right_time_ms = ceiling;
        true
    }

    /// Whether the right anchor sits ahead of the live edge — the view is showing future.
    fn is_ahead_of_now(&self, now_ms: f64) -> bool {
        self.right_time_ms > now_ms
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
        self.manual_persistent = false;
    }

    /// Explicitly and persistently disables live from the toolbar, with no automatic return.
    pub fn set_manual_persistent(&mut self) {
        self.follow = false;
        self.manual_until = 0.0;
        self.manual_persistent = true;
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
        // ABSOLUTE distance, because the anchor can now sit on either side of `now`. A signed
        // comparison reads every future anchor as "near", so releasing a drag one whole window into
        // the future would snap the view back to live and delete the empty chart just drawn on.
        if (now_ms - self.right_time_ms).abs() <= tolerance_ms {
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
    ///
    /// The floor and the equality epsilon are RELATIVE to the window being applied. Absolute ones
    /// belonged to a single order of magnitude: on a 1.2e-8 instrument `1e-6` both widened the
    /// requested window by ~800× and made two genuinely different windows compare equal, so a
    /// micro-priced follower silently stopped tracking its anchor.
    ///
    /// The anchor lock also re-references the scale: the window handed over belongs to the ANCHOR,
    /// so the percentage must be read against the anchor's price, not against whatever this chart
    /// was showing before the lock.
    pub fn set_y_window(&mut self, center: f32, range: f32) -> bool {
        let range = range.max(Self::min_range(center));
        let eps = Self::min_range(center);
        let same = self.manual_price
            && (self.center_price - center).abs() < eps
            && (self.price_range - range).abs() < eps;
        if same {
            return false;
        }
        self.auto_price = false;
        self.manual_price = true;
        self.center_price = center;
        self.price_range = range;
        self.render_center = center;
        self.render_range = range;
        if center.is_finite() && center > 0.0 {
            self.scale_anchor = center;
        }
        true
    }

    /// The price a scale percentage is measured against, or `None` while the view has no price at
    /// all (a chart opened before its first tick).
    ///
    /// [`Self::scale_anchor`] holds it: the rendered centre, frozen for as long as the user holds a
    /// manual Y view. `center_price` is the fallback for the frames before the first `update_y`.
    /// NO fallback to `price_range` — a range is not a price, and dividing the window by itself
    /// would report a confident "100%" for a chart that knows nothing.
    ///
    /// Accepts any positive finite price: instruments trade down to 1e-8, and a threshold that
    /// looks generous for a major coin silently blanks the badge for a micro-priced one.
    pub fn scale_ref_price(&self) -> Option<f32> {
        [self.scale_anchor, self.center_price]
            .into_iter()
            .find(|p| p.is_finite() && *p > 0.0)
    }

    /// The visible Y window as a percentage of [`Self::scale_ref_price`], or `None` when there is
    /// no price to measure against or the result is not a number a badge can show.
    ///
    /// This is what the on-chart scale badge reports, and the only place the figure is computed.
    pub fn visible_scale_percent(&self) -> Option<f32> {
        let reference = self.scale_ref_price()?;
        let pct = self.render_range / reference * 100.0;
        // Bounded on purpose: `as i32` saturates, so an absurd ratio would paint "2147483647%"
        // instead of admitting it has nothing sensible to say.
        (self.render_range > 0.0 && pct.is_finite() && pct < 100_000.0).then_some(pct)
    }

    /// The smallest window allowed for a chart priced at `price` — see [`MIN_RANGE_FRAC`].
    fn min_range(price: f32) -> f32 {
        (price.abs() * MIN_RANGE_FRAC).max(f32::MIN_POSITIVE)
    }

    /// Fixed percentage: visible range = price*percent (like the moonweb ZoomBar).
    pub fn set_scale_percent(&mut self, percent: f32) {
        self.auto_price = false;
        self.manual_price = false;
        self.scale_percent = percent;
        // Same reference the badge divides by, so the chosen step and the reported figure agree.
        let base = self.scale_ref_price().unwrap_or(self.price_range);
        self.price_range = (base * percent).max(Self::min_range(base));
        self.render_range = self.price_range;
        self.render_center = self.center_price;
    }

    // ── Mouse pan/zoom ───────────────────────────────────────────────────────────
    // X drag detaches the view from live immediately; re-anchoring is checked separately on mouse-up.

    /// Pans X by dx pixels (LMB drag/Shift-wheel).
    ///
    /// Panning LEFT walks into the future, up to [`Self::max_right_time_ms`], so a plan can be
    /// drawn on empty chart ahead of the live edge.
    pub fn pan_x_px(&mut self, dx: f32, now_ms: f64, area_w: f32) {
        let dt_ms = dx as f64 / self.px_per_ms.max(MIN_PX_PER_MS) as f64;
        self.right_time_ms -= dt_ms;
        self.follow = false;
        self.clamp_future_anchor(now_ms, area_w);
        if self.is_ahead_of_now(now_ms) || self.manual_persistent {
            // Ahead of the live edge there is nothing to follow, and an automatic return would
            // erase the future the user just panned into. The condition is the CURRENT position and
            // not a memory of having been there: panning back onto the data restores the ordinary
            // three-second return, so a stray forward nudge cannot silently switch it off for good.
            self.manual_until = 0.0;
        } else {
            // Item 9: panning does not disable live permanently. Start a manual hold window,
            // after which `tick_auto_live` re-anchors to now. Every pan frame advances the deadline,
            // so the return occurs about 3 s AFTER release.
            self.manual_until = now_ms + MANUAL_HOLD_MS;
        }
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
    ///
    /// Args:
    ///     factor: Multiplicative zoom step.
    ///     area_w: Plot width in logical pixels.
    ///     cursor_x: Cursor coordinate within the plot.
    ///     now_ms: Current Unix time in milliseconds.
    ///
    /// Returns:
    ///     Nothing; the X scale and anchor are updated in place.
    pub fn zoom_x_at(&mut self, factor: f32, area_w: f32, cursor_x: f32, now_ms: f64) {
        let was_follow = self.follow;
        let right_before = self.right_time_ms;
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
        // Manual zoom remains independent of the much wider built-in default and reaches the same
        // 30-second minimum window at every plot width.
        let hi = (area_w.max(1.0) / MIN_WINDOW_MS).max(lo);
        self.px_per_ms = next.clamp(lo, hi);
        self.x_default_scale = (self.px_per_ms - self.phase_default_px_per_ms).abs() <= 1e-9;
        if was_follow {
            self.right_time_ms = now_ms;
            self.follow = true;
            return;
        }
        let new_window = area_w / self.px_per_ms.max(MIN_PX_PER_MS);
        let left = cursor_time - self.epoch_ms - cursor_x as f64 / self.px_per_ms as f64;
        // Zoom's OWN rule: it must not SCROLL the chart. Anywhere left of the right margin the cursor
        // holds its time while the window grows around it, which walks the anchor forward — measured,
        // one 0.5x notch with the cursor at the left edge landed 0.42 windows into the future, and
        // repeating it crossed the live edge with nothing to pull it back. So a zoom may KEEP a view
        // already parked ahead, and may never push one further ahead than it already was.
        self.right_time_ms =
            (self.epoch_ms + left + new_window as f64 * (1.0 - self.right_margin_frac as f64))
                .min(right_before.max(now_ms));
        // And the general rule on top: the ceiling shrinks with the window, so zooming IN can leave
        // an anchor that was legal at the old scale above it.
        self.clamp_future_anchor(now_ms, area_w);
        self.snap_to_live_if_near(now_ms, area_w);
    }

    /// Zooms Y by RMB drag from the press-time snapshot. Up=zoom out, down=zoom in.
    pub fn rmb_zoom(&mut self, start_center: f32, start_range: f32, cum_dy: f32, now_ms: f64) {
        let factor = 2f32.powf(-cum_dy / YSCALE_PX_PER_2X);
        let r = (start_range * factor).clamp(start_range * 0.25, start_range * 4.0);
        self.center_price = start_center;
        self.price_range = r.max(Self::min_range(start_center));
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
        if !self.manual_price {
            let frame_ref = 1000.0 / 60.0;
            let auto_lerp = (1.0 - (1.0 - AUTO_LERP as f64).powf(dt_ms / frame_ref)) as f32;
            let tick_lerp = (1.0 - (1.0 - TICK_LERP as f64).powf(dt_ms / frame_ref)) as f32;
            let visible_mid = visible.map(|(lo, hi)| (lo + hi) * 0.5);
            // No fallback to the last price while X is manual. `None` here means the window holds
            // nothing to fit ([`fit_band`] decides that), and the last price is then by definition
            // not in the window: centring on it would drag the Y view after a price the user cannot
            // see. Live is the opposite case — there the last price is what the window is about.
            let target_center = if live {
                last_price.or(visible_mid)
            } else {
                visible_mid
            };
            let target_range = match (self.auto_price, visible, target_center) {
                (true, Some((lo, hi)), Some(c)) if live => {
                    // In live mode, keep the last price centered and expand the range symmetrically
                    // so neither tick tails nor order lines are clipped. ×1.20 leaves about 10%
                    // padding at each edge, keeping lines away from the top and bottom.
                    let half = (c - lo)
                        .max(hi - c)
                        .max(c.abs() * 0.0005 + Self::min_range(c));
                    Some(half * 2.0 * 1.20)
                }
                (true, Some((lo, hi)), Some(c)) => {
                    Some((hi - lo).abs().max(c.abs() * 0.0005 + Self::min_range(c)) * 1.20)
                }
                // Nothing to fit. Only a LIVE chart takes the seed range: it is a chart that has not
                // received data yet, and a thousandth of the price is a starting window. Off the live
                // edge the same seed is a collapse — the branch below assigns it with neither lerp
                // nor hysteresis — so the scale the user is reading is kept instead.
                (true, None, Some(c)) if live => Some((c.abs() * 0.001).max(Self::min_range(c))),
                // Fixed percent: sized off the SAME reference `set_scale_percent` and the badge
                // use, or the click's range would be overwritten on the very next frame by a
                // differently-based one.
                (false, _, Some(c)) => {
                    let base = self.scale_ref_price().unwrap_or_else(|| c.abs());
                    Some((base * self.scale_percent).max(Self::min_range(base)))
                }
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
                } else if self.price_range > Self::min_range(self.center_price) {
                    let drift = (c - self.center_price).abs() / self.price_range;
                    if drift > CENTER_BUFFER {
                        self.center_price += (c - self.center_price) * tick_lerp;
                    }
                }
            }
        }
        // A degenerate range gets a default that is a FRACTION OF THE PRICE. The old `+ 1.0`
        // was a whole price unit — on a 1.2e-8 instrument, eight orders of magnitude of chart —
        // and it became reachable once the floors above stopped being absolute.
        if !(self.price_range > Self::min_range(self.center_price)) {
            self.price_range = if self.center_price.abs() > 0.0 {
                self.center_price.abs() * 0.10
            } else {
                1.0
            };
        }
        // Snap Y. In live mode, render_* is piecewise constant: keep the range until the target
        // moves beyond ±RANGE_HYST, and keep the center until the price moves by more than
        // CENTER_SNAP_PX px. In manual mode, follow input exactly for responsive dragging;
        // the render cache is rebuilt transiently during interaction.
        if live && !self.manual_price {
            let target = self.price_range.max(Self::min_range(self.center_price));
            if !self.auto_price
                || !(self.render_range > Self::min_range(self.center_price))
                || target > self.render_range * RANGE_HYST
                || target < self.render_range / RANGE_HYST
            {
                self.render_range = target;
            }
            let ppp =
                (area_h / self.render_range.max(Self::min_range(self.center_price))).max(1e-6);
            if (self.center_price - self.render_center).abs() * ppp > CENTER_SNAP_PX {
                self.render_center = self.center_price;
            }
        } else {
            self.render_range = self.price_range.max(Self::min_range(self.center_price));
            self.render_center = self.center_price;
        }
        // Guarded by the same relative floor as the range itself: an absolute 1e-9 here would
        // misstate pixels-per-price by half on a window narrower than that, which is an ordinary
        // window for a micro-priced instrument.
        self.px_per_price =
            (area_h / self.render_range.max(Self::min_range(self.center_price))).max(1e-6);
        // Re-anchor the scale reference while the view follows the data — and NOT while the user
        // holds a manual Y view, which is what makes a vertical drag leave the reported scale
        // alone. Taken after the snap, so the reference is as piecewise-constant as the range it
        // divides and the figure cannot dither on ordinary ticks.
        if !self.manual_price && self.render_center.is_finite() && self.render_center > 0.0 {
            self.scale_anchor = self.render_center;
        }
    }
}

#[cfg(test)]
mod tests;
