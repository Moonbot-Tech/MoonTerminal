//! Backend-free decisions for the trade clusters: the grid the tape is summed on and how big each
//! cell's disc is.
//!
//! A cluster is one cell of a time × price grid: every trade whose time falls into the cell's
//! interval and whose price rounds to the cell's row adds its quote notional to it. The cell draws
//! one disc, its AREA proportional to the cell's volume against the biggest cell on screen, its
//! colour the side that traded more. The grid follows the zoom on both axes so the largest disc
//! still fits its cell and no two discs overlap: the time interval from the X scale, the price row
//! from the market's tick and the reader's step.

use std::collections::BTreeMap;

/// Steps the reader can pick for the price row, in ticks. `0` is Auto. The fixed steps are the
/// list a DOM offers (`x1 x2 x4 x10`).
pub const CLUSTER_STEPS: [u16; 5] = [0, 1, 2, 4, 10];

/// Time intervals a cell can span, in milliseconds, finest first.
pub const TIME_STEPS_MS: [i64; 16] = [
    1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000,
    1_800_000, 3_600_000, 7_200_000, 14_400_000, 86_400_000,
];

/// The smallest cell, in logical pixels on either axis. Sized for the LABEL, not the dot: at the
/// default label size the biggest disc of a cell this size holds a four-glyph volume (`2.3M`), and
/// a disc at about half its cell's volume still holds three (`11K`).
pub const MIN_CELL_PX: f32 = 32.0;

/// Gap kept between the largest disc and its cell's edge, physical pixels, so neighbours never
/// touch.
pub const CELL_GAP_PX: f32 = 1.0;

/// The smallest disc a non-empty cell draws, physical pixels — still a visible dot however small
/// its volume.
pub const MIN_DISC_PX: f32 = 1.5;

/// Snap a stored step onto [`CLUSTER_STEPS`]: Auto stays Auto, anything else lands on the nearest
/// fixed step, so a hand-edited value lights one segment instead of none.
pub fn snap_step(step: u16) -> u16 {
    if step == 0 {
        return 0;
    }
    CLUSTER_STEPS[1..]
        .iter()
        .copied()
        .min_by_key(|s| s.abs_diff(step))
        .unwrap_or(1)
}

/// The grid a tape is summed on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClusterGrid {
    /// One cell's time interval in milliseconds; cells are aligned to whole multiples of it in
    /// absolute time, so a pan never re-buckets a trade.
    pub time_step_ms: i64,
    /// The market's price tick.
    pub tick: f64,
    /// How many ticks one price row spans.
    pub price_step: u32,
}

impl ClusterGrid {
    /// Height of one price row in price units.
    pub fn row_height(&self) -> f64 {
        self.tick * f64::from(self.price_step)
    }
}

/// The finest interval of [`TIME_STEPS_MS`] whose cell is at least `min_px` wide.
///
/// Args:
///     px_per_ms: Physical pixels per millisecond of the plot.
///     min_px: Physical width a cell needs.
///
/// Returns:
///     The interval, or `None` for an unusable scale or a zoom so far out that even a day is too
///     narrow — the chart then draws no clusters.
pub fn time_step_ms(px_per_ms: f32, min_px: f32) -> Option<i64> {
    if !(px_per_ms.is_finite() && px_per_ms > 0.0) {
        return None;
    }
    TIME_STEPS_MS
        .iter()
        .copied()
        .find(|&step| step as f32 * px_per_ms >= min_px)
}

/// The price step, in ticks: for Auto the smallest of 1, 2, 4, 10 … whose row is at least
/// `min_px` tall (`crate::ladder::row_step`); a fixed `choice` is a MINIMUM — the row never gets
/// thinner than Auto's, or the discs of neighbouring rows would overlap at a zoom too far out for
/// the chosen grid. Where Auto finds no readable row at all, neither does a fixed step: `None`,
/// and the pane keeps its crosses.
pub fn price_step(choice: u16, tick: f64, px_per_price: f32, min_px: f32) -> Option<u32> {
    if !(tick.is_finite() && tick > 0.0) {
        return None;
    }
    let auto = crate::ladder::row_step(tick, px_per_price, min_px);
    if choice == 0 {
        return auto;
    }
    auto.map(|a| a.max(u32::from(choice)))
}

/// One cell of the grid: its indices and the quote notional bought and sold in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClusterCell {
    /// Index of the cell's interval: its start is `time_idx × time_step_ms` in absolute time.
    pub time_idx: i64,
    /// Index of the cell's row: its centre price is `price_idx × row_height`.
    pub price_idx: i64,
    pub buy: f32,
    pub sell: f32,
}

impl ClusterCell {
    /// Everything traded in the cell.
    pub fn total(&self) -> f32 {
        self.buy + self.sell
    }
}

/// Sum a tape into the grid's cells.
///
/// A trade's row is its price rounded to the tick first and then to the nearest row, so an `f32`
/// price never lands a row off; its interval is its absolute time floored to the step.
///
/// Args:
///     trades: `(absolute time ms, price, quantity, is_buy)` per trade, any order.
///     grid: The grid to sum on.
///     out: Cleared, then filled with the non-empty cells ordered by time, then price.
pub fn aggregate(
    trades: impl IntoIterator<Item = (f64, f32, f32, bool)>,
    grid: ClusterGrid,
    out: &mut Vec<ClusterCell>,
) {
    out.clear();
    if grid.time_step_ms <= 0 || !(grid.tick.is_finite() && grid.tick > 0.0) || grid.price_step == 0
    {
        return;
    }
    let step = i64::from(grid.price_step);
    let mut cells: BTreeMap<(i64, i64), (f32, f32)> = BTreeMap::new();
    for (time_ms, price, qty, is_buy) in trades {
        let notional = price * qty;
        if !time_ms.is_finite() || !(notional > 0.0) || !notional.is_finite() {
            continue;
        }
        let ticks = (f64::from(price) / grid.tick).round();
        if !ticks.is_finite() || ticks.abs() > 9.0e15 {
            continue;
        }
        let ticks = ticks as i64;
        // Nearest row: half a row either side of the row's centre.
        let price_idx = (ticks + step.div_euclid(2)).div_euclid(step);
        let time_idx = (time_ms as i64).div_euclid(grid.time_step_ms);
        let cell = cells.entry((time_idx, price_idx)).or_insert((0.0, 0.0));
        if is_buy {
            cell.0 += notional;
        } else {
            cell.1 += notional;
        }
    }
    out.extend(
        cells
            .into_iter()
            .map(|((time_idx, price_idx), (buy, sell))| ClusterCell {
                time_idx,
                price_idx,
                buy,
                sell,
            }),
    );
}

/// The largest disc's radius for cells of `cell_w × cell_h` physical pixels: half the narrower
/// side, less the gap that keeps neighbours apart.
pub fn max_radius(cell_w: f32, cell_h: f32) -> f32 {
    (cell_w.min(cell_h) * 0.5 - CELL_GAP_PX).max(MIN_DISC_PX)
}

/// A cell's disc radius: its AREA proportional to its volume against the biggest cell on screen,
/// floored at [`MIN_DISC_PX`] so a small cell still shows.
pub fn disc_radius(total: f32, max_total: f32, max_r: f32) -> f32 {
    if !(max_total > 0.0) || !(total > 0.0) {
        return MIN_DISC_PX;
    }
    (max_r * (total / max_total).min(1.0).sqrt()).max(MIN_DISC_PX)
}

/// A cell's volume printed short enough to sit inside its disc: at most three digits and an SI
/// suffix — one decimal below ten of a unit (`2.3M`), whole above it (`11K`, `906`) — where the
/// chart's usual format (`10.8K`) costs the glyph that decides whether the label fits. Below one
/// it keeps a decimal, and a volume that rounds to nothing prints `<0.1` rather than a `0.0` that
/// reads as an empty cell.
pub fn short_volume(v: f32) -> String {
    if !v.is_finite() {
        return String::new();
    }
    const UNITS: [(f32, &str); 5] = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K"), (1.0, "")];
    let a = v.abs();
    let sign = if v < 0.0 { "-" } else { "" };
    if a < 1.0 {
        let one = format!("{a:.1}");
        let body = match one.as_str() {
            "0.0" => "<0.1",
            "1.0" => "1",
            s => s,
        };
        return format!("{sign}{body}");
    }
    for (i, &(scale, suffix)) in UNITS.iter().enumerate() {
        if a < scale {
            continue;
        }
        let n = a / scale;
        // Rounding can carry into the next unit (999.7K is 1000K): print that unit instead.
        if n.round() >= 1000.0 && i > 0 {
            return format!("{sign}1{}", UNITS[i - 1].1);
        }
        let digits = if n >= 10.0 {
            format!("{:.0}", n)
        } else {
            let one = format!("{:.1}", n);
            one.strip_suffix(".0").map(str::to_string).unwrap_or(one)
        };
        return format!("{sign}{digits}{suffix}");
    }
    format!("{v:.0}")
}

/// Whether a label `text_w` wide and `text_h` tall fits inside a disc of `radius`, all in the same
/// pixels, with a pixel of margin on each side.
pub fn label_fits(radius: f32, text_w: f32, text_h: f32) -> bool {
    let half_w = text_w * 0.5 + 1.0;
    let half_h = text_h * 0.5 + 1.0;
    half_w * half_w + half_h * half_h <= radius * radius
}

#[cfg(test)]
mod tests;
