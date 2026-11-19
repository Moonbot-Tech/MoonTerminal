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
//! A bar with a non-finite or non-positive price, or `high < low`, is skipped wherever it would be
//! read — it never panics a detector and never becomes a touch.

use moon_core::config::auto_figures::{BrokenPolicy, DetectorKind, DetectorParams, ToleranceUnit};
use moon_core::market::candles::ChartCandle;

pub mod confluence;
pub mod draw;
pub mod events;
pub mod extremes;
pub mod formations;
pub mod period_levels;
pub mod pivots;
pub mod round_levels;
pub mod scan;
pub mod squeeze;
pub mod swing;
pub mod take;
pub mod tf;
pub mod trend;
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

/// A figure — a horizontal level, a squeeze's level or a sloped trend line — that a bar has
/// CLOSED beyond after its last touch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Broken {
    /// Open time of the first bar of the LATEST break episode: a close back on the original side
    /// ends an episode, and the next close beyond starts a new one (see `scan_break`).
    pub at_ms: f64,
    /// Closed bars after the breaking one, up to the last bar of the input.
    pub bars_since: u32,
    /// The last valid close is back on the figure's original side — the break has, so far, been a
    /// false one.
    pub returned: bool,
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
    /// closed bar, came within the row's `pressed_pct` of the level (or past it).
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
    /// A pair has two lines and no primary one; the upper is the answer where no reference price
    /// says which matters (a caption placed on the line, a sort key). Wherever a reference price
    /// exists — which side of price a figure is on, how far it is, whether price approaches it —
    /// the caller asks [`FoundShape::price_near`] instead, which picks the nearer side.
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
        let FoundShape::Pair { upper, lower } = self else {
            return self.price_at(t_ms);
        };
        match (upper.price_at(t_ms), lower.price_at(t_ms)) {
            (Some(u), Some(l)) => {
                if ref_price.is_finite() && (l - ref_price).abs() < (u - ref_price).abs() {
                    Some(l)
                } else {
                    Some(u)
                }
            }
            (u, l) => u.or(l),
        }
    }
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
}

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

/// A bar every price of which can be read: finite, positive, `high >= low`, finite open time.
pub(crate) fn bar_ok(b: &ChartCandle) -> bool {
    let ok = |v: f32| v.is_finite() && v > 0.0;
    b.t_open_ms.is_finite()
        && ok(b.open)
        && ok(b.high)
        && ok(b.low)
        && ok(b.close)
        && b.high >= b.low
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
/// `prev_close` the close of the nearest earlier VALID bar. So it reads the last `period + 1` valid
/// bars and nothing older (see `moon_core::config::auto_figures::ATR_PERIOD` for why a simple mean).
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
        let prev_close = f64::from(older.close);
        let hi = f64::from(newer.high).max(prev_close);
        let lo = f64::from(newer.low).min(prev_close);
        sum += hi - lo;
        newer = older;
    }
    positive(sum / period as f64)
}

/// The touch tolerance a row groups by, as a FRACTION of price, in the unit the row chose.
///
/// `Pct` reads `pct` (`pct_frac`); `Atr` reads `atr_share` as a multiple of `atr` divided by the
/// last valid close of `bars` — once per run, so every touch of the run is grouped by the same
/// band. `None` under `Atr` when there is no ATR (`atr` is `None`) or no valid close: the detector
/// then finds nothing rather than silently falling back to a percentage the row did not ask for.
/// A share that is not a positive finite number reads as zero, like a bad percentage.
pub(crate) fn tolerance_frac(
    unit: ToleranceUnit,
    pct: f32,
    atr_share: f32,
    atr: Option<f64>,
    bars: &[ChartCandle],
) -> Option<f64> {
    match unit {
        ToleranceUnit::Pct => Some(pct_frac(pct)),
        ToleranceUnit::Atr => {
            let atr = atr?;
            let close = bars
                .iter()
                .rev()
                .find(|b| bar_ok(b))
                .and_then(|b| positive(f64::from(b.close)))?;
            let share = if atr_share.is_finite() && atr_share > 0.0 {
                f64::from(atr_share)
            } else {
                0.0
            };
            let frac = share * atr / close;
            frac.is_finite().then_some(frac)
        }
    }
}

/// `price` lies within `frac` of `line`, measured as a fraction of the LINE's price.
pub(crate) fn within(price: f64, line: f64, frac: f64) -> bool {
    line > 0.0 && (price - line).abs() <= line * frac
}

/// The LATEST break episode after `from_idx`: the first close of the most recent run of closes
/// beyond the line on the side opposite `origin`, with the bars since that close and whether the
/// last valid close is back on `origin`'s side.
///
/// An episode ends at a close back on `origin`'s side; a close beyond after that starts a new one,
/// so a level that broke, came back and broke again reports its SECOND break — the one a
/// `Keep { bars }` countdown must run from. A close exactly on the line neither ends nor starts an
/// episode. `line_at` gives the line's price at a bar's open time; a bar where it gives `None` is
/// skipped. Cost O(bars after `from_idx`).
pub(crate) fn scan_break(
    bars: &[ChartCandle],
    from_idx: usize,
    origin: Side,
    line_at: impl Fn(f64) -> Option<f64>,
) -> Option<Broken> {
    let mut broken_at: Option<usize> = None;
    let mut beyond = false;
    let mut last_side: Option<Side> = None;
    for (k, b) in bars.iter().enumerate().skip(from_idx.saturating_add(1)) {
        if !bar_ok(b) {
            continue;
        }
        let Some(line) = line_at(b.t_open_ms) else {
            continue;
        };
        let Some(side) = side_of(f64::from(b.close), line) else {
            continue;
        };
        last_side = Some(side);
        if side == origin {
            beyond = false;
        } else if !beyond {
            beyond = true;
            broken_at = Some(k);
        }
    }
    let at = broken_at?;
    let last = bars.len().saturating_sub(1);
    Some(Broken {
        at_ms: bars[at].t_open_ms,
        bars_since: u32::try_from(last - at).unwrap_or(u32::MAX),
        returned: last_side == Some(origin),
    })
}

/// Whether a figure with this break survives `policy`: always when unbroken; never under
/// `Remove`; under `Keep { bars }` while at most `bars` bars have closed after the break, or after
/// that once the last close is back on the original side.
pub(crate) fn survives(broken: Option<Broken>, policy: BrokenPolicy) -> bool {
    match (broken, policy) {
        (None, _) => true,
        (Some(_), BrokenPolicy::Remove) => false,
        (Some(b), BrokenPolicy::Keep { bars }) => b.bars_since <= u32::from(bars) || b.returned,
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
