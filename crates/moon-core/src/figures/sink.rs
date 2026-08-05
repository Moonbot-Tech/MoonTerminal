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
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LabelPlace {
    /// Above the anchor point.
    Above,
    /// At the plot's right edge, at the anchor's PRICE: the anchor's time is ignored. A
    /// full-width line has no point to aim at, and pinning its label to the edge is where every
    /// price label on this chart already sits.
    RightEdge,
    /// At the right end of a horizontal line spanning `t0_ms..t1_ms`, clipped INTO the plot.
    ///
    /// A scale of eleven levels is read as a column of numbers; anchoring each to its own line
    /// keeps that column beside the lines it names, and clipping keeps it on screen while any
    /// part of the line is — panning the line's end away must not blank the whole scale.
    LineEnd { t0_ms: f64, t1_ms: f64 },
}

/// What a label says. Formatting happens in the chart's text pass, with the same precision as the
/// price axis beside it, so a figure's readout and the axis never disagree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LabelText {
    /// A price, formatted with the chart's own price precision.
    Price(f64),
    /// A signed percentage from `from` to `to`, e.g. `+1.25%`.
    PctDelta { from: f64, to: f64 },
    /// A level of a ratio scale and the price it sits at, e.g. `0.618 (6109.48)`.
    Level { ratio: f64, price: f64 },
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

    /// Filled band between two prices, bounded in time by two nodes' times.
    ///
    /// The band is axis-aligned: it fills a price range over a time range, which is what a
    /// Fibonacci zone, a rectangle, a range and a position box all are. A slanted or rotated fill
    /// (a parallel channel, a pitchfork) needs a polygon primitive and is deliberately not this.
    fn band(&mut self, t0_ms: f64, t1_ms: f64, p0: f64, p1: f64, color: [f32; 4]);

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
    /// Colour a FILL must use, resolved from the figure's own colour and NOT from its interaction
    /// state.
    ///
    /// Fills are drawn in the chart's base cache, which is re-baked whenever their signature
    /// changes. A fill that brightened under the cursor would re-bake the background, the grid,
    /// the candles and the order book of every pane on a mouse-over — for a tint nobody asked for.
    pub fill: [f32; 4],
    /// Whether the figure is under the cursor or being drawn: it may emit a readout label.
    pub hot: bool,
    /// Whether the figure is selected: it emits its drag handles.
    pub handles: bool,
}
