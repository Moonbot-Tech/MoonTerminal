//! Selection and scaling for the horizontal volumes — Moonbot's `HVol`: bought and sold turnover
//! by PRICE over a trailing window, drawn as rows beside the plot, each row's length its turnover
//! against the largest row on screen.
//!
//! The rows come from [`moon_core::market::PriceProfileRow`]; everything here is pure over a
//! slice of them and the tab's configuration, so it can be asserted in tests the way
//! [`crate::side_volume`] is. The chart layer owns the zone geometry and the upload.

use moon_core::config::{ChartGraphicsCfg, HVOL_TF_MAX_S};
use moon_core::market::{PriceProfileRow, ProfileWindow};

/// Windows the reader can pick, in seconds — Moonbot's `TimeFrame` list on its `HVol` popup,
/// between `Auto` (0) and `Max` ([`HVOL_TF_MAX_S`]).
pub const HVOL_TF_CHOICES_S: [u32; 9] = [60, 300, 900, 1_800, 3_600, 7_200, 21_600, 43_200, 86_400];

/// Moonbot's `Auto` table for the horizontal volumes, verbatim: the window against the VISIBLE
/// span in minutes. Pairs of (minutes visible, at least; window in seconds), widest first. The
/// candle timeframe plays no part — only how much time fits on screen. `Auto` never reaches the
/// day or `Max`: those are the reader's to pick by hand.
const AUTO_TF_TABLE: [(f64, u32); 8] = [
    (120.0, 43_200),
    (60.0, 21_600),
    (30.0, 7_200),
    (20.0, 3_600),
    (10.0, 1_800),
    (4.0, 900),
    (2.0, 300),
    (0.0, 60),
];

/// The price window as a percentage of price: the range the popup offers and the config is
/// clamped to.
pub const PRICE_FRAME_PCT_MIN: f32 = 0.01;
pub const PRICE_FRAME_PCT_MAX: f32 = 5.0;

/// Zone width as a fraction of the pane width: the range the config is clamped to.
pub const WIDTH_MIN: f32 = 0.05;
pub const WIDTH_MAX: f32 = 0.5;

/// Narrowest zone that still shows a row, logical pixels. A pane too narrow to hold it gets no
/// zone rather than a sliver.
pub const ZONE_MIN_PX: f32 = 40.0;

/// How wide the zone is and whether it takes that width from the plot — what the pane layout
/// needs from the configuration. The zone always sits at the LEFT, as the reference draws it, so
/// its side is not a parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HvolZoneSpec {
    /// Zone width as a fraction of the pane width, already clamped.
    pub width_frac: f32,
    /// Laid over the plot's left edge (the plot keeps its full width) rather than carved out of
    /// the pane beside it. See `ChartGraphicsCfg::hvol_overlay`.
    pub overlay: bool,
}

/// The zone the tab asks for, or `None` when the horizontal volumes are off.
pub fn zone_spec(cfg: &ChartGraphicsCfg) -> Option<HvolZoneSpec> {
    cfg.hvol_enabled.then(|| HvolZoneSpec {
        width_frac: clamp_width(cfg.hvol_width),
        overlay: cfg.hvol_overlay,
    })
}

/// Snap a configured window onto the reader's list.
///
/// Zero is `Auto` and [`HVOL_TF_MAX_S`] is `Max`; both stay as they are. Anything else lands on the
/// nearest listed window, so a hand-edited value between two steps lights one segment instead of
/// none and the profile covers the window the popup shows.
pub fn snap_tf_s(tf_s: u32) -> u32 {
    if tf_s == 0 || tf_s == HVOL_TF_MAX_S {
        return tf_s;
    }
    HVOL_TF_CHOICES_S
        .iter()
        .copied()
        .min_by_key(|c| c.abs_diff(tf_s))
        .unwrap_or(HVOL_TF_CHOICES_S[0])
}

/// The window `Auto` picks for a visible span: the row of [`AUTO_TF_TABLE`] the span reaches.
///
/// Args:
///     visible_ms: How much time the plot shows edge to edge, milliseconds.
///
/// Returns:
///     Window in seconds; the narrowest row when the span is not a usable number.
pub fn auto_tf_s(visible_ms: f64) -> u32 {
    let narrowest = AUTO_TF_TABLE[AUTO_TF_TABLE.len() - 1].1;
    if !visible_ms.is_finite() || visible_ms <= 0.0 {
        return narrowest;
    }
    let minutes = visible_ms / 60_000.0;
    AUTO_TF_TABLE
        .iter()
        .find(|(at_least, _)| minutes >= *at_least)
        .map(|(_, tf_s)| *tf_s)
        .unwrap_or(narrowest)
}

/// The window the profile covers: the configured one, or `Auto`'s pick for the visible span.
///
/// Args:
///     tf_s: Configured window in seconds, `0` for `Auto`, [`HVOL_TF_MAX_S`] for `Max`.
///     visible_ms: How much time the plot shows edge to edge, milliseconds.
pub fn effective_window(tf_s: u32, visible_ms: f64) -> ProfileWindow {
    match snap_tf_s(tf_s) {
        HVOL_TF_MAX_S => ProfileWindow::All,
        0 => ProfileWindow::Millis(i64::from(auto_tf_s(visible_ms)) * 1_000),
        s => ProfileWindow::Millis(i64::from(s) * 1_000),
    }
}

/// The seconds a window covers, for the caption; `None` for `Max`.
pub fn window_seconds(window: ProfileWindow) -> Option<u32> {
    match window {
        ProfileWindow::Millis(ms) => Some((ms / 1_000).clamp(0, i64::from(u32::MAX)) as u32),
        ProfileWindow::All => None,
    }
}

/// Clamp a configured price window into the drawable range.
pub fn clamp_price_frame_pct(pct: f32) -> f32 {
    if pct.is_finite() {
        pct.clamp(PRICE_FRAME_PCT_MIN, PRICE_FRAME_PCT_MAX)
    } else {
        ChartGraphicsCfg::default().hvol_price_frame_pct
    }
}

/// Clamp a configured zone width into the drawable range.
pub fn clamp_width(frac: f32) -> f32 {
    if frac.is_finite() {
        frac.clamp(WIDTH_MIN, WIDTH_MAX)
    } else {
        ChartGraphicsCfg::default().hvol_width
    }
}

/// The window in price units: Moonbot's `PriceFrame` percentage of the price, floored at the
/// market's tick and rounded to whole ticks.
///
/// `PriceFrame` is to the horizontal volumes what `TimeFrame` is to the vertical ones: not the
/// height of a bar but the width of a ROLLING window — at every pixel of the zone's height the
/// profile shows what traded within this much price around it, which is what makes the reference's
/// picture a continuous contour rather than a stack of blocks. The percentage is taken of a
/// REFERENCE price the caller holds still (the pane's last price, refreshed only when it has moved
/// by [`REF_PRICE_BAND`]), so the window does not move with every print. With a tick known the
/// window is a whole number of ticks, never below one — a window narrower than the tick would hold
/// nothing — and this is why Moonbot prints `PriceFrame: 0.40%` under a `0.12%` slider on a coin
/// whose tick is that wide. Without a tick the raw width stands.
///
/// Args:
///     pct: Window as a percentage of the price, already clamped.
///     ref_price: The price the percentage is taken of.
///     price_step: The market's tick, if known and positive.
///
/// Returns:
///     The window width, or `None` when the reference price is not usable.
pub fn window_width(pct: f32, ref_price: f64, price_step: Option<f64>) -> Option<f64> {
    if !(ref_price.is_finite() && ref_price > 0.0) {
        return None;
    }
    let raw = ref_price * f64::from(pct) / 100.0;
    let width = match price_step.filter(|s| s.is_finite() && *s > 0.0) {
        Some(step) => (raw / step).round().max(1.0) * step,
        None => raw,
    };
    (width.is_finite() && width > 0.0).then_some(width)
}

/// Finest bin the profile is kept at, as a share of the window: the rolling sum is only as
/// smooth as the bins under it, and sixteen to a window is past what the eye separates at any
/// zone height. Coarser than the tick where the tick is finer, so a micro-price coin does not
/// bin a day of trades into a hundred thousand rows.
pub const BINS_PER_WINDOW: f64 = 16.0;

/// The width the profile's bins are built at for a window: the tick, or a sixteenth of the
/// window, whichever is wider.
pub fn bin_width(window: f64, price_step: Option<f64>) -> f64 {
    let fine = window / BINS_PER_WINDOW;
    match price_step.filter(|s| s.is_finite() && *s > 0.0) {
        Some(step) => fine.max(step),
        None => fine,
    }
}

/// Where the zone's samples are taken: one per device pixel of its height, on the pane view's
/// price mapping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleGrid {
    /// Price at the zone's TOP edge.
    pub price_top: f64,
    /// Device pixels per unit of price.
    pub px_per_price: f64,
    /// Zone height in device pixels; the number of samples.
    pub height_px: u32,
}

/// The rolling sums the zone draws: at every pixel of its height, what was bought and sold
/// within `window` of price around that pixel's price, from the profile's bins.
///
/// Moonbot's `HVol` picture, and the horizontal twin of the sides band's rolling sums: a bin
/// stays in the picture for a whole window either side of itself, so a thin market reads as a
/// continuous contour instead of lone bars, and the window's width is the smoothing. The samples
/// are one pixel tall and abut exactly, so the layer draws them with no seams and no overlap.
///
/// Two pointers walk the sorted bins once, so the cost is the zone's height plus the bins —
/// never height × bins. A bin counts when its centre lies inside the window; at sixteen bins to a
/// window the edge bin's share is under what the eye separates.
///
/// Args:
///     bins: Profile bins sorted by price, as the source serves them.
///     window: Rolling window width in price units.
///     grid: Where the samples are taken.
///     out: Reused buffer; cleared first. Filled bottom-up, so the rows are sorted by price.
pub fn rolling_samples(
    bins: &[PriceProfileRow],
    window: f64,
    grid: SampleGrid,
    out: &mut Vec<PriceProfileRow>,
) {
    out.clear();
    if !(window.is_finite() && window > 0.0)
        || !(grid.px_per_price.is_finite() && grid.px_per_price > 0.0)
        || !grid.price_top.is_finite()
        || grid.height_px == 0
        || bins.is_empty()
    {
        return;
    }
    let half = window * 0.5;
    let px = 1.0 / grid.px_per_price;
    let mut lo = 0usize;
    let mut hi = 0usize;
    let (mut buy, mut sell) = (0.0f64, 0.0f64);
    // Bottom-up: prices ascend with the pointers. Each edge is the SAME expression for the two
    // samples that share it, so neighbours share the seam bit for bit.
    for y in (0..grid.height_px).rev() {
        let price_hi = grid.price_top - f64::from(y) * px;
        let price_lo = grid.price_top - f64::from(y + 1) * px;
        let centre = (price_hi + price_lo) * 0.5;
        let (from, to) = (centre - half, centre + half);
        while hi < bins.len() && bin_centre(&bins[hi]) < to {
            buy += f64::from(bins[hi].buy_quote.max(0.0));
            sell += f64::from(bins[hi].sell_quote.max(0.0));
            hi += 1;
        }
        while lo < hi && bin_centre(&bins[lo]) < from {
            buy -= f64::from(bins[lo].buy_quote.max(0.0));
            sell -= f64::from(bins[lo].sell_quote.max(0.0));
            lo += 1;
        }
        // A window that emptied out leaves rounding dust behind the subtractions.
        if lo == hi {
            buy = 0.0;
            sell = 0.0;
        }
        if buy > 0.0 || sell > 0.0 {
            out.push(PriceProfileRow {
                price_lo: price_lo as f32,
                price_hi: price_hi as f32,
                buy_quote: buy.max(0.0) as f32,
                sell_quote: sell.max(0.0) as f32,
            });
        }
    }
}

fn bin_centre(bin: &PriceProfileRow) -> f64 {
    (f64::from(bin.price_lo) + f64::from(bin.price_hi)) * 0.5
}

/// How far the market may move from the reference price before the price window follows it.
///
/// The window is a percentage of the price, and a reference that tracked every print would re-bin
/// the whole profile on each tick. A five-percent band keeps the drawn window within a few percent
/// of the slider's ask while the reference moves only on a move the eye can see; and it is a band
/// rather than a rounding step, so a price oscillating across a digit boundary cannot flap the
/// width back and forth.
pub const REF_PRICE_BAND: f64 = 0.05;

/// Whether the reference price behind [`window_width`] should follow the market to `price`.
///
/// Args:
///     reference: The reference price the rows were built for, `0` for none yet.
///     price: The market's current price.
pub fn ref_price_moved(reference: f64, price: f64) -> bool {
    if !(price.is_finite() && price > 0.0) {
        return false;
    }
    if !(reference.is_finite() && reference > 0.0) {
        return true;
    }
    (price / reference - 1.0).abs() > REF_PRICE_BAND
}

/// The effective window as a percentage of the reference price, for the caption.
pub fn price_frame_pct_of(window: f64, ref_price: f64) -> f32 {
    if !(ref_price.is_finite() && ref_price > 0.0 && window.is_finite()) {
        return 0.0;
    }
    (window / ref_price * 100.0) as f32
}

/// The longest row among those inside a price range, in the quote currency.
///
/// Overlaid (`Smooth graph`) a row is as long as its larger side; stacked, as long as both
/// together — so the same rows normalise differently under the two kinds. Only rows that touch the
/// visible price range count: the scale follows what is on screen, as the reference terminal's
/// does.
///
/// `None` when nothing is visible or every visible row is empty — the caller must not build a
/// reciprocal from a zero maximum.
pub fn visible_row_max(
    rows: &[PriceProfileRow],
    price_lo: f32,
    price_hi: f32,
    stacked: bool,
) -> Option<f32> {
    let mut best = 0.0f32;
    for r in rows {
        if r.price_hi <= price_lo || r.price_lo >= price_hi {
            continue;
        }
        let buy = r.buy_quote.max(0.0);
        let sell = r.sell_quote.max(0.0);
        let len = if stacked { buy + sell } else { buy.max(sell) };
        if len.is_finite() {
            best = best.max(len);
        }
    }
    (best > 0.0).then_some(best)
}

/// The sample (or bin) a price falls in, for the cursor readout: rows are half-open `[lo, hi)`
/// and sorted, so a binary search answers.
pub fn row_at(rows: &[PriceProfileRow], price: f32) -> Option<PriceProfileRow> {
    if !price.is_finite() {
        return None;
    }
    let ix = rows.partition_point(|r| r.price_hi <= price);
    rows.get(ix)
        .copied()
        .filter(|r| r.price_lo <= price && price < r.price_hi)
}

/// Localizable spelling of a window in seconds: `1m`, `2h`, `1d`; `None` for `Max`.
///
/// Delegates to the bottom band's own `bucket_label` so the two popups can never spell one
/// duration two ways.
pub fn tf_label(tf_s: u32) -> Option<String> {
    if tf_s == HVOL_TF_MAX_S {
        return None;
    }
    crate::volume_bars::bucket_label(f64::from(tf_s) * 1_000.0)
}

#[cfg(test)]
mod tests;
