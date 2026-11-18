//! How a row's lines look, and what is printed on them.
//!
//! Both types serialize through `wire`, which writes only the fields that differ from the defaults
//! below: a row that changed nothing but its colour states its colour, and follows every other
//! default as later builds move it.

use serde::{Deserialize, Serialize};

use crate::config::chart_labels::LabelColor;
use crate::figures::LineKind;

/// How far a horizontal line is drawn along the time axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Extend {
    /// From the first touch to the plot's right edge: where the level was born, and everything
    /// after it — the reading a trader asks a level for.
    #[default]
    RightEdge,
    /// Across the whole plot.
    BothEdges,
    /// Only between the first and the last touch: the stretch the detector actually saw.
    BetweenTouches,
}

impl Extend {
    /// Every choice, in the order the row editor lists them.
    pub const ALL: [Extend; 3] = [Extend::RightEdge, Extend::BothEdges, Extend::BetweenTouches];
}

/// How a row's lines are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(from = "super::wire::StyleWire", into = "super::wire::StyleWire")]
pub struct LineStyle {
    /// The colour, as a REFERENCE — the chart caption's own type, so a line follows the theme
    /// exactly as a caption does and the colour picker is the one the labels already have.
    ///
    /// `Theme` is the chart's caption colour. `BySign` means the theme's positive colour for a
    /// line below price (support) and its negative colour above (resistance) — the sign of the
    /// distance from price, which is the only sign a line has. `Fixed` is the user's own pick.
    pub color: LabelColor,
    /// Dash pattern, the figure layer's own enum: the line is drawn as a figure.
    pub line_kind: LineKind,
    /// Stroke width in logical pixels, within [`AF_THICKNESS_MIN`, `AF_THICKNESS_MAX`].
    ///
    /// [`AF_THICKNESS_MIN`]: super::AF_THICKNESS_MIN
    /// [`AF_THICKNESS_MAX`]: super::AF_THICKNESS_MAX
    pub thickness: f32,
    pub extend: Extend,
    /// Opacity of the tolerance band drawn around a level as a zone, `0..=1`, or `None` for a bare
    /// line. A band shows what "the level" means to the detector: every pivot inside it counted.
    pub zone: Option<f32>,
    /// Dash pattern a level is drawn with once it has been BROKEN but is still kept (see
    /// `BrokenPolicy::Keep`): the same colour and width, a weaker pattern, so a kept broken level
    /// reads as history rather than as a live one.
    pub broken_kind: LineKind,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self {
            color: LabelColor::Theme,
            line_kind: LineKind::Solid,
            thickness: 1.0,
            extend: Extend::RightEdge,
            zone: None,
            broken_kind: LineKind::Dot,
        }
    }
}

/// One piece of the caption printed on a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionPart {
    /// The row's name — its own, or its preset's, translated.
    RowName,
    /// The line's price at the live edge.
    Price,
    /// Distance from the current price, in percent.
    DistancePct,
    /// How many pivots the line collected.
    Touches,
    /// The timeframe it was found on.
    Tf,
    /// How long ago its first touch was.
    Age,
    /// The last event on the line, if any.
    Event,
}

impl CaptionPart {
    /// Every part, in the order the row editor lists them.
    pub const ALL: [CaptionPart; 7] = [
        CaptionPart::RowName,
        CaptionPart::Price,
        CaptionPart::DistancePct,
        CaptionPart::Touches,
        CaptionPart::Tf,
        CaptionPart::Age,
        CaptionPart::Event,
    ];
}

/// Where a line's caption sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptionPlace {
    LeftEnd,
    /// At the line's right end, where the eye already is on a chart read from the live edge.
    #[default]
    RightEnd,
    AtPriceAxis,
    AboveLine,
    BelowLine,
}

impl CaptionPlace {
    /// Every place, in the order the row editor lists them.
    pub const ALL: [CaptionPlace; 5] = [
        CaptionPlace::LeftEnd,
        CaptionPlace::RightEnd,
        CaptionPlace::AtPriceAxis,
        CaptionPlace::AboveLine,
        CaptionPlace::BelowLine,
    ];
}

/// When a line's caption is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptionShow {
    /// Always — the decorated default a new display feature ships with.
    #[default]
    Always,
    /// Only while the pointer is over the line.
    OnHover,
}

/// The caption printed on a line itself, and where.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "super::wire::CaptionWire", into = "super::wire::CaptionWire")]
pub struct LineCaption {
    /// The pieces, in print order, each at most once. Empty prints nothing.
    pub parts: Vec<CaptionPart>,
    pub place: CaptionPlace,
    pub show: CaptionShow,
}

impl Default for LineCaption {
    /// The row's name and the price: which row the line belongs to, and where it is.
    fn default() -> Self {
        Self {
            parts: vec![CaptionPart::RowName, CaptionPart::Price],
            place: CaptionPlace::RightEnd,
            show: CaptionShow::Always,
        }
    }
}
