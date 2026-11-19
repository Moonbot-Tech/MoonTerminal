//! The editor's numeric fields: one entry per number a row can carry, with the row it reads and
//! writes and the unit its label names.
//!
//! A table rather than one input per struct field written out by hand: the four detectors share
//! most of their numbers (`Squeeze` nests a whole `SwingParams`), and the editor has to know for
//! EACH field whether the row in front of it has one at all. Answering that once, in `get`, is what
//! keeps a field from showing on a detector that ignores it — the same rule the captions editor
//! follows when it offers a setting only on the fields that read it.

use moon_core::config::auto_figures::{AutoFigureRow, BrokenPolicy, DetectorParams, RowDetector};

/// One number of a row, as the editor shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum NumField {
    /// Closed bars searched.
    Lookback,
    /// Bars a pivot must out-rank on its left.
    PivotLeft,
    /// Bars a pivot must out-rank on its right.
    PivotRight,
    /// Touch tolerance, % of price.
    Tolerance,
    /// Fewest pivots a level or line needs.
    MinTouches,
    /// Trend only: bar bodies allowed through the line.
    BodyBreaks,
    /// Trend only: slope bounds, % of price per bar.
    SlopeMin,
    SlopeMax,
    /// Squeeze only: the sloped half's touches and tolerance, and the "pressed" distance.
    TrendTouches,
    TrendTolerance,
    Pressed,
    /// Bars a broken line is kept, while the policy is `Keep`.
    BrokenBars,
    /// Lines kept (per side when the side is `Both`).
    TakeCount,
    /// Approach distance of the event marks, % of price.
    ApproachPct,
    /// Stroke width, logical pixels.
    Thickness,
    /// Zone opacity, shown in percent while a zone is on.
    ZoneOpacity,
}

impl NumField {
    /// Every field, in the order the editor lays the detector's out.
    pub(super) const ALL: [NumField; 16] = [
        NumField::Lookback,
        NumField::PivotLeft,
        NumField::PivotRight,
        NumField::Tolerance,
        NumField::MinTouches,
        NumField::BodyBreaks,
        NumField::SlopeMin,
        NumField::SlopeMax,
        NumField::TrendTouches,
        NumField::TrendTolerance,
        NumField::Pressed,
        NumField::BrokenBars,
        NumField::TakeCount,
        NumField::ApproachPct,
        NumField::Thickness,
        NumField::ZoneOpacity,
    ];

    /// The fields that describe the DETECTOR, in layout order — the left pane's numbers. The rest
    /// (take, events, look) are placed by the pane that owns them.
    pub(super) const DETECTOR: [NumField; 11] = [
        NumField::Lookback,
        NumField::PivotLeft,
        NumField::PivotRight,
        NumField::Tolerance,
        NumField::MinTouches,
        NumField::BodyBreaks,
        NumField::SlopeMin,
        NumField::SlopeMax,
        NumField::TrendTouches,
        NumField::TrendTolerance,
        NumField::Pressed,
    ];

    /// Locale key of the field's label. Every label names its unit — bars, %, touches, px — so a
    /// number is never read in the wrong one.
    pub(super) fn locale_key(self) -> &'static str {
        match self {
            NumField::Lookback => "auto_figures.edit.lookback",
            NumField::PivotLeft => "auto_figures.edit.pivot_left",
            NumField::PivotRight => "auto_figures.edit.pivot_right",
            NumField::Tolerance => "auto_figures.edit.tolerance",
            NumField::MinTouches => "auto_figures.edit.min_touches",
            NumField::BodyBreaks => "auto_figures.edit.body_breaks",
            NumField::SlopeMin => "auto_figures.edit.slope_min",
            NumField::SlopeMax => "auto_figures.edit.slope_max",
            NumField::TrendTouches => "auto_figures.edit.trend_touches",
            NumField::TrendTolerance => "auto_figures.edit.trend_tolerance",
            NumField::Pressed => "auto_figures.edit.pressed",
            NumField::BrokenBars => "auto_figures.edit.broken_bars",
            NumField::TakeCount => "auto_figures.edit.take_count",
            NumField::ApproachPct => "auto_figures.edit.approach_pct",
            NumField::Thickness => "auto_figures.edit.thickness",
            NumField::ZoneOpacity => "auto_figures.edit.zone_opacity",
        }
    }

    /// Element id of the field's input, unique within the dialog.
    pub(super) fn element_id(self) -> String {
        format!("afe-num-{self:?}")
    }

    /// Whether the field holds a whole number.
    pub(super) fn integer(self) -> bool {
        !matches!(
            self,
            NumField::Tolerance
                | NumField::SlopeMin
                | NumField::SlopeMax
                | NumField::TrendTolerance
                | NumField::Pressed
                | NumField::ApproachPct
                | NumField::Thickness
                | NumField::ZoneOpacity
        )
    }

    /// The field's value on `row`, or `None` when this row has no such number — a field of
    /// another detector, a broken-bar count under `Remove`, a zone opacity with the zone off, or
    /// any detector field of a row this build cannot interpret.
    pub(super) fn get(self, row: &AutoFigureRow) -> Option<f64> {
        let swing_like = |d: &DetectorParams| match d {
            DetectorParams::Swing(p) => Some(*p),
            DetectorParams::Squeeze(p) => Some(p.level),
            _ => None,
        };
        let known = row.detector.known();
        let v = match self {
            NumField::Lookback => known.and_then(|d| match d {
                DetectorParams::Trend(p) => Some(f64::from(p.lookback_bars)),
                d => swing_like(d).map(|p| f64::from(p.lookback_bars)),
            }),
            NumField::PivotLeft => known.and_then(|d| match d {
                DetectorParams::Trend(p) => Some(f64::from(p.pivot_left)),
                d => swing_like(d).map(|p| f64::from(p.pivot_left)),
            }),
            NumField::PivotRight => known.and_then(|d| match d {
                DetectorParams::Trend(p) => Some(f64::from(p.pivot_right)),
                d => swing_like(d).map(|p| f64::from(p.pivot_right)),
            }),
            NumField::Tolerance => known.and_then(|d| match d {
                DetectorParams::Trend(p) => Some(f64::from(p.tolerance_pct)),
                d => swing_like(d).map(|p| f64::from(p.tolerance_pct)),
            }),
            NumField::MinTouches => known.and_then(|d| match d {
                DetectorParams::Trend(p) => Some(f64::from(p.min_touches)),
                d => swing_like(d).map(|p| f64::from(p.min_touches)),
            }),
            NumField::BodyBreaks => known.and_then(|d| match d {
                DetectorParams::Trend(p) => Some(f64::from(p.max_body_breaks)),
                _ => None,
            }),
            NumField::SlopeMin => known.and_then(|d| match d {
                DetectorParams::Trend(p) => Some(f64::from(p.min_slope_pct_per_bar)),
                _ => None,
            }),
            NumField::SlopeMax => known.and_then(|d| match d {
                DetectorParams::Trend(p) => Some(f64::from(p.max_slope_pct_per_bar)),
                _ => None,
            }),
            NumField::TrendTouches => known.and_then(|d| match d {
                DetectorParams::Squeeze(p) => Some(f64::from(p.min_trend_touches)),
                _ => None,
            }),
            NumField::TrendTolerance => known.and_then(|d| match d {
                DetectorParams::Squeeze(p) => Some(f64::from(p.trend_tolerance_pct)),
                _ => None,
            }),
            NumField::Pressed => known.and_then(|d| match d {
                DetectorParams::Squeeze(p) => Some(f64::from(p.pressed_pct)),
                _ => None,
            }),
            NumField::BrokenBars => known.and_then(broken_of).and_then(|b| match b {
                BrokenPolicy::Keep { bars } => Some(f64::from(bars)),
                BrokenPolicy::Remove => None,
            }),
            NumField::TakeCount => Some(f64::from(row.take.count)),
            NumField::ApproachPct => Some(f64::from(row.events.approach_pct)),
            NumField::Thickness => Some(f64::from(row.style.thickness)),
            // Rounded to a thousandth of a percent: the fraction is stored, and 0.07 × 100 in
            // binary is 7.00000003 — which the field would otherwise read back.
            NumField::ZoneOpacity => row
                .style
                .zone
                .map(|a| (f64::from(a) * 100_000.0).round() / 1000.0),
        };
        v.filter(|v| v.is_finite())
    }

    /// Write `v` into `row`, returning whether the row has this field at all.
    ///
    /// Clamped only to what the TYPE can hold — never to the model's ranges, which `sanitize`
    /// owns. Applying those here would rewrite a number while it is being typed: "1" on the way to
    /// "150" would snap to the lookback minimum and the field would fight the keyboard. The editor
    /// sanitizes a copy for its preview and the row itself on OK.
    pub(super) fn set(self, row: &mut AutoFigureRow, v: f64) -> bool {
        if !v.is_finite() {
            return false;
        }
        let v = if self.integer() { v.round() } else { v };
        let as_u8 = v.clamp(0.0, f64::from(u8::MAX)) as u8;
        let as_u16 = v.clamp(0.0, f64::from(u16::MAX)) as u16;
        let as_u32 = v.clamp(0.0, f64::from(u32::MAX)) as u32;
        let as_f32 = v as f32;
        match self {
            NumField::TakeCount => {
                row.take.count = as_u8;
                return true;
            }
            NumField::ApproachPct => {
                row.events.approach_pct = as_f32;
                return true;
            }
            NumField::Thickness => {
                row.style.thickness = as_f32;
                return true;
            }
            NumField::ZoneOpacity => {
                if row.style.zone.is_none() {
                    return false;
                }
                row.style.zone = Some(as_f32 / 100.0);
                return true;
            }
            _ => {}
        }
        let RowDetector::Known(d) = &mut row.detector else {
            return false;
        };
        match (self, d) {
            (NumField::Lookback, DetectorParams::Swing(p)) => p.lookback_bars = as_u32,
            (NumField::Lookback, DetectorParams::Squeeze(p)) => p.level.lookback_bars = as_u32,
            (NumField::Lookback, DetectorParams::Trend(p)) => p.lookback_bars = as_u32,
            (NumField::PivotLeft, DetectorParams::Swing(p)) => p.pivot_left = as_u8,
            (NumField::PivotLeft, DetectorParams::Squeeze(p)) => p.level.pivot_left = as_u8,
            (NumField::PivotLeft, DetectorParams::Trend(p)) => p.pivot_left = as_u8,
            (NumField::PivotRight, DetectorParams::Swing(p)) => p.pivot_right = as_u8,
            (NumField::PivotRight, DetectorParams::Squeeze(p)) => p.level.pivot_right = as_u8,
            (NumField::PivotRight, DetectorParams::Trend(p)) => p.pivot_right = as_u8,
            (NumField::Tolerance, DetectorParams::Swing(p)) => p.tolerance_pct = as_f32,
            (NumField::Tolerance, DetectorParams::Squeeze(p)) => p.level.tolerance_pct = as_f32,
            (NumField::Tolerance, DetectorParams::Trend(p)) => p.tolerance_pct = as_f32,
            (NumField::MinTouches, DetectorParams::Swing(p)) => p.min_touches = as_u8,
            (NumField::MinTouches, DetectorParams::Squeeze(p)) => p.level.min_touches = as_u8,
            (NumField::MinTouches, DetectorParams::Trend(p)) => p.min_touches = as_u8,
            (NumField::BodyBreaks, DetectorParams::Trend(p)) => p.max_body_breaks = as_u8,
            (NumField::SlopeMin, DetectorParams::Trend(p)) => p.min_slope_pct_per_bar = as_f32,
            (NumField::SlopeMax, DetectorParams::Trend(p)) => p.max_slope_pct_per_bar = as_f32,
            (NumField::TrendTouches, DetectorParams::Squeeze(p)) => p.min_trend_touches = as_u8,
            (NumField::TrendTolerance, DetectorParams::Squeeze(p)) => {
                p.trend_tolerance_pct = as_f32
            }
            (NumField::Pressed, DetectorParams::Squeeze(p)) => p.pressed_pct = as_f32,
            (NumField::BrokenBars, d) => match broken_of_mut(d) {
                Some(BrokenPolicy::Keep { bars }) => *bars = as_u16,
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    /// The field's value as the input shows it: whole numbers bare, fractions without trailing
    /// zeros.
    pub(super) fn display(self, v: f64) -> String {
        if self.integer() {
            format!("{}", v.round() as i64)
        } else {
            // Through f32, which is what the model stores: an f64 print of an f32 value shows the
            // binary tail (0.25 reads as 0.25, but 0.15 as 0.15000000596).
            format!("{}", v as f32)
        }
    }
}

/// Characters a reader may group thousands with: none of them is ever a decimal separator.
const GROUPING: [char; 3] = [' ', '\u{a0}', '_'];

/// Parse what the reader typed into a field that holds a whole number (`integer`) or a fraction.
///
/// A whole number has no decimal separator, so every comma, blank, no-break space and underscore
/// in it groups thousands: "1,000" and "1 000" are a thousand, never one. In a fraction a comma is
/// the decimal separator only when it is the ONLY separator — "0,25" is a quarter — while beside a
/// point, or repeated, it groups thousands ("1,000.5", "1,000,000"). Blanks around the number are
/// ignored.
pub(super) fn parse_number(text: &str, integer: bool) -> Option<f64> {
    let text: String = text
        .trim()
        .chars()
        .filter(|c| !GROUPING.contains(c))
        .collect();
    let commas = text.matches(',').count();
    let normalized = if integer || text.contains('.') || commas > 1 {
        text.replace(',', "")
    } else {
        text.replace(',', ".")
    };
    normalized.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The broken-line policy a detector carries, if it has one of its own. A period extreme, a
/// period level, a volume level and a round price have none: each line is a fact of the calendar,
/// the profile or the price grid.
pub(super) fn broken_of(d: &DetectorParams) -> Option<BrokenPolicy> {
    match d {
        DetectorParams::Swing(p) => Some(p.broken),
        DetectorParams::Trend(p) => Some(p.broken),
        DetectorParams::Squeeze(p) => Some(p.level.broken),
        DetectorParams::PeriodExtreme(_)
        | DetectorParams::VolumeProfile(_)
        | DetectorParams::PeriodLevel(_)
        | DetectorParams::RoundLevels(_) => None,
    }
}

/// Mutable twin of [`broken_of`].
pub(super) fn broken_of_mut(d: &mut DetectorParams) -> Option<&mut BrokenPolicy> {
    match d {
        DetectorParams::Swing(p) => Some(&mut p.broken),
        DetectorParams::Trend(p) => Some(&mut p.broken),
        DetectorParams::Squeeze(p) => Some(&mut p.level.broken),
        DetectorParams::PeriodExtreme(_)
        | DetectorParams::VolumeProfile(_)
        | DetectorParams::PeriodLevel(_)
        | DetectorParams::RoundLevels(_) => None,
    }
}
