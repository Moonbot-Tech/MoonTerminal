//! The primitive vocabulary a tool draws with, and the sink that receives it.
//!
//! A tool never touches a GPU buffer: it calls [`GeomSink`] with figure coordinates, and the
//! renderer (`moon-chart`) turns each call into instances for the own-pass layers. Adding a
//! primitive — a bounded fill, a polygon — is a method here plus its renderer side, and every
//! existing tool keeps compiling.
//!
//! Text is emitted as a [`LabelText`] VALUE, not as a finished string: price and percentage
//! formatting belongs to the chart, which already formats its axes and order labels, and a second
//! copy of that formatting here would drift from it.

use super::node::FigNode;
use super::style::LineKind;

/// Resolved stroke for one figure in its current state (idle, armed, hovered or selected).
///
/// The state → stroke decision belongs to the renderer, which knows the theme; a tool only draws
/// with what it is handed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    /// Premultiplied-by-nothing RGBA in `0..=1`, ready for a vertex color.
    pub color: [f32; 4],
    /// Thickness in physical pixels.
    pub thickness: f32,
    pub kind: LineKind,
}

/// Where a label sits relative to its anchor node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelPlace {
    /// Above the anchor point.
    Above,
    /// At the plot's right edge, at the anchor's PRICE: the anchor's time is ignored. A
    /// full-width line has no point to aim at, and pinning its label to the edge is where every
    /// price label on this chart already sits.
    RightEdge,
}

/// What a label says. Formatting happens in the chart's text pass, with the same precision as the
/// price axis beside it, so a figure's readout and the axis never disagree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LabelText {
    /// A price, formatted with the chart's own price precision.
    Price(f64),
    /// A signed percentage from `from` to `to`, e.g. `+1.25%`.
    PctDelta { from: f64, to: f64 },
}

/// Receiver for the primitives a figure is made of.
///
/// Every position is in FIGURE coordinates (Unix ms, price). Conversion to the view's relative
/// time and to pixels belongs to the implementor.
pub trait GeomSink {
    /// Horizontal line at `price`, spanning the whole plot.
    fn hline(&mut self, price: f64, stroke: &Stroke);

    /// Line segment between two nodes.
    fn seg(&mut self, a: FigNode, b: FigNode, stroke: &Stroke);

    /// Square drag handle at a node. Emitted by the generic build for a selected figure whose
    /// tool grabs by handle, so a tool does not push its own.
    fn handle(&mut self, at: FigNode, color: [f32; 4]);

    /// Text anchored to a node. Only a figure under the cursor, or the one being drawn, emits a
    /// label ([`BuildCtx::hot`]) — never a merely selected one, which is a sticky state — so an
    /// idle chart pays nothing for them.
    fn label(&mut self, at: FigNode, place: LabelPlace, text: LabelText, color: [f32; 4]);
}

/// Everything a tool needs to know while emitting its geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuildCtx {
    /// Stroke resolved for this figure's current state.
    pub stroke: Stroke,
    /// Whether the figure is under the cursor or being drawn: it may emit a readout label.
    pub hot: bool,
    /// Whether the figure is selected: it emits its drag handles.
    pub handles: bool,
}
