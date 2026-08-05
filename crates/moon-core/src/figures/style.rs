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

/// Current drawing style (color, thickness, and line style), applied to NEW figures and edited
/// in the pencil popup. Stored in the UI backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawStyle {
    /// RGBA color; `a` is the "Opacity" value from the popup.
    pub color: [u8; 4],
    pub thickness: f32,
    pub kind: LineKind,
}

impl Default for DrawStyle {
    fn default() -> Self {
        // Light blue to distinguish figures from order lines, 1 px, dashed.
        Self {
            color: [64, 196, 255, 255],
            thickness: 1.0,
            kind: LineKind::Dash,
        }
    }
}
