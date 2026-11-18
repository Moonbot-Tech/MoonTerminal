//! The repair every read runs, and the ranges it repairs into.
//!
//! IDEMPOTENT by construction, and that is load-bearing: the configuration is compared to decide
//! whether a chart's settings changed, so a repair that still has work to do on its own output — a
//! NaN above all, which is not equal to itself — would report a change on every notification.
//! Each rule below therefore lands on a value the same rule leaves alone.
//!
//! What is NOT repaired here: a `Fixed` timeframe finer than the chart's own. The chart's
//! timeframe is a property of the tab, not of this configuration, so the check belongs to the
//! terminal, which knows both.

use super::{
    AutoFigureRow, AutoFiguresCfg, BrokenPolicy, DetectorParams, EventMarks, LineCaption,
    LineStyle, RowDetector, SqueezeParams, SwingParams, Take, TfSource, TrendParams,
};

/// Most rows one configuration keeps. Every visible row is a detector run, so an unbounded list —
/// only reachable by hand-editing — is a per-candle cost nobody chose.
pub const AF_ROWS_MAX: usize = 64;
/// Longest row name kept, in characters; the same ceiling as a caption row's name.
pub const AF_ROW_NAME_MAX: usize = crate::config::chart_labels::LABEL_ROW_NAME_MAX;
/// Most lines a row keeps (per side when the side is `Both`). Past this a chart is a grid.
pub const AF_TAKE_MAX: u8 = 10;
/// Stroke width bounds, logical pixels.
pub const AF_THICKNESS_MIN: f32 = 0.5;
pub const AF_THICKNESS_MAX: f32 = 8.0;
/// Weakest zone opacity kept; a fainter band is invisible and is dropped to "no zone".
pub const AF_ZONE_ALPHA_MIN: f32 = 0.02;
/// Bars searched. Below the minimum no pivot can be confirmed. The maximum is a COST bound, not a
/// data one: every detector pass scales with the bars and the pivots they hold, and 5 000 bars
/// already reach past the local 5-minute cache's whole retention (15 days, 4 320 bars) and cover
/// ~3.5 days of 1-minute bars, 7 months of 1-hour ones. The line search is capped separately, by
/// pivots rather than bars (`moon_chart::auto_figures::trend::MAX_LINE_PIVOTS`).
pub const AF_LOOKBACK_MIN: u32 = 10;
pub const AF_LOOKBACK_MAX: u32 = 5_000;
/// Pivot width, bars per side. Zero makes every bar a pivot.
pub const AF_PIVOT_MIN: u8 = 1;
pub const AF_PIVOT_MAX: u8 = 20;
/// Most touches a row may demand; more than this is never met inside the lookback.
pub const AF_TOUCHES_MAX: u8 = 20;
/// Any percentage-of-price tolerance (touch, press, approach), upper bound.
pub const AF_PCT_MAX: f32 = 10.0;
/// Smallest touch tolerance: a zero band only ever matches the very same price.
pub const AF_TOLERANCE_MIN: f32 = 0.01;
/// Largest slope a trend may be asked to allow, % of price per bar.
pub const AF_SLOPE_MAX: f32 = 100.0;
/// Longest a broken level may be kept, in bars.
pub const AF_BROKEN_KEEP_MAX: u16 = 1_000;

impl AutoFiguresCfg {
    /// Repair whatever a hand-edited or older file can state. Idempotent.
    pub fn sanitize(&mut self) {
        self.rows.truncate(AF_ROWS_MAX);
        for row in &mut self.rows {
            row.sanitize();
        }
    }
}

impl AutoFigureRow {
    /// Repair one row. Idempotent.
    pub fn sanitize(&mut self) {
        // Trim, cut, trim again: a cut can land on a space, and a repair that leaves work for its
        // own next run never equals itself.
        let repaired = {
            let cut: String = self.name.trim().chars().take(AF_ROW_NAME_MAX).collect();
            cut.trim_end().to_string()
        };
        if self.name != repaired {
            self.name = repaired;
        }
        // An unknown detector is written back exactly as read: repairing what this build cannot
        // interpret would corrupt it for the build that can.
        if let RowDetector::Known(params) = &mut self.detector {
            sanitize_detector(params);
            // A fixed timeframe the detector cannot answer on draws nothing for ever; pull it down
            // to the coarsest one that works. `TfId` orders smallest first.
            if let TfSource::Fixed(tf) = &mut self.tf {
                *tf = (*tf).min(params.coarsest_tf());
            }
        }
        sanitize_take(&mut self.take);
        self.style.sanitize();
        self.caption.sanitize();
        sanitize_events(&mut self.events);
    }
}

impl LineStyle {
    /// Repair the stroke. Idempotent.
    pub fn sanitize(&mut self) {
        self.thickness = finite_or(self.thickness, LineStyle::default().thickness)
            .clamp(AF_THICKNESS_MIN, AF_THICKNESS_MAX);
        // Too faint, negative or NaN: no band at all, which is what such a value draws anyway.
        self.zone = self
            .zone
            .filter(|a| a.is_finite() && *a >= AF_ZONE_ALPHA_MIN)
            .map(|a| a.min(1.0));
    }
}

impl LineCaption {
    /// Drop repeated parts, keeping the first of each. Idempotent.
    pub fn sanitize(&mut self) {
        let mut seen = Vec::with_capacity(self.parts.len());
        self.parts.retain(|part| {
            let first = !seen.contains(part);
            seen.push(*part);
            first
        });
    }
}

/// Repair one detector's parameters into their documented ranges.
fn sanitize_detector(params: &mut DetectorParams) {
    match params {
        DetectorParams::Swing(p) => sanitize_swing(p),
        // A calendar period and a side: every combination is meaningful.
        DetectorParams::PeriodExtreme(_) => {}
        DetectorParams::Trend(p) => sanitize_trend(p),
        DetectorParams::Squeeze(p) => sanitize_squeeze(p),
    }
}

/// Pivot widths, the lookback they need, tolerance, touches and the broken-level policy.
fn sanitize_swing(p: &mut SwingParams) {
    let d = SwingParams::default();
    p.pivot_left = p.pivot_left.clamp(AF_PIVOT_MIN, AF_PIVOT_MAX);
    p.pivot_right = p.pivot_right.clamp(AF_PIVOT_MIN, AF_PIVOT_MAX);
    p.lookback_bars = lookback(p.lookback_bars, p.pivot_left, p.pivot_right);
    p.tolerance_pct = pct(p.tolerance_pct, d.tolerance_pct, AF_TOLERANCE_MIN);
    p.min_touches = p.min_touches.clamp(1, AF_TOUCHES_MAX);
    sanitize_broken(&mut p.broken);
}

/// A `Keep` policy holds a broken line at most [`AF_BROKEN_KEEP_MAX`] bars.
fn sanitize_broken(policy: &mut BrokenPolicy) {
    if let BrokenPolicy::Keep { bars } = policy {
        *bars = (*bars).min(AF_BROKEN_KEEP_MAX);
    }
}

/// As [`sanitize_swing`] (the broken-line policy included), plus the slope window: both bounds
/// non-negative and in order.
fn sanitize_trend(p: &mut TrendParams) {
    let d = TrendParams::default();
    p.pivot_left = p.pivot_left.clamp(AF_PIVOT_MIN, AF_PIVOT_MAX);
    p.pivot_right = p.pivot_right.clamp(AF_PIVOT_MIN, AF_PIVOT_MAX);
    p.lookback_bars = lookback(p.lookback_bars, p.pivot_left, p.pivot_right);
    p.tolerance_pct = pct(p.tolerance_pct, d.tolerance_pct, AF_TOLERANCE_MIN);
    // Two pivots define a line; a "trend" through one is any line at all.
    p.min_touches = p.min_touches.clamp(2, AF_TOUCHES_MAX);
    // The bounds are by absolute value, so a negative one is read as its magnitude.
    let lo = finite_or(p.min_slope_pct_per_bar, d.min_slope_pct_per_bar)
        .abs()
        .min(AF_SLOPE_MAX);
    let hi = finite_or(p.max_slope_pct_per_bar, d.max_slope_pct_per_bar)
        .abs()
        .min(AF_SLOPE_MAX);
    (p.min_slope_pct_per_bar, p.max_slope_pct_per_bar) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    sanitize_broken(&mut p.broken);
}

/// The level half as a swing, plus the sloped half's touches and both distances.
fn sanitize_squeeze(p: &mut SqueezeParams) {
    let d = SqueezeParams::default();
    sanitize_swing(&mut p.level);
    p.min_trend_touches = p.min_trend_touches.clamp(2, AF_TOUCHES_MAX);
    p.trend_tolerance_pct = pct(
        p.trend_tolerance_pct,
        d.trend_tolerance_pct,
        AF_TOLERANCE_MIN,
    );
    p.pressed_pct = pct(p.pressed_pct, d.pressed_pct, AF_TOLERANCE_MIN);
}

/// At least one line: a row that keeps none is a hidden row, and the eye switch says that.
fn sanitize_take(take: &mut Take) {
    take.count = take.count.clamp(1, AF_TAKE_MAX);
}

/// The approach distance: zero is allowed (a mark only on an exact touch), negative is not.
fn sanitize_events(events: &mut EventMarks) {
    events.approach_pct = pct(events.approach_pct, EventMarks::default().approach_pct, 0.0);
}

/// A lookback long enough to confirm one pivot of the given widths, within the global bounds.
fn lookback(bars: u32, left: u8, right: u8) -> u32 {
    let need = u32::from(left) + u32::from(right) + 1;
    bars.clamp(AF_LOOKBACK_MIN.max(need), AF_LOOKBACK_MAX)
}

/// A percentage of price: a non-finite value takes `default`, the rest is clamped to
/// `min..=AF_PCT_MAX`.
fn pct(v: f32, default: f32, min: f32) -> f32 {
    finite_or(v, default).clamp(min, AF_PCT_MAX)
}

/// `v`, or `default` when `v` is NaN or infinite. `f32::clamp` passes NaN straight through, so
/// every clamp above is preceded by this.
fn finite_or(v: f32, default: f32) -> f32 {
    if v.is_finite() { v } else { default }
}
