//! Where the crosshair's three values print around the order book (#1118).
//!
//! The crosshair carries three figures beside its horizontal line: the armed manual order size,
//! the book depth summed from the best price to the cursor, and the cursor's percent from the
//! best price of its side. They used to have fixed homes — the size on the chart against the
//! book's edge, the depth and the percent inside the book at its left edge — and on the ladder
//! that edge is where every row prints its volume, so the two covered each other. Each figure now
//! names its own place, and figures sharing a place stack in a column in the reader's order.
//!
//! The decision of WHICH figure goes WHERE and HOW FAR out lives in `moon_chart::cursor_labels`;
//! this module only holds what is stored and how a stored file is read.

use super::*;

/// One of the crosshair's figures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorLabel {
    /// The manual order size currently armed, in quote.
    OrderSize,
    /// Cumulative book notional from the best price of the cursor's side to the cursor.
    Depth,
    /// The cursor's percent from the best price of its side.
    Percent,
}

impl CursorLabel {
    /// The three figures in the order they stack by default: nearest the line first.
    pub const DEFAULT_ORDER: [CursorLabel; 3] = [
        CursorLabel::OrderSize,
        CursorLabel::Depth,
        CursorLabel::Percent,
    ];
}

/// The horizontal home a crosshair figure prints at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CursorLabelAnchor {
    /// On the chart, right-aligned against the order book's left edge.
    Chart,
    /// Inside the book, left-aligned at its left edge.
    Book,
    /// Inside the book, past the widest row volume the ladder prints. Without ladder row text
    /// (the bars, or rows too thin to print) it is [`Self::Book`].
    AfterVolumes,
    /// Left of the crosshair's vertical line.
    CrossLeft,
    /// Right of the crosshair's vertical line.
    CrossRight,
}

/// Where one crosshair figure prints: a home and a side of the horizontal line, or nowhere.
///
/// One flat list rather than a home plus a side, because it is ONE choice in the popup — the reader
/// picks "Book, below" from one dropdown — and because a pair would let a hidden figure keep a
/// side that means nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorLabelSpot {
    /// On the chart against the book, above the line.
    ChartAbove,
    /// On the chart against the book, below the line.
    ChartBelow,
    /// Inside the book at its left edge, above the line.
    BookAbove,
    /// Inside the book at its left edge, below the line.
    BookBelow,
    /// Inside the book past the row volumes, above the line.
    AfterVolumesAbove,
    /// Inside the book past the row volumes, below the line.
    AfterVolumesBelow,
    /// Left of the crosshair, above the line.
    CrossLeftAbove,
    /// Left of the crosshair, below the line.
    CrossLeftBelow,
    /// Right of the crosshair, above the line.
    CrossRightAbove,
    /// Right of the crosshair, below the line.
    CrossRightBelow,
    /// Not drawn.
    Hidden,
}

impl CursorLabelSpot {
    /// Every spot, in the order the popup lists them.
    pub const ALL: [CursorLabelSpot; 11] = [
        CursorLabelSpot::ChartAbove,
        CursorLabelSpot::ChartBelow,
        CursorLabelSpot::BookAbove,
        CursorLabelSpot::BookBelow,
        CursorLabelSpot::AfterVolumesAbove,
        CursorLabelSpot::AfterVolumesBelow,
        CursorLabelSpot::CrossLeftAbove,
        CursorLabelSpot::CrossLeftBelow,
        CursorLabelSpot::CrossRightAbove,
        CursorLabelSpot::CrossRightBelow,
        CursorLabelSpot::Hidden,
    ];

    /// The home and whether the figure sits BELOW the line; `None` when hidden.
    pub fn place(self) -> Option<(CursorLabelAnchor, bool)> {
        use CursorLabelAnchor as A;
        Some(match self {
            CursorLabelSpot::ChartAbove => (A::Chart, false),
            CursorLabelSpot::ChartBelow => (A::Chart, true),
            CursorLabelSpot::BookAbove => (A::Book, false),
            CursorLabelSpot::BookBelow => (A::Book, true),
            CursorLabelSpot::AfterVolumesAbove => (A::AfterVolumes, false),
            CursorLabelSpot::AfterVolumesBelow => (A::AfterVolumes, true),
            CursorLabelSpot::CrossLeftAbove => (A::CrossLeft, false),
            CursorLabelSpot::CrossLeftBelow => (A::CrossLeft, true),
            CursorLabelSpot::CrossRightAbove => (A::CrossRight, false),
            CursorLabelSpot::CrossRightBelow => (A::CrossRight, true),
            CursorLabelSpot::Hidden => return None,
        })
    }

    /// The dictionary key of this spot's name in the popup's list.
    pub fn locale_key(self) -> &'static str {
        match self {
            CursorLabelSpot::ChartAbove => "chart.graphics.cursor_spot.chart_above",
            CursorLabelSpot::ChartBelow => "chart.graphics.cursor_spot.chart_below",
            CursorLabelSpot::BookAbove => "chart.graphics.cursor_spot.book_above",
            CursorLabelSpot::BookBelow => "chart.graphics.cursor_spot.book_below",
            CursorLabelSpot::AfterVolumesAbove => "chart.graphics.cursor_spot.after_volumes_above",
            CursorLabelSpot::AfterVolumesBelow => "chart.graphics.cursor_spot.after_volumes_below",
            CursorLabelSpot::CrossLeftAbove => "chart.graphics.cursor_spot.cross_left_above",
            CursorLabelSpot::CrossLeftBelow => "chart.graphics.cursor_spot.cross_left_below",
            CursorLabelSpot::CrossRightAbove => "chart.graphics.cursor_spot.cross_right_above",
            CursorLabelSpot::CrossRightBelow => "chart.graphics.cursor_spot.cross_right_below",
            CursorLabelSpot::Hidden => "chart.graphics.cursor_spot.hidden",
        }
    }
}

/// Where each crosshair figure prints and the order figures sharing a place stack in.
///
/// ABSENT, every figure keeps the home it had before the choice existed — the size on the chart
/// above the line, the depth inside the book above it, the percent inside the book below it — so
/// an upgrade moves nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorLabelsCfg {
    /// Where the armed order size prints.
    #[serde(
        default = "def_order_size_spot",
        deserialize_with = "de_order_size_spot"
    )]
    pub order_size: CursorLabelSpot,
    /// Where the book depth prints.
    #[serde(default = "def_depth_spot", deserialize_with = "de_depth_spot")]
    pub depth: CursorLabelSpot,
    /// Where the percent prints.
    #[serde(default = "def_percent_spot", deserialize_with = "de_percent_spot")]
    pub percent: CursorLabelSpot,
    /// The stacking order, nearest the line first: figures sharing a place and a side stack
    /// outward in this order. Always the three figures once each; [`Self::normalized`] repairs a
    /// hand-edited list that is not.
    #[serde(default = "def_order", deserialize_with = "de_order")]
    pub order: [CursorLabel; 3],
}

impl Default for CursorLabelsCfg {
    /// The homes the figures had before they could be moved.
    fn default() -> Self {
        Self {
            order_size: def_order_size_spot(),
            depth: def_depth_spot(),
            percent: def_percent_spot(),
            order: def_order(),
        }
    }
}

impl CursorLabelsCfg {
    /// The spot stored for one figure.
    pub fn spot(&self, label: CursorLabel) -> CursorLabelSpot {
        match label {
            CursorLabel::OrderSize => self.order_size,
            CursorLabel::Depth => self.depth,
            CursorLabel::Percent => self.percent,
        }
    }

    /// Store a spot for one figure.
    pub fn set_spot(&mut self, label: CursorLabel, spot: CursorLabelSpot) {
        match label {
            CursorLabel::OrderSize => self.order_size = spot,
            CursorLabel::Depth => self.depth = spot,
            CursorLabel::Percent => self.percent = spot,
        }
    }

    /// Move one figure a step nearer the line (`toward_line`) or a step out in the stacking
    /// order. A figure already at that end stays put.
    pub fn shift(&mut self, label: CursorLabel, toward_line: bool) {
        let Some(at) = self.order.iter().position(|l| *l == label) else {
            return;
        };
        let to = if toward_line {
            at.checked_sub(1)
        } else {
            Some(at + 1).filter(|i| *i < self.order.len())
        };
        if let Some(to) = to {
            self.order.swap(at, to);
        }
    }

    /// The same settings with an order that names each figure exactly once.
    ///
    /// A hand-edited file can repeat a figure and drop another; a figure missing from the order
    /// would never be drawn, which no choice in the popup can state, so such a list reads as the
    /// default order.
    pub fn normalized(self) -> Self {
        let complete = CursorLabel::DEFAULT_ORDER
            .iter()
            .all(|label| self.order.contains(label));
        Self {
            order: if complete { self.order } else { def_order() },
            ..self
        }
    }
}

/// The armed order size's home before it could move: on the chart, above the line.
fn def_order_size_spot() -> CursorLabelSpot {
    CursorLabelSpot::ChartAbove
}

/// The depth's home before it could move: inside the book, above the line.
fn def_depth_spot() -> CursorLabelSpot {
    CursorLabelSpot::BookAbove
}

/// The percent's home before it could move: inside the book, below the line.
fn def_percent_spot() -> CursorLabelSpot {
    CursorLabelSpot::BookBelow
}

/// The default stacking order.
fn def_order() -> [CursorLabel; 3] {
    CursorLabel::DEFAULT_ORDER
}

/// Read the order size's spot leniently: a spelling this build does not know keeps its default.
fn de_order_size_spot<'de, D>(d: D) -> Result<CursorLabelSpot, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, CursorLabelSpot>(d)?.unwrap_or_else(def_order_size_spot))
}

/// Read the depth's spot leniently.
fn de_depth_spot<'de, D>(d: D) -> Result<CursorLabelSpot, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, CursorLabelSpot>(d)?.unwrap_or_else(def_depth_spot))
}

/// Read the percent's spot leniently.
fn de_percent_spot<'de, D>(d: D) -> Result<CursorLabelSpot, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, CursorLabelSpot>(d)?.unwrap_or_else(def_percent_spot))
}

/// Read the stacking order leniently; [`CursorLabelsCfg::normalized`] repairs a usable but
/// incomplete one.
fn de_order<'de, D>(d: D) -> Result<[CursorLabel; 3], D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, [CursorLabel; 3]>(d)?.unwrap_or_else(def_order))
}

/// Read the whole block leniently: a malformed table costs the crosshair's places, not the rest
/// of the chart graphics around it.
pub(super) fn de_cursor_labels<'de, D>(d: D) -> Result<CursorLabelsCfg, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_lenient::<D, CursorLabelsCfg>(d)?
        .unwrap_or_default()
        .normalized())
}

#[cfg(test)]
mod tests;
