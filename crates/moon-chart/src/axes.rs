//! Pure axis-layout math: "nice" price/time grid intervals and decimal precision.
//! Selected-zone tick positions and clock formatting live in `moon-ui-gpui::chartdx::axes`, while
//! this crate remains UI- and platform-agnostic. Ported from moonweb's
//! `coords.ts` niceInterval/priceDecimals helpers.

/// Snapshot of the view state captured for the current GPUI/chartdx scale consumer.
/// Values come from the current render frame after that frame is rendered.
#[derive(Clone, Copy)]
pub struct AxisSnapshot {
    /// Physical pixels per millisecond — the width of the time window.
    pub px_per_ms: f32,
    /// Fraction of the "future" window on the right (right_margin_frac).
    pub right_margin_frac: f32,
    /// Price at the center of the area and the visible range.
    pub render_center: f32,
    pub render_range: f32,
    /// Time origin (unix ms) and the time at the right anchor (unix ms).
    pub epoch_ms: f64,
    pub right_time_ms: f64,
}

/// "Nice" price-grid interval for approximately `target_lines` lines (ported from niceInterval).
pub fn nice_interval(range: f32, target_lines: f32) -> f32 {
    let rough = range / target_lines.max(1.0);
    if !(rough > 0.0) {
        return 1.0;
    }
    let mag = 10f32.powf(rough.log10().floor());
    let n = rough / mag;
    let nice = if n < 1.5 {
        1.0
    } else if n < 3.0 {
        2.0
    } else if n < 7.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Number of decimal places for a price of this magnitude (ported from priceDecimals).
pub fn price_decimals(price: f32) -> usize {
    let p = price.abs();
    if p >= 1000.0 {
        1
    } else if p >= 10.0 {
        2
    } else if p >= 1.0 {
        3
    } else {
        4
    }
}

/// Format an axis or cursor price with the market grid and MoonBot's small-price unit.
///
/// At or below `0.00001`, MoonBot prints price × 10^8 and removes eight decimal places.
/// This is a display unit only, including for non-BTC markets. Without a usable tick (not
/// arrived yet, or refused as a placeholder by the source), keep the ordinary magnitude format, but give small prices four significant digits (up to the
/// same twelve-decimal limit as the ladder) instead of rounding them to zero.
///
/// Generic over the width because a figure stores its price as `f64`: narrowing that to `f32`
/// would misprint the last tick on large prices, while an `f32` axis value widens exactly.
pub fn format_price(price: impl Into<f64>, tick: Option<f64>) -> String {
    let price: f64 = price.into();
    let decimals = tick
        .filter(|tick| tick.is_finite() && *tick > 0.0)
        .map(crate::ladder::tick_decimals)
        .unwrap_or_else(|| {
            // Counted in `f32` for every caller: 0.01f32 widens to just under 0.01, so `log10` on
            // the widened value would add a digit to what the axis prints before its tick arrives.
            let magnitude = (price as f32).abs();
            if magnitude > 0.0 && magnitude < 1.0 {
                (3.0 - magnitude.log10().floor())
                    .clamp(4.0, crate::ladder::MAX_TICK_DECIMALS as f32) as usize
            } else {
                price_decimals(price as f32)
            }
        });
    if price.abs() <= 0.00001 {
        let satoshis = price * 1e8;
        let decimals = decimals.saturating_sub(8);
        format!("{satoshis:.decimals$} sat")
    } else {
        format!("{price:.decimals$}")
    }
}

/// One civil day in seconds; a time step of at least this long labels dates, not clocks.
pub const DAY_SECS: f64 = 86_400.0;

/// The nominal length of one calendar month in seconds.
///
/// It only sizes a month step against the window in [`time_step`]; the ticks themselves fall on
/// the first of each month, which [`calendar_months`] tells the tick generator.
pub const MONTH_SECS: f64 = 30.0 * DAY_SECS;

/// Free space kept between two neighbouring time captions, as a share of a caption's width.
pub const TIME_CAPTION_GAP_SHARE: f32 = 0.8;

/// How a time-axis caption prints at a given step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeCaptionKind {
    /// A clock with seconds, for steps under a minute.
    Seconds,
    /// A clock without seconds, for steps from a minute to six hours.
    Minutes,
    /// A date alone, for day and month steps: every caption sits on a local midnight.
    Date,
}

impl TimeCaptionKind {
    /// The caption kind a step prints with.
    ///
    /// Args:
    ///     step_secs: A step returned by [`time_step`].
    ///
    /// Returns:
    ///     Seconds under one minute, a date from one day, minutes between.
    pub fn of_step(step_secs: f64) -> Self {
        if step_secs < 60.0 {
            Self::Seconds
        } else if step_secs < DAY_SECS {
            Self::Minutes
        } else {
            Self::Date
        }
    }
}

/// Measured width of one caption of each kind, in the same pixels as the plot width.
///
/// The renderer measures them in the axis font at the pane's zoom, so the step follows what is
/// actually drawn rather than an assumed width. (The axis font has a fixed size; the "Font"
/// slider does not reach it.)
#[derive(Clone, Copy, Debug)]
pub struct TimeCaptionWidths {
    /// A clock with seconds, dated when the window is not all today.
    pub seconds: f32,
    /// A clock without seconds, dated when the window is not all today.
    pub minutes: f32,
    /// A date caption.
    pub date: f32,
}

impl TimeCaptionWidths {
    /// The width of a caption of `kind`.
    fn of(&self, kind: TimeCaptionKind) -> f32 {
        match kind {
            TimeCaptionKind::Seconds => self.seconds,
            TimeCaptionKind::Minutes => self.minutes,
            TimeCaptionKind::Date => self.date,
        }
    }
}

/// The densest round time step whose captions fit the plot with room between them.
///
/// A step fits when neighbouring captions stand at least their own width plus
/// [`TIME_CAPTION_GAP_SHARE`] of it apart. Picking the first round step past a fixed caption
/// target (one per 190 px) left captions 190–475 px apart, because neighbouring round steps
/// differ by up to 2.5×, and short `HH:MM` or `DD.MM` captions sat in wide empty runs.
///
/// Clock steps run from one second to twelve hours, then whole days (1, 2, 7, 14), then calendar
/// months (1, 2, 3, 6, 12), each month step expressed as a multiple of [`MONTH_SECS`]. Without the
/// day and month steps a window of weeks or months stayed on six-hour marks a pixel or two apart,
/// so the spacing walk kept captions at arbitrary hours (`06:00`, `12:00`, `18:00`, `00:00`).
///
/// Args:
///     window_sec: Width of the visible window in seconds.
///     plot_width_px: Width of the plot the window spans.
///     widths: Measured caption widths per kind.
///
/// Returns:
///     The smallest step that fits; when none does, or the inputs are degenerate, the coarsest
///     step, twelve months.
pub fn time_step(window_sec: f64, plot_width_px: f32, widths: &TimeCaptionWidths) -> f64 {
    const STEPS: [f64; 26] = [
        1.0,
        2.0,
        5.0,
        10.0,
        15.0,
        30.0,
        60.0,
        120.0,
        300.0,
        600.0,
        900.0,
        1800.0,
        3600.0,
        7200.0,
        14400.0,
        21600.0,
        43200.0,
        DAY_SECS,
        2.0 * DAY_SECS,
        7.0 * DAY_SECS,
        14.0 * DAY_SECS,
        MONTH_SECS,
        2.0 * MONTH_SECS,
        3.0 * MONTH_SECS,
        6.0 * MONTH_SECS,
        12.0 * MONTH_SECS,
    ];
    let coarsest = STEPS[STEPS.len() - 1];
    let px_per_sec = f64::from(plot_width_px) / window_sec;
    if !(px_per_sec.is_finite() && px_per_sec > 0.0) {
        return coarsest;
    }
    STEPS
        .into_iter()
        .find(|&step| {
            let need = widths.of(TimeCaptionKind::of_step(step)) * (1.0 + TIME_CAPTION_GAP_SHARE);
            step * px_per_sec >= f64::from(need)
        })
        .unwrap_or(coarsest)
}

/// The calendar-month count a [`time_step`] result stands for, if it is a month step.
///
/// Args:
///     step_secs: A step returned by [`time_step`].
///
/// Returns:
///     `Some(n)` for a step of `n` months, `None` for a clock or day step.
pub fn calendar_months(step_secs: f64) -> Option<u32> {
    let months = step_secs / MONTH_SECS;
    (months >= 1.0 && (months - months.round()).abs() < 1e-9).then(|| months.round() as u32)
}

/// The left-to-right spacing walk over the round time-axis captions.
///
/// A caption is kept when its left edge clears the previous kept caption's right edge by
/// [`Self::MIN_GAP_PX`]. Which captions are kept never depends on the crosshair: the readout plate
/// only hides the kept caption under it, and that caption still holds its slot. Otherwise every
/// caption to the right of the cursor was chosen again one step off, so a zoomed-out row there
/// re-phased (`00:00`/`12:00` → `06:00`/`18:00`) on every mouse move while the left side stood
/// (#1117).
pub struct TimeCaptionWalk {
    last_right: f32,
    plot_left: f32,
}

impl TimeCaptionWalk {
    /// Smallest horizontal gap between two kept captions, in logical pixels.
    pub const MIN_GAP_PX: f32 = 6.0;

    /// Starts a walk over one pane's time axis.
    ///
    /// Args:
    ///     plot_left: Left edge of the plot; a caption starting more than a pixel left of it is
    ///         dropped rather than clipped.
    pub fn new(plot_left: f32) -> Self {
        Self {
            last_right: f32::NEG_INFINITY,
            plot_left,
        }
    }

    /// Decides one caption; captions must be visited left to right.
    ///
    /// Args:
    ///     left: The caption's left edge.
    ///     right: The caption's right edge.
    ///     covered: Whether the crosshair readout plate covers it.
    ///
    /// Returns:
    ///     Whether to draw it: it holds a slot and is not covered.
    pub fn place(&mut self, left: f32, right: f32, covered: bool) -> bool {
        let slot = left >= self.last_right + Self::MIN_GAP_PX && left >= self.plot_left - 1.0;
        if slot {
            self.last_right = right;
        }
        slot && !covered
    }
}

#[cfg(test)]
mod tests;
