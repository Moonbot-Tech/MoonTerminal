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

/// "Nice" time interval in seconds that fits approximately `target` labels.
pub fn nice_time_step(window_sec: f64, target: f64) -> f64 {
    const STEPS: [f64; 16] = [
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
        14400.0, 21600.0,
    ];
    let want = window_sec / target.max(1.0);
    for s in STEPS {
        if s >= want {
            return s;
        }
    }
    STEPS[STEPS.len() - 1]
}

/// Returns the count of round time-axis labels appropriate for a plot width.
///
/// Args:
///     plot_width_px: Available plot width in logical pixels.
///
/// Returns:
///     The nearest target at one label per 190 logical pixels, floored at three so narrow charts
///     retain a readable time axis.
pub fn time_label_target(plot_width_px: f32) -> f64 {
    (plot_width_px / 190.0).round().max(3.0) as f64
}

#[cfg(test)]
mod tests;
