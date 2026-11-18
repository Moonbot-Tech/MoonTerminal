//! The detector-facing half of the auto-figure model: which detector a row runs, with which
//! parameters, on which timeframe, and which of the found lines it keeps.
//!
//! These types are the CONTRACT between the two crates that meet here. `moon-chart`'s detectors
//! (`moon_chart::auto_figures`) read them as plain values — pure functions over closed candles,
//! no I/O, no clock — and this crate persists them. They live in `moon-core` because the model has
//! to serialize them and `moon-chart` already depends on this crate, never the other way round.
//!
//! Units are part of every field's name or doc: a percentage is a percentage of PRICE (`0.3` means
//! 0.3 %), a bar count is counted in bars of the row's OWN timeframe.
//!
//! Every struct is `#[serde(default)]`: a field added later loads as its default from an older
//! file, and a value missing from a hand-edited one does not drop the whole row.

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

/// What happens to a horizontal level once a bar has CLOSED beyond it.
///
/// A per-row choice (LinKvo, 18.11.2026): removing it at once hides a false breakout, keeping it
/// for a while shows one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum BrokenPolicy {
    /// Dropped on the first close beyond it.
    Remove,
    /// Kept for `bars` more bars after the break, flagged as broken so it can be drawn in a
    /// weaker style; dropped after that unless price has come back inside.
    Keep { bars: u16 },
}

impl Default for BrokenPolicy {
    fn default() -> Self {
        BrokenPolicy::Keep { bars: 3 }
    }
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
    /// Two pivots belong to the same level when their prices differ by at most this, in % of price.
    pub tolerance_pct: f32,
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
            tolerance_pct: 0.25,
            min_touches: 2,
            broken: BrokenPolicy::default(),
        }
    }
}

/// Which calendar period a `PeriodExtreme` row reads, in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExtremePeriod {
    /// The current UTC day, still forming.
    #[default]
    Day,
    /// The previous complete UTC day.
    PrevDay,
    /// The current ISO week (Monday 00:00 UTC onward), still forming.
    Week,
    /// The previous complete ISO week.
    PrevWeek,
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
    /// A pivot counts as a touch when it lies within this distance of the line, in % of price.
    pub tolerance_pct: f32,
    /// Fewest pivots on the line, the two that define it included.
    pub min_touches: u8,
    /// How many bar BODIES between the first and the last touch may cross the line before it is
    /// rejected. Wicks never count.
    pub max_body_breaks: u8,
    /// Slope bounds, in % of price per bar, by absolute value: a near-flat line is a level, not a
    /// trend, and a near-vertical one is noise.
    pub min_slope_pct_per_bar: f32,
    pub max_slope_pct_per_bar: f32,
}

impl Default for TrendParams {
    fn default() -> Self {
        Self {
            lookback_bars: 150,
            pivot_left: 3,
            pivot_right: 3,
            side: TrendSide::Both,
            tolerance_pct: 0.15,
            min_touches: 3,
            max_body_breaks: 0,
            min_slope_pct_per_bar: 0.005,
            max_slope_pct_per_bar: 2.0,
        }
    }
}

/// `Squeeze`: a horizontal level plus a sloped line on the OTHER side converging into it — rising
/// lows pressing a resistance, or falling highs pressing a support. One formation, one figure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SqueezeParams {
    /// The horizontal half, found the way `Swing` finds it.
    pub level: SwingParams,
    /// Fewest pivots on the converging sloped line.
    pub min_trend_touches: u8,
    /// Touch tolerance of the sloped line, in % of price.
    pub trend_tolerance_pct: f32,
    /// The formation counts as "pressed" once the sloped line, projected to the last closed bar,
    /// is within this distance of the level, in % of price.
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
            pressed_pct: 0.5,
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
}

impl DetectorKind {
    /// Every kind, in the order the row editor lists them.
    pub const ALL: [DetectorKind; 4] = [
        DetectorKind::Swing,
        DetectorKind::PeriodExtreme,
        DetectorKind::Trend,
        DetectorKind::Squeeze,
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
}

impl DetectorParams {
    pub fn kind(&self) -> DetectorKind {
        match self {
            DetectorParams::Swing(_) => DetectorKind::Swing,
            DetectorParams::PeriodExtreme(_) => DetectorKind::PeriodExtreme,
            DetectorParams::Trend(_) => DetectorKind::Trend,
            DetectorParams::Squeeze(_) => DetectorKind::Squeeze,
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
    /// Highest detector strength first (touches, age, volume — the detector's own score).
    Strongest,
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
}

impl Default for Take {
    fn default() -> Self {
        Self {
            count: 2,
            side: TakeSide::Both,
            order: TakeOrder::Nearest,
        }
    }
}

/// An event on a line, derived from the current price and the last closed bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoEvent {
    /// Price came within the row's approach distance of the line.
    Approach,
    /// A `Squeeze` formation is pressed: its sloped side has reached the level.
    Squeeze,
    /// A bar closed beyond the line.
    Breakout,
    /// A bar closed beyond the line and price returned inside within the allowed bars.
    FalseBreak,
    /// Price touched the line and closed back on its original side.
    Bounce,
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
    /// "Approach" distance, in % of price.
    pub approach_pct: f32,
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
            approach_pct: 0.3,
        }
    }
}
