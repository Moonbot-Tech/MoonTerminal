//! Ready-made rows, and the set a chart ships with.
//!
//! A preset is a STARTING POINT, not a type: it fills a new row, and from then on the row is an
//! ordinary row. The row remembers which preset it came from only so its name can be translated.
//!
//! The shipped set is LinKvo's decision of 18.11.2026: on by default, four rows — 1h levels, 4h
//! levels in another colour and heavier, the squeeze, and the day's extremes. The volume, period
//! and round-price presets of 19.11.2026, and the formation presets after them, are offered in the
//! "Add" menu only and never join it.

use serde::{Deserialize, Serialize};

use super::{
    AutoFigureRow, DetectorParams, ExtremePeriod, ExtremeWhich, FormationParams, LineStyle,
    PeriodExtremeParams, PeriodLevelParams, ProfilePeriod, RoundLevelsParams, SqueezeParams,
    SwingParams, TfId, TfSource, TrendParams, VolumeProfileParams,
};
use crate::config::chart_labels::LabelColor;
use crate::figures::LineKind;
use crate::palette;

/// Lookback of the shipped "Levels 4h" row, in four-hour bars.
///
/// Bounded by the local 5-minute candle cache the 4h bars are built from: it keeps 15 days
/// (≈ 90 four-hour bars), so a deeper default would leave the row permanently short of history.
/// 72 bars (12 days) leaves room for the partial first bucket and for a cache that is a few days
/// short of full — a fresh install, or the 14-day set the atlas is shot on.
pub const LEVELS_4H_LOOKBACK: u32 = 72;

/// One ready-made row.
///
/// Serialized, because a row remembers the preset it came from: that is what keeps its name
/// translatable. The serde name is the wire form — a variant may be reordered freely but never
/// renamed without migrating `layout.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoFigurePreset {
    /// Swing levels on the hour chart, two per side, nearest first.
    Levels1h,
    /// Swing levels on the four-hour chart: the same search on bigger bars, drawn heavier and in
    /// another colour so the two scales never read as one.
    Levels4h,
    /// A level with a sloped line pressing into it, on 15-minute bars.
    Squeeze,
    /// Today's high and low (UTC).
    DayExtremes,
    /// Yesterday's high and low (UTC).
    ///
    /// A row of its own rather than a second period inside `DayExtremes`: the contract's
    /// `PeriodExtremeParams` holds ONE period, and a separate row gets its own style and caption,
    /// which is what tells "today" and "yesterday" apart on the chart. Offered in the "Add" menu,
    /// not in the shipped set — the four shipped rows are LinKvo's choice.
    PrevDayExtremes,
    /// Sloped support and resistance lines through pivots. Offered, not shipped.
    TrendLines,
    /// Yesterday's volume profile (UTC): its POC and the two edges of its value area. Offered, not
    /// shipped.
    PrevDayProfile,
    /// The POCs of the last five UTC days that price has not traded at since — and nothing else.
    /// Offered, not shipped.
    NakedPoc,
    /// Yesterday's close and this week's open (UTC). Offered, not shipped.
    PeriodLevels,
    /// Round prices on the half-order grid within 10 % of the last close. Offered, not shipped.
    RoundLevels,
    /// Triangles and wedges: two converging lines. Offered, not shipped.
    Triangles,
    /// Channels: two parallel lines, sloped or horizontal. Offered, not shipped.
    Channels,
    /// Flags and pennants: a short channel or triangle right after an impulse. Offered, not
    /// shipped.
    Flags,
    /// Double tops and double bottoms. Offered, not shipped.
    DoubleTops,
}

impl AutoFigurePreset {
    /// Every preset, in "Add" menu order.
    pub const ALL: [AutoFigurePreset; 14] = [
        AutoFigurePreset::Levels1h,
        AutoFigurePreset::Levels4h,
        AutoFigurePreset::Squeeze,
        AutoFigurePreset::DayExtremes,
        AutoFigurePreset::PrevDayExtremes,
        AutoFigurePreset::TrendLines,
        AutoFigurePreset::PrevDayProfile,
        AutoFigurePreset::NakedPoc,
        AutoFigurePreset::PeriodLevels,
        AutoFigurePreset::RoundLevels,
        AutoFigurePreset::Triangles,
        AutoFigurePreset::Channels,
        AutoFigurePreset::Flags,
        AutoFigurePreset::DoubleTops,
    ];

    /// The rows a chart ships with, in draw order.
    pub const DEFAULT_SET: [AutoFigurePreset; 4] = [
        AutoFigurePreset::Levels1h,
        AutoFigurePreset::Levels4h,
        AutoFigurePreset::Squeeze,
        AutoFigurePreset::DayExtremes,
    ];

    /// Locale key of the preset's name, which is also the name its row takes. The keys are added
    /// to `locales/` together with the popup that prints them.
    pub fn locale_key(self) -> &'static str {
        match self {
            AutoFigurePreset::Levels1h => "auto_figures.preset.levels_1h",
            AutoFigurePreset::Levels4h => "auto_figures.preset.levels_4h",
            AutoFigurePreset::Squeeze => "auto_figures.preset.squeeze",
            AutoFigurePreset::DayExtremes => "auto_figures.preset.day_extremes",
            AutoFigurePreset::PrevDayExtremes => "auto_figures.preset.prev_day_extremes",
            AutoFigurePreset::TrendLines => "auto_figures.preset.trend_lines",
            AutoFigurePreset::PrevDayProfile => "auto_figures.preset.prev_day_profile",
            AutoFigurePreset::NakedPoc => "auto_figures.preset.naked_poc",
            AutoFigurePreset::PeriodLevels => "auto_figures.preset.period_levels",
            AutoFigurePreset::RoundLevels => "auto_figures.preset.round_levels",
            AutoFigurePreset::Triangles => "auto_figures.preset.triangles",
            AutoFigurePreset::Channels => "auto_figures.preset.channels",
            AutoFigurePreset::Flags => "auto_figures.preset.flags",
            AutoFigurePreset::DoubleTops => "auto_figures.preset.double_tops",
        }
    }

    /// A new row built from this preset.
    ///
    /// Every preset row takes `Take::default()` (two per side, nearest first) and
    /// `EventMarks::default()` (every event marked): those defaults ARE the decision, and stating
    /// them again here would make a later change to them skip the presets.
    pub fn row(self) -> AutoFigureRow {
        let (detector, tf, style) = match self {
            AutoFigurePreset::Levels1h => (
                DetectorParams::Swing(SwingParams::default()),
                TfSource::Fixed(TfId::H1),
                LineStyle::default(),
            ),
            AutoFigurePreset::Levels4h => (
                // The default 200-bar lookback is 33 days of 4h bars, and the 4h bars come from the
                // local 5-minute cache, which keeps 15 days (`market::kline_cache::retention_days`):
                // shipped at 200 the row could never fill and would always report short history.
                DetectorParams::Swing(SwingParams {
                    lookback_bars: LEVELS_4H_LOOKBACK,
                    ..SwingParams::default()
                }),
                TfSource::Fixed(TfId::H4),
                LineStyle {
                    color: LabelColor::Fixed(rgb(palette::ACCENT)),
                    thickness: 2.0,
                    ..LineStyle::default()
                },
            ),
            AutoFigurePreset::Squeeze => (
                DetectorParams::Squeeze(SqueezeParams::default()),
                TfSource::Fixed(TfId::M15),
                LineStyle {
                    color: LabelColor::Fixed(rgb(palette::TP)),
                    thickness: 1.5,
                    ..LineStyle::default()
                },
            ),
            AutoFigurePreset::DayExtremes => (
                DetectorParams::PeriodExtreme(PeriodExtremeParams {
                    period: ExtremePeriod::Day,
                    which: ExtremeWhich::Both,
                }),
                // The detector reads CLOSED bars only, so a new high or low of the day shows up
                // when the bar that printed it closes: the lag is one bar of this timeframe. Five
                // minutes keeps it at most five minutes; the pass stays cheap — one day is 288 bars.
                TfSource::Fixed(TfId::M5),
                LineStyle {
                    line_kind: LineKind::Dash,
                    ..LineStyle::default()
                },
            ),
            AutoFigurePreset::PrevDayExtremes => (
                DetectorParams::PeriodExtreme(PeriodExtremeParams {
                    period: ExtremePeriod::PrevDay,
                    which: ExtremeWhich::Both,
                }),
                // Yesterday's range is complete, but a break of it is read from today's closed
                // bars — the same one-bar lag as the day's own extremes, so the same timeframe.
                TfSource::Fixed(TfId::M5),
                LineStyle {
                    line_kind: LineKind::DashDot,
                    ..LineStyle::default()
                },
            ),
            AutoFigurePreset::TrendLines => (
                DetectorParams::Trend(TrendParams::default()),
                TfSource::Auto,
                LineStyle {
                    color: LabelColor::BySign,
                    ..LineStyle::default()
                },
            ),
            AutoFigurePreset::PrevDayProfile => (
                DetectorParams::VolumeProfile(VolumeProfileParams {
                    period: ProfilePeriod::PrevDay,
                    ..VolumeProfileParams::default()
                }),
                // A profile spreads each bar's volume over its range, so its resolution is the
                // bar: five-minute bars put 288 of them into the day's 100 bins, fifteen-minute
                // ones fewer than one per bin. Two days of them is 577 bars.
                TfSource::Fixed(TfId::M5),
                LineStyle {
                    color: LabelColor::Fixed(rgb(palette::ORANGE)),
                    line_kind: LineKind::Dash,
                    ..LineStyle::default()
                },
            ),
            AutoFigurePreset::NakedPoc => (
                DetectorParams::VolumeProfile(VolumeProfileParams {
                    period: ProfilePeriod::Day,
                    poc: false,
                    vah: false,
                    val: false,
                    naked_poc_days: 5,
                    ..VolumeProfileParams::default()
                }),
                // The same resolution as the day's profile; six days of five-minute bars (1 729)
                // fit the 15-day cache with room.
                TfSource::Fixed(TfId::M5),
                LineStyle {
                    color: LabelColor::Fixed(rgb(palette::ORANGE)),
                    line_kind: LineKind::Dot,
                    thickness: 1.5,
                    ..LineStyle::default()
                },
            ),
            AutoFigurePreset::PeriodLevels => (
                DetectorParams::PeriodLevel(PeriodLevelParams::default()),
                // The prices are facts of the calendar; the timeframe only sets how fast a break
                // of them is read, the same one-bar lag as the day's extremes.
                TfSource::Fixed(TfId::M5),
                LineStyle {
                    line_kind: LineKind::DashDotDot,
                    ..LineStyle::default()
                },
            ),
            AutoFigurePreset::RoundLevels => (
                DetectorParams::RoundLevels(RoundLevelsParams::default()),
                // The grid comes from the price alone; the timeframe only feeds the events, so it
                // follows the chart's zoom.
                TfSource::Auto,
                LineStyle {
                    line_kind: LineKind::Dot,
                    ..LineStyle::default()
                },
            ),
            // The formation presets follow the chart's zoom, like the trend lines they are built
            // from: a triangle of 1-minute swings and one of 4-hour swings are both triangles.
            AutoFigurePreset::Triangles => (
                DetectorParams::Formation(FormationParams {
                    channel: false,
                    flag: false,
                    double: false,
                    ..FormationParams::default()
                }),
                TfSource::Auto,
                LineStyle {
                    color: LabelColor::Fixed(rgb(palette::ACCENT)),
                    thickness: 1.5,
                    ..LineStyle::default()
                },
            ),
            AutoFigurePreset::Channels => (
                DetectorParams::Formation(FormationParams {
                    triangle: false,
                    wedge: false,
                    flag: false,
                    double: false,
                    ..FormationParams::default()
                }),
                TfSource::Auto,
                LineStyle {
                    color: LabelColor::Fixed(rgb(palette::TP)),
                    ..LineStyle::default()
                },
            ),
            // A flag is short and stands right after its pole, so the last hundred bars hold it,
            // and two touches a side are what a fifteen-bar flag has room for; the pole and the
            // retrace bound are what keep noise out.
            AutoFigurePreset::Flags => (
                DetectorParams::Formation(FormationParams {
                    triangle: false,
                    wedge: false,
                    channel: false,
                    double: false,
                    lookback_bars: 100,
                    min_touches: 2,
                    ..FormationParams::default()
                }),
                TfSource::Auto,
                LineStyle {
                    color: LabelColor::Fixed(rgb(palette::ORANGE)),
                    thickness: 1.5,
                    ..LineStyle::default()
                },
            ),
            AutoFigurePreset::DoubleTops => (
                DetectorParams::Formation(FormationParams {
                    triangle: false,
                    wedge: false,
                    channel: false,
                    flag: false,
                    ..FormationParams::default()
                }),
                TfSource::Auto,
                LineStyle {
                    line_kind: LineKind::Dash,
                    ..LineStyle::default()
                },
            ),
        };
        AutoFigureRow {
            preset: Some(self),
            tf,
            style,
            ..AutoFigureRow::new(detector)
        }
    }
}

/// The shipped rows, built from [`AutoFigurePreset::DEFAULT_SET`].
pub(super) fn shipped_rows() -> Vec<AutoFigureRow> {
    AutoFigurePreset::DEFAULT_SET
        .into_iter()
        .map(AutoFigurePreset::row)
        .collect()
}

/// A palette colour as the `0xRRGGBB` a [`LabelColor::Fixed`] holds.
///
/// The presets name palette constants rather than spelling hex: `LabelColor` has only one theme
/// slot (`Theme`), so a second shipped colour has to be a `Fixed` one, and taking it from the
/// palette keeps the one source of those numbers.
const fn rgb(c: [u8; 3]) -> u32 {
    ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32
}
