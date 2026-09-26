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

/// `trade_candles` / `hide_candles` step meaning the zone runs as far as retained
/// trades reach, rather than a fixed candle count.
///
/// Stored as [`u16::MAX`]. An older build that only knows numeric counts reads it as
/// a very wide zone and still loads the file; existing numeric values are unchanged.
/// The popup labels this step "Max".
pub const CANDLE_ZONE_MAX: u16 = u16::MAX;

/// Whether `count` is the Max zone step ([`CANDLE_ZONE_MAX`]).
///
/// Args:
///     count: A `trade_candles` or `hide_candles` value.
///
/// Returns:
///     `true` only for the Max sentinel, never for a large numeric count.
pub fn is_candle_zone_max(count: u16) -> bool {
    count == CANDLE_ZONE_MAX
}

/// `CandleReadParams::trades_from_rel_ms` meaning "do not cut the trade read at a
/// candle window".
///
/// Finite, so the read still draws trades (`f32::INFINITY` hides them). The history
/// read starts at the oldest retained row and the ring capacity is the clamp.
pub const TRADES_FROM_UNBOUNDED: f32 = f32::MIN;

/// Bottom candle-volume styles used by `crate::config::ChartGraphicsCfg::candle_volume_style`.
///
/// A `u8` rather than an enum for the same reason [`CANDLE_MODE_FILLED`] is one: the value travels
/// into a shader uniform as a float and is clamped at the edge, so an open integer keeps the whole
/// path — `layout.toml`, `charts.json`, `VolumeStyleGpu.m.x` — free of per-representation
/// conversions. Both files are hand-editable; a bare enum string in them would be a new value
/// shape to parse.
///
/// Only two styles are live: OFF and HILLS. The band is ONE switch in the volumes popup — on, it
/// is always Moonbot's `Vol` (`candle_volume_sides`) on hills; off, it is gone. The other ids below
/// are what older builds wrote, and `moon_chart::normalize_chart_graphics` folds every one of them
/// into that pair: nothing else may treat them as a style.
pub const VOLUME_STYLE_OFF: u8 = 0;
/// A retired style: thin per-candle bars, one bar per bucket. A stored file carrying it reads as
/// hills with the switch on.
pub const VOLUME_STYLE_LEGACY_BARS: u8 = 1;
/// Moonbot-style "hills": a filled area whose top edge joins neighbouring buckets.
pub const VOLUME_STYLE_HILLS: u8 = 2;
/// A style id one build wrote for "Moonbot's `Vol` as its own style" before the bought/sold split
/// became the `candle_volume_sides` switch. A stored file carrying it reads as hills with the
/// switch on, like every other non-OFF id.
pub const VOLUME_STYLE_LEGACY_SIDES: u8 = 3;

/// Candle/trade chart display settings controlled by the candle button in the tab strip.
/// `layout.toml` stores the global default as `WindowLayout::candle_view`, while
/// `charts.json` stores optional per-tab overrides.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
// Deserialized through [`CandleViewWire`], which migrates the pre-split `price_lines` flag and
// supplies the per-field defaults `#[serde(default)]` used to give this struct directly, and
// serialized through [`CandleViewOut`], which writes a still-carried line switch back under its
// old key.
#[serde(from = "CandleViewWire", into = "CandleViewOut")]
pub struct CandleViewCfg {
    /// Candle timeframe in minutes, selected from [`CANDLE_TF_CHOICES_MIN`].
    pub tf_min: u32,
    /// Mode: 0 = filled, 1 = outlines, 2 = outlines in the trade zone, 3 = off.
    pub mode: u8,
    /// Number of MOST RECENT candles redrawn with trades in the trade zone; crosses are
    /// drawn only inside those buckets. A value of 0 disables trades entirely.
    /// [`CANDLE_ZONE_MAX`] extends the zone to the oldest retained trade.
    pub trade_candles: u16,
    /// Number of MOST RECENT candles not drawn at all, leaving only trades in those
    /// buckets. A value of 0 shows every candle. Usually no greater than `trade_candles`.
    /// [`CANDLE_ZONE_MAX`] hides candles over the same stretch the Max trade zone covers.
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
    /// The three line switches a file written BEFORE they moved to
    /// `crate::config::ChartGraphicsCfg` still carries under this table, or `None` once the
    /// carry-over has consumed them (or the file never had them).
    ///
    /// Written back under the OLD keys for exactly as long as it is carried: a save that lands
    /// before the carry-over has been committed — the pass could not write `charts.json`, or
    /// something else saved the layout first — must not strip the keys the next launch's pass
    /// will read. Once consumed it is `None`, the keys go, and no marker is needed to keep the
    /// pass from running twice: a file with no keys carries nothing. Neutralized by
    /// [`Self::history_inputs`], and `None` on a value the popup writes once the pass has run —
    /// a popup edit seeded from a default the pass had to hand back keeps carrying, which is
    /// harmless: the next launch carries the same switches into the same tab.
    pub carried_lines: Option<CarriedLines>,
}

/// The line switches an old `CandleViewCfg` table carried, each `None` where the key was absent.
///
/// The pre-split `price_lines` flag is already folded in: it stands in for either split flag the
/// table does not name, exactly as it did while the switches lived here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarriedLines {
    pub last_price_line: Option<bool>,
    pub mark_price_line: Option<bool>,
    pub moonshot_zone: Option<bool>,
}

impl CarriedLines {
    /// What a table that named NONE of the three drew while the switches lived here: the
    /// struct's own defaults, all on, regardless of any other table — a kind's or a tab's own
    /// `candle_view` replaced the inherited one whole. The carry-over stamps this for such a table
    /// in a profile that is otherwise old, so the switches it drew stay the switches it draws.
    pub const SHIPPED: CarriedLines = CarriedLines {
        last_price_line: Some(true),
        mark_price_line: Some(true),
        moonshot_zone: Some(true),
    };

    /// This carrier with every switch it did not name read as the shipped default: what a table
    /// of a kind's or a tab's OWN drew for it, since that table replaced the inherited one whole.
    pub fn or_shipped(self) -> CarriedLines {
        CarriedLines {
            last_price_line: self.last_price_line.or(Self::SHIPPED.last_price_line),
            mark_price_line: self.mark_price_line.or(Self::SHIPPED.mark_price_line),
            moonshot_zone: self.moonshot_zone.or(Self::SHIPPED.moonshot_zone),
        }
    }

    /// Whether the table named at least one of the three.
    pub fn any(self) -> bool {
        self.last_price_line.is_some()
            || self.mark_price_line.is_some()
            || self.moonshot_zone.is_some()
    }
}

/// Serialization form of [`CandleViewCfg`]: its fields, plus a still-carried line switch under
/// the old key it was read from, so a save before the carry-over is committed loses nothing.
#[derive(Serialize)]
struct CandleViewOut {
    tf_min: u32,
    mode: u8,
    trade_candles: u16,
    hide_candles: u16,
    trades_limit: u32,
    outline_px: f32,
    wicks_in_zone: bool,
    neutral_in_zone: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_price_line: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mark_price_line: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    moonshot_zone: Option<bool>,
}

impl From<CandleViewCfg> for CandleViewOut {
    fn from(c: CandleViewCfg) -> Self {
        let carried = c.carried_lines.unwrap_or(CarriedLines {
            last_price_line: None,
            mark_price_line: None,
            moonshot_zone: None,
        });
        Self {
            tf_min: c.tf_min,
            mode: c.mode,
            trade_candles: c.trade_candles,
            hide_candles: c.hide_candles,
            trades_limit: c.trades_limit,
            outline_px: c.outline_px,
            wicks_in_zone: c.wicks_in_zone,
            neutral_in_zone: c.neutral_in_zone,
            last_price_line: carried.last_price_line,
            mark_price_line: carried.mark_price_line,
            moonshot_zone: carried.moonshot_zone,
        }
    }
}

impl Default for CandleViewCfg {
    /// Start an unsaved candle view with matching 50-candle trade and hidden zones.
    fn default() -> Self {
        Self {
            tf_min: 5,
            mode: CANDLE_MODE_OUTLINE_IN_ZONE,
            trade_candles: 50,
            hide_candles: 50,
            trades_limit: 50_000,
            outline_px: 1.0,
            wicks_in_zone: true,
            neutral_in_zone: false,
            carried_lines: None,
        }
    }
}

/// Deserialization form of [`CandleViewCfg`], carrying the pre-split `price_lines` flag so a
/// `layout.toml` or `charts.json` written before the split still loads with the user's choice.
///
/// It exists because a plain `#[serde(default)]` cannot express "default to ANOTHER field": a
/// config that only says `price_lines = false` would silently come back with both lines ON, which
/// is the setting the user explicitly turned off. Every field is optional so a file missing any key
/// still loads. Missing zone widths retain the historical 3/0 values in saved objects; other
/// fields resolve against [`CandleViewCfg::default`]. An entirely absent candle view uses the
/// fresh default instead.
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
    /// The three switches that now live in `ChartGraphicsCfg`, still read here so a profile
    /// written before the move keeps the user's choice — see [`CandleViewCfg::carried_lines`].
    last_price_line: Option<bool>,
    mark_price_line: Option<bool>,
    moonshot_zone: Option<bool>,
}

impl From<CandleViewWire> for CandleViewCfg {
    /// Preserve saved choices and historical zone widths when an older object omits them.
    fn from(w: CandleViewWire) -> Self {
        let d = CandleViewCfg::default();
        Self {
            tf_min: w.tf_min.unwrap_or(d.tf_min),
            mode: w.mode.unwrap_or(d.mode),
            trade_candles: w.trade_candles.unwrap_or(3),
            hide_candles: w.hide_candles.unwrap_or(0),
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
            carried_lines: Some(CarriedLines {
                last_price_line: w.last_price_line.or(w.price_lines),
                mark_price_line: w.mark_price_line.or(w.price_lines),
                moonshot_zone: w.moonshot_zone,
            })
            .filter(|c| c.any()),
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
    /// protocol at all; `carried_lines` is a carry-over the startup pass has already consumed.
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
            carried_lines: None,
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
    /// Trade volume in the bucket, denominated in the base currency.
    ///
    /// May be an ESTIMATE rather than a wire figure: a Binance Futures deep-history row carries
    /// its volume in quote money, and [`split_wire_volume`] derives this field from it the same
    /// way [`estimate_quote_volume`] derives `quote_volume` from a base figure on other sources.
    pub volume: f32,
    /// Turnover in the bucket, denominated in the quote currency.
    ///
    /// Carried as DATA rather than computed at render time, because `volume * price` uses the
    /// CURRENT price and is wrong for every historical bar. This is a source-ingestion contract:
    /// downstream readers carry or sum this field but never derive it from `volume` and a current
    /// price. `0.0` means "nothing traded, or this source does not know". Where a source reports
    /// base volume, this is an ESTIMATE produced by [`estimate_quote_volume`]; where a Binance
    /// Futures deep-history row already carries turnover on the wire, this is that figure passed
    /// through unchanged — see [`split_wire_volume`], which decides per-row which of the two
    /// happened.
    pub quote_volume: f32,
}

/// Whether five wire-derived values are all finite and non-negative.
///
/// The single guard shared by [`estimate_quote_volume`], [`estimate_base_volume`] and
/// [`split_wire_volume`]'s quote branch, so what counts as a valid input cannot drift between the
/// three: a real price or a real volume/turnover figure is never negative, and a raw persisted or
/// wire-sourced `f32` cannot otherwise be assumed finite.
fn finite_non_negative(a: f32, b: f32, c: f32, d: f32, e: f32) -> bool {
    a.is_finite()
        && b.is_finite()
        && c.is_finite()
        && d.is_finite()
        && e.is_finite()
        && a >= 0.0
        && b >= 0.0
        && c >= 0.0
        && d >= 0.0
        && e >= 0.0
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
    if !finite_non_negative(volume, open, high, low, close) {
        return 0.0;
    }
    let estimate = volume * ((open + high + low + close) * 0.25);
    if estimate.is_finite() { estimate } else { 0.0 }
}

/// Estimates a bucket's base-currency volume from its OHLC and quote-currency turnover, for
/// sources whose wire `volume` is already quote money. The inverse of [`estimate_quote_volume`]:
/// `quote / OHLC4`.
///
/// Shares [`estimate_quote_volume`]'s guard via [`finite_non_negative`], plus the
/// division-specific `<= 0.0` check on OHLC4, so the two functions cannot drift apart on what
/// counts as a valid input.
///
/// On a REINTERPRETED legacy v1 row (`crate::market::kline_cache::legacy_volume_is_quote`) this
/// is the function that turns the stored `volume` slot from quote into base, and it hard-zeroes
/// whenever OHLC4 is `<= 0` — a zeroing path `ChartCandle::volume` never had before this task,
/// since nothing previously computed the base field, only `quote_volume`. Left unguarded on
/// purpose rather than special-cased: `volume` itself is not a rendered field (nothing displays
/// it — the band reads `quote_volume`), and `kline_cache::upsert_one`'s write-time `high > 0`
/// filter already excludes the all-zero-OHLC case for anything this cache itself wrote, so the
/// zeroing path is reachable only on already-malformed persisted bytes it did not write.
///
/// # Args
/// * `quote` - quote-currency turnover for the bucket; rejected if negative.
/// * `open`, `high`, `low`, `close` - the bucket's OHLC; each rejected if negative.
///
/// # Returns
/// The estimated base-currency volume, or `0.0` when any input is invalid, when OHLC4 is `<= 0`,
/// or when the result would not be finite.
pub(crate) fn estimate_base_volume(quote: f32, open: f32, high: f32, low: f32, close: f32) -> f32 {
    if !finite_non_negative(quote, open, high, low, close) {
        return 0.0;
    }
    let ohlc4 = (open + high + low + close) * 0.25;
    if ohlc4 <= 0.0 {
        return 0.0;
    }
    let estimate = quote / ohlc4;
    if estimate.is_finite() { estimate } else { 0.0 }
}

/// Splits a source's single wire `volume` figure into `(base, quote)`, given whether that figure
/// is already quote money.
///
/// The one place that decides which of a row's two `ChartCandle` volume fields is the wire figure
/// and which is the OHLC4 estimate — see the field docs on [`ChartCandle::volume`] and
/// [`ChartCandle::quote_volume`]. Pure arithmetic: this module stays venue- and
/// moonproto-agnostic, so callers that know a row's venue (`crate::venue`) decide
/// `volume_is_quote` themselves and pass it in.
///
/// `quote_volume`'s finite-and-non-negative invariant is preserved on BOTH branches, because
/// `crate::market::source::history` writes it straight into the kline cache and
/// `moon-chart`'s volume band (`volume_bars.rs`) sums it into a scale figure that a single
/// `+Infinity` would poison. On the `false` branch that invariant already came from
/// [`estimate_quote_volume`]'s own guard; on the `true` branch — where `volume` IS the wire
/// figure returned verbatim as `quote_volume` — it is enforced HERE via [`finite_non_negative`],
/// because nothing downstream re-validates a value that looks like it was already computed.
///
/// # Args
/// * `volume` - the wire figure as the source reported it.
/// * `open`, `high`, `low`, `close` - the bucket's OHLC.
/// * `volume_is_quote` - `true` when `volume` is quote-currency turnover, `false` when it is
///   base-currency volume.
///
/// # Returns
/// `(base, quote)`: when `volume_is_quote` is `true` and every input is finite and non-negative,
/// `(estimate_base_volume(volume, ..), volume)`; when `true` and an input fails that guard,
/// `(0.0, 0.0)`; when `false`, `(volume, estimate_quote_volume(volume, ..))`.
pub(crate) fn split_wire_volume(
    volume: f32,
    open: f32,
    high: f32,
    low: f32,
    close: f32,
    volume_is_quote: bool,
) -> (f32, f32) {
    if volume_is_quote {
        if !finite_non_negative(volume, open, high, low, close) {
            return (0.0, 0.0);
        }
        (estimate_base_volume(volume, open, high, low, close), volume)
    } else {
        (
            volume,
            estimate_quote_volume(volume, open, high, low, close),
        )
    }
}

/// Returns the timeframe bucket start for a timestamp, floored on the Unix-epoch grid.
pub fn bucket_open_ms(time_ms: f64, tf_ms: i64) -> f64 {
    let tf = tf_ms.max(1) as f64;
    (time_ms / tf).floor() * tf
}

/// Lower bound the history read should use for displayed trades, relative to `epoch_ms`.
///
/// Numeric counts keep the last-N window: the open of the bucket `count - 1` back from
/// `now_ms`, or `f32::INFINITY` when the count is 0 (the read treats a non-finite bound
/// as "draw no trades"). Max returns [`TRADES_FROM_UNBOUNDED`]: a finite value the read
/// does not raise to the visible window, so it copies as far as the ring still holds and
/// the drawn edge is applied afterwards from the oldest trade that copy returned.
///
/// Args:
///     trade_candles: Zone width, or [`CANDLE_ZONE_MAX`].
///     now_ms: Wall clock, Unix milliseconds.
///     tf_ms: Candle timeframe, milliseconds.
///     epoch_ms: Pane epoch, Unix milliseconds.
///
/// Returns:
///     Relative milliseconds. Non-finite means draw no trades.
pub fn trades_read_from_rel(trade_candles: u16, now_ms: f64, tf_ms: i64, epoch_ms: f64) -> f32 {
    if is_candle_zone_max(trade_candles) {
        return TRADES_FROM_UNBOUNDED;
    }
    trade_zone_start_rel(trade_candles, now_ms, tf_ms, epoch_ms, f32::NAN)
}

/// Shader start of the trade zone, in milliseconds relative to `epoch_ms`.
///
/// Candles whose bucket opens at or after this value are inside the zone. A non-finite
/// result means there is no zone (candles everywhere, no crosses from this setting).
/// Max uses the open of the bucket that holds `oldest_trade_rel`; a non-finite oldest
/// trade means nothing is retained, so Max draws no empty stretch. Numeric counts ignore
/// `oldest_trade_rel` and use the clock window.
///
/// Args:
///     trade_candles: Zone width, or [`CANDLE_ZONE_MAX`].
///     now_ms: Wall clock, Unix milliseconds.
///     tf_ms: Candle timeframe, milliseconds.
///     epoch_ms: Pane epoch, Unix milliseconds.
///     oldest_trade_rel: Oldest retained trade relative to the epoch, or NaN if none.
///
/// Returns:
///     Zone start relative to the epoch, or a non-finite value when the zone is off.
pub fn trade_zone_start_rel(
    trade_candles: u16,
    now_ms: f64,
    tf_ms: i64,
    epoch_ms: f64,
    oldest_trade_rel: f32,
) -> f32 {
    if is_candle_zone_max(trade_candles) {
        return max_trade_zone_edge_rel(oldest_trade_rel, tf_ms, epoch_ms);
    }
    if trade_candles == 0 {
        return f32::INFINITY;
    }
    let zone_open = bucket_open_ms(now_ms, tf_ms) - (trade_candles as f64 - 1.0) * tf_ms as f64;
    (zone_open - epoch_ms) as f32
}

/// Left edge of a Max trade zone: the open of the bucket holding the oldest retained trade.
///
/// Args:
///     oldest_trade_rel: Oldest retained trade relative to `epoch_ms`, or non-finite if none.
///     tf_ms: Candle timeframe, milliseconds.
///     epoch_ms: Pane epoch, Unix milliseconds.
///
/// Returns:
///     That bucket's open relative to the epoch, or `f32::INFINITY` when no trade is retained.
pub fn max_trade_zone_edge_rel(oldest_trade_rel: f32, tf_ms: i64, epoch_ms: f64) -> f32 {
    if !oldest_trade_rel.is_finite() {
        return f32::INFINITY;
    }
    let open = bucket_open_ms(epoch_ms + oldest_trade_rel as f64, tf_ms);
    (open - epoch_ms) as f32
}

/// Shader start of the hide-candles zone, in milliseconds relative to the pane epoch.
///
/// Candles whose bucket opens at or after it are omitted and only the trade crosses stay.
/// `f32::MAX` means the zone is off and every candle is drawn.
///
/// A numeric count is that many buckets back from the one holding `now_ms`, never reaching
/// left of the first resident trade: the setting means "draw ticks here rather than
/// candles", so a bucket with no ticks keeps its candle. The clamp is the open of the
/// bucket holding that first tick. A raw tick timestamp sits later than its own open, and
/// comparing bucket opens against the tick hid one candle fewer than asked.
///
/// Max uses the same edge as the Max trade zone ([`max_trade_zone_edge_rel`]). No retained
/// trade suppresses the zone instead of punching an empty stretch.
///
/// Args:
///     hide_candles: Hidden width, or [`CANDLE_ZONE_MAX`]. Zero disables the zone.
///     now_ms: Wall clock, Unix milliseconds.
///     tf_ms: Candle timeframe, milliseconds.
///     epoch_ms: Pane epoch, Unix milliseconds.
///     oldest_trade_rel: Oldest retained trade relative to the epoch, or NaN if none.
///
/// Returns:
///     Hide start relative to the epoch, or `f32::MAX` when candles stay everywhere.
pub fn hide_zone_start_rel(
    hide_candles: u16,
    now_ms: f64,
    tf_ms: i64,
    epoch_ms: f64,
    oldest_trade_rel: f32,
) -> f32 {
    if hide_candles == 0 {
        return f32::MAX;
    }
    if is_candle_zone_max(hide_candles) {
        let edge = max_trade_zone_edge_rel(oldest_trade_rel, tf_ms, epoch_ms);
        return if edge.is_finite() { edge } else { f32::MAX };
    }
    if oldest_trade_rel.is_nan() {
        return f32::MAX;
    }
    let hide_open = bucket_open_ms(now_ms, tf_ms) - (hide_candles as f64 - 1.0) * tf_ms as f64;
    let first_cross_open = bucket_open_ms(epoch_ms + oldest_trade_rel as f64, tf_ms);
    (hide_open.max(first_cross_open) - epoch_ms) as f32
}

/// Whether a Max trade zone must copy its crosses again.
///
/// `drawn_oldest_ms` is the oldest trade the last full copy uploaded. `ring_oldest_ms` is
/// the oldest trade the ring still holds. `None` means that side has no trade.
///
/// A live print at the right edge must not recopy: a cross reset rebuilds the candle
/// series, and that cost belongs on a bucket boundary, not on every frame. Older history
/// arriving (the ring's oldest timestamp moves left) recopies immediately so the zone can
/// extend. Eviction recopies only once the oldest trade has left its candle bucket, which
/// is when the drawn left edge actually moves.
///
/// Args:
///     drawn_oldest_ms: Oldest uploaded trade, Unix milliseconds.
///     ring_oldest_ms: Oldest retained trade, Unix milliseconds.
///     tf_ms: Candle timeframe, milliseconds.
///
/// Returns:
///     Whether the cross buffer is stale relative to the ring.
pub fn max_zone_recopy_due(
    drawn_oldest_ms: Option<i64>,
    ring_oldest_ms: Option<i64>,
    tf_ms: i64,
) -> bool {
    match (drawn_oldest_ms, ring_oldest_ms) {
        (None, None) => false,
        (Some(_), None) | (None, Some(_)) => true,
        (Some(drawn), Some(ring)) if ring < drawn => true,
        (Some(drawn), Some(ring)) => {
            bucket_open_ms(drawn as f64, tf_ms) != bucket_open_ms(ring as f64, tf_ms)
        }
    }
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

/// Thins a tick run to at most four REAL points per bucket, never inventing one.
///
/// A candle can fabricate an OHLC body because nobody reads its open/close as "a trade happened
/// at that exact instant" — a chart POINT is read exactly that way, so a bucket-mid point with no
/// matching trade would be a lie about when the market moved. Keeping the first, highest, lowest
/// and last real tick per bucket instead preserves the shape a candle would draw (open/high/low/
/// close) while every emitted point stays a value that actually traded.
///
/// Args:
///     ticks: Ascending by time (the caller sorts).
///     bucket_ms: Bucket width; `<= 0` emits the input unchanged — there is nothing to thin to.
///     out: Cleared first, then filled ascending, the same discipline [`aggregate_trades`] uses.
pub(crate) fn thin_ticks(ticks: &[Tick], bucket_ms: i64, out: &mut Vec<Tick>) {
    out.clear();
    if bucket_ms <= 0 {
        out.extend_from_slice(ticks);
        return;
    }
    let mut open_ms: Option<f64> = None;
    // Indices into `ticks` for the four roles of the bucket currently being collected.
    let mut first_i = 0usize;
    let mut last_i = 0usize;
    let mut high_i = 0usize;
    let mut low_i = 0usize;
    // Emits one bucket's up-to-four representative ticks, ascending, each index kept once — a
    // quiet bucket with a single trade collapses all four roles onto the same index.
    let flush =
        |first_i: usize, high_i: usize, low_i: usize, last_i: usize, out: &mut Vec<Tick>| {
            let mut idx = [first_i, high_i, low_i, last_i];
            idx.sort_unstable();
            let mut prev: Option<usize> = None;
            for i in idx {
                if prev != Some(i) {
                    out.push(ticks[i]);
                    prev = Some(i);
                }
            }
        };

    for (i, t) in ticks.iter().enumerate() {
        let this_open = bucket_open_ms(t.time_ms, bucket_ms);
        match open_ms {
            Some(open) if open == this_open => {
                last_i = i;
                if t.price > ticks[high_i].price {
                    high_i = i;
                }
                if t.price < ticks[low_i].price {
                    low_i = i;
                }
            }
            _ => {
                if open_ms.is_some() {
                    flush(first_i, high_i, low_i, last_i, out);
                }
                open_ms = Some(this_open);
                first_i = i;
                last_i = i;
                high_i = i;
                low_i = i;
            }
        }
    }
    if open_ms.is_some() {
        flush(first_i, high_i, low_i, last_i, out);
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

/// One base source offered to [`merge_bases`]: rows sorted by `t_open_ms`, aggregated at `tf_ms`.
///
/// Not a [`CoarseLayer`], though it looks like one: a layer is drawn at its own width into holes
/// and the FIRST layer offered wins, while a part is resampled into the series and the LAST part
/// offered wins. Extending one merge by analogy with the other's order inverts the result.
pub struct BasePart<'a> {
    pub rows: &'a [ChartCandle],
    pub tf_ms: i64,
}

/// Merges the base sources of one series into its timeframe, later parts winning a bucket.
///
/// Every part is resampled to `tf_ms` and merged by bucket key; a part whose timeframe is coarser
/// than or does not divide the target — a 5-minute snapshot under a 1-minute series — contributes
/// nothing. The caller lists the parts in ASCENDING priority, and that order is the whole
/// contract: the range-only 5-minute snapshot first, then every cached kind finer than the native
/// one (finest first, so a coarser exchange row overrides a bucket assembled from finer ones), then
/// the native cached kind, then the live deep history.
///
/// The finer kinds are what fills a HOLEY native kind. A 5-minute chart whose kind-5 cache covers
/// a fifth of its window used to draw the rest from the snapshot, whose rows carry only high and
/// low and so render as bodies without wicks — while the kind-1 rows the 1-minute chart had
/// written back covered nearly all of the same hours. A bucket resampled from fewer than a whole
/// period of finer rows is taken as it is: it is still an exchange candle for the part it covers,
/// which is more than the snapshot has, and the exchange's own coarse row overrides it the moment
/// it lands in the cache.
///
/// The result is ascending by `t_open_ms as i64`, with one candle per key; the last row with
/// that key is kept.
pub fn merge_bases(tf_ms: i64, parts: &[BasePart<'_>], out: &mut Vec<ChartCandle>) {
    out.clear();
    let mut scratch: Vec<ChartCandle> = Vec::new();
    for part in parts {
        if part.rows.is_empty() || part.tf_ms <= 0 || tf_ms < part.tf_ms || tf_ms % part.tf_ms != 0
        {
            continue;
        }
        resample(part.rows, tf_ms, &mut scratch);
        out.append(&mut scratch);
    }
    out.sort_by_key(|c| c.t_open_ms as i64);
    // The later element is passed first, and a true result drops it. Copy it into the retained
    // slot first so the last row with this key survives.
    out.dedup_by(|later, earlier| {
        if (later.t_open_ms as i64) != (earlier.t_open_ms as i64) {
            return false;
        }
        *earlier = *later;
        true
    });
}

/// Whether time-sorted `rows` aggregated at `tf_ms` leave a hole in `from_ms..to_ms`.
///
/// A hole is a missing bucket before the first row, between two consecutive rows, or after the
/// last row — "missing" meaning more than one bucket short of the edge, so the partial bucket at
/// either end is not one. The tail counts, unlike in [`compose_with_coarse`]: a native cache that
/// ends where the previous session did is exactly the stretch the finer kinds may cover when
/// another window has since fetched them. An empty set is one hole. This is the gate on reading
/// the finer cached kinds: a native kind without holes has nothing for them to fill.
pub fn has_holes(rows: &[ChartCandle], tf_ms: i64, from_ms: i64, to_ms: i64) -> bool {
    let tf = tf_ms.max(1) as f64;
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return true;
    };
    if first.t_open_ms - from_ms as f64 > tf || to_ms as f64 - (last.t_open_ms + tf) > tf {
        return true;
    }
    rows.windows(2)
        .any(|w| w[1].t_open_ms - w[0].t_open_ms > tf)
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
    /// Bumps when the series' bucket layout changes in a way a tail patch cannot express: rebuild,
    /// invalidate, the first candle, or an append that leaves a gap.
    layout_revision: u64,
    /// Lowest index changed since the last `take_dirty_from`; `usize::MAX` = clean.
    dirty_from: usize,
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
            layout_revision: 0,
            dirty_from: usize::MAX,
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

/// Re-applies a changed series tail to its composed list without recomposing.
///
/// Sound because compose rule 1 puts no hole after the last candle, so the composed list ends in
/// series entries; a gap-free append adds none either (a gap bumps `layout_revision` instead).
/// Returns the composed index the patch starts at, or `None` when the tail is not what that
/// argument assumes (a filler met among the entries to replace, or pairing failed) — the caller
/// then recomposes.
pub fn patch_composed_tail(
    series: &[ChartCandle],
    series_tf_ms: f64,
    dirty_from: usize,
    fill: &mut Vec<(ChartCandle, f32)>,
) -> Option<usize> {
    let stf = series_tf_ms as f32;
    if dirty_from >= series.len() {
        return None;
    }
    let last_series = fill.iter().rposition(|(_, tf)| *tf == stf)?;
    let t_last = fill[last_series].0.t_open_ms;
    let prev_len = series.partition_point(|c| c.t_open_ms <= t_last);
    if dirty_from > prev_len {
        return None;
    }
    // Walk back over the series entries being replaced; a filler among them means the tail is not
    // pure series and only a recompose is right.
    let mut fi = fill.len();
    let mut k = prev_len;
    while k > dirty_from {
        fi = fi.checked_sub(1)?;
        if fill[fi].1 != stf {
            return None;
        }
        k -= 1;
    }
    if fi > 0 && fill[fi - 1].0.t_open_ms >= series[dirty_from].t_open_ms {
        return None;
    }
    fill.truncate(fi);
    fill.extend(series[dirty_from..].iter().map(|c| (*c, stf)));
    Some(fi)
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
    let mut wide: Vec<(f64, f64)> = Vec::new();
    for layer in layers {
        if holes.is_empty() || layer.rows.is_empty() || !(layer.tf_ms > 0.0) {
            continue;
        }
        covered.clear();
        wide.clear();
        wide.extend(
            holes
                .iter()
                .copied()
                .filter(|&(start, end)| end - start >= layer.tf_ms),
        );
        // ROWS outer, holes inner, so a row is taken AT MOST ONCE per layer. Scanning per hole
        // instead would take a row twice whenever its bucket straddles two holes — which is not an
        // exotic case but the ordinary one this feature exists for: a single isolated candle
        // between two large disjoint blocks leaves a gap narrower than the coarse timeframe on
        // either side of it. The duplicate survived into the render list, where it drew the same
        // candle twice and made the volume band count that bucket twice.
        // The per-row lookup binary-searches the holes wide enough for this layer and probes at
        // most two neighbours: those holes are disjoint and each at least one bucket wide.
        for c in layer.rows.iter() {
            let i = wide.partition_point(|h| h.1 <= c.t_open_ms);
            let fills_a_hole = [wide.get(i), wide.get(i + 1)]
                .into_iter()
                .flatten()
                .any(|&(start, end)| c.t_open_ms + layer.tf_ms > start && c.t_open_ms < end);
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
/// `holes` are ascending and disjoint. `covered` is ascending by start; overlapping members are
/// fine and are coalesced as the walk proceeds. Both inputs are half-open `[start, end)`.
///
/// One coverage index is shared across every hole. An interval is dropped only after its end is
/// at or before the hole cursor. An interval that starts at or after the hole end, and an interval
/// whose end reaches or passes the hole end, stay at that index for the next hole.
fn subtract_covered(holes: &[(f64, f64)], covered: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(holes.len());
    let mut covered_i = 0usize;
    for &(start, end) in holes {
        let mut cursor = start;
        while covered_i < covered.len() {
            let (cs, ce) = covered[covered_i];
            if ce <= cursor {
                covered_i += 1;
                continue;
            }
            // Starts at or after this hole. Later holes lie further right, so leave the index.
            if cs >= end {
                break;
            }
            if cs > cursor {
                out.push((cursor, cs.min(end)));
            }
            cursor = cursor.max(ce);
            // Still reaches this hole's end, so the next hole may need the same interval.
            if cursor >= end {
                break;
            }
            covered_i += 1;
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

    /// Revision of the bucket layout; see the field.
    pub fn layout_revision(&self) -> u64 {
        self.layout_revision
    }

    /// Lowest index changed since the previous call, `None` when nothing changed; resets it.
    pub fn take_dirty_from(&mut self) -> Option<usize> {
        let d = std::mem::replace(&mut self.dirty_from, usize::MAX);
        (d != usize::MAX).then_some(d)
    }

    fn bump_layout(&mut self) {
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.dirty_from = usize::MAX;
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
        self.candles.clear();
        self.bump_layout();
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
    /// once and cannot repair either. The production caller satisfies this through
    /// [`merge_bases`], which sorts and deduplicates the base; an unsorted slice passes through
    /// unsorted and drops local candles.
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
        self.bump_layout();
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
            let n = self.candles.len();
            match self.candles.last_mut() {
                Some(last) if last.t_open_ms == open_ms && !live_here => {
                    *last = candle_from_tick(open_ms, t);
                    self.live_from = open_ms;
                    self.dirty_from = self.dirty_from.min(n - 1);
                    changed = true;
                }
                Some(last) if last.t_open_ms == open_ms => {
                    last.high = last.high.max(t.price);
                    last.low = last.low.min(t.price);
                    last.close = t.price;
                    last.volume += t.qty.max(0.0);
                    last.quote_volume += t.price * t.qty.max(0.0);
                    self.dirty_from = self.dirty_from.min(n - 1);
                    changed = true;
                }
                Some(last) if open_ms > last.t_open_ms => {
                    // Bucket opens are aligned, so the next contiguous bucket is exactly one
                    // timeframe on; anything later leaves a gap a tail patch cannot express.
                    let gap = open_ms > last.t_open_ms + tf_ms as f64;
                    self.candles.push(candle_from_tick(open_ms, t));
                    self.live_from = self.live_from.min(open_ms);
                    if gap {
                        self.bump_layout();
                    } else {
                        self.dirty_from = self.dirty_from.min(n);
                    }
                    changed = true;
                }
                None => {
                    self.candles.push(candle_from_tick(open_ms, t));
                    self.live_from = self.live_from.min(open_ms);
                    self.bump_layout();
                    changed = true;
                }
                _ => {
                    // Update high, low, and volume for a late resend into a recent old bucket.
                    let lo = n.saturating_sub(4);
                    if let Some(i) = self.candles[lo..]
                        .iter()
                        .rposition(|c| c.t_open_ms == open_ms)
                        .map(|i| lo + i)
                    {
                        let c = &mut self.candles[i];
                        c.high = c.high.max(t.price);
                        c.low = c.low.min(t.price);
                        c.volume += t.qty.max(0.0);
                        c.quote_volume += t.price * t.qty.max(0.0);
                        self.dirty_from = self.dirty_from.min(i);
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
    ///
    /// Scans only the rows around the window: `candles` is ascending by `t_open_ms` (`rebuild`
    /// merges a sorted base with sorted trade buckets, `push_trades` only appends later buckets).
    pub fn price_range(&self, from_ms: f64, to_ms: f64) -> Option<(f32, f32)> {
        let tf = self.tf_ms.max(1) as f64;
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        let start = self
            .candles
            .partition_point(|c| c.t_open_ms + tf <= from_ms);
        let end = self
            .candles
            .partition_point(|c| c.t_open_ms <= to_ms)
            .max(start);
        for c in &self.candles[start..end] {
            #[cfg(test)]
            PRICE_RANGE_VISITED.with(|n| n.set(n.get() + 1));
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
thread_local! {
    static PRICE_RANGE_VISITED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Rows `price_range` inspected on this thread since the last call; resets the count.
#[cfg(test)]
#[allow(dead_code)] // read by the prover's before/after measurement
pub(crate) fn take_price_range_visited() -> u64 {
    PRICE_RANGE_VISITED.with(|n| n.replace(0))
}

#[cfg(test)]
mod tests;
