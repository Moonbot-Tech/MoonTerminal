//! Ready-made rows, and the set a chart ships with.
//!
//! A preset is a STARTING POINT, not a type: it fills a new row, and from then on the row is an
//! ordinary row. The row remembers which preset it came from only so its name can be translated.
//!
//! The shipped set is LinKvo's decision of 18.11.2026: on by default, four rows — 1h levels, 4h
//! levels in another colour and heavier, the squeeze, and the day's extremes.

use serde::{Deserialize, Serialize};

use super::{
    AutoFigureRow, DetectorParams, ExtremePeriod, ExtremeWhich, LineStyle, PeriodExtremeParams,
    SqueezeParams, SwingParams, TfId, TfSource, TrendParams,
};
use crate::config::chart_labels::LabelColor;
use crate::figures::LineKind;
use crate::palette;

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
}

impl AutoFigurePreset {
    /// Every preset, in "Add" menu order.
    pub const ALL: [AutoFigurePreset; 6] = [
        AutoFigurePreset::Levels1h,
        AutoFigurePreset::Levels4h,
        AutoFigurePreset::Squeeze,
        AutoFigurePreset::DayExtremes,
        AutoFigurePreset::PrevDayExtremes,
        AutoFigurePreset::TrendLines,
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
                DetectorParams::Swing(SwingParams::default()),
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
                // The extreme of a calendar day is the same price on any bars that cover it; the
                // hour chart is fine enough to place it and coarse enough to be cheap.
                TfSource::Fixed(TfId::H1),
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
                TfSource::Fixed(TfId::H1),
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
