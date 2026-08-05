//! Drawing style: the palette the pencil popup edits and the stroke a tool draws with.

use serde::{Deserialize, Serialize};

/// Line style corresponding to Moonbot's "Kind" and Delphi's `TPenStyle` at blob offset 13:
/// Solid=0, Dash=1, Dot=2, DashDot=3, DashDotDot=4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LineKind {
    Solid,
    #[default]
    Dash,
    Dot,
    DashDot,
    DashDotDot,
}

impl LineKind {
    pub const ALL: [LineKind; 5] = [
        LineKind::Solid,
        LineKind::Dash,
        LineKind::Dot,
        LineKind::DashDot,
        LineKind::DashDotDot,
    ];

    /// `TPenStyle` value stored at blob offset 13.
    pub fn to_pen(self) -> u32 {
        match self {
            LineKind::Solid => 0,
            LineKind::Dash => 1,
            LineKind::Dot => 2,
            LineKind::DashDot => 3,
            LineKind::DashDotDot => 4,
        }
    }

    pub fn from_pen(v: u32) -> Self {
        match v {
            0 => LineKind::Solid,
            2 => LineKind::Dot,
            3 => LineKind::DashDot,
            4 => LineKind::DashDotDot,
            _ => LineKind::Dash,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineKind::Solid => "Solid",
            LineKind::Dash => "Dash",
            LineKind::Dot => "Dot",
            LineKind::DashDot => "DashDot",
            LineKind::DashDotDot => "DashDotDot",
        }
    }

    /// Whether the line is solid, used to map horizontal lines to `LineInstance.style` 0/1.
    pub fn is_solid(self) -> bool {
        self == LineKind::Solid
    }
}

/// Current drawing style (color, thickness, line style and fill), applied to NEW figures and
/// edited in the pencil popup. Stored in the UI backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawStyle {
    /// RGBA color of the LINES; `a` is the "Opacity" value from the popup.
    pub color: [u8; 4],
    pub thickness: f32,
    pub kind: LineKind,
    /// RGBA color of the FILL of any tool that encloses an area — a Fibonacci band, a price
    /// corridor, a rectangle. Its alpha is the fill's own opacity, and **zero means no fill**, so
    /// one field carries the colour, the strength and the on/off switch.
    ///
    /// Separate from `color` on purpose: a fill readable at the same opacity as its outline buries
    /// the candles under it, and the two are chosen independently in every charting package.
    pub fill: [u8; 4],
}

/// Fill opacity a new figure starts with, as a fraction of full.
///
/// Faint enough to read candles through, strong enough to see at a glance on a dark chart — the
/// range every charting package's default sits in.
pub const DEFAULT_FILL_ALPHA: u8 = 46;

impl DrawStyle {
    /// Whether a fill is drawn at all.
    pub fn has_fill(&self) -> bool {
        self.fill[3] > 0
    }
}

impl Default for DrawStyle {
    fn default() -> Self {
        // Light blue to distinguish figures from order lines, 1 px, dashed, with a faint fill of
        // the same hue.
        Self {
            color: [64, 196, 255, 255],
            thickness: 1.0,
            kind: LineKind::Dash,
            fill: [64, 196, 255, DEFAULT_FILL_ALPHA],
        }
    }
}
