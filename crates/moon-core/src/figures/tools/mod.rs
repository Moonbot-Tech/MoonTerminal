//! The tool registry: one module per drawing tool, one row per tool, and the generic operations
//! every tool inherits.
//!
//! A tool implements [`ToolShape`] — its handles, its geometry, its hit test — and appears once in
//! [`REGISTRY`]. Everything the panel does with a figure is written ONCE here against that trait:
//! picking a handle, dragging a handle or the body, emitting drag knots, previewing a draft.
//! There is no per-tool branch outside a tool's own module, and [`super::FigureKind`]'s dispatch
//! `match` is the only place that lists them all.
//!
//! Adding a tool: a module here, a `FigureKind` arm, a `REGISTRY` row, and its locale keys. Two
//! places outside this crate still name tools one by one and are meant to: `hotkeys.rs`, because
//! each per-tool binding is a separate user-editable field in `hotkeys.toml`, and `alert_blob.rs`,
//! because the core's chart-object types are its format, not ours.

mod channel;
mod fib_retracement;
mod hline;
mod mb_fib;
mod position;
mod ray;
mod rect;
mod segment;
mod triangle;

pub use channel::Channel;
pub use fib_retracement::FibRetracement;
pub use hline::HLine;
pub use mb_fib::{MbFib, MB_FIB_LEVELS, MB_FIB_RATIOS};
pub use position::Position;
pub use ray::Ray;
pub use rect::Rect;
pub use segment::Segment;
pub use triangle::Triangle;

use super::kind::FigureKind;
use super::node::FigNode;
use super::proj::{hline_dist, pt_dist, Proj, PxPoint};
use super::sink::{BuildCtx, GeomSink};
use super::Figure;

/// What a drag is holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    /// A specific handle, by index into the tool's handle list.
    Handle(usize),
    /// The whole figure, translated in time and price.
    Body,
}

/// How a tool's handles are grabbed and drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabMode {
    /// Square knots at the handle points, grabbed by pixel distance to the point. Segments,
    /// triangles and anything else with real `(time, price)` vertices.
    Knots,
    /// No knots: grab the horizontal line the handle sits on, by vertical distance alone. A
    /// horizontal line or a price channel has no meaningful X to aim at.
    PriceLines,
}

/// One switch a tool offers in the per-figure settings, beyond the style every figure has.
///
/// Toggles only, deliberately: what a tool needs to expose today is "draw this part or not" — the
/// levels of a ratio scale — and a settings surface that can only render one control kind cannot
/// grow a per-tool special case. A tool needing a number or a choice adds a variant here, and the
/// one settings module learns to draw it once for every tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSetting {
    /// Stable key the UI passes back to [`ToolShape::set_setting`]. Never localized.
    pub key: String,
    /// What to show beside the switch. A ratio scale labels its levels with the ratios themselves,
    /// which are numbers rather than words, so this is TEXT and not a locale key.
    pub label: String,
    pub on: bool,
}

/// A tool's switches as the user left them, keyed by [`ToolSetting::key`].
///
/// Sparse on purpose: only what was changed away from the tool's own default is stored, so a switch
/// added to a tool later starts at that tool's default rather than at whatever an older map happens
/// to hold. `BTreeMap` and not `HashMap` because the order is then stable to read and to compare.
pub type ToolSettings = std::collections::BTreeMap<String, bool>;

/// A throwaway figure of `tool`, used to ask a tool about itself before any figure of it exists.
///
/// The nodes are arbitrary but distinct: `settings()` describes the tool, not the geometry, and a
/// degenerate figure could make a tool refuse to build. Never drawn, never stored.
fn proto(tool: FigureTool) -> Option<FigureKind> {
    let def = tool.def();
    let nodes: Vec<FigNode> = (0..def.clicks)
        .map(|i| FigNode::new(i as f64, 1.0 + i as f64))
        .collect();
    (def.make)(&nodes)
}

/// The switches `tool` offers, with `stored` already applied — what a settings surface for the TOOL
/// (rather than for one figure) draws.
///
/// Goes through the same [`ToolShape::settings`] a live figure answers with, so the toolbar and the
/// right-click panel cannot end up offering different switches for the same tool.
pub fn settings_of(tool: FigureTool, stored: &ToolSettings) -> Vec<ToolSetting> {
    let Some(mut kind) = proto(tool) else {
        return Vec::new();
    };
    apply_settings(&mut kind, stored);
    kind.shape().settings()
}

/// Applies stored switches to a figure. Returns whether any of them changed it.
///
/// The one place a tool's defaults reach a new figure, so a tool cannot be given switches that are
/// offered in the toolbar and then ignored by what is drawn. Unknown keys are dropped by
/// [`ToolShape::set_setting`] itself.
pub fn apply_settings(kind: &mut FigureKind, stored: &ToolSettings) -> bool {
    let shape = kind.shape_mut();
    let mut changed = false;
    for (key, on) in stored {
        changed |= shape.set_setting(key, *on);
    }
    changed
}

/// Geometry, handles and hit test of one drawing tool.
///
/// Implementations are pure math on figure coordinates: no GPU type, no UI type, no clock.
pub trait ToolShape {
    /// This tool's registry row — the tool's own `DEF`, so a figure names its tool without a
    /// second `match` over every type.
    fn def(&self) -> &'static ToolDef;

    /// Number of editable handles.
    fn handle_count(&self) -> usize;

    /// Handle `i`, or `None` when out of range.
    fn handle(&self, i: usize) -> Option<FigNode>;

    /// Moves handle `i` to `to`. Returns whether anything changed. Price-only tools ignore the
    /// time component.
    fn move_handle(&mut self, i: usize, to: FigNode) -> bool;

    /// Translates the whole figure. Price-only tools ignore `dt_ms`.
    fn translate(&mut self, dt_ms: f64, dp: f64) -> bool;

    /// Pixel distance from `pos` to the figure's body. Values above the caller's threshold are a
    /// miss; the caller compares figures by this distance to pick the nearest.
    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32;

    /// Emits the figure's own primitives. Drag knots are NOT emitted here — [`build_figure`] adds
    /// them generically for every tool that grabs by knot.
    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink);

    fn grab_mode(&self) -> GrabMode {
        GrabMode::Knots
    }

    /// Switches this tool adds to the per-figure settings. Empty for a tool whose whole appearance
    /// is the shared style, which is most of them.
    fn settings(&self) -> Vec<ToolSetting> {
        Vec::new()
    }

    /// Applies one switch by its key. Returns whether anything changed, so the caller knows
    /// whether to persist and redraw. An unknown key is ignored rather than panicking: the UI and
    /// the tool are versioned together, but a stale popup must not take the app down.
    fn set_setting(&mut self, _key: &str, _on: bool) -> bool {
        false
    }
}

/// Everything generic code needs to know about a tool without naming its type.
pub struct ToolDef {
    pub tool: FigureTool,
    /// Stable identifier used for element IDs and, later, for persisted per-tool settings. Never
    /// change one: it outlives the enum order.
    pub key: &'static str,
    /// Locale key of the tool's display name.
    pub locale_key: &'static str,
    /// Placeholder glyph for the toolbar button until the icon set lands.
    pub glyph: &'static str,
    /// Clicks needed to finish a figure.
    pub clicks: u8,
    /// Whether the core's `TChartObject` blob can carry this figure, i.e. whether it can be armed
    /// as an alert. A tool the core does not know is drawn locally only.
    pub alertable: bool,
    /// Whether the tool READS the style's fill. The settings panel hides the fill controls while a
    /// tool that does not is selected, rather than offering a setting that changes nothing.
    ///
    /// Not the same as "encloses an area": a triangle does, but the fill primitive paints an
    /// axis-aligned band and a triangle needs a polygon, so it joins them when that lands.
    pub fills: bool,
    /// The tool's own fill colour, when it has one, as a hue that stands for it.
    ///
    /// `Some` means the tool colours its own fills and the style contributes only the opacity — a
    /// Fibonacci level is recognised by its hue, and a position's two zones ARE profit and loss.
    /// The settings panel then offers this swatch instead of its palette, because a colour it let
    /// the user pick would change nothing. Returned by a function rather than stored flat so the
    /// swatch cannot drift from the colours the tool actually draws.
    pub scale_swatch: Option<fn() -> [u8; 3]>,
    /// Builds the finished figure from exactly `clicks` placed nodes. Returns `None` when given
    /// fewer, so a caller cannot construct a half-placed figure.
    pub make: fn(&[FigNode]) -> Option<FigureKind>,
    /// Builds the preview under the cursor from the nodes placed so far. `None` means "nothing to
    /// preview yet".
    pub preview: fn(&[FigNode], FigNode) -> Option<FigureKind>,
}

/// Drawing tool selected in the settings panel; the application-wide selection lives in the UI
/// backend. The enum is the stable handle used by hotkeys and state; everything else about a tool
/// is read from its [`ToolDef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigureTool {
    HLine,
    Segment,
    /// Moonbot's own Fibonacci object, drawn here and on Moonbot's terms; see [`MbFib`].
    MbFib,
    Triangle,
    Channel,
    FibRetracement,
    Rect,
    /// Planned trade: entry, target, stop and the two zones between them; see [`Position`].
    Position,
    /// Half-infinite line: an origin and a direction; see [`Ray`].
    Ray,
}

/// Every tool, in menu order. The toolbar, the hotkey CYCLE, the alerts list and the tests all
/// read it; the per-tool hotkey fields in `hotkeys.toml` are the one list still written by hand,
/// so a tool added here is reachable through the cycle but has no binding of its own until it is
/// added there too.
///
/// Ordered by the core's own chart-object types — line, segment, fibo, triangle, channel — then by
/// what only we have, so the tools a Moonbot user already knows read in the order they know them.
/// The toolbar splits its two groups on [`ToolDef::alertable`] and not on a position here, so a
/// tool changes group by becoming sendable rather than by being moved.
///
/// The two Fibonacci tools sit at type 3's position, though the toolbar shows them in different
/// groups — it splits on [`ToolDef::alertable`], and only [`MbFib`] goes to the core.
/// [`FibRetracement`] is ours to change and stays local. Note that this order is also the
/// `switch_figure` hotkey CYCLE order.
pub const REGISTRY: &[ToolDef] = &[
    hline::DEF,
    segment::DEF,
    fib_retracement::DEF,
    mb_fib::DEF,
    triangle::DEF,
    channel::DEF,
    rect::DEF,
    ray::DEF,
    position::DEF,
];

/// `def()` and `next()` index the registry; an empty one would panic in the frame loop.
const _: () = assert!(!REGISTRY.is_empty());

impl FigureTool {
    /// This tool's registry row.
    ///
    /// Falls back to the first row when a tool is missing from the registry, which the registry
    /// contract test rules out; a panic here would take the frame loop down for a table typo.
    pub fn def(self) -> &'static ToolDef {
        REGISTRY
            .iter()
            .find(|d| d.tool == self)
            .unwrap_or(&REGISTRY[0])
    }

    /// Next tool in the cycle; the `switch_figure` hotkey advances through the registry and wraps.
    pub fn next(self) -> FigureTool {
        let i = REGISTRY.iter().position(|d| d.tool == self).unwrap_or(0);
        REGISTRY[(i + 1) % REGISTRY.len()].tool
    }
}

/// Emits a figure's geometry plus, for a selected knot-grabbed figure, its drag handles.
///
/// This is the single entry point the renderer calls, so no tool can forget its knots and no two
/// tools can disagree about how a handle looks.
pub fn build_figure(kind: &FigureKind, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
    let shape = kind.shape();
    shape.build(ctx, sink);
    if ctx.handles && shape.grab_mode() == GrabMode::Knots {
        for i in 0..shape.handle_count() {
            if let Some(n) = shape.handle(i) {
                sink.handle(n, ctx.stroke.color);
            }
        }
    }
}

/// Picks the handle under the cursor within `threshold` pixels, preferring the nearest.
///
/// [`GrabMode::Knots`] measures the distance to the handle POINT; [`GrabMode::PriceLines`]
/// measures the vertical distance to the handle's price, because such a tool draws a full-width
/// line with no point to aim at.
pub fn pick_handle(
    kind: &FigureKind,
    pos: PxPoint,
    proj: &dyn Proj,
    threshold: f32,
) -> Option<usize> {
    let shape = kind.shape();
    let by_line = shape.grab_mode() == GrabMode::PriceLines;
    let mut best: Option<(usize, f32)> = None;
    for i in 0..shape.handle_count() {
        let Some(n) = shape.handle(i) else { continue };
        let d = if by_line {
            hline_dist(pos, n.price, proj)
        } else {
            pt_dist(pos, proj.px_of(n))
        };
        if d <= threshold && best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// Picks the nearest figure body under the cursor within `threshold` pixels.
///
/// The panel hit-tests for hover, for selection and for the context menu; all three ask this one
/// question, so the answer lives here beside [`pick_handle`] rather than being re-inlined per call
/// site in the UI.
pub fn pick_figure<'a>(
    figures: impl IntoIterator<Item = &'a Figure>,
    pos: PxPoint,
    proj: &dyn Proj,
    threshold: f32,
) -> Option<u64> {
    let mut best: Option<(u64, f32)> = None;
    for fig in figures {
        let d = fig.kind.shape().hit(pos, proj);
        if d <= threshold && best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((fig.id, d));
        }
    }
    best.map(|(id, _)| id)
}

/// Applies one drag step to a figure. Returns whether it changed.
///
/// Drags move by DELTA rather than to an absolute target: the figure then follows the cursor
/// without jumping to it, no per-tool anchor table is needed to say which point the grab offset is
/// measured from, and a price-only tool ignores the time component by itself.
pub fn drag_figure(fig: &mut Figure, grab: Grab, dt_ms: f64, dp: f64) -> bool {
    if dt_ms == 0.0 && dp == 0.0 {
        return false;
    }
    let shape = fig.kind.shape_mut();
    match grab {
        Grab::Body => shape.translate(dt_ms, dp),
        Grab::Handle(i) => match shape.handle(i) {
            Some(n) => shape.move_handle(i, n.shifted(dt_ms, dp)),
            None => false,
        },
    }
}

#[cfg(test)]
mod tests;
