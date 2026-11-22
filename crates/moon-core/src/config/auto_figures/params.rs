//! The detector-facing half of the auto-figure model: which detector a row runs, with which
//! parameters, on which timeframe, and which of the found lines it keeps.
//!
//! These types are the CONTRACT between the two crates that meet here. `moon-chart`'s detectors
//! (`moon_chart::auto_figures`) read them as plain values — pure functions over closed candles,
//! no I/O, no clock — and this crate persists them. They live in `moon-core` because the model has
//! to serialize them and `moon-chart` already depends on this crate, never the other way round.
//!
//! Units are part of every field's name or doc: a percentage is a percentage of PRICE (`0.3` means
//! 0.3 %), a share of the ATR is a multiple of the Average True Range over [`ATR_PERIOD`] bars
//! (`0.5` means half an ATR), a bar count is counted in bars of the row's OWN timeframe.
//!
//! Every struct is `#[serde(default)]`: a field added later loads as its default from an older
//! file, and a value missing from a hand-edited one does not drop the whole row.
//!
//! Extending the contract without costing an older build anything: a new DETECTOR is a new
//! [`DetectorKind`] (an older build keeps a kind it does not know verbatim, `RowDetector::Unknown`),
//! and a new OPTION of a known type is a new FIELD (an older build ignores a field it does not
//! know). A new VALUE of an existing field's enum is neither: an older build reading it fails that
//! field and resets it to its default, which is a silent rewrite of what the user chose.

use serde::{Deserialize, Serialize};

/// A timeframe a row can be pinned to.
///
/// The set matches what the candle pipeline can actually produce without asking the core for a
/// second kline kind: the chart's own series, or the local 5-minute cache resampled
/// (`market::candles::resample`), or the cache's daily rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TfId {
    M1,
    M5,
    M15,
    M30,
    H1,
    H4,
    D1,
}

impl TfId {
    /// Every timeframe, smallest first — the order a picker lists them in and `Auto` walks.
    pub const ALL: [TfId; 7] = [
        TfId::M1,
        TfId::M5,
        TfId::M15,
        TfId::M30,
        TfId::H1,
        TfId::H4,
        TfId::D1,
    ];

    /// Bar length in milliseconds.
    pub const fn ms(self) -> i64 {
        const MIN: i64 = 60_000;
        match self {
            TfId::M1 => MIN,
            TfId::M5 => 5 * MIN,
            TfId::M15 => 15 * MIN,
            TfId::M30 => 30 * MIN,
            TfId::H1 => 60 * MIN,
            TfId::H4 => 240 * MIN,
            TfId::D1 => 1_440 * MIN,
        }
    }

    /// The timeframe whose bar is exactly `ms` long, if it is one of ours.
    pub fn from_ms(ms: i64) -> Option<TfId> {
        Self::ALL.into_iter().find(|tf| tf.ms() == ms)
    }
}

/// Where a row takes its timeframe from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "mode", content = "tf")]
pub enum TfSource {
    /// Chosen from the chart's visible time span, with hysteresis, so zooming changes the scale of
    /// the levels the way it changes the scale of the candles.
    #[default]
    Auto,
    /// The same timeframe at every zoom — how "1h levels in one colour, 4h in another" is built.
    Fixed(TfId),
}

/// What happens to a level or a line once a bar has CLOSED beyond it — a horizontal `Swing`
/// level, the level of a `Squeeze`, and a sloped `Trend` line alike.
///
/// A per-row choice (LinKvo, 18.11.2026): removing it at once hides a false breakout, keeping it
/// for a while shows one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum BrokenPolicy {
    /// Dropped on the first close beyond it.
    Remove,
    /// Kept for `bars` more bars after the break, flagged as broken so it can be drawn in a
    /// weaker style. A close back on the original side within those bars was a false break and
    /// the line stays; a stay beyond longer than `bars` drops it for good, even when price comes
    /// back later (`moon_chart::auto_figures::Broken::longest_bars`).
    Keep { bars: u16 },
}

impl Default for BrokenPolicy {
    fn default() -> Self {
        BrokenPolicy::Keep { bars: 3 }
    }
}

/// Bars the Average True Range of a [`ToleranceUnit::Atr`] tolerance is taken over, in bars of
/// the row's OWN timeframe.
///
/// The ATR is the SIMPLE mean of the last `ATR_PERIOD` true ranges of the closed bars a detector
/// is handed, a bar's true range being `max(high, prev_close) - min(low, prev_close)`; so it reads
/// the last `ATR_PERIOD + 1` bars and nothing older. A simple mean rather than Wilder's smoothing
/// on purpose: Wilder's value depends on every bar since the start of the series, so the chart
/// (which fetches a row's lookback) and the all-coins scanner (which may fetch a different depth
/// of the same cache) would group the same pivots with different tolerances. The simple mean gives
/// both the same number from the same last bars. 14 is the period an ATR is quoted with almost
/// everywhere.
pub const ATR_PERIOD: u32 = 14;

/// What a row's touch tolerance is measured in.
///
/// A per-row choice: a percentage of price is the same band on every coin, too narrow on a
/// volatile one and too wide on a quiet one; a share of the ATR follows the coin's own swing.
/// A field of its own rather than a reading of which number is set, so switching units back and
/// forth keeps both values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToleranceUnit {
    /// The `*_pct` fields apply, in % of price.
    #[default]
    Pct,
    /// The `*_atr` fields apply, as a share of the ATR over [`ATR_PERIOD`] bars (`0.5` = half an
    /// ATR).
    ///
    /// The detector converts each into a fraction of price ONCE per run, dividing by the last
    /// valid closed bar's close, and reports the one it grouped touches by in
    /// `moon_chart::auto_figures::Found::tolerance_frac` — which is what the zone band is drawn
    /// from. With fewer than `ATR_PERIOD + 1` valid bars there is no ATR, and the detector finds
    /// nothing rather than guessing a band.
    Atr,
}

/// `Swing`: horizontal levels from pivots — local highs and lows whose prices cluster within a
/// tolerance and have been touched at least `min_touches` times.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SwingParams {
    /// How many of the most recent closed bars are searched.
    pub lookback_bars: u32,
    /// Bars on each side a pivot must out-rank (a pivot high is the highest of
    /// `pivot_left + 1 + pivot_right` bars).
    pub pivot_left: u8,
    pub pivot_right: u8,
    /// Which of the two tolerance fields below applies. Added after the first files were written:
    /// an older row loads as `Pct`, its old behaviour.
    pub tolerance_unit: ToleranceUnit,
    /// Two pivots belong to the same level when their prices differ by at most this, in % of price.
    /// Read under [`ToleranceUnit::Pct`].
    pub tolerance_pct: f32,
    /// The same distance as a share of the ATR (`0.5` = half an ATR over [`ATR_PERIOD`] bars).
    /// Read under [`ToleranceUnit::Atr`].
    pub tolerance_atr: f32,
    /// Fewest pivots a level needs to be drawn.
    pub min_touches: u8,
    pub broken: BrokenPolicy,
}

impl Default for SwingParams {
    fn default() -> Self {
        Self {
            lookback_bars: 200,
            pivot_left: 3,
            pivot_right: 3,
            tolerance_unit: ToleranceUnit::Pct,
            tolerance_pct: 0.25,
            tolerance_atr: 0.5,
            min_touches: 2,
            broken: BrokenPolicy::default(),
        }
    }
}

/// Which period a `PeriodExtreme` row reads: a UTC calendar day or week, or a rolling window of the
/// last hours.
///
/// The rolling windows are what crypto, which trades around the clock, reads by default (LinKvo,
/// 20.11.2026): "the last 24 hours" is equally full at any moment, where "today" is one hour long
/// right after UTC midnight — and 24 hours always fit the core's 41-hour 5-minute ring. The
/// calendar periods stay for the reader who trades from the daily candle, whose high and low every
/// exchange draws from 00:00 UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExtremePeriod {
    /// The UTC day of the caller's reference time (`moon_chart::auto_figures::DetectCtx::now_ms`),
    /// from its CLOSED bars only: the bar still forming is not in the range, so a new extreme it
    /// prints lags by at most one bar of the row's timeframe. Right after UTC midnight, before a
    /// bar of the new day has closed, this is nothing — not yesterday.
    #[default]
    Day,
    /// The complete UTC day before the reference time's.
    PrevDay,
    /// The ISO week (Monday 00:00 UTC onward) of the reference time, from its closed bars only,
    /// like `Day`.
    Week,
    /// The complete ISO week before the reference time's.
    PrevWeek,
    /// The last 4 hours before the reference time, from closed bars: a window that moves with the
    /// clock, so its high or low steps when the bar that set it leaves the window.
    Last4h,
    /// The last 12 hours, like [`Self::Last4h`].
    Last12h,
    /// The last 24 hours, like [`Self::Last4h`] — the exchanges' own "24h high / low".
    Last24h,
    /// The last 48 hours, like [`Self::Last4h`]. Longer than the core's ring: complete only once the
    /// candle cache holds the hours before it.
    Last48h,
}

impl ExtremePeriod {
    /// The window's length in hours for a rolling period, `None` for a calendar one.
    pub fn rolling_hours(self) -> Option<u32> {
        match self {
            ExtremePeriod::Day
            | ExtremePeriod::PrevDay
            | ExtremePeriod::Week
            | ExtremePeriod::PrevWeek => None,
            ExtremePeriod::Last4h => Some(4),
            ExtremePeriod::Last12h => Some(12),
            ExtremePeriod::Last24h => Some(24),
            ExtremePeriod::Last48h => Some(48),
        }
    }
}

/// Which side of the period's range a row draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExtremeWhich {
    High,
    Low,
    #[default]
    Both,
}

/// `PeriodExtreme`: the high and/or low of a calendar period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PeriodExtremeParams {
    pub period: ExtremePeriod,
    pub which: ExtremeWhich,
}

/// Which side of price a sloped line may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrendSide {
    /// Through pivot lows, below price.
    Support,
    /// Through pivot highs, above price.
    Resistance,
    #[default]
    Both,
}

/// `Trend`: sloped lines through pivots.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrendParams {
    pub lookback_bars: u32,
    pub pivot_left: u8,
    pub pivot_right: u8,
    pub side: TrendSide,
    /// Which of the two tolerance fields below applies; an older row loads as `Pct`.
    pub tolerance_unit: ToleranceUnit,
    /// A pivot counts as a touch when it lies within this distance of the line, in % of price.
    /// Read under [`ToleranceUnit::Pct`].
    pub tolerance_pct: f32,
    /// The same distance as a share of the ATR (`0.3` = three tenths of an ATR over
    /// [`ATR_PERIOD`] bars). Read under [`ToleranceUnit::Atr`].
    pub tolerance_atr: f32,
    /// Fewest pivots on the line, the two that define it included.
    pub min_touches: u8,
    /// How many bar BODIES between the first and the last touch may cross the line before it is
    /// rejected. Wicks never count.
    pub max_body_breaks: u8,
    /// Slope bounds, in % of price per bar, by absolute value: a near-flat line is a level, not a
    /// trend, and a near-vertical one is noise. Always in %, whatever `tolerance_unit` says.
    pub min_slope_pct_per_bar: f32,
    pub max_slope_pct_per_bar: f32,
    /// What happens to a line once a bar has CLOSED beyond it after its last touch — the same
    /// choice a level row makes. Added after the first files were written: `#[serde(default)]`
    /// on the struct loads a file without it as the default policy.
    pub broken: BrokenPolicy,
}

impl Default for TrendParams {
    fn default() -> Self {
        Self {
            lookback_bars: 150,
            pivot_left: 3,
            pivot_right: 3,
            side: TrendSide::Both,
            tolerance_unit: ToleranceUnit::Pct,
            tolerance_pct: 0.15,
            tolerance_atr: 0.3,
            min_touches: 3,
            max_body_breaks: 0,
            min_slope_pct_per_bar: 0.005,
            max_slope_pct_per_bar: 2.0,
            broken: BrokenPolicy::default(),
        }
    }
}

/// `Squeeze`: a horizontal level plus a sloped line on the OTHER side converging into it — rising
/// lows pressing a resistance, or falling highs pressing a support. One formation, one figure.
///
/// ONE unit for both halves: `level.tolerance_unit` decides whether the level reads
/// `level.tolerance_pct` or `level.tolerance_atr`, AND whether the sloped half reads
/// `trend_tolerance_pct` or `trend_tolerance_atr`. A formation measured half in % and half in ATR
/// is not a choice anybody makes, and one switch cannot disagree with itself.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SqueezeParams {
    /// The horizontal half, found the way `Swing` finds it. Its `tolerance_unit` is the
    /// formation's unit.
    pub level: SwingParams,
    /// Fewest pivots on the converging sloped line.
    pub min_trend_touches: u8,
    /// Touch tolerance of the sloped line, in % of price. Read when `level.tolerance_unit` is
    /// [`ToleranceUnit::Pct`].
    pub trend_tolerance_pct: f32,
    /// Touch tolerance of the sloped line as a share of the ATR over [`ATR_PERIOD`] bars. Read
    /// when `level.tolerance_unit` is [`ToleranceUnit::Atr`].
    pub trend_tolerance_atr: f32,
    /// The formation counts as "pressed" once the sloped line, projected to the last closed bar,
    /// is within this distance of the level, in % of price — always %, whatever the unit — while
    /// that bar closed inside the formation (not beyond the level, nor beyond the line).
    pub pressed_pct: f32,
}

impl Default for SqueezeParams {
    fn default() -> Self {
        Self {
            level: SwingParams {
                min_touches: 2,
                ..SwingParams::default()
            },
            min_trend_touches: 3,
            trend_tolerance_pct: 0.2,
            trend_tolerance_atr: 0.4,
            pressed_pct: 0.5,
        }
    }
}

/// Which span a `VolumeProfile` row builds its profile over: a UTC day, or a rolling window of the
/// last hours (why both: [`ExtremePeriod`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProfilePeriod {
    /// The UTC day of the reference time, from its CLOSED bars only — like `ExtremePeriod::Day`,
    /// it grows bar by bar and is nothing right after midnight.
    #[default]
    Day,
    /// The complete UTC day before the reference time's.
    PrevDay,
    /// The last 4 hours before the reference time, from closed bars; the profile moves with the
    /// clock as old bars leave the window and new ones close.
    Last4h,
    /// The last 12 hours, like [`Self::Last4h`].
    Last12h,
    /// The last 24 hours, like [`Self::Last4h`].
    Last24h,
    /// The last 48 hours, like [`Self::Last4h`]; longer than the core's ring.
    Last48h,
}

impl ProfilePeriod {
    /// The window's length in hours for a rolling period, `None` for a calendar day.
    pub fn rolling_hours(self) -> Option<u32> {
        match self {
            ProfilePeriod::Day | ProfilePeriod::PrevDay => None,
            ProfilePeriod::Last4h => Some(4),
            ProfilePeriod::Last12h => Some(12),
            ProfilePeriod::Last24h => Some(24),
            ProfilePeriod::Last48h => Some(48),
        }
    }
}

/// `VolumeProfile`: levels from the volume traded at each price over a UTC day or a rolling window
/// of the last hours ([`ProfilePeriod`]) — its point of control (POC, the price bin with the most
/// volume) and the edges of its value area.
///
/// The profile is built by the detector itself (`moon_chart::auto_figures::volume_profile`): the
/// day's range is cut into `bins` equal bins and each bar's `quote_volume` is spread uniformly over
/// the bins its `[low, high]` range covers. Nothing else in the tree builds it — the chart's
/// horizontal volume (`moon_chart::hvol`) only selects and scales the trade-built `HVol` rows the
/// core sends. A bar with no volume adds nothing, so a period whose bars all report zero (a source
/// that does not know) has no profile and the row draws nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VolumeProfileParams {
    /// The span the profile is built over.
    pub period: ProfilePeriod,
    /// Draw the period's point of control.
    pub poc: bool,
    /// Draw the value area's high edge.
    pub vah: bool,
    /// Draw the value area's low edge.
    pub val: bool,
    /// Share of the period's volume the value area holds, in % (`70` = 70 %): grown from the POC
    /// outward, one bin at a time toward the side with more volume, until it holds this much.
    pub value_area_pct: f32,
    /// "Naked" POCs: the POC of each of the last `naked_poc_days` complete UTC days BEFORE the
    /// reference day that price has not traded at since that day ended. `0` = none. Independent of
    /// `period` and of `poc`: a row can draw naked POCs alone.
    pub naked_poc_days: u8,
    /// Price bins the period's range is cut into. More bins, a sharper POC on a wide range and a
    /// noisier one on a narrow range.
    pub bins: u16,
}

impl Default for VolumeProfileParams {
    fn default() -> Self {
        Self {
            period: ProfilePeriod::Day,
            poc: true,
            vah: true,
            val: true,
            value_area_pct: 70.0,
            naked_poc_days: 0,
            bins: 100,
        }
    }
}

/// `PeriodLevel`: the opening and closing prices of UTC calendar periods, each a horizontal level.
///
/// Days are UTC days and weeks ISO weeks from Monday 00:00 UTC, cut the way `PeriodExtreme` cuts
/// them. A period's open is the open of its first bar, its close the close of its last; a period
/// the input does not cover from its start (or, for a close, to its end) gives nothing for that
/// flag. Trading sessions (Asia / Europe / US) and an opening range are deliberately NOT here
/// (LinKvo, 19.11.2026: a crypto market trades around the clock).
///
/// A detector of its own rather than more `ExtremePeriod` values: an older build keeps an unknown
/// detector TYPE verbatim, but would reset an unknown VALUE of a known type's field to its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct PeriodLevelParams {
    /// The close of the UTC day before the reference day.
    pub prev_day_close: bool,
    /// The open of the reference UTC day.
    pub day_open: bool,
    /// The open of the reference ISO week.
    pub week_open: bool,
    /// The close of the ISO week before the reference one.
    pub prev_week_close: bool,
}

impl Default for PeriodLevelParams {
    fn default() -> Self {
        Self {
            prev_day_close: true,
            day_open: false,
            week_open: true,
            prev_week_close: false,
        }
    }
}

/// The step of a `RoundLevels` grid, derived from the price's order of magnitude.
///
/// With `major = 10^floor(log10(price))` — 1 for a price of 1..10, 0.01 for 0.01..0.1, 10 000 for
/// a bitcoin — the grid's step is that, its half, or its tenth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoundGrid {
    /// The order of magnitude itself: 1, 2, 3 … on a price of 3.4.
    Major,
    /// Half of it: 3.0, 3.5, 4.0 … on a price of 3.4.
    #[default]
    Half,
    /// A tenth of it: 3.3, 3.4, 3.5 … on a price of 3.4.
    Minor,
}

/// `RoundLevels`: round prices near the current one, where orders crowd because people type them.
///
/// How many of them are drawn is the row's `Take`, as for every other detector.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoundLevelsParams {
    /// The grid's step.
    pub grid: RoundGrid,
    /// Round prices are looked for within this distance of the last closed bar's close, in % of
    /// that close, on both sides.
    pub range_pct: f32,
}

impl Default for RoundLevelsParams {
    fn default() -> Self {
        Self {
            grid: RoundGrid::Half,
            range_pct: 10.0,
        }
    }
}

/// `Formation`: chart patterns made of two lines — triangles and wedges (two converging sloped
/// lines), channels (two parallel ones), flags and pennants (a short channel or triangle right
/// after an impulse), and double tops and bottoms (two equal extremes with a trough between).
///
/// The rules each kind is recognised by are `moon_chart::auto_figures::formations`'s; this struct
/// only names the switches and the numbers they read. One tolerance unit for the whole row, as in
/// `Squeeze`: `tolerance_unit` decides whether the touches read `tolerance_pct` or
/// `tolerance_atr`, AND whether the equality of a double's two extremes reads
/// `double_tolerance_pct` or `double_tolerance_atr`. Every other distance is always in %.
///
/// A detector TYPE of its own rather than a value of an existing field: an older build keeps an
/// unknown type verbatim (`RowDetector::Unknown`), but would reset an unknown value of a known
/// type's field to its default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FormationParams {
    /// Look for triangles: two converging lines sloping toward each other, or one of them
    /// horizontal (an ascending or descending triangle).
    pub triangle: bool,
    /// Look for wedges: two converging lines sloping the same way.
    pub wedge: bool,
    /// Look for channels: two parallel lines, horizontal (a range) or sloped.
    pub channel: bool,
    /// Look for flags and pennants: a short channel or converging pair right after an impulse.
    /// With this off, such a pair is still reported as the channel or triangle it is, when that
    /// kind is on.
    pub flag: bool,
    /// Look for double tops and double bottoms.
    pub double: bool,
    /// How many of the most recent closed bars are searched.
    pub lookback_bars: u32,
    /// Bars on each side a pivot must out-rank, as in [`SwingParams`].
    pub pivot_left: u8,
    pub pivot_right: u8,
    /// Which of the tolerance fields applies, for the touches and for a double's equality alike.
    pub tolerance_unit: ToleranceUnit,
    /// A pivot touches a side when it lies within this distance of the line, in % of price. Read
    /// under [`ToleranceUnit::Pct`].
    pub tolerance_pct: f32,
    /// The same distance as a share of the ATR over [`ATR_PERIOD`] bars. Read under
    /// [`ToleranceUnit::Atr`].
    pub tolerance_atr: f32,
    /// Fewest pivots on EACH side, the two that define the side included. At least two.
    pub min_touches: u8,
    /// Shortest formation, in bars from its first touch to its last (a double: from its first
    /// extreme to its second). Shorter is noise between two neighbouring swings.
    pub min_bars: u32,
    /// Two sides are parallel (a channel) when the formation's width — upper line minus lower
    /// line — changes by at most this over the stretch both sides share (from the later first
    /// touch to the earlier last touch), in % of the width at its start.
    /// A width that shrinks by more is a converging pair (a triangle or a wedge); one that grows
    /// by more is a broadening pattern, which is none of the kinds here and is not reported.
    pub parallel_width_pct: f32,
    /// A flag's pole: the net move INTO the formation — from the open of the `impulse_bars` bars
    /// ending at its first touch to that touch's bar — must reach this, in % of price.
    pub impulse_pct: f32,
    /// How many bars, the bar of the formation's first touch included, the pole is measured over.
    pub impulse_bars: u32,
    /// Longest a flag may be, in bars from its first touch to its last; a longer consolidation is
    /// the channel or triangle it is, not a flag.
    pub flag_max_bars: u32,
    /// A double's two extremes count as one level when they differ by at most this, in % of
    /// price. Read under [`ToleranceUnit::Pct`].
    pub double_tolerance_pct: f32,
    /// The same as a share of the ATR over [`ATR_PERIOD`] bars. Read under
    /// [`ToleranceUnit::Atr`].
    pub double_tolerance_atr: f32,
    /// Shallowest trough (peak, for a double bottom) between a double's two extremes, in % of the
    /// price of the extreme NEARER it — the lower of two tops, the higher of two bottoms. So the
    /// trough is at least this deep below BOTH tops (above both bottoms): measured from the higher
    /// top, a trough could pass the test while sitting less deep than that under the lower one,
    /// and the pattern the reader sees is only as deep as its shallower half.
    pub double_depth_pct: f32,
    /// What happens to a formation once a bar has CLOSED outside it after its last touch — beyond
    /// either line — the same choice a level row makes.
    pub broken: BrokenPolicy,
}

impl Default for FormationParams {
    fn default() -> Self {
        Self {
            triangle: true,
            wedge: true,
            channel: true,
            flag: true,
            double: true,
            lookback_bars: 150,
            // Narrower than a level's 3/3: a formation is drawn through the smaller swings inside
            // it, and a flag of fifteen bars holds few 3/3 pivots.
            pivot_left: 2,
            pivot_right: 2,
            tolerance_unit: ToleranceUnit::Pct,
            tolerance_pct: 0.2,
            tolerance_atr: 0.4,
            // Three a side, the classic five-to-six point pattern: with two a side, ANY two pairs
            // of swings make a "formation", and noise is full of them.
            min_touches: 3,
            min_bars: 15,
            parallel_width_pct: 20.0,
            impulse_pct: 3.0,
            impulse_bars: 10,
            flag_max_bars: 30,
            double_tolerance_pct: 0.5,
            double_tolerance_atr: 0.5,
            double_depth_pct: 1.5,
            broken: BrokenPolicy::default(),
        }
    }
}

/// The kind of detector a row runs — one variant per module under `moon_chart::auto_figures`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectorKind {
    Swing,
    PeriodExtreme,
    Trend,
    Squeeze,
    VolumeProfile,
    PeriodLevel,
    RoundLevels,
    Formation,
}

impl DetectorKind {
    /// Every kind, in the order the row editor lists them.
    pub const ALL: [DetectorKind; 8] = [
        DetectorKind::Swing,
        DetectorKind::PeriodExtreme,
        DetectorKind::Trend,
        DetectorKind::Squeeze,
        DetectorKind::VolumeProfile,
        DetectorKind::PeriodLevel,
        DetectorKind::RoundLevels,
        DetectorKind::Formation,
    ];

    /// The kind's parameters at their defaults — what a row switched to this kind starts with.
    pub fn default_params(self) -> DetectorParams {
        match self {
            DetectorKind::Swing => DetectorParams::Swing(SwingParams::default()),
            DetectorKind::PeriodExtreme => {
                DetectorParams::PeriodExtreme(PeriodExtremeParams::default())
            }
            DetectorKind::Trend => DetectorParams::Trend(TrendParams::default()),
            DetectorKind::Squeeze => DetectorParams::Squeeze(SqueezeParams::default()),
            DetectorKind::VolumeProfile => {
                DetectorParams::VolumeProfile(VolumeProfileParams::default())
            }
            DetectorKind::PeriodLevel => DetectorParams::PeriodLevel(PeriodLevelParams::default()),
            DetectorKind::RoundLevels => DetectorParams::RoundLevels(RoundLevelsParams::default()),
            DetectorKind::Formation => DetectorParams::Formation(FormationParams::default()),
        }
    }
}

/// One row's detector together with its parameters.
///
/// Internally tagged by `kind`. A kind this build does not know (a file written by a newer one)
/// never reaches this enum: the model's `wire` keeps such a row verbatim and writes it back, the
/// way `figures::store` keeps unknown figures.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DetectorParams {
    Swing(SwingParams),
    PeriodExtreme(PeriodExtremeParams),
    Trend(TrendParams),
    Squeeze(SqueezeParams),
    VolumeProfile(VolumeProfileParams),
    PeriodLevel(PeriodLevelParams),
    RoundLevels(RoundLevelsParams),
    Formation(FormationParams),
}

/// One UTC day, ms — what the calendar detectors' bar counts are sized by.
const DAY_MS: i64 = 86_400_000;

/// Bars of `tf` that cover `days` whole UTC days, rounded UP, plus one: the slack for a bar that
/// opens exactly at the period's boundary. An upper bound, never short.
fn bars_for_days(days: i64, tf: TfId) -> u32 {
    bars_for_span(days * DAY_MS, tf)
}

/// Bars of `tf` that cover a span of `span_ms` plus one (see [`bars_for_days`]).
fn bars_for_span(span_ms: i64, tf: TfId) -> u32 {
    // `TfId::ms` is a positive constant for every variant, so the division is safe; the ceiling
    // keeps a timeframe that does not divide the period from coming short.
    let tf_ms = tf.ms().max(1);
    let bars = (span_ms + tf_ms - 1) / tf_ms + 1;
    u32::try_from(bars).unwrap_or(u32::MAX)
}

/// Milliseconds in one hour, for the rolling windows' spans.
const HOUR_MS: i64 = 3_600_000;

/// A pivot detector's lookback, raised to what its ATR needs when its tolerance is in ATR.
fn with_atr(lookback: u32, unit: ToleranceUnit) -> u32 {
    match unit {
        ToleranceUnit::Pct => lookback,
        ToleranceUnit::Atr => lookback.max(ATR_PERIOD + 1),
    }
}

impl DetectorParams {
    /// The detector kind these parameters belong to.
    pub fn kind(&self) -> DetectorKind {
        match self {
            DetectorParams::Swing(_) => DetectorKind::Swing,
            DetectorParams::PeriodExtreme(_) => DetectorKind::PeriodExtreme,
            DetectorParams::Trend(_) => DetectorKind::Trend,
            DetectorParams::Squeeze(_) => DetectorKind::Squeeze,
            DetectorParams::VolumeProfile(_) => DetectorKind::VolumeProfile,
            DetectorParams::PeriodLevel(_) => DetectorKind::PeriodLevel,
            DetectorParams::RoundLevels(_) => DetectorKind::RoundLevels,
            DetectorParams::Formation(_) => DetectorKind::Formation,
        }
    }

    /// The row's broken-line policy — what the event layer's false-break window is read from, for
    /// every kind alike.
    ///
    /// The calendar, volume and round-price kinds have no policy of their own (their lines are a
    /// fact of the period, the profile or the price grid and are never dropped), so they answer
    /// [`BrokenPolicy::default`]: a false break of yesterday's high is then judged over the same
    /// few bars a level row uses by default.
    pub fn broken_policy(&self) -> BrokenPolicy {
        match self {
            DetectorParams::Swing(p) => p.broken,
            DetectorParams::Trend(p) => p.broken,
            DetectorParams::Squeeze(p) => p.level.broken,
            DetectorParams::Formation(p) => p.broken,
            DetectorParams::PeriodExtreme(_)
            | DetectorParams::VolumeProfile(_)
            | DetectorParams::PeriodLevel(_)
            | DetectorParams::RoundLevels(_) => BrokenPolicy::default(),
        }
    }

    /// The coarsest timeframe on which this detector can answer at all.
    ///
    /// Detectors read CLOSED bars only, so a row whose bar is as long as the period it measures
    /// never sees that period: on D1 the current day's bar is always still forming, and a `Day`
    /// row there would draw nothing for ever. So a current-day extreme and a day's open stop at
    /// H4. A rolling extremes window stops at H1: its start falls between bar boundaries, and a
    /// coarser first bar opens too late for its coverage. The four-hour window stops at M15, so it
    /// still holds sixteen bars. A volume
    /// profile stops at H1: it spreads each bar's volume over the bar's whole range, so a day of
    /// four-hour bars is six smears, not a profile. `D1` — no limit — for every other kind. Applied to a `Fixed` timeframe by `sanitize`, and to a resolved `Auto` one by the
    /// caller.
    pub fn coarsest_tf(&self) -> TfId {
        match self {
            DetectorParams::PeriodExtreme(PeriodExtremeParams {
                period: ExtremePeriod::Day,
                ..
            }) => TfId::H4,
            // A rolling window starts at an arbitrary moment, and its coverage needs a bar opening
            // within `coverage::MAX_GAP_MS` (an hour) of that start: so its bars are at most an
            // hour long, whatever its length — on H4 the first bar opens up to four hours late and
            // the window is reported as uncovered three times in four. Four hours of M15 still hold
            // sixteen bars.
            DetectorParams::PeriodExtreme(PeriodExtremeParams {
                period: ExtremePeriod::Last4h,
                ..
            }) => TfId::M15,
            DetectorParams::PeriodExtreme(PeriodExtremeParams {
                period: ExtremePeriod::Last12h | ExtremePeriod::Last24h | ExtremePeriod::Last48h,
                ..
            }) => TfId::H1,
            DetectorParams::PeriodLevel(PeriodLevelParams { day_open: true, .. }) => TfId::H4,
            DetectorParams::VolumeProfile(_) => TfId::H1,
            _ => TfId::D1,
        }
    }

    /// How many closed bars of `tf` the detector needs to give a full answer — what the caller
    /// fetches, and what it compares against before saying "not enough history" instead of
    /// silently drawing nothing.
    ///
    /// - The pivot detectors (`Swing`, `Trend`, `Squeeze`, `Formation`): their lookback
    ///   (`Squeeze`: its level's), raised to `ATR_PERIOD + 1` when the tolerance is in ATR — the
    ///   ATR reads that many bars. A flag's pole lies inside the lookback, so it adds nothing.
    /// - A period extreme: the bars that cover the requested period from its START up to the
    ///   latest possible reference time — the current day or week in full, the previous one plus
    ///   all of the current one, a rolling window its hours — plus one (see `bars_for_days`): the
    ///   detector refuses a period whose start is missing.
    /// - A volume profile: the same for its span (`Day` 1 day, `PrevDay` 2, a rolling window its
    ///   hours) plus one day per naked POC day asked for — each of those days and every day since
    ///   must be read to know price has not come back.
    /// - A period level: the longest of its enabled flags — the previous day's close 2 days, the
    ///   day's open 1, the week's open 7, the previous week's close 14. None enabled: one bar.
    /// - Round prices: one bar, the last close they are measured from.
    ///
    /// This is the DETECTOR's need only, and it says nothing about the events. The events pass
    /// (`moon_chart::auto_figures::events::line_events`) reads its own tail of the newest closed
    /// bars, `moon_chart::auto_figures::events::bars_read` of [`Self::broken_policy`], and that
    /// tail is the CALLER's to supply on top: the chart's job (`moon-ui-gpui`,
    /// `chartdx::auto_figures_sync::job`) fetches the larger of the two, hands the detector the
    /// newest `bars_needed` and keeps the newest `bars_read` (plus slack) for the events. A row
    /// whose detector needs one bar — round prices — still gets breakouts and bounces that way.
    /// "Not enough history" is measured against this number alone: a short events tail costs a
    /// mark, not a line.
    pub fn bars_needed(&self, tf: TfId) -> u32 {
        match self {
            DetectorParams::Swing(p) => with_atr(p.lookback_bars, p.tolerance_unit),
            DetectorParams::Trend(p) => with_atr(p.lookback_bars, p.tolerance_unit),
            DetectorParams::Squeeze(p) => with_atr(p.level.lookback_bars, p.level.tolerance_unit),
            DetectorParams::Formation(p) => with_atr(p.lookback_bars, p.tolerance_unit),
            DetectorParams::PeriodExtreme(p) => {
                let span = match p.period {
                    ExtremePeriod::Day => DAY_MS,
                    ExtremePeriod::PrevDay => 2 * DAY_MS,
                    ExtremePeriod::Week => 7 * DAY_MS,
                    ExtremePeriod::PrevWeek => 14 * DAY_MS,
                    ExtremePeriod::Last4h => 4 * HOUR_MS,
                    ExtremePeriod::Last12h => 12 * HOUR_MS,
                    ExtremePeriod::Last24h => 24 * HOUR_MS,
                    ExtremePeriod::Last48h => 48 * HOUR_MS,
                };
                bars_for_span(span, tf)
            }
            DetectorParams::VolumeProfile(p) => {
                // The naked POCs read their days and every day since: `naked_poc_days` whole days
                // before the reference day, plus the reference day itself.
                let naked = i64::from(p.naked_poc_days);
                let span = match p.period {
                    ProfilePeriod::Day => (1 + naked) * DAY_MS,
                    ProfilePeriod::PrevDay => (2 + naked) * DAY_MS,
                    ProfilePeriod::Last4h
                    | ProfilePeriod::Last12h
                    | ProfilePeriod::Last24h
                    | ProfilePeriod::Last48h => {
                        let hours = i64::from(p.period.rolling_hours().unwrap_or(24));
                        let window = hours * HOUR_MS;
                        if naked > 0 {
                            window.max((1 + naked) * DAY_MS)
                        } else {
                            window
                        }
                    }
                };
                bars_for_span(span, tf)
            }
            DetectorParams::PeriodLevel(p) => {
                let days = [
                    (p.prev_day_close, 2),
                    (p.day_open, 1),
                    (p.week_open, 7),
                    (p.prev_week_close, 14),
                ]
                .into_iter()
                .filter_map(|(on, days)| on.then_some(days))
                .max();
                days.map_or(1, |d| bars_for_days(d, tf))
            }
            DetectorParams::RoundLevels(_) => 1,
        }
    }
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams::Swing(SwingParams::default())
    }
}

/// Which side of the current price a kept line may be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TakeSide {
    Above,
    Below,
    #[default]
    Both,
}

/// How the kept lines are ranked before `count` cuts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TakeOrder {
    /// Closest to the current price first.
    #[default]
    Nearest,
    /// Strongest first, by [`Take::strength`]. Under [`StrengthBasis::Touches`], for the pivot
    /// detectors (`Swing`, `Trend`, `Squeeze`, `Formation`) the strength is the touch count plus the fraction
    /// of the searched window the touches span — the fraction is below one, so one more touch
    /// always outranks a longer life. A period extreme's strength is a constant 1, so its lines
    /// tie and fall back to nearest first.
    Strongest,
}

/// What "strongest" ranks by when a row takes [`TakeOrder::Strongest`].
///
/// A FIELD of `Take` rather than a third `TakeOrder` value: an older build reading an unknown
/// value of a known field resets that field to its default, while an unknown field is simply
/// ignored — so a row saved as "strongest by volume" still reads as "strongest" there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StrengthBasis {
    /// The detector's own strength: for the pivot detectors, the touches first and their span
    /// second (see [`TakeOrder::Strongest`]).
    #[default]
    Touches,
    /// The quote volume traded on the bars that touched the line
    /// (`moon_chart::auto_figures::Found::touch_volume`): a level the market defended with money
    /// outranks one it brushed past. Ties — a detector without touches reports zero — fall back to
    /// nearest first.
    Volume,
}

/// Which of the found lines a row keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Take {
    /// How many lines are kept — PER SIDE when `side` is `Both`, so "two above and two below" is
    /// `count = 2, side = Both`.
    pub count: u8,
    pub side: TakeSide,
    pub order: TakeOrder,
    /// What `TakeOrder::Strongest` ranks by; ignored under `Nearest`. Added after the first files
    /// were written: an older row loads as `Touches`, its old behaviour.
    pub strength: StrengthBasis,
}

impl Default for Take {
    fn default() -> Self {
        Self {
            count: 2,
            side: TakeSide::Both,
            order: TakeOrder::Nearest,
            strength: StrengthBasis::Touches,
        }
    }
}

/// An event on a line, derived from the current price and the last closed bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoEvent {
    /// Price came within the row's approach distance of the line.
    Approach,
    /// A `Squeeze` formation is pressed: its sloped side has reached the level and price still
    /// stands between the two.
    Squeeze,
    /// A bar closed beyond the line.
    Breakout,
    /// A bar closed beyond the line and price returned inside within the allowed bars.
    FalseBreak,
    /// Price touched the line and closed back on its original side.
    Bounce,
    /// The line changed role: a bar closed beyond it, and a later bar touched it from that NEW
    /// side and closed on the new side again — a broken resistance holding as support, or the
    /// other way round.
    RoleFlip,
    /// A stop hunt: a bar's wick pierced the line by more than [`EventMarks::stop_hunt_pct`] and
    /// the bar still closed on the line's original side — the stops behind the line were taken
    /// and price came straight back.
    StopHunt,
    /// The line lies within [`EventMarks::confluence_pct`] of a line of ANOTHER row on a
    /// DIFFERENT timeframe — a 1h level next to a 4h one, a place both scales agree on.
    ///
    /// Judged across rows, not inside a detector: no detector sees another row's lines.
    Confluence,
}

/// Which events get a mark on a row's lines. All on by default: a mark costs nothing to ignore,
/// and a row that hides one is a deliberate choice.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventMarks {
    pub approach: bool,
    pub squeeze: bool,
    pub breakout: bool,
    pub false_break: bool,
    pub bounce: bool,
    pub role_flip: bool,
    pub stop_hunt: bool,
    pub confluence: bool,
    /// "Approach" distance, in % of price.
    pub approach_pct: f32,
    /// Smallest pierce that makes a stop hunt, in % of the LINE's price: how far beyond the line
    /// the wick reached. A shallower pierce is a touch, not a hunt.
    pub stop_hunt_pct: f32,
    /// Largest distance between two lines of different timeframes that still counts as one place,
    /// in % of price.
    pub confluence_pct: f32,
}

impl EventMarks {
    /// Whether this row marks `event`.
    pub fn marks(&self, event: AutoEvent) -> bool {
        match event {
            AutoEvent::Approach => self.approach,
            AutoEvent::Squeeze => self.squeeze,
            AutoEvent::Breakout => self.breakout,
            AutoEvent::FalseBreak => self.false_break,
            AutoEvent::Bounce => self.bounce,
            AutoEvent::RoleFlip => self.role_flip,
            AutoEvent::StopHunt => self.stop_hunt,
            AutoEvent::Confluence => self.confluence,
        }
    }
}

impl Default for EventMarks {
    fn default() -> Self {
        Self {
            approach: true,
            squeeze: true,
            breakout: true,
            false_break: true,
            bounce: true,
            role_flip: true,
            stop_hunt: true,
            confluence: true,
            approach_pct: 0.3,
            stop_hunt_pct: 0.3,
            confluence_pct: 0.3,
        }
    }
}
