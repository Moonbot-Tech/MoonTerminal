//! Auto figures: levels, sloped lines, squeezes and two-line formations found in closed candles.
//!
//! Every detector here is a PURE function over the closed bars of ONE timeframe: no clock, no I/O,
//! no market lookup, no chart state. The chart calls them when a bar of the row's timeframe closes
//! (never per frame), and the all-coins scanner planned after it calls the very same functions —
//! that is why nothing in this module may reach for a view, a store or "now".
//!
//! Layout follows `moon_core::figures::tools`: one module per detector and one row per detector in
//! [`REGISTRY`]. Adding a detector is a module, a registry row and the contract's
//! [`DetectorKind`] / [`DetectorParams`] arm — nothing else here changes.
//!
//! Units. Percentages are of PRICE (`0.3` = 0.3 %), bar counts are bars of the row's own
//! timeframe. Prices of found figures are `f64` although candles carry `f32`: the figures are
//! drawn through `moon_core::figures::FigNode`, which is `f64`, and the derived values (a cluster
//! mean, a line projected hundreds of bars forward) would lose the low digits of a sub-cent coin in
//! `f32`. Every `f32` candle price converts to `f64` exactly, so nothing is lost on the way in.
//!
//! Input contract for every detector: `bars` are closed candles sorted by `t_open_ms`, ascending.
//! A bar with a non-finite or non-positive high or low, or `high < low`, is skipped wherever it
//! would be read — it never panics a detector and never becomes a touch.
//!
//! # Bars without an open and close
//!
//! Much of the 5-minute history comes from the core's own ring, which the kline cache keeps under a
//! kind of its own (`moon_core::market::kline_cache::RING_5M_KIND`), and which carries only each
//! bucket's high and low: its open and close are the core's `open == high`, `close == low`
//! placeholder (measured 19.11.2026), not prices the market traded at the bucket's edges. Read as
//! closes they would break every level the bucket's range straddles: a ring bar spanning a line
//! "closes" below it whenever its low is below it.
//!
//! So the bar assembly ([`with_ring_rows`]) gives every bar taken from the ring kind an UNKNOWN
//! open and close — `NaN` in both — before anything resamples or detects. The mark comes from the
//! row's SOURCE, the kind it was read from, never from its shape: a real candle that opened at its
//! high and closed at its low keeps both, and a real candle always wins its bucket over a ring
//! row. The mark is in the bar itself, so
//! it survives a resample with the meaning it should have: a bucket's open is its first sub-bar's
//! and its close its last's, so a coarse bar whose last 5 minutes came from the ring has an unknown
//! close and one ending on a real candle has the real close. Every reader of a close or an open
//! then asks [`close_of`] / [`open_of`] — or, for the side of a line, [`close_side`] — and leaves
//! an unknown one out:
//!
//! - breaks (`scan_break`), the closes outside a formation's span, the bodies a trend line may not
//!   cross and the bar events (breakout, false break, bounce, role flip, stop hunt) ask which side
//!   of the line a bar closed on ([`close_side`]): a known close answers, and a bar without one
//!   answers only when its whole range lies on one side — its close, wherever it was, lay there
//!   too. A bar whose range reaches the line has no side: it neither breaks nor holds anything.
//!   Breaks and bar events ask it through [`line_side`], which moves the line by the break buffer
//!   ([`BREAK_BUFFER_SHARE`]) on the side the figure did not hold;
//! - a flag's pole reads only known opens and closes — its size is a price, not a side;
//! - the ATR takes a bar's own range as its true range when the previous close is unknown;
//! - a period level (a day's open, yesterday's close) is the first or last bar's own price, and is
//!   not found when that bar has none — no other bar's price is that one;
//! - what only needs "about where price is" — the tolerance's reference, round prices, a profile's
//!   POC tie-break — reads [`last_price`]: the newest bar's close, or the middle of its range when
//!   the close is unknown;
//! - highs and lows — pivots, swings, extremes, touches, the profile's ranges — are read as ever.
//!
//! A NaN rather than a parallel quality array, because it travels through `resample` and every
//! slice and copy without any of them knowing, and because a reader that forgets to ask fails
//! closed: every comparison with a NaN is false, so a forgotten check reads as "no break", never as
//! a break the market did not make.

use moon_core::config::auto_figures::{BrokenPolicy, DetectorKind, DetectorParams, ToleranceUnit};
use moon_core::market::candles::{ChartCandle, bucket_open_ms, resample};
use moon_core::market::kline_cache::KIND_5M_MS;

pub mod confluence;
pub mod coverage;
pub mod draw;
pub mod events;
pub mod extremes;
pub mod formations;
pub mod history;
pub mod period_levels;
pub mod pivots;
pub mod round_levels;
pub mod scan;
pub mod squeeze;
pub mod swing;
pub mod take;
pub mod tf;
pub mod trend;
pub mod twins;
pub mod volume_profile;

#[cfg(test)]
mod test_bars;
#[cfg(test)]
mod tests;

/// One point where price met a figure: the pivot bar's open time and the pivot's price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Touch {
    pub t_ms: f64,
    pub price: f64,
}

/// A figure — a horizontal level, a squeeze's level, a sloped trend line or a side of a two-line
/// formation (`FoundShape::Pair`: a close above its upper side or below its lower one) — that a
/// bar has CLOSED beyond after its last touch, by more than the break buffer
/// ([`BREAK_BUFFER_SHARE`] of the figure's touch tolerance).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Broken {
    /// Open time of the first bar of the LATEST break episode: a close back on the original side
    /// ends an episode, and the next close beyond starts a new one (see `scan_break`).
    pub at_ms: f64,
    /// Closed bars after the breaking one, up to the last bar of the input.
    pub bars_since: u32,
    /// How long the LONGEST break episode after the last touch lasted, in bars: from its breaking
    /// bar to the bar that closed back on the original side, or to the last bar of the input for
    /// an episode still open (then it equals [`Self::bars_since`]).
    ///
    /// One deadline for both: under `Keep { bars }` the figure lives through the `bars` bars after
    /// the breaking one, and on the bar after them it is gone whether that bar closed back or not
    /// — a return there comes too late, which is why a closed episode is measured to its return
    /// bar and an open one to the last bar.
    ///
    /// What a `Keep { bars }` policy judges by ([`survives`]): an earlier stay beyond the figure
    /// that outlasted the policy disproved it, and price coming back later does not undo that —
    /// otherwise a support price stood under for hours came back to life on the first close above
    /// it (DRAMUSDT 21.11.2026: broken for 13 and then 14 bars and still drawn as a squeeze).
    pub longest_bars: u32,
}

/// Share of a figure's touch tolerance a close must clear the line by to count as beyond it — a
/// break, and the bar events' "other side".
///
/// Without it a close one tick past a level's price broke it, while a pivot at that same price
/// was a touch: one price, two meanings (21.11.2026 audit: in 902 of 2408 kept broken lines the
/// deepest close of the latest break lay inside the touch band). Half the band: a close past the
/// middle of the band is no longer "at" the level. A close back on the ORIGINAL side needs no
/// buffer — any close there ends an episode — so a buffer never keeps a level broken longer.
pub const BREAK_BUFFER_SHARE: f64 = 0.5;

/// The break buffer of a figure with touch tolerance `tolerance_frac` (fraction of price), as a
/// fraction of price: [`BREAK_BUFFER_SHARE`] of it, or zero for a tolerance that is not a positive
/// finite number (period extremes, round prices — lines without a band).
pub fn break_buffer(tolerance_frac: f64) -> f64 {
    if tolerance_frac.is_finite() && tolerance_frac > 0.0 {
        tolerance_frac * BREAK_BUFFER_SHARE
    } else {
        0.0
    }
}

/// The geometry of a found figure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FoundShape {
    /// A level at one price.
    Horizontal { price: f64 },
    /// A line through `(t0_ms, p0)` and `(t1_ms, p1)` — its first and last touch, `t0_ms < t1_ms`.
    Sloped {
        t0_ms: f64,
        p0: f64,
        t1_ms: f64,
        p1: f64,
    },
    /// A level plus the converging sloped line from its other side (rising lows under a
    /// resistance, falling highs over a support). The sloped half runs through its first and last
    /// touch like [`FoundShape::Sloped`]; `pressed` says the sloped half, projected to the last
    /// closed bar, came within the row's `pressed_pct` of the level (or past it) while that bar
    /// closed inside the formation — not beyond the level, nor beyond a line that has not reached
    /// it yet, each by more than its break buffer (`squeeze` module doc).
    Squeeze {
        level: f64,
        t0_ms: f64,
        p0: f64,
        t1_ms: f64,
        p1: f64,
        pressed: bool,
    },
    /// A formation of two lines (`Formation`): a triangle, wedge, channel or flag of two sloped or
    /// horizontal sides, or a double top / bottom drawn as its level and its neckline. `upper`
    /// lies above `lower` over the whole formation; each runs through its OWN first and last
    /// touch. What the pair is is [`Found::tag`].
    Pair { upper: Seg, lower: Seg },
}

/// One side of a [`FoundShape::Pair`]: the line through `(t0_ms, p0)` and `(t1_ms, p1)`, its first
/// and last touch, `t0_ms < t1_ms`. A horizontal side has `p0 == p1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seg {
    pub t0_ms: f64,
    pub p0: f64,
    pub t1_ms: f64,
    pub p1: f64,
}

impl Seg {
    /// The side's price at `t_ms`, through [`line_price_at`]: `None` on a degenerate or unusable
    /// line.
    pub fn price_at(&self, t_ms: f64) -> Option<f64> {
        line_price_at(self.t0_ms, self.p0, self.t1_ms, self.p1, t_ms)
    }
}

impl FoundShape {
    /// Price of the figure's primary line at `t_ms`: the level of a horizontal or a squeeze, the
    /// projection of a sloped line, the UPPER side of a pair.
    ///
    /// A pair has two lines and no primary one; the upper is only a default for a caller that has
    /// no side to go by (a sort key). Wherever a reference price exists — which side of price a
    /// figure is on, how far it is, whether price approaches it — the caller asks
    /// [`FoundShape::price_near`] instead, which picks the nearer side; and whoever SELECTED a pair
    /// against a price keeps the side it selected by ([`FoundShape::near_side`]) and reads that
    /// side later through [`FoundShape::side_price_at`] — the chart's caption, event mark and
    /// confluence all do.
    ///
    /// Returns `None` where no usable price exists — a non-finite input, a degenerate line, or a
    /// sloped line extrapolated to zero or below. Callers divide by this price, so a value that is
    /// not a positive finite number is never handed out.
    pub fn price_at(&self, t_ms: f64) -> Option<f64> {
        match *self {
            FoundShape::Horizontal { price } | FoundShape::Squeeze { level: price, .. } => {
                positive(price)
            }
            FoundShape::Sloped {
                t0_ms,
                p0,
                t1_ms,
                p1,
            } => line_price_at(t0_ms, p0, t1_ms, p1, t_ms),
            FoundShape::Pair { upper, .. } => upper.price_at(t_ms),
        }
    }

    /// Price at `t_ms` of the figure's line NEAREST `ref_price`: for a pair, whichever of its two
    /// sides is closer to `ref_price` (the upper one on a tie, or when `ref_price` is not finite);
    /// for every other shape, [`FoundShape::price_at`].
    ///
    /// What "the figure's price" means wherever it is measured against the current price: the
    /// side of price a row keeps it on (`take`), its distance, an `Approach`. Price inside a
    /// channel is near the boundary it will meet first, and that boundary is what the reader
    /// watches. A side with no usable price at `t_ms` is skipped; `None` when neither has one.
    pub fn price_near(&self, t_ms: f64, ref_price: f64) -> Option<f64> {
        self.side_price_at(t_ms, self.near_side(t_ms, ref_price))
    }

    /// Which side of a pair [`FoundShape::price_near`] answers with at `t_ms` for `ref_price`: the
    /// nearer one, the upper on a tie or a non-finite `ref_price`, the only one with a usable
    /// price when the other has none. `None` for every shape that is not a pair, and for a pair
    /// neither side of which has a price at `t_ms`.
    ///
    /// Public so a caller that SELECTED a pair against one price can keep the side it selected by
    /// and read that same side later ([`FoundShape::side_price_at`]) — the chart's caption, event
    /// mark and confluence then name the boundary `take` kept the pair by, not whichever side a
    /// later moment happens to be nearer.
    pub fn near_side(&self, t_ms: f64, ref_price: f64) -> Option<PairSide> {
        let FoundShape::Pair { upper, lower } = self else {
            return None;
        };
        match (upper.price_at(t_ms), lower.price_at(t_ms)) {
            (Some(u), Some(l)) => Some(
                if ref_price.is_finite() && (l - ref_price).abs() < (u - ref_price).abs() {
                    PairSide::Lower
                } else {
                    PairSide::Upper
                },
            ),
            (Some(_), None) => Some(PairSide::Upper),
            (None, Some(_)) => Some(PairSide::Lower),
            (None, None) => None,
        }
    }

    /// Price at `t_ms` of `side` of a pair; [`FoundShape::price_at`] for every other shape and for
    /// `side = None` (a pair's upper side).
    pub fn side_price_at(&self, t_ms: f64, side: Option<PairSide>) -> Option<f64> {
        match (self, side) {
            (FoundShape::Pair { lower, .. }, Some(PairSide::Lower)) => lower.price_at(t_ms),
            _ => self.price_at(t_ms),
        }
    }
}

/// One side of a [`FoundShape::Pair`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PairSide {
    Upper,
    Lower,
}

/// Price of the line through `(t0_ms, p0)` and `(t1_ms, p1)` at `t_ms`, interpolated or
/// extrapolated linearly in TIME (not in bar index, so a gap in the series does not bend it).
///
/// Returns `None` when `t1_ms <= t0_ms`, when any input is not finite, or when the result is not a
/// positive finite price.
pub fn line_price_at(t0_ms: f64, p0: f64, t1_ms: f64, p1: f64, t_ms: f64) -> Option<f64> {
    let dt = t1_ms - t0_ms;
    // `!(dt > 0.0)` also rejects a NaN span.
    if !(dt > 0.0) || !p0.is_finite() || !p1.is_finite() || !t_ms.is_finite() {
        return None;
    }
    positive(p0 + (p1 - p0) * ((t_ms - t0_ms) / dt))
}

/// What a found figure IS, for detectors whose figures are not interchangeable — so the caption
/// can tell a day's POC from its value-area edge, the week's open from yesterday's close, or a
/// triangle from a channel, which share a row, a style and often a neighbourhood.
///
/// `None` in [`Found::tag`] for `Swing`, `Trend`, `Squeeze` and round prices: their lines are all
/// of one kind, and the price (or the touch count) is the whole story. A `Formation` always tags
/// its pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineTag {
    /// A period's high (`PeriodExtreme`).
    High,
    /// A period's low (`PeriodExtreme`).
    Low,
    /// The point of control of the profiled day (`VolumeProfile`).
    Poc,
    /// The value area's upper edge (`VolumeProfile`).
    Vah,
    /// The value area's lower edge (`VolumeProfile`).
    Val,
    /// The POC of an earlier complete day that price has not traded at since (`VolumeProfile`).
    NakedPoc,
    /// The open of the reference UTC day (`PeriodLevel`).
    DayOpen,
    /// The close of the UTC day before the reference one (`PeriodLevel`).
    PrevDayClose,
    /// The open of the reference ISO week (`PeriodLevel`).
    WeekOpen,
    /// The close of the ISO week before the reference one (`PeriodLevel`).
    PrevWeekClose,
    /// Two converging sides sloping toward each other, or one of them horizontal (`Formation`).
    Triangle,
    /// Two converging sides sloping the same way (`Formation`).
    Wedge,
    /// Two parallel sides, sloped or horizontal (`Formation`).
    Channel,
    /// A short channel or converging pair right after an impulse — a flag or a pennant
    /// (`Formation`).
    Flag,
    /// Two equal highs with a trough between: `upper` is the tops' level, `lower` the neckline
    /// (`Formation`).
    DoubleTop,
    /// Two equal lows with a peak between: `lower` is the bottoms' level, `upper` the neckline
    /// (`Formation`).
    DoubleBottom,
}

/// One figure a detector found.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub shape: FoundShape,
    /// Every touch, in time order.
    pub touches: Vec<Touch>,
    /// The detector's own rank for `TakeOrder::Strongest`. Comparable only between figures of the
    /// same detector; see each detector for its formula.
    pub strength: f32,
    /// Open time of the earliest and the latest bar the figure is anchored to — its first and last
    /// touch for pivot detectors, the period's bars for a period extreme.
    pub first_ms: f64,
    pub last_ms: f64,
    /// The latest break episode after the last touch, set when the row's `BrokenPolicy` still
    /// keeps the figure (`Keep { bars }`; `Remove` drops it instead). For a squeeze it is the
    /// level's break; for a two-line formation, the later-starting break of its two sides (a close
    /// above the upper or below the lower). A previous period's extreme reports it informationally and is never
    /// dropped: a period's high is a fact, not a hypothesis a break disproves.
    pub broken: Option<Broken>,
    /// Quote volume traded on the bars that touched the figure — the sum of their
    /// `ChartCandle::quote_volume`, each touching bar ONCE even when it touched twice (a squeeze's
    /// level and its line on one bar). What `StrengthBasis::Volume` ranks by. `0` for a figure
    /// without touches, and for bars whose source reports no turnover.
    ///
    /// A volume-profile line has no touching bars — it is a property of the profile, not a place
    /// pivots met — so it reports the volume its profile holds in the line's own price bin: the
    /// money traded AT that price, which is what this field measures for every other level too.
    pub touch_volume: f64,
    /// The tolerance the detector actually grouped touches by, as a FRACTION of price (`0.0025` =
    /// 0.25 %) — under `ToleranceUnit::Atr` the ATR share already converted with this run's ATR.
    /// The zone band is drawn from this, never re-derived from the row's parameters, so it shows
    /// the band the detector used whatever the unit. A squeeze reports its LEVEL's tolerance (the
    /// band is drawn around the level); a formation its touch tolerance, a double its equality
    /// tolerance; `0` for a detector without a tolerance. A volume-profile
    /// line reports its bin's width as a fraction of its price, so its band is the bin it stands
    /// for.
    pub tolerance_frac: f64,
    /// What the line is, where a detector draws several kinds of line in one row (see
    /// [`LineTag`]); `None` where every line of the detector is the same kind.
    pub tag: Option<LineTag>,
    /// The side the figure held price on — the side of its latest touch: `Below` for a resistance
    /// (a pivot high touched it), `Above` for a support. The side `broken` is measured from, handed
    /// to the event layer so that its false break and breakout tell the story the detector's
    /// `broken` tells ([`events`]).
    ///
    /// Set by the detectors whose figures a break disproves and whose row's policy then drops
    /// them: `Swing`, `Trend` and `Squeeze` (its level). `None` for a two-line formation — each of
    /// its sides has a fixed side, judged by the event layer itself — and for lines a break does
    /// not end: period extremes, period levels, round prices and profile lines.
    pub origin: Option<Side>,
}

/// What a detector legitimately needs beyond its bars and parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectCtx {
    /// Bar length of `bars`, ms. Sloped lines are built in time, and their slope bounds are in %
    /// per BAR, so the bar length is what converts one into the other. A value `<= 0` makes the
    /// sloped detectors return nothing.
    pub tf_ms: i64,
    /// The caller's reference time, ms: the open of the bar still forming, or the last tick's
    /// time. Handed in rather than read from a clock, so a detector stays a pure function of its
    /// arguments. Only calendar detectors read it ("today" is the UTC day of this moment); a
    /// value that is not a positive finite number falls back to the last valid bar's open time.
    pub now_ms: f64,
    /// The exchange's price tick of the market, in price units, when the caller knows it — the
    /// grid prices stand on (`moon_core::market::source::MarketSource::price_tick`). A touch
    /// tolerance is never narrower than [`TICK_FLOOR_STEPS`] of it ([`tolerance_frac`]): on a coin
    /// whose tick is coarser than the row's tolerance every tick was a level of its own
    /// (1000BTTUSDT, tick 0.32 % of price, 21.11.2026). `None` (no core, a demo preview) leaves
    /// the tolerance as the row sets it.
    pub price_tick: Option<f64>,
}

/// Fewest price ticks a touch tolerance spans ([`DetectCtx::price_tick`]): two, so that pivots on
/// neighbouring ticks share a level.
pub const TICK_FLOOR_STEPS: f64 = 2.0;

impl DetectCtx {
    /// The reference time a calendar detector uses: `now_ms` when usable, never earlier than the
    /// last valid bar (a stale `now_ms` must not turn yesterday back into "today"); the last valid
    /// bar's open when `now_ms` is unusable; `None` when there is neither.
    pub fn reference_ms(&self, bars: &[ChartCandle]) -> Option<f64> {
        let last = bars.iter().rev().find(|b| bar_ok(b)).map(|b| b.t_open_ms);
        match (positive(self.now_ms), last) {
            (Some(now), Some(last)) => Some(now.max(last)),
            (Some(now), None) => Some(now),
            (None, last) => last,
        }
    }
}

/// A detector: one module, one [`REGISTRY`] row.
pub trait Detector: Sync {
    /// The contract kind this detector answers for.
    fn kind(&self) -> DetectorKind;

    /// Closed bars of ONE timeframe in, figures out. Pure: no clock, no I/O, no market lookup.
    /// Parameters of another kind yield an empty result.
    fn detect(&self, bars: &[ChartCandle], params: &DetectorParams, ctx: &DetectCtx) -> Vec<Found>;
}

/// Every detector, one row per [`DetectorKind`]. The registry test pins "exactly one row per kind".
pub static REGISTRY: &[&dyn Detector] = &[
    &swing::Swing,
    &extremes::PeriodExtreme,
    &trend::Trend,
    &squeeze::Squeeze,
    &volume_profile::VolumeProfile,
    &period_levels::PeriodLevel,
    &round_levels::RoundLevels,
    &formations::Formation,
];

/// Run the detector `params` names over `bars`.
///
/// Returns an empty result when no registry row answers for the kind — the registry test rules that
/// out; a panic here would take a candle-close handler down for a table typo.
pub fn detect(params: &DetectorParams, bars: &[ChartCandle], ctx: &DetectCtx) -> Vec<Found> {
    let kind = params.kind();
    REGISTRY
        .iter()
        .find(|d| d.kind() == kind)
        .map(|d| d.detect(bars, params, ctx))
        .unwrap_or_default()
}

/// Which side of a line a price is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Above,
    Below,
}

impl Side {
    /// The other side.
    pub fn opposite(self) -> Side {
        match self {
            Side::Above => Side::Below,
            Side::Below => Side::Above,
        }
    }
}

/// Side of `price` relative to `line`; `None` exactly on it or when either is not finite.
pub(crate) fn side_of(price: f64, line: f64) -> Option<Side> {
    if price > line {
        Some(Side::Above)
    } else if price < line {
        Some(Side::Below)
    } else {
        None
    }
}

/// `v` when it is a positive finite number.
pub(crate) fn positive(v: f64) -> Option<f64> {
    (v.is_finite() && v > 0.0).then_some(v)
}

/// A bar whose range can be read: a finite open time, a finite positive high and low, `high >=
/// low`, and an open and close each either a finite positive price or UNKNOWN (`NaN`, see the
/// module doc). Whoever reads the open or the close asks [`open_of`] / [`close_of`].
pub(crate) fn bar_ok(b: &ChartCandle) -> bool {
    let ok = |v: f32| v.is_finite() && v > 0.0;
    let known_or_unknown = |v: f32| v.is_nan() || ok(v);
    b.t_open_ms.is_finite()
        && ok(b.high)
        && ok(b.low)
        && b.high >= b.low
        && known_or_unknown(b.open)
        && known_or_unknown(b.close)
}

/// The bar's close when it is known; `None` for a bar whose source carried none (module doc).
pub(crate) fn close_of(b: &ChartCandle) -> Option<f64> {
    positive(f64::from(b.close))
}

/// The bar's open when it is known; `None` for a bar whose source carried none (module doc).
pub(crate) fn open_of(b: &ChartCandle) -> Option<f64> {
    positive(f64::from(b.open))
}

/// The side of `line` bar `b` CLOSED on — the one question every break and bar event asks.
///
/// A known close answers it ([`side_of`]). A bar without one (a ring bar, see the module doc) still
/// closed somewhere inside its own range, so when the WHOLE range lies on one side of the line
/// (`low > line`, `high < line`) the close did too and that side is the answer, as it would be for
/// a real candle of that range whatever its close. A range that reaches or straddles the line
/// leaves the side unknown: `None`, as exactly on the line. Without it a market the cache knows
/// only from the ring never broke anything — its levels hung unbroken over price that had left
/// them by a whole bar. `None` too when `line` is not a number.
pub(crate) fn close_side(b: &ChartCandle, line: f64) -> Option<Side> {
    if let Some(close) = close_of(b) {
        return side_of(close, line);
    }
    if f64::from(b.low) > line {
        Some(Side::Above)
    } else if f64::from(b.high) < line {
        Some(Side::Below)
    } else {
        None
    }
}

/// The side of `line` bar `b` closed on once the break buffer is applied — what a break episode
/// and the bar events read.
///
/// A close past the line on a side the figure did NOT hold price on must clear it by `buffer_frac`
/// of the line's price (`line · (1 ± buffer_frac)`, by [`close_side`]) to count as that side; a
/// close on the `origin` side counts at once. With no origin (a line without a held side) both
/// sides need the buffer. A close inside the buffer has no side: it neither breaks, nor returns,
/// nor holds — like a close exactly on the line. A zero buffer is [`close_side`] itself.
pub(crate) fn line_side(
    b: &ChartCandle,
    line: f64,
    origin: Option<Side>,
    buffer_frac: f64,
) -> Option<Side> {
    let buf = |side: Side| {
        if origin == Some(side) {
            0.0
        } else {
            buffer_frac
        }
    };
    if close_side(b, line * (1.0 + buf(Side::Above))) == Some(Side::Above) {
        Some(Side::Above)
    } else if close_side(b, line * (1.0 - buf(Side::Below))) == Some(Side::Below) {
        Some(Side::Below)
    } else {
        None
    }
}

/// Where price was at the end of `bars`, for what needs only "about where": the newest valid bar's
/// close, or the middle of its range when its close is unknown — the close lies inside that range,
/// so the middle is off by at most half of one bar. `None` when no bar is valid.
///
/// Not for anything a close DECIDES (a break, an event): those ask [`close_side`], which answers
/// from a known close or a range wholly on one side, and leave every other bar out — a middle of the
/// range is a guess, never a side.
pub fn last_price(bars: &[ChartCandle]) -> Option<f64> {
    bars.iter().rev().find(|b| bar_ok(b)).and_then(price_of)
}

/// One valid bar's [`last_price`]: its close when known, else the middle of its range.
pub(crate) fn price_of(b: &ChartCandle) -> Option<f64> {
    close_of(b).or_else(|| positive((f64::from(b.high) + f64::from(b.low)) / 2.0))
}

/// The 5-minute base of a market, assembled into `out` from the cache's two kinds: `real`, the
/// real candles (kind 5), and `ring`, the core's ring rows (`kline_cache::RING_5M_KIND`) — the
/// bar assembly's first step (see the module doc).
///
/// - A real candle is taken as it is, when every price of it is a positive finite number and
///   `high >= low`; a broken one is dropped and does not hold its bucket.
/// - A ring row fills only a 5-minute bucket no real candle holds, and comes out with an UNKNOWN
///   open and close (`NaN`), its high, low and volumes kept — by its source, whatever its shape. A
///   ring row whose high or low is unreadable is dropped.
///
/// `out` is in ascending open time, each bucket once (both kinds key their rows by open time, so
/// neither repeats one), whatever order the two inputs come in. Cost O((r + g) log(r + g)), and
/// linear when both are already sorted, as the cache returns them.
pub fn with_ring_rows(real: &[ChartCandle], ring: &[ChartCandle], out: &mut Vec<ChartCandle>) {
    out.clear();
    let price = |v: f32| v.is_finite() && v > 0.0;
    let bucket = |b: &ChartCandle| (b.t_open_ms as i64).div_euclid(KIND_5M_MS);
    out.extend(real.iter().copied().filter(|b| {
        b.t_open_ms.is_finite()
            && [b.open, b.high, b.low, b.close].into_iter().all(price)
            && b.high >= b.low
    }));
    let mut held: Vec<i64> = out.iter().map(bucket).collect();
    held.sort_unstable();
    let before = out.len();
    out.extend(
        ring.iter()
            .filter(|b| b.t_open_ms.is_finite() && price(b.high) && price(b.low) && b.high >= b.low)
            .filter(|b| held.binary_search(&bucket(b)).is_err())
            .map(|b| ChartCandle {
                open: f32::NAN,
                close: f32::NAN,
                ..*b
            }),
    );
    if out.len() > before || !out.is_sorted_by(|a, b| a.t_open_ms <= b.t_open_ms) {
        out.sort_by(|a, b| a.t_open_ms.total_cmp(&b.t_open_ms));
    }
}

/// `m5` (5-minute rows, ascending — the bar assembly's base, [`with_ring_rows`], real candles and
/// ring rows alike) resampled to `tf_ms` into `out`, without the buckets that hold an empty stretch
/// longer than [`coverage::MAX_GAP_MS`]: those are holes, not bars.
///
/// A row exists only where something traded, so a quiet market's bucket has empty slots and is
/// still the bar of what traded in it. A bucket the terminal and the core both missed for hours is
/// not: its high and low are those of the minutes that were recorded, and a pivot, a swing, an ATR
/// or a day's extremes read from it see a market that never turned there. The line between the two
/// is the one [`coverage`] draws for a calendar period — no empty stretch longer than an hour from
/// the bucket's start to its first row, between two rows, or from its last row's close to its end
/// — applied to each bucket, so a hole inside a 4-hour or daily bar is seen the way a hole between
/// 5-minute bars is.
///
/// The one resample of the 5-minute base for every reader of it — the chart's rows and the
/// screener's scan — so both drop the same buckets. A bucket no longer than the allowed stretch
/// cannot hold one, so `tf_ms` up to an hour passes every bucket. The bucket still forming is
/// judged like the others and dropped, as the readers drop it anyway. Buckets are cut exactly as
/// `moon_core::market::candles::resample` cuts them. Cost O(n).
pub fn resample_5m(m5: &[ChartCandle], tf_ms: i64, out: &mut Vec<ChartCandle>) {
    resample(m5, tf_ms, out);
    if tf_ms as f64 <= coverage::MAX_GAP_MS {
        return;
    }
    let tf = tf_ms as f64;
    let step = KIND_5M_MS as f64;
    // `resample` opens a bucket with every row whose bucket differs from the last one opened, so
    // the rows come in the same runs as `out`'s buckets: each bucket takes the row that opened it,
    // then every following row of the same bucket.
    let mut rows = m5.iter().peekable();
    let mut opens = Vec::new();
    out.retain(|bucket| {
        opens.clear();
        if let Some(r) = rows.next() {
            opens.push(r.t_open_ms);
        }
        while let Some(r) = rows.next_if(|r| bucket_open_ms(r.t_open_ms, tf_ms) == bucket.t_open_ms)
        {
            opens.push(r.t_open_ms);
        }
        let end = bucket.t_open_ms + tf;
        coverage::covered(
            opens.iter().copied(),
            bucket.t_open_ms,
            Some(end),
            end,
            step,
        )
    });
}

/// The most recent `lookback` bars — all of them when fewer exist, none for `lookback = 0`.
pub(crate) fn window(bars: &[ChartCandle], lookback: u32) -> &[ChartCandle] {
    let n = usize::try_from(lookback)
        .unwrap_or(usize::MAX)
        .min(bars.len());
    &bars[bars.len() - n..]
}

/// A percentage parameter as a fraction of price; anything not a positive finite number reads as
/// zero, so a NaN or negative tolerance never widens a band or flips a comparison.
pub(crate) fn pct_frac(pct: f32) -> f64 {
    if pct.is_finite() && pct > 0.0 {
        f64::from(pct) / 100.0
    } else {
        0.0
    }
}

/// Quote volume of the bars `touches` were stamped on: each touch's bar found by its open time in
/// `bars` (sorted ascending, as every detector's input is), each bar counted once. A touch whose
/// bar is not in `bars`, and a volume that is not a non-negative finite number, add nothing.
/// Cost O(T log n).
pub(crate) fn touch_volume(bars: &[ChartCandle], touches: &[Touch]) -> f64 {
    let mut times: Vec<f64> = touches.iter().map(|t| t.t_ms).collect();
    times.sort_by(f64::total_cmp);
    times.dedup();
    times
        .into_iter()
        .filter_map(|t| {
            let ix = bars.binary_search_by(|b| b.t_open_ms.total_cmp(&t)).ok()?;
            let v = f64::from(bars[ix].quote_volume);
            (v.is_finite() && v > 0.0).then_some(v)
        })
        .sum()
}

/// The Average True Range over the last `period` true ranges of the valid bars of `bars`: the
/// SIMPLE mean, a bar's true range being `max(high, prev_close) - min(low, prev_close)` with
/// `prev_close` the close of the nearest earlier VALID bar — or the bar's own `high - low` when
/// that close is unknown (a ring bar, see the module doc): a range-only close would stretch the
/// range to wherever the ring put it. So it reads the last `period + 1` valid bars and nothing
/// older (see `moon_core::config::auto_figures::ATR_PERIOD` for why a simple mean).
///
/// Returns `None` for `period = 0`, for fewer than `period + 1` valid bars, and for a result that
/// is not a positive finite number — a flat series has no range to measure a tolerance in.
/// Cost O(period) plus the unreadable bars skipped at the tail.
pub fn atr(bars: &[ChartCandle], period: u32) -> Option<f64> {
    let period = usize::try_from(period).ok().filter(|&p| p > 0)?;
    let mut valid = bars.iter().rev().filter(|b| bar_ok(b));
    // Newest first: each bar pairs with the NEXT one in this order, its previous close.
    let mut newer = valid.next()?;
    let mut sum = 0.0;
    for _ in 0..period {
        let older = valid.next()?;
        let (high, low) = (f64::from(newer.high), f64::from(newer.low));
        sum += match close_of(older) {
            Some(prev_close) => high.max(prev_close) - low.min(prev_close),
            None => high - low,
        };
        newer = older;
    }
    positive(sum / period as f64)
}

/// The touch tolerance a row groups by, as a FRACTION of price, in the unit the row chose.
///
/// `Pct` reads `pct` (`pct_frac`); `Atr` reads `atr_share` as a multiple of `atr` divided by
/// [`last_price`] of `bars` — once per run, so every touch of the run is grouped by the same
/// band. `None` under `Atr` when there is no ATR (`atr` is `None`) or no valid bar: the detector
/// then finds nothing rather than silently falling back to a percentage the row did not ask for.
/// A share that is not a positive finite number reads as zero, like a bad percentage.
///
/// A positive tolerance is raised to [`TICK_FLOOR_STEPS`] of `price_tick` (the exchange's tick,
/// [`DetectCtx::price_tick`]) as a fraction of [`last_price`]: a band narrower than the grid
/// prices stand on cannot group anything. A zero tolerance stays zero — it means "no band", which
/// the tick does not change — and so does every tolerance when the tick is unknown or not a
/// positive finite number.
pub(crate) fn tolerance_frac(
    unit: ToleranceUnit,
    pct: f32,
    atr_share: f32,
    atr: Option<f64>,
    bars: &[ChartCandle],
    price_tick: Option<f64>,
) -> Option<f64> {
    let frac = match unit {
        ToleranceUnit::Pct => pct_frac(pct),
        ToleranceUnit::Atr => {
            let atr = atr?;
            let close = last_price(bars)?;
            let share = if atr_share.is_finite() && atr_share > 0.0 {
                f64::from(atr_share)
            } else {
                0.0
            };
            let frac = share * atr / close;
            if !frac.is_finite() {
                return None;
            }
            frac
        }
    };
    if !(frac > 0.0) {
        return Some(frac);
    }
    let floor = price_tick
        .and_then(positive)
        .zip(last_price(bars))
        .map_or(0.0, |(tick, price)| TICK_FLOOR_STEPS * tick / price);
    Some(if floor.is_finite() {
        frac.max(floor)
    } else {
        frac
    })
}

/// `price` lies within `frac` of `line`, measured as a fraction of the LINE's price.
pub(crate) fn within(price: f64, line: f64, frac: f64) -> bool {
    line > 0.0 && (price - line).abs() <= line * frac
}

/// The break episodes after `from_idx`, summarised: the LATEST one — the first close of the most
/// recent run of closes beyond the line on the side opposite `origin` — with the bars since that
/// close and the length of the LONGEST episode.
///
/// An episode ends at a close back on `origin`'s side; a close beyond after that starts a new one,
/// so a level that broke, came back and broke again reports its SECOND break as the latest — and
/// still remembers how long the first one lasted ([`Broken::longest_bars`]). A bar's side is
/// [`line_side`] with `buffer_frac` (normally [`break_buffer`] of the figure's tolerance): a close
/// past the line by no more than the buffer, a close exactly on it, and a bar whose close is
/// unknown (module doc) with a range reaching the shifted line neither end nor start an episode;
/// such a bar still counts in `bars_since` and in an episode's length, which are time passing.
/// `line_at` gives the line's price at a bar's open time; a bar where it gives `None` is skipped.
/// Cost O(bars after `from_idx`).
pub(crate) fn scan_break(
    bars: &[ChartCandle],
    from_idx: usize,
    origin: Side,
    buffer_frac: f64,
    line_at: impl Fn(f64) -> Option<f64>,
) -> Option<Broken> {
    let mut broken_at: Option<usize> = None;
    // Start of the episode still open, if one is.
    let mut open: Option<usize> = None;
    let mut longest = 0usize;
    for (k, b) in bars.iter().enumerate().skip(from_idx.saturating_add(1)) {
        if !bar_ok(b) {
            continue;
        }
        let Some(line) = line_at(b.t_open_ms) else {
            continue;
        };
        let Some(side) = line_side(b, line, Some(origin), buffer_frac) else {
            continue;
        };
        if side == origin {
            if let Some(start) = open.take() {
                longest = longest.max(k - start);
            }
        } else if open.is_none() {
            open = Some(k);
            broken_at = Some(k);
        }
    }
    let at = broken_at?;
    let last = bars.len().saturating_sub(1);
    if let Some(start) = open {
        longest = longest.max(last - start);
    }
    Some(Broken {
        at_ms: bars[at].t_open_ms,
        bars_since: u32::try_from(last - at).unwrap_or(u32::MAX),
        longest_bars: u32::try_from(longest).unwrap_or(u32::MAX),
    })
}

/// Whether a figure with this break survives `policy`: always when unbroken; never under
/// `Remove`; under `Keep { bars }` while no break episode has lasted more than `bars` bars
/// ([`Broken::longest_bars`]) — a close back on the original side within them was a false break
/// and the figure stays, a stay beyond longer than that drops it for good, however price moves
/// afterwards.
pub(crate) fn survives(broken: Option<Broken>, policy: BrokenPolicy) -> bool {
    match (broken, policy) {
        (None, _) => true,
        (Some(_), BrokenPolicy::Remove) => false,
        (Some(b), BrokenPolicy::Keep { bars }) => b.longest_bars <= u32::from(bars),
    }
}

/// A rank that grows with the touch count first and the figure's age second: `touches` plus the
/// fraction of the searched window the touches span, which is below one, so one more touch always
/// outranks any longer span.
pub(crate) fn touch_strength(touches: usize, first_idx: usize, last_idx: usize, len: usize) -> f32 {
    let span = if len > 0 {
        last_idx.saturating_sub(first_idx) as f64 / len as f64
    } else {
        0.0
    };
    (touches as f64 + span.min(0.999)) as f32
}
