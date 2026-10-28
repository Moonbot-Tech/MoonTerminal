//! Backend-free decisions for the trade bubbles: which trades make one print, how big a print's
//! disc is, and which discs merge at the current zoom.
//!
//! A bubble is a PRINT, as in a scalping terminal's tape: trades that follow each other at one
//! price on one side within a short window (`trade_merge_ms`) add up into one disc at the first
//! trade's time and its own price. Its size is measured against this market's OWN prints, never
//! in the quote currency: the percentiles of the print sizes in the summed tape set the scale, so
//! a BTC pair, a stablecoin, a coin-margined or a quarterly contract each read on their own scale.
//! A print under the small percentile is a dot with no label. Zoomed out, discs of one price that
//! cover each other merge into one, their volume summed and the disc grown by the same scale;
//! discs of different prices may overlap, which is what keeps the tape dense.

/// Merge windows the reader can pick, milliseconds. `0` still joins the fills an exchange reports
/// under one timestamp.
pub const MERGE_WINDOWS_MS: [u16; 6] = [0, 50, 100, 250, 500, 1000];

/// The merge window an absent setting reads as.
pub const DEFAULT_MERGE_MS: u16 = 100;

/// Small-print thresholds the reader can pick, percent of this market's prints. `0` sizes every
/// print.
pub const SMALL_PCTS: [u8; 5] = [0, 25, 50, 75, 90];

/// The small-print threshold an absent setting reads as: the smaller half of the prints are dots.
pub const DEFAULT_SMALL_PCT: u8 = 50;

use std::collections::HashMap;

/// The percentile of this market's prints that gets the full disc; bigger prints stay at it.
const BIG_PERCENTILE: f32 = 99.0;

/// How far a re-measured scale may drift and still keep the old one, as a share of it: the live
/// edge re-folds every few hundred milliseconds, and a scale that followed every tick would make
/// every disc on screen breathe.
const SCALE_HOLD: f32 = 0.1;

/// A small print's dot, logical pixels.
pub const DOT_RADIUS_PX: f32 = 2.0;

/// The smallest sized disc — a print right at the small threshold — logical pixels.
pub const MIN_RADIUS_PX: f32 = 3.5;

/// The full disc, logical pixels: big enough to hold a four-glyph volume (`2.3M`) at the default
/// label size.
pub const MAX_RADIUS_PX: f32 = 16.0;

/// Snap a stored merge window onto [`MERGE_WINDOWS_MS`], so a hand-edited value lights one segment.
pub fn snap_merge_ms(ms: u16) -> u16 {
    MERGE_WINDOWS_MS
        .iter()
        .copied()
        .min_by_key(|w| w.abs_diff(ms))
        .unwrap_or(DEFAULT_MERGE_MS)
}

/// Snap a stored small-print threshold onto [`SMALL_PCTS`].
pub fn snap_small_pct(pct: u8) -> u8 {
    SMALL_PCTS
        .iter()
        .copied()
        .min_by_key(|p| p.abs_diff(pct))
        .unwrap_or(DEFAULT_SMALL_PCT)
}

/// One print: trades of one price and one side within the merge window, in quote notional.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Print {
    /// The first trade's absolute time, milliseconds.
    pub time_ms: f64,
    /// The trades' price.
    pub price: f32,
    /// The price's identity for grouping, see [`price_key`].
    pub key: i64,
    pub buy: f32,
    pub sell: f32,
}

impl Print {
    /// Everything the print traded.
    pub fn total(&self) -> f32 {
        self.buy + self.sell
    }
}

/// A price's identity for grouping prints: its whole number of ticks when the tick is known, so
/// an `f32` rounding never splits one level in two; its exact bits otherwise.
pub fn price_key(price: f32, tick: Option<f64>) -> Option<i64> {
    if !(price.is_finite() && price > 0.0) {
        return None;
    }
    match tick.filter(|t| t.is_finite() && *t > 0.0) {
        Some(tick) => {
            let ticks = (f64::from(price) / tick).round();
            (ticks.is_finite() && ticks.abs() < 9.0e15).then_some(ticks as i64)
        }
        None => Some(i64::from(price.to_bits())),
    }
}

/// Fold a tape into prints.
///
/// A trade joins the open print of its price and side even when trades at other prices came in
/// between: a tape bouncing between the bid and the ask still folds within the window.
///
/// Args:
///     trades: `(absolute time ms, price, quantity, is_buy)` in tape order.
///     tick: The market's tick, `None` while unknown.
///     window_ms: How far after a print's first trade, inclusive, a trade of its price and side
///         still joins it.
///     out: Cleared, then filled with the prints in tape order of their first trade.
pub fn aggregate_prints(
    trades: impl IntoIterator<Item = (f64, f32, f32, bool)>,
    tick: Option<f64>,
    window_ms: f64,
    out: &mut Vec<Print>,
) {
    out.clear();
    let mut open: HashMap<i64, [Option<usize>; 2]> = HashMap::new();
    for (time_ms, price, qty, is_buy) in trades {
        let notional = price * qty;
        if !time_ms.is_finite() || !(notional > 0.0) || !notional.is_finite() {
            continue;
        }
        let Some(key) = price_key(price, tick) else {
            continue;
        };
        // Both sides share a price lookup; replacing an expired slot does not hash it again.
        let slot = &mut open.entry(key).or_insert([None; 2])[usize::from(is_buy)];
        if let Some(i) = *slot {
            let print = &mut out[i];
            if time_ms >= print.time_ms && time_ms - print.time_ms <= window_ms {
                if is_buy {
                    print.buy += notional;
                } else {
                    print.sell += notional;
                }
                continue;
            }
        }
        *slot = Some(out.len());
        out.push(Print {
            time_ms,
            price,
            key,
            buy: if is_buy { notional } else { 0.0 },
            sell: if is_buy { 0.0 } else { notional },
        });
    }
}

/// Order prints by price, then time, caching keys so comparisons do not chase scattered prints.
/// Equal keys retain tape order, including signed zero and NaN times under `total_cmp` ordering.
pub fn price_time_order(prints: &[Print], order: &mut Vec<u32>) {
    order.clear();
    order.extend(0..prints.len() as u32);
    order.sort_by_cached_key(|&i| {
        let p = &prints[i as usize];
        let bits = p.time_ms.to_bits();
        let time = bits ^ (((bits as i64 >> 63) as u64) | (1 << 63));
        // Pack the signed-price rank and total-time rank into one scalar comparison key.
        (u128::from(p.key as u64 ^ (1 << 63)) << 64) | u128::from(time)
    });
}

/// The scale a market's prints are sized on: the volume of the smallest sized print and of the
/// full disc, both percentiles of the prints themselves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintScale {
    pub small: f32,
    pub big: f32,
}

impl PrintScale {
    /// Whether `fresh` is within [`SCALE_HOLD`] of this scale on both ends, so this one can stay.
    pub fn holds_against(&self, fresh: &PrintScale) -> bool {
        let near = |a: f32, b: f32| a > 0.0 && ((b / a) - 1.0).abs() <= SCALE_HOLD;
        near(self.small, fresh.small) && near(self.big, fresh.big)
    }
}

/// Measure the scale with two rank selections instead of sorting every print volume.
/// The scratch buffer has unspecified order; the selected percentile values match a full sort.
///
/// Returns:
///     `None` for a tape without a usable print.
pub fn print_scale(prints: &[Print], small_pct: u8, scratch: &mut Vec<f32>) -> Option<PrintScale> {
    scratch.clear();
    scratch.extend(
        prints
            .iter()
            .map(Print::total)
            .filter(|v| v.is_finite() && *v > 0.0),
    );
    if scratch.is_empty() {
        return None;
    }
    let index = |pct: f32| {
        let i = ((pct / 100.0) * (scratch.len() - 1) as f32).round() as usize;
        i.min(scratch.len() - 1)
    };
    let small_index = index(f32::from(small_pct.min(100)));
    let big_index = index(BIG_PERCENTILE);
    let low = small_index.min(big_index);
    let high = small_index.max(big_index);
    let (below, selected, _) = scratch.select_nth_unstable_by(high, f32::total_cmp);
    let high_value = *selected;
    let low_value = if low == high {
        high_value
    } else {
        *below.select_nth_unstable_by(low, f32::total_cmp).1
    };
    let (small, big) = if small_index <= big_index {
        (low_value, high_value)
    } else {
        (high_value, low_value)
    };
    Some(PrintScale {
        small,
        big: big.max(small),
    })
}

/// The radii a pane draws prints with, physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiscRadii {
    pub dot: f32,
    pub min: f32,
    pub max: f32,
}

impl DiscRadii {
    /// The logical radii at `ppp` physical pixels per logical one.
    pub fn at(ppp: f32) -> Self {
        Self {
            dot: DOT_RADIUS_PX * ppp,
            min: MIN_RADIUS_PX * ppp,
            max: MAX_RADIUS_PX * ppp,
        }
    }
}

/// A volume's disc on a market's scale: a dot below the small print, otherwise growing with the
/// LOGARITHM of the volume from the small print to the full disc at the big one — prints spread
/// over orders of magnitude, so a linear scale would leave all but the giants as specks.
///
/// Returns:
///     The radius and whether the volume is a small print (a dot with no label).
pub fn print_radius(total: f32, scale: PrintScale, r: DiscRadii) -> (f32, bool) {
    if !(total >= scale.small) {
        return (r.dot, true);
    }
    let span = (scale.big / scale.small).ln();
    // The small and the big percentile coincide — a fixed-lot coin, or a single print: that one
    // size is the ordinary print, and only what trades above it stands out.
    if !(span.is_finite() && span > 0.0) {
        return if total > scale.big {
            (r.max, false)
        } else {
            (r.min, false)
        };
    }
    let t = ((total / scale.small).ln() / span).clamp(0.0, 1.0);
    (r.min + (r.max - r.min) * t, false)
}

/// One disc as drawn: a print, or prints of one price merged because they covered each other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintDisc {
    /// The disc's absolute time, milliseconds: its prints' times weighted by their volume.
    pub time_ms: f64,
    pub price: f32,
    pub key: i64,
    pub buy: f32,
    pub sell: f32,
    pub radius: f32,
    /// Below the small print: a dot with no label.
    pub small: bool,
    /// The first print's time, milliseconds — stable while the disc absorbs later prints, so it
    /// keys the disc's label slot.
    pub first_ms: f64,
}

impl PrintDisc {
    /// Everything the disc traded.
    pub fn total(&self) -> f32 {
        self.buy + self.sell
    }
}

/// Build the discs of one zoom: prints of one price whose discs would cover each other merge,
/// with no cap on how many.
///
/// Args:
///     prints: The tape's prints.
///     order: `prints` by price, then time ([`price_time_order`]).
///     px_per_ms: Physical pixels per millisecond of the plot.
///     size: A volume's radius and small flag ([`print_radius`] on the market's scale).
///     out: Cleared, then filled with the discs, by price, then time.
pub fn merge_overlapping(
    prints: &[Print],
    order: &[u32],
    px_per_ms: f64,
    size: impl Fn(f32) -> (f32, bool),
    out: &mut Vec<PrintDisc>,
) {
    out.clear();
    for &i in order {
        let Some(p) = prints.get(i as usize) else {
            continue;
        };
        let (radius, small) = size(p.total());
        let mut disc = PrintDisc {
            time_ms: p.time_ms,
            price: p.price,
            key: p.key,
            buy: p.buy,
            sell: p.sell,
            radius,
            small,
            first_ms: p.time_ms,
        };
        // A merge moves the disc's centre back and grows it, so it can reach the one before too.
        loop {
            let covers = out.last().is_some_and(|last| {
                last.key == disc.key
                    && (disc.time_ms - last.time_ms).abs() * px_per_ms
                        < f64::from(last.radius + disc.radius)
            });
            if !covers {
                break;
            }
            let last = out.pop().expect("checked above");
            let (wa, wb) = (f64::from(last.total()), f64::from(disc.total()));
            let buy = last.buy + disc.buy;
            let sell = last.sell + disc.sell;
            let time_ms = if wa + wb > 0.0 {
                (last.time_ms * wa + disc.time_ms * wb) / (wa + wb)
            } else {
                last.time_ms
            };
            let (radius, small) = size(buy + sell);
            disc = PrintDisc {
                time_ms,
                price: last.price,
                key: last.key,
                buy,
                sell,
                radius,
                small,
                first_ms: last.first_ms,
            };
        }
        out.push(disc);
    }
}

/// A volume printed short enough to sit inside its disc: at most three digits and an SI suffix —
/// one decimal below ten of a unit (`2.3M`), whole above it (`11K`, `906`) — where the chart's
/// usual format (`10.8K`) costs the glyph that decides whether the label fits. Below one it keeps
/// a decimal, and a volume that rounds to nothing prints `<0.1` rather than a `0.0` that reads as
/// an empty disc.
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
