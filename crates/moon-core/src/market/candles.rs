//! Chart candles: trade aggregation, base-history resampling, and a merged per-pane
//! series. The production caller supplies a sorted base already merged and resampled to
//! the target timeframe, while the trade ring supplies the local overlay and live edge.
//! Pure functions plus a revisioned series let the renderer re-upload the GPU buffer only
//! when the revision changes.
//!
//! Overlay rule: the two sources are merged PER BUCKET. A trade-derived candle replaces the
//! base candle in the bucket it covers; a bucket with no trades keeps its base candle. There
//! is deliberately no seam — cutting the base off at the first traded bucket assumed trades
//! cover every bucket after it, which holds on a liquid market and fails badly on a thin one,
//! where it left the series with only the few buckets that happened to contain a trade.
//!
//! The first local bucket is the one exception: the read window starts mid-bucket, so its
//! trades are partial, and where the base covers that bucket the base candle is preferred.

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

/// Bottom candle-volume styles used by `crate::config::ChartGraphicsCfg::candle_volume_style`.
///
/// A `u8` rather than an enum for the same reason [`CANDLE_MODE_FILLED`] is one: the value travels
/// into a shader uniform as a float and is clamped at the edge, so an open integer keeps the whole
/// path — `layout.toml`, `charts.json`, the chart popup's style row, `VolumeStyleGpu.m.x` — free of
/// per-representation conversions. Both files are hand-editable; a bare enum string in them would be
/// a new value shape to parse.
pub const VOLUME_STYLE_OFF: u8 = 0;
/// Thin per-candle bars, one bar per bucket.
pub const VOLUME_STYLE_BARS: u8 = 1;
/// Moonbot-style "hills": a filled area whose top edge joins neighbouring buckets.
pub const VOLUME_STYLE_HILLS: u8 = 2;
/// Highest valid style id, for clamping a hand-edited chart configuration.
pub const VOLUME_STYLE_MAX: u8 = VOLUME_STYLE_HILLS;

/// Candle/trade chart display settings controlled by the candle button in the tab strip.
/// `layout.toml` stores the global default as `WindowLayout::candle_view`, while
/// `charts.json` stores optional per-tab overrides.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
// Deserialized through [`CandleViewWire`], which migrates the pre-split `price_lines` flag and
// supplies the per-field defaults `#[serde(default)]` used to give this struct directly.
#[serde(from = "CandleViewWire")]
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
    /// Whether to draw the orange LastPrice line.
    pub last_price_line: bool,
    /// Whether to draw the blue MarkPrice line. A market whose provider reports no mark price
    /// draws nothing regardless.
    pub mark_price_line: bool,
    /// Whether a MoonShot order fills its corridor between `corridor_price_down` and
    /// `corridor_price_up`. This is the ORDER's own area, unrelated to the layout popup's
    /// `show_zone`, which shades the trading control strip.
    pub moonshot_zone: bool,
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
            last_price_line: true,
            mark_price_line: true,
            moonshot_zone: true,
        }
    }
}

/// Deserialization form of [`CandleViewCfg`], carrying the pre-split `price_lines` flag so a
/// `layout.toml` or `charts.json` written before the split still loads with the user's choice.
///
/// It exists because a plain `#[serde(default)]` cannot express "default to ANOTHER field": a
/// config that only says `price_lines = false` would silently come back with both lines ON, which
/// is the setting the user explicitly turned off. Every field is optional so a file missing any key
/// still loads; [`From`] resolves each against [`CandleViewCfg::default`], which keeps the defaults
/// named once instead of once per struct.
#[derive(Default, Deserialize)]
#[serde(default)]
struct CandleViewWire {
    tf_min: Option<u32>,
    mode: Option<u8>,
    trade_candles: Option<u16>,
    hide_candles: Option<u16>,
    trades_limit: Option<u32>,
    outline_px: Option<f32>,
    wicks_in_zone: Option<bool>,
    neutral_in_zone: Option<bool>,
    /// Pre-split toggle that drove BOTH price lines at once. Read only where the split flag for
    /// that line is absent, so a file carrying both keeps the newer one.
    price_lines: Option<bool>,
    last_price_line: Option<bool>,
    mark_price_line: Option<bool>,
    moonshot_zone: Option<bool>,
}

impl From<CandleViewWire> for CandleViewCfg {
    fn from(w: CandleViewWire) -> Self {
        let d = CandleViewCfg::default();
        Self {
            tf_min: w.tf_min.unwrap_or(d.tf_min),
            mode: w.mode.unwrap_or(d.mode),
            trade_candles: w.trade_candles.unwrap_or(d.trade_candles),
            hide_candles: w.hide_candles.unwrap_or(d.hide_candles),
            trades_limit: w.trades_limit.unwrap_or(d.trades_limit),
            // A hand-edited `nan` compares unequal to itself, so it would report a change forever:
            // `set_candle_view` would mark the view dirty and rebuild order geometry every single
            // frame. The renderer's own `.max(1.0)` cannot save it, because NaN survives the
            // comparison it feeds. The pane's history gate is already safe — `history_inputs`
            // neutralizes this field — but nothing else is.
            outline_px: w
                .outline_px
                .filter(|px| px.is_finite())
                .unwrap_or(d.outline_px),
            wicks_in_zone: w.wicks_in_zone.unwrap_or(d.wicks_in_zone),
            neutral_in_zone: w.neutral_in_zone.unwrap_or(d.neutral_in_zone),
            last_price_line: w
                .last_price_line
                .or(w.price_lines)
                .unwrap_or(d.last_price_line),
            mark_price_line: w
                .mark_price_line
                .or(w.price_lines)
                .unwrap_or(d.mark_price_line),
            moonshot_zone: w.moonshot_zone.unwrap_or(d.moonshot_zone),
        }
    }
}

impl CandleViewCfg {
    /// Returns this config with every purely visual field neutralized, leaving only what the
    /// HISTORY read consumes. Compare two of these to decide whether a pane must reset.
    ///
    /// A reset is expensive — a window re-read, a candle-series rebuild, a combo re-upload and both
    /// price-line cursors re-seeded, per pane, multiplied by every tab and window when the candle
    /// popup's ⧉ distributes a setting. Six fields cannot change what is READ and so must not buy
    /// one: `outline_px`, `wicks_in_zone`, `neutral_in_zone` and `hide_candles` only reach the
    /// candle STYLE, which the renderer gates separately; `trades_limit` is not passed to the read
    /// protocol at all; `moonshot_zone` is order-line geometry.
    ///
    /// Neutralizing those by name rather than listing the survivors is deliberate: a field added
    /// later keeps forcing a reset until someone decides otherwise, which is the safe direction to
    /// be wrong in.
    pub fn history_inputs(self) -> Self {
        Self {
            hide_candles: 0,
            trades_limit: 0,
            outline_px: 0.0,
            wicks_in_zone: false,
            neutral_in_zone: false,
            moonshot_zone: false,
            ..self
        }
    }

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
    /// Turnover in the bucket, denominated in the quote currency.
    ///
    /// Carried as DATA rather than computed at render time, because `volume * price` uses the
    /// CURRENT price and is wrong for every historical bar. `0.0` means "nothing traded, or this
    /// source does not know". Where a source has no turnover of its own, the value is an
    /// estimate and is produced ONLY by [`estimate_quote_volume`].
    pub quote_volume: f32,
}

/// Estimates a bucket's quote-currency turnover from its OHLC and base volume, for sources that
/// report no turnover of their own.
///
/// Returns `volume * ((open + high + low + close) * 0.25)` (OHLC4, averaged before multiplying so
/// the result cannot overflow to infinity ahead of that averaging), or `0.0` when any input is
/// negative, any input is non-finite, or the computed result itself is not finite.
///
/// True turnover is `volume * vwap` with `vwap` somewhere in `[low, high]`. OHLC4 itself lies in
/// `[low + (high - low) / 4, high - (high - low) / 4]`, so the estimate's worst-case error is
/// `|err| <= 0.75 * (high - low) * volume` — a quarter of the `(high - low) * volume` bound a
/// `close`-based estimate would carry.
///
/// OHLC4 is also chosen for RANGE-ONLY rows, where `open == high` and `close == low` and
/// [`orient_range_rows`] may later swap them: OHLC4 collapses to `(high + low) / 2` on those rows
/// and is invariant under that swap, whereas `close` would pick a different extreme each time.
///
/// # Args
/// * `volume` - base-currency volume for the bucket; rejected if negative.
/// * `open`, `high`, `low`, `close` - the bucket's OHLC; each rejected if negative, since a real
///   price is never negative and `unpack_rows_v1` calls this on raw persisted bytes with no
///   upstream validation of its own.
///
/// # Returns
/// The estimated quote-currency turnover, or `0.0` when any input is invalid or the result would
/// not be finite.
pub fn estimate_quote_volume(volume: f32, open: f32, high: f32, low: f32, close: f32) -> f32 {
    if !(volume.is_finite()
        && open.is_finite()
        && high.is_finite()
        && low.is_finite()
        && close.is_finite())
        || volume < 0.0
        || open < 0.0
        || high < 0.0
        || low < 0.0
        || close < 0.0
    {
        return 0.0;
    }
    let estimate = volume * ((open + high + low + close) * 0.25);
    if estimate.is_finite() { estimate } else { 0.0 }
}

/// Returns the timeframe bucket start for a timestamp, floored on the Unix-epoch grid.
pub fn bucket_open_ms(time_ms: f64, tf_ms: i64) -> f64 {
    let tf = tf_ms.max(1) as f64;
    (time_ms / tf).floor() * tf
}

/// Returns the first bucket that a source starting at `oldest_ms` covers in FULL.
///
/// A bucket the source only partly covers is not the same fact as an empty one, and the kline
/// cache cannot tell them apart afterwards: its merge is last-writer-wins per timestamp, so a
/// half-covered bucket silently REPLACES a complete row an earlier session stored. Callers use
/// this to drop the leading partial bucket rather than publishing it.
///
/// An `oldest_ms` already sitting exactly on a boundary starts a complete bucket and is returned
/// unchanged; anything inside a bucket rounds up to the next one.
pub fn first_full_bucket_ms(oldest_ms: f64, tf_ms: i64) -> i64 {
    let tf = tf_ms.max(1);
    let oldest = oldest_ms as i64;
    let floor = oldest.div_euclid(tf) * tf;
    if floor == oldest { floor } else { floor + tf }
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
                last.quote_volume += t.price * t.qty.max(0.0);
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
                    c.quote_volume += t.price * t.qty.max(0.0);
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
        quote_volume: t.price * t.qty.max(0.0),
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
                last.quote_volume += r.quote_volume;
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
pub struct CandleSeries {
    tf_ms: i64,
    candles: Vec<ChartCandle>,
    revision: u64,
    valid: bool,
    /// Resampling scratch space whose allocation is reused between rebuilds.
    scratch: Vec<ChartCandle>,
    /// Merge scratch space, reused for the same reason: a rebuild runs on the chart's prepare
    /// path and happens on every frame of a pan.
    merge_scratch: Vec<ChartCandle>,
    /// Oldest bucket this series accumulates from trades itself.
    ///
    /// Buckets at or after it are trade-derived and may be added to; earlier ones still hold a
    /// base candle whose volume and turnover are already complete. `INFINITY` while nothing is
    /// trade-derived.
    live_from: f64,
}

impl Default for CandleSeries {
    fn default() -> Self {
        Self {
            tf_ms: 0,
            candles: Vec::new(),
            revision: 0,
            valid: false,
            scratch: Vec::new(),
            merge_scratch: Vec::new(),
            // Nothing is trade-derived yet, so every bucket is "base" until a rebuild says so.
            live_from: f64::INFINITY,
        }
    }
}

/// One cache-only coarser layer offered to [`compose_with_coarse`], finest first.
///
/// `rows` must be sorted by `t_open_ms` and must already be aggregated at `tf_ms`; the layer is
/// drawn at its own width rather than resampled into the series timeframe, because a coarse bucket
/// cannot be split into finer ones without inventing the shape inside it.
pub struct CoarseLayer<'a> {
    pub rows: &'a [ChartCandle],
    pub tf_ms: f64,
}

/// Builds the render list: the series itself plus coarser fillers for the stretches it does not
/// cover, each entry tagged with the timeframe it is drawn at.
///
/// The chart's fine layer is never continuous. A CoinCard reply carries only about 500 bars and
/// `request_coin_card` takes no time range, so across sessions the local cache accumulates DISJOINT
/// blocks with unfetchable holes between them. This is the one place those holes are answered, from
/// coarser rows that are already on disk.
///
/// It generalises what used to be a left-edge PREFIX. Filling only left of the series threw away
/// every coarse row that happened to land inside a hole — measured on a live cache, 44 of the 67
/// five-minute buckets covering one 331-minute hole.
///
/// Rules, in order:
///
/// 1. A hole is the open stretch before the first candle plus every stretch between consecutive
///    candles. There is deliberately NO hole after the last candle: the live edge belongs to the
///    trade tail, which extends the series itself.
/// 2. A layer is offered a hole only when the hole is at least one of that layer's timeframes wide,
///    so a daily candle is never dropped into a five-hour gap it would misrepresent. The bound is
///    inclusive: a hole exactly one coarse period wide is answered by exactly one aligned row, and
///    rejecting it would leave the one case the layer fits perfectly.
/// 3. A row is taken when its bucket overlaps the hole at all, so a filler may hang over each seam
///    by up to one of its own timeframes. That overlap is intentional and invisible: fillers render
///    muted and beneath the finer candles. Demanding containment instead left a gap as wide as the
///    coarse timeframe at every seam.
/// 4. Each layer's coverage is subtracted before the next, coarser one runs, so the daily layer only
///    ever reaches what the five-minute layer could not — including the five-minute layer's own
///    internal holes.
/// 5. The result is ascending by `t_open_ms`, and a filler sharing a timestamp with a series candle
///    is emitted just before it rather than dropped — see the merge for why dropping it would leave
///    a hole this function had already counted as filled. Order is load-bearing: the gap
///    diagnostic, the volume band's visible-range statistics and hit-testing all walk this array in
///    sequence.
///
/// Every candle, series or filler, is copied whole into the output, so `quote_volume` travels
/// with it automatically and needs no field-by-field handling here.
pub fn compose_with_coarse(
    series: &[ChartCandle],
    series_tf_ms: f64,
    layers: &[CoarseLayer<'_>],
    out: &mut Vec<(ChartCandle, f32)>,
) {
    out.clear();
    // Holes are half-open `[start, end)` and always ascending. An empty series is ONE unbounded
    // hole, which reproduces the old behaviour of taking every coarse row when nothing else exists.
    let mut holes: Vec<(f64, f64)> = Vec::new();
    match series.first() {
        None => holes.push((f64::NEG_INFINITY, f64::INFINITY)),
        Some(first) => {
            holes.push((f64::NEG_INFINITY, first.t_open_ms));
            for w in series.windows(2) {
                let start = w[0].t_open_ms + series_tf_ms;
                let end = w[1].t_open_ms;
                if end > start {
                    holes.push((start, end));
                }
            }
        }
    }

    let mut fillers: Vec<(ChartCandle, f32)> = Vec::new();
    let mut covered: Vec<(f64, f64)> = Vec::new();
    for layer in layers {
        if holes.is_empty() || layer.rows.is_empty() || !(layer.tf_ms > 0.0) {
            continue;
        }
        covered.clear();
        // ROWS outer, holes inner, so a row is taken AT MOST ONCE per layer. Scanning per hole
        // instead would take a row twice whenever its bucket straddles two holes — which is not an
        // exotic case but the ordinary one this feature exists for: a single isolated candle
        // between two large disjoint blocks leaves a gap narrower than the coarse timeframe on
        // either side of it. The duplicate survived into the render list, where it drew the same
        // candle twice and made the volume band count that bucket twice.
        for c in layer.rows.iter() {
            let fills_a_hole = holes.iter().any(|&(start, end)| {
                end - start >= layer.tf_ms && c.t_open_ms + layer.tf_ms > start && c.t_open_ms < end
            });
            if fills_a_hole {
                fillers.push((*c, layer.tf_ms as f32));
                covered.push((c.t_open_ms, c.t_open_ms + layer.tf_ms));
            }
        }
        if covered.is_empty() {
            continue;
        }
        // Ascending by construction when the layer is sorted as documented, but sorted anyway: that
        // precondition is a comment, not something the type system holds anyone to, and
        // `subtract_covered` walks this in order.
        covered.sort_by(|a, b| a.0.total_cmp(&b.0));
        holes = subtract_covered(&holes, &covered);
    }

    out.reserve(series.len() + fillers.len());
    fillers.sort_by(|a, b| a.0.t_open_ms.total_cmp(&b.0.t_open_ms));
    let series_tf = series_tf_ms as f32;
    let mut si = 0usize;
    for (filler, tf) in fillers.drain(..) {
        // Strictly less, so a filler sharing an opening timestamp with a series candle is emitted
        // BEFORE it and both survive. Dropping the filler on that tie would be wrong twice over:
        // its interval was already subtracted from the hole set, so the minutes it covers PAST the
        // series candle would end up drawn by nothing and offered to no later layer — the composer
        // would preserve a hole it had reported as filled. And the tie is not a real conflict: a
        // coarse filler spans its whole period rather than claiming that one timestamp, which is
        // the same seam overlap this design already accepts, drawn muted beneath the finer candle.
        while si < series.len() && series[si].t_open_ms < filler.t_open_ms {
            out.push((series[si], series_tf));
            si += 1;
        }
        out.push((filler, tf));
    }
    for c in &series[si..] {
        out.push((*c, series_tf));
    }
}

/// Removes `covered` from `holes`, returning what is left of each hole.
///
/// `covered` must be ascending by start; overlapping members are fine and are coalesced as the
/// walk proceeds. Both inputs are half-open `[start, end)`.
fn subtract_covered(holes: &[(f64, f64)], covered: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(holes.len());
    for &(start, end) in holes {
        let mut cursor = start;
        for &(cs, ce) in covered {
            if ce <= cursor {
                continue;
            }
            if cs >= end {
                break;
            }
            if cs > cursor {
                out.push((cursor, cs.min(end)));
            }
            cursor = cursor.max(ce);
            if cursor >= end {
                break;
            }
        }
        if cursor < end {
            out.push((cursor, end));
        }
    }
    out
}

/// Does a candle bucket overlap `[from_ms, to_ms]`?
///
/// The one authority on what "visible" means for a candle. The chart's auto-Y range and the bottom
/// volume band both scale themselves from the visible set, and if they disagreed by one bucket the
/// band would normalise against a candle the price scale had already dropped.
///
/// Half-open at the left so a bucket that merely ENDS on the window edge is out, and inclusive at
/// the right so the bucket the right edge falls inside is in.
pub fn candle_intersects_window(t_open_ms: f64, tf_ms: f64, from_ms: f64, to_ms: f64) -> bool {
    t_open_ms + tf_ms > from_ms && t_open_ms <= to_ms
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
    /// the visible window's trades.
    ///
    /// The two are merged per bucket: a trade-derived candle wins the bucket it covers, and
    /// every other base bucket survives. The first local bucket is dropped when the base
    /// covers it, because the window clips it and the base candle is the complete one.
    ///
    /// `base` MUST be ascending by `t_open_ms` with one candle per bucket — the merge walks it
    /// once and cannot repair either. The production caller satisfies this by building it from
    /// a `BTreeMap`; an unsorted slice passes through unsorted and drops local candles.
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

        // Overlay the trade-derived candles PER BUCKET. Trades are the better source for a bucket
        // they cover, but only for that one: a thin market trades a few times an hour, and cutting
        // the base off at the first trade in the window discarded every later base candle, leaving
        // the series with only the buckets that happened to contain a trade. Measured on a live
        // thin market: 15 candles across a span holding 57, with a full cached history unused.
        //
        // The first local bucket is the exception. The window starts mid-bucket, so its trades are
        // partial; where the base covers that bucket, the base candle is the complete one.
        let skip_partial_first = local
            .first()
            .is_some_and(|first| self.candles.iter().any(|c| c.t_open_ms == first.t_open_ms));
        let mut merged = std::mem::take(&mut self.merge_scratch);
        merged.clear();
        merged.reserve(self.candles.len() + local.len());
        let mut base_at = 0usize;
        for candle in local.iter().skip(usize::from(skip_partial_first)) {
            while base_at < self.candles.len() && self.candles[base_at].t_open_ms < candle.t_open_ms
            {
                merged.push(self.candles[base_at]);
                base_at += 1;
            }
            // Same bucket: the trade-derived candle wins and the base one is dropped. A loop, not
            // an `if`, so a base that repeats a bucket cannot leave its stale twin sitting after
            // the winner and break the series' ascending order.
            while base_at < self.candles.len()
                && self.candles[base_at].t_open_ms == candle.t_open_ms
            {
                base_at += 1;
            }
            merged.push(*candle);
        }
        merged.extend_from_slice(&self.candles[base_at..]);
        std::mem::swap(&mut self.candles, &mut merged);
        self.merge_scratch = merged;
        // Buckets from here on are ours to accumulate into; earlier ones keep a base candle whose
        // volume is already complete. See `push_trades`.
        self.live_from = local
            .iter()
            .skip(usize::from(skip_partial_first))
            .next()
            .map_or(f64::INFINITY, |c| c.t_open_ms);

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
            // A bucket this series has not been accumulating itself still holds its BASE candle,
            // whose volume already covers the whole period from the source. Adding trade quantity
            // on top of it would double count, so the first live trade in such a bucket takes the
            // bucket over instead of joining it. Before the per-bucket merge the series always
            // ended in a trade-derived candle and this could not arise.
            let live_here = open_ms >= self.live_from;
            match self.candles.last_mut() {
                Some(last) if last.t_open_ms == open_ms && !live_here => {
                    *last = candle_from_tick(open_ms, t);
                    self.live_from = open_ms;
                    changed = true;
                }
                Some(last) if last.t_open_ms == open_ms => {
                    last.high = last.high.max(t.price);
                    last.low = last.low.min(t.price);
                    last.close = t.price;
                    last.volume += t.qty.max(0.0);
                    last.quote_volume += t.price * t.qty.max(0.0);
                    changed = true;
                }
                Some(last) if open_ms > last.t_open_ms => {
                    self.candles.push(candle_from_tick(open_ms, t));
                    self.live_from = self.live_from.min(open_ms);
                    changed = true;
                }
                None => {
                    self.candles.push(candle_from_tick(open_ms, t));
                    self.live_from = self.live_from.min(open_ms);
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
                        c.quote_volume += t.price * t.qty.max(0.0);
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
            if !candle_intersects_window(c.t_open_ms, tf, from_ms, to_ms) {
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
