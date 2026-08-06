//! The figure type union and its dispatch to the tool that owns it.
//!
//! This `match` is deliberately the only place INSIDE the drawing layer that lists every tool:
//! the panel, the renderer, the toolbar and the persistence all go through [`ToolShape`] or the
//! registry. Two lists outside it stay hand-written on purpose — the per-tool hotkey fields in
//! `hotkeys.toml` and the core's chart-object types in `alert_blob` (see `tools`).

use serde::{Deserialize, Serialize};

use super::tools::{
    Channel, FibRetracement, FigureTool, HLine, MbFib, Position, Ray, Rect, Segment, ToolShape, Triangle,
};

/// Figure type. The JSON representation matches Moonbot's `TChartObject` names that were used
/// before tools became modules: `{"HLine":{"price":…}}`, `{"Segment":{"a":…,"b":…}}` and so on,
/// so `figures.json` written by an older build still loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FigureKind {
    HLine(HLine),
    Segment(Segment),
    Ray(Ray),
    Position(Position),
    Triangle(Triangle),
    Channel(Channel),
    FibRetracement(FibRetracement),
    Rect(Rect),
    /// Moonbot's own Fibonacci object: drawn here on Moonbot's terms, and decoded from the core.
    MbFib(MbFib),
}

impl FigureKind {
    /// The tool that draws this figure, read from its own registry row.
    pub fn tool(&self) -> FigureTool {
        self.shape().def().tool
    }

    /// Read access to the tool's geometry and hit test.
    pub fn shape(&self) -> &dyn ToolShape {
        match self {
            FigureKind::HLine(t) => t,
            FigureKind::Segment(t) => t,
            FigureKind::Ray(t) => t,
            FigureKind::Position(t) => t,
            FigureKind::Triangle(t) => t,
            FigureKind::Channel(t) => t,
            FigureKind::FibRetracement(t) => t,
            FigureKind::Rect(t) => t,
            FigureKind::MbFib(t) => t,
        }
    }

    /// Write access, for dragging.
    pub fn shape_mut(&mut self) -> &mut dyn ToolShape {
        match self {
            FigureKind::HLine(t) => t,
            FigureKind::Segment(t) => t,
            FigureKind::Ray(t) => t,
            FigureKind::Position(t) => t,
            FigureKind::Triangle(t) => t,
            FigureKind::Channel(t) => t,
            FigureKind::FibRetracement(t) => t,
            FigureKind::Rect(t) => t,
            FigureKind::MbFib(t) => t,
        }
    }

    /// Reference price used by the alert list's Price column and sorting: the figure's first
    /// handle, which every tool orders first.
    pub fn anchor_price(&self) -> f64 {
        self.shape().handle(0).map_or(0.0, |n| n.price)
    }
}
