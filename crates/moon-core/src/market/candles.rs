//! Chart candles: trade aggregation, base-history resampling, and a merged per-pane
//! series. The production caller supplies a sorted base already merged and resampled to
//! the target timeframe, while the trade ring supplies the local overlay and live edge.
//! Pure functions plus a revisioned series let the renderer re-upload the GPU buffer only
//! when the revision changes.
//!
//! Seam rule: when the base covers the first local bucket's timestamp, that base candle
//! is retained and the local overlay begins at the next bucket. Otherwise the overlay
//! begins at the first local bucket. This rule tests timestamp coverage without inferring
//! how much of the bucket interval the local trades represent.

use serde::{Deserialize, Serialize};

use crate::feed::Tick;

/// Supported candle timeframes in minutes. The 30-second timeframe (code 0) was REMOVED
/// from the set at the user's request (2026-07-12): it relied only on trades and had no
/// deep-history base. A 5-minute snapshot can contribute only when the target timeframe
/// is at least 5 minutes and divisible by 5; the 1-minute timeframe has no such fallback.
pub const CANDLE_TF_CHOICES_MIN: [u32; 6] = [1, 5, 30, 60, 240, 1440];

/// Candle rendering modes used by `CandleViewCfg::mode`.
pub const CANDLE_MODE_FILLED: u8 = 0;
pub const CANDLE_MODE_OUTLINE: u8 = 1;
pub const CANDLE_MODE_OUTLINE_IN_ZONE: u8 = 2;
/// Disables candles completely, leaving a pure tick chart across the full window.
pub const CANDLE_MODE_OFF: u8 = 3;

/// Candle/trade chart display settings controlled by the candle button in the tab strip.
/// `layout.toml` stores the global default as `WindowLayout::candle_view`, while
/// `charts.json` stores optional per-tab overrides.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CandleViewCfg {
    /// Candle timeframe in minutes, selected from [`CANDLE_TF_CHOICES_MIN`].
    pub tf_min: u32,
    /// Mode: 0 = filled, 1 = outlines, 2 = outlines in the trade zone, 3 = off.
    pub mode: u8,
    /// Number of MOST RECENT candles redrawn with trades in the trade zone; crosses are
    /// drawn only inside those buckets. A value of 0 disables trades entirely.
    pub trade_candles: u16,
    /// Number of MOST RECENT candles not drawn at all, leaving only trades in those
    /// buckets. A value of 0 shows every candle. Usually no greater than `trade_candles`.
    pub hide_candles: u16,
    /// Hard cap on displayed trades to protect against bursts.
    pub trades_limit: u32,
    /// Candle outline width in logical pixels.
    pub outline_px: f32,
    /// Whether to draw candle shadows (wicks) in the trade zone.
    pub wicks_in_zone: bool,
    /// Whether to use a neutral candle color in the trade zone to avoid competing with
    /// the cross colors.
    pub neutral_in_zone: bool,
    /// Whether to draw last/mark price lines: orange LastPrice and blue MarkPrice.
    pub price_lines: bool,
}

impl Default for CandleViewCfg {
    fn default() -> Self {
        Self {
            tf_min: 5,
            mode: CANDLE_MODE_OUTLINE_IN_ZONE,
            trade_candles: 3,
            hide_candles: 0,
            trades_limit: 50_000,
            outline_px: 1.0,
            wicks_in_zone: true,
            neutral_in_zone: false,
            price_lines: true,
        }
    }
}

impl CandleViewCfg {
    /// Returns the timeframe in milliseconds, clamped to the supported set.
    ///
    /// Legacy 30-second code 0 maps to 1 minute because sub-minute settings were removed.
    pub fn tf_ms(&self) -> i64 {
        let tf = if self.tf_min == 0 {
            1
        } else if CANDLE_TF_CHOICES_MIN.contains(&self.tf_min) {
            self.tf_min
        } else {
            5
        };
        tf as i64 * 60_000
    }
}

/// One chart candle whose timestamp is the bucket's Unix opening time in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChartCandle {
    pub t_open_ms: f64,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    /// Total trade volume in the bucket, denominated in the base currency.
    pub volume: f32,
}

/// Returns the timeframe bucket start for a timestamp, floored on the Unix-epoch grid.
pub fn bucket_open_ms(time_ms: f64, tf_ms: i64) -> f64 {
    let tf = tf_ms.max(1) as f64;
    (time_ms / tf).floor() * tf
}

/// Returns the native CoinCard-history timeframe in minutes for a series timeframe.
///
/// Exact history exists for 1/5/30/60/240/1440 minutes. Sub-minute timeframes have no
/// base and use only trades; every other timeframe is resampled from 5-minute history.
pub fn deep_kind_min_for_tf(tf_min: u32) -> u32 {
    match tf_min {
        1 => 1,
        30 => 30,
        60 => 60,
        240 => 240,
        1440 => 1440,
        _ => 5,
    }
}

/// Orients range-only candles whose real open and close values are unavailable.
///
/// Measurements show that the core's bulk 5-minute snapshot carries ONLY high/low,
/// encoded as `open == high` and `close == low`. Without real open/close values, bodies
/// would always appear bearish and wickless. Until CoinCard history with real OHLC
/// arrives, orient these rows relative to the previous midpoint: rows whose midpoint is
/// non-decreasing, including a tie, become `open = low, close = high`, while decreasing
/// rows remain unchanged.
pub fn orient_range_rows(rows: &mut [ChartCandle]) {
    let mut prev_mid: Option<f32> = None;
    for c in rows.iter_mut() {
        let mid = (c.high + c.low) * 0.5;
        if c.open == c.high && c.close == c.low {
            if let Some(pm) = prev_mid {
                if mid >= pm {
                    c.open = c.low;
                    c.close = c.high;
                }
            }
        }
        prev_mid = Some(mid);
    }
}

/// Normalizes server-candle OHLC values with a potentially swapped wire order.
///
/// Detects `(high, low, open, close)` stored in `(open, close, high, low)` fields by the
/// valid-candle invariant `h ≥ max(o,c) && l ≤ min(o,c)`, and swaps ONLY rows that violate
/// it. Correct CoinCard-history and sealed live rows pass through unchanged.
pub fn normalize_ohlc(o: f32, h: f32, l: f32, c: f32) -> (f32, f32, f32, f32) {
    if h >= o.max(c) && l <= o.min(c) {
        return (o, h, l, c); // The candle is already valid.
    }
    if o >= h.max(l) && c <= h.min(l) {
        // The (o,c,h,l) fields contain (high,low,open,close): real o=h, h=o, l=c, c=l.
        return (h, o, c, l);
    }
    // For unrecognized garbage, span the range across all four values and preserve o/c.
    let hi = o.max(c).max(h).max(l);
    let lo = o.min(c).min(h).min(l);
    (o, hi, lo, c)
}

/// Aggregates trades into candles of any timeframe.
///
/// Trades are nearly time-sorted. Late UDP resend rows may enter an OLD bucket, whose
/// high, low, and volume are updated without refining time-based open/close values: the
/// difference is visually negligible and the ring does not preserve exact ordering.
/// Empty buckets are omitted, producing a sparse series like the trade stream itself.
pub fn aggregate_trades(trades: &[Tick], tf_ms: i64, out: &mut Vec<ChartCandle>) {
    out.clear();
    for t in trades {
        if !(t.price.is_finite() && t.price > 0.0) {
            continue;
        }
        let open_ms = bucket_open_ms(t.time_ms, tf_ms);
        match out.last_mut() {
            Some(last) if last.t_open_ms == open_ms => {
                last.high = last.high.max(t.price);
                last.low = last.low.min(t.price);
                last.close = t.price;
                last.volume += t.qty.max(0.0);
            }
            Some(last) if open_ms > last.t_open_ms => {
                out.push(candle_from_tick(open_ms, t));
            }
            None => out.push(candle_from_tick(open_ms, t)),
            _ => {
                // Search backward for a late resend's old bucket, which is usually nearby.
                if let Some(c) = out.iter_mut().rev().find(|c| c.t_open_ms == open_ms) {
                    c.high = c.high.max(t.price);
                    c.low = c.low.min(t.price);
                    c.volume += t.qty.max(0.0);
                }
                // Ignore a trade older than the entire series because its window has moved on.
            }
        }
    }
}

fn candle_from_tick(open_ms: f64, t: &Tick) -> ChartCandle {
    ChartCandle {
        t_open_ms: open_ms,
        open: t.price,
        high: t.price,
        low: t.price,
        close: t.price,
        volume: t.qty.max(0.0),
    }
}

/// Resamples time-sorted candles into a larger timeframe, such as 5 to 15 minutes.
///
/// Divisibility is not enforced; a non-multiple timeframe simply uses its floored grid.
pub fn resample(rows: &[ChartCandle], tf_ms: i64, out: &mut Vec<ChartCandle>) {
    out.clear();
    for r in rows {
        let open_ms = bucket_open_ms(r.t_open_ms, tf_ms);
        match out.last_mut() {
            Some(last) if last.t_open_ms == open_ms => {
                last.high = last.high.max(r.high);
                last.low = last.low.min(r.low);
                last.close = r.close;
                last.volume += r.volume;
            }
            _ => out.push(ChartCandle {
                t_open_ms: open_ms,
                ..*r
            }),
        }
    }
}

/// Merged per-pane candle series combining base history with a local trade-derived tail.
///
/// It lives in `ChartHistoryCursor`, rebuilds on a combo reset, and updates its live edge
/// through `push_trades` from the same drain that feeds the crosses.
#[derive(Default)]
pub struct CandleSeries {
    tf_ms: i64,
    candles: Vec<ChartCandle>,
    revision: u64,
    valid: bool,
    /// Resampling scratch space whose allocation is reused between rebuilds.
    scratch: Vec<ChartCandle>,
}

impl CandleSeries {
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    pub fn tf_ms(&self) -> i64 {
        self.tf_ms
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn candles(&self) -> &[ChartCandle] {
        &self.candles
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
        self.candles.clear();
    }

    /// Rebuilds the series from sorted base candles and nearly sorted trades.
    ///
    /// `base` contains candles sorted at `base_tf_ms`; the production caller currently
    /// supplies a merged base already resampled to the target timeframe. `trades` contains
    /// the visible window's trades. If the base covers the first local bucket's timestamp,
    /// the local overlay begins at the next bucket; otherwise it begins at that first
    /// local bucket.
    pub fn rebuild(&mut self, tf_ms: i64, base: &[ChartCandle], base_tf_ms: i64, trades: &[Tick]) {
        let tf_ms = tf_ms.max(1);
        self.tf_ms = tf_ms;
        self.candles.clear();

        // Build the local trade tail in a temporary buffer so scratch survives clear.
        let mut local = std::mem::take(&mut self.scratch);
        aggregate_trades(trades, tf_ms, &mut local);

        // The declared base timeframe must divide the series timeframe for valid resampling.
        if base_tf_ms > 0 && tf_ms >= base_tf_ms && tf_ms % base_tf_ms == 0 && !base.is_empty() {
            if tf_ms == base_tf_ms {
                self.candles.extend_from_slice(base);
            } else {
                let mut resampled = Vec::new();
                resample(base, tf_ms, &mut resampled);
                self.candles = resampled;
            }
        }

        // Choose the seam by timestamp coverage. If the base contains the first local
        // bucket, retain that base candle and begin the overlay at the next bucket.
        // Without base coverage, retain the local series from its first bucket.
        let overlay_from = match local.first() {
            None => f64::INFINITY,
            Some(first) if self.candles.is_empty() => first.t_open_ms,
            Some(first) => {
                // Base coverage keeps the first local timestamp on the base side of the
                // seam; without coverage, include that local bucket to avoid a gap.
                let covered = self.candles.iter().any(|c| c.t_open_ms == first.t_open_ms);
                if covered {
                    first.t_open_ms + tf_ms as f64
                } else {
                    first.t_open_ms
                }
            }
        };
        self.candles.retain(|c| c.t_open_ms < overlay_from);
        self.candles
            .extend(local.iter().filter(|c| c.t_open_ms >= overlay_from));

        local.clear();
        self.scratch = local;
        self.valid = true;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Applies new trades from the same drain that feeds crosses to the live edge.
    ///
    /// Updates the last candle or opens a new one across a bucket boundary. Returns `true`
    /// when the series changes.
    pub fn push_trades(&mut self, trades: &[Tick]) -> bool {
        if !self.valid || trades.is_empty() {
            return false;
        }
        let tf_ms = self.tf_ms.max(1);
        let mut changed = false;
        for t in trades {
            if !(t.price.is_finite() && t.price > 0.0) {
                continue;
            }
            let open_ms = bucket_open_ms(t.time_ms, tf_ms);
            match self.candles.last_mut() {
                Some(last) if last.t_open_ms == open_ms => {
                    last.high = last.high.max(t.price);
                    last.low = last.low.min(t.price);
                    last.close = t.price;
                    last.volume += t.qty.max(0.0);
                    changed = true;
                }
                Some(last) if open_ms > last.t_open_ms => {
                    self.candles.push(candle_from_tick(open_ms, t));
                    changed = true;
                }
                None => {
                    self.candles.push(candle_from_tick(open_ms, t));
                    changed = true;
                }
                _ => {
                    // Update high, low, and volume for a late resend into a recent old bucket.
                    if let Some(c) = self
                        .candles
                        .iter_mut()
                        .rev()
                        .take(4)
                        .find(|c| c.t_open_ms == open_ms)
                    {
                        c.high = c.high.max(t.price);
                        c.low = c.low.min(t.price);
                        c.volume += t.qty.max(0.0);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    /// Returns the low-to-high range of candles intersecting a time window for chart auto-Y.
    pub fn price_range(&self, from_ms: f64, to_ms: f64) -> Option<(f32, f32)> {
        let tf = self.tf_ms.max(1) as f64;
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for c in &self.candles {
            if c.t_open_ms + tf <= from_ms || c.t_open_ms > to_ms {
                continue;
            }
            lo = lo.min(c.low);
            hi = hi.max(c.high);
        }
        (lo <= hi).then_some((lo, hi))
    }
}

#[cfg(test)]
mod tests;
