//! Renderer side of the drawing layer: figures from `moon_core::figures` → own-pass instances.
//! APPENDS to the buffers (does not clear them): called after `build_order_geometry`, with
//! figures using the same userdata layers.
//!
//! The geometry of a figure belongs to its tool (`moon_core::figures::tools`); this module owns
//! only what a tool must not know — the theme-facing decision of how a figure LOOKS in each
//! interaction state, and the mapping of the tool's primitives onto GPU instances. Adding a tool
//! therefore never touches this file; adding a PRIMITIVE touches it and nothing else.
//!
//! Fills go to the ORDER-ZONE layer, which is baked into the chart's base cache. Their colour is
//! therefore resolved from the figure and its tool alone — never from hover or selection: a fill
//! that brightened under the cursor would re-bake the background, grid, candles and book of every
//! pane. (A tool with a typed scale takes the hue from its own level and only the OPACITY from the
//! figure; the rule that matters here is the same — no interaction state reaches a fill.)
//!
//! The ONE exception is the band being drawn: a Zone or rectangle in progress paints its fill and
//! pays that re-bake while the draft follows the cursor, because there the area is precisely what
//! the user is aiming. See `Sink::fills`.
//!
//! Visual language (to distinguish figures from order lines and make their state visible):
//! - regular figure — THIN BASE-STYLE LINE;
//! - armed (alert) figure — THICK BASE-STYLE LINE (clearly shows that it is armed);
//! - hovered / selected figure — SOLID, THICK, and bright (it sharply "comes alive"), with square
//!   knots at the editable points of a SELECTED figure and a readout beside a HOVERED one.

use std::sync::Arc;

use moon_core::figures::{
    build_figure, BuildCtx, FigNode, Figure, GeomSink, LabelPlace, LabelText, LineKind, Stroke,
};

use crate::layers::{
    LineInstance, MarkerInstance, SegInstance, ZoneInstance, MARKER_SHAPE_KNOT, SEG_EXTEND_NONE,
    SEG_EXTEND_RAY, TIME_UNBOUNDED,
};

/// Opacity of an idle (inactive) figure.
const FIG_IDLE_ALPHA: f32 = 0.85;
/// Thickness multiplier for an active (hovered/selected) figure.
const FIG_ACTIVE_THICKNESS: f32 = 1.9;
const FIG_ARMED_THICKNESS: f32 = 2.2;
/// Size of a selected figure's knot/handle, in pixels.
const FIG_KNOT_SIZE: f32 = 4.5;
/// Outline thickness of that knot, in pixels.
const FIG_KNOT_THICKNESS: f32 = 1.5;
/// A figure's text readout, drawn by the chart's text pass.
///
/// Carries a VALUE rather than a finished string: the text pass formats a price with the same
/// precision as the axis it sits beside. Most tools label only the figure under the cursor and
/// the one being drawn; a ratio scale labels its levels always, because a level whose price shows
/// only under the cursor cannot be read at a glance.
/// What a figure's readout says, resolved for the renderer.
///
/// A price and a percentage stay VALUES so the text pass can format them with the price axis's
/// own precision. A ratio level does not: its format is pure and deliberately unlike the axis, so
/// it is rendered to text here — once per rebuild — and the per-frame pass only draws it.
#[derive(Debug, Clone, PartialEq)]
pub enum LabelValue {
    Price(f64),
    PctDelta {
        from: f64,
        to: f64,
    },
    /// Finished text; cloning it is a refcount bump, not an allocation.
    Ready(Arc<str>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FigureLabel {
    /// Time relative to the chart epoch, in milliseconds. Unused by [`LabelPlace::RightEdge`] and
    /// by [`LabelPlace::LineSpan`], which carries its own span — in ABSOLUTE milliseconds, as every
    /// tool speaks, unlike this field.
    pub t_rel: f32,
    pub price: f32,
    pub place: LabelPlace,
    pub text: LabelValue,
    /// Whether this readout belongs to a figure that is already on the chart, rather than to the
    /// one being drawn.
    ///
    /// A permanent readout is what the per-tab "line labels" switch hides, exactly as it hides the
    /// order column. The figure being DRAWN keeps its readout regardless: the numbers are what the
    /// user is aiming with.
    pub permanent: bool,
    /// `0xRRGGBB` for the text layer, which has no alpha of its own.
    pub color: u32,
}

/// Buffers the figure layer appends into.
pub struct FigureBuffers<'a> {
    /// Filled bands. They ride the ORDER-ZONE layer, which draws over the grid and under the
    /// candles, so a fill tints the plot without burying the price action on it.
    pub zones: &'a mut Vec<ZoneInstance>,
    pub hlines: &'a mut Vec<LineInstance>,
    pub segs: &'a mut Vec<SegInstance>,
    pub markers: &'a mut Vec<MarkerInstance>,
    pub labels: &'a mut Vec<FigureLabel>,
}

/// Interaction state of the figure layer for one pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FigureView {
    /// Chart epoch the instance time fields are relative to.
    pub epoch_ms: f64,
    pub hovered: Option<u64>,
    pub selected: Option<u64>,
    /// Figure currently held by the mouse, if any.
    ///
    /// Its fills are suppressed for the duration: a fill lives in the chart's base cache, and one
    /// that moved with the cursor would re-bake the background, grid, candles and order book of
    /// every pane at mouse-move rate. They come back on release, in one bake.
    ///
    /// A DRAFT band pays that cost on purpose — see `Sink::fills` — because there the moving area
    /// is the thing being chosen. Moving an existing figure is not: its outline already says where
    /// it is going.
    pub dragging: Option<u64>,
}

/// The shader's pattern code for a line kind: its `TPenStyle` index, unchanged.
///
/// There is no table here on purpose. The blob carries the pen index at `@13`, the shaders switch
/// on the same number, and a figure drawn in Moonbot is therefore drawn in the style Moonbot named.
/// Until all five patterns existed in the shaders, Dash and DashDot were folded into DashDotDot —
/// three kinds, one look.
fn seg_pattern(kind: LineKind) -> f32 {
    kind.to_pen() as f32
}

fn rgba(c: [u8; 4], alpha_mul: f32) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        (c[3] as f32 / 255.0) * alpha_mul,
    ]
}

/// Names a ratio-scale level and the price it sits at: `0.618 (6109.48)`.
///
/// The price keeps MORE precision than the axis carries. The axis rounds to one decimal above
/// 1000 and four below 1 — enough to read the gutter, not enough to tell two neighbouring levels
/// apart, and not enough to show any level of a coin priced at 0.00001234 at all. A level is a
/// number the reader acts on.
fn fmt_level(ratio: f64, price: f64) -> String {
    // The ratio's own formatting belongs to the scale that defines it: the settings panel labels
    // its switches with the same call, and two copies would drift.
    let r = moon_core::figures::levels::fmt_ratio(ratio);
    let a = price.abs();
    let decimals = if a >= 1000.0 {
        2
    } else if a >= 1.0 {
        4
    } else if a > 0.0 {
        // Push past the leading zeros of a sub-1 price instead of cutting them off.
        (6 - a.log10().floor() as i32).clamp(4, 12) as usize
    } else {
        2
    };
    let p = format!("{price:.decimals$}");
    let p = if p.contains('.') {
        p.trim_end_matches('0').trim_end_matches('.')
    } else {
        &p
    };
    format!("{r} ({p})")
}

/// Packs a float color into the `0xRRGGBB` the text layer takes.
fn rgb_u32(c: [f32; 4]) -> u32 {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (ch(c[0]) << 16) | (ch(c[1]) << 8) | ch(c[2])
}

/// Turns a tool's primitives into own-pass instances.
struct Sink<'a, 'b> {
    epoch_ms: f64,
    /// Whether fills are emitted at all.
    ///
    /// Off for the figure being DRAGGED: a fill entering the base-cache signature on every mouse
    /// move re-bakes the background, grid, candles and book of every pane, and a figure being moved
    /// is recognised by its lines. It returns on release, in one bake.
    ///
    /// For the DRAFT it is on only for a tool that draws a BAND: there the area IS what the user is
    /// choosing, so the same cost buys the thing being aimed rather than a decoration.
    fills: bool,
    /// Whether the figure being built is the DRAFT, whose labels are transient by nature.
    ///
    /// Everything else's labels are permanent: they stay on the chart after the pointer leaves,
    /// which is what the per-tab "line labels" switch is for. Deriving this from hover instead
    /// would let pointing at a figure defeat that switch.
    draft: bool,
    out: &'a mut FigureBuffers<'b>,
}

impl Sink<'_, '_> {
    fn to_rel(&self, time_ms: f64) -> f32 {
        (time_ms - self.epoch_ms) as f32
    }
}

impl GeomSink for Sink<'_, '_> {
    fn hline(&mut self, price: f64, stroke: &Stroke) {
        self.out.hlines.push(LineInstance {
            price: price as f32,
            color: stroke.color,
            // The pen index, like the segment's: a full-width line has all five styles too.
            style: seg_pattern(stroke.kind),
            thickness: stroke.thickness,
        });
    }

    fn seg(&mut self, a: FigNode, b: FigNode, stroke: &Stroke) {
        self.out.segs.push(SegInstance {
            t0_rel: self.to_rel(a.time_ms),
            p0: a.price as f32,
            t1_rel: self.to_rel(b.time_ms),
            p1: b.price as f32,
            thickness: stroke.thickness,
            pattern: seg_pattern(stroke.kind),
            extend: SEG_EXTEND_NONE,
            color: stroke.color,
        });
    }

    fn ray(&mut self, a: FigNode, b: FigNode, stroke: &Stroke) {
        self.out.segs.push(SegInstance {
            t0_rel: self.to_rel(a.time_ms),
            p0: a.price as f32,
            t1_rel: self.to_rel(b.time_ms),
            p1: b.price as f32,
            thickness: stroke.thickness,
            pattern: seg_pattern(stroke.kind),
            // The second point stays exactly where the tool put it: it is the DIRECTION, and the
            // shader extrapolates through it to whichever plot edge the ray points at.
            extend: SEG_EXTEND_RAY,
            color: stroke.color,
        });
    }

    fn band(&mut self, t0_ms: f64, t1_ms: f64, p0: f64, p1: f64, color: [f32; 4]) {
        if !self.fills {
            return;
        }
        // An invisible fill is not a fill: skipping it here keeps a figure with fills turned off
        // out of the zone buffer entirely, so it neither uploads a quad nor changes the signature
        // that re-bakes the chart's base cache.
        if color[3] <= 0.0 {
            return;
        }
        // The order path refuses a non-finite or non-positive band (`build_order_geometry`), and
        // figure fills share ONE draw call with it: a NaN from a hand-edited file would take the
        // whole call down, not just this band.
        // Exactly flat is dropped — it would rasterize to nothing at any zoom — but no EPSILON
        // above that: an absolute one discards a band many pixels tall on a market priced at 1e-8,
        // the pitfall `view.rs` documents.
        if !(p0.is_finite() && p1.is_finite() && p0 > 0.0 && p1 > 0.0) || p0 == p1 {
            return;
        }
        // A tool says "no bound on this side" with an infinity, which is the natural way to say it
        // in figure coordinates; the instance carries the finite sentinel the shaders clamp.
        let bound = |t_ms: f64| {
            if t_ms == f64::NEG_INFINITY {
                -TIME_UNBOUNDED
            } else if t_ms == f64::INFINITY {
                TIME_UNBOUNDED
            } else {
                self.to_rel(t_ms)
            }
        };
        let (t0, t1) = (bound(t0_ms), bound(t1_ms));
        if !(t0.is_finite() && t1.is_finite()) {
            return;
        }
        self.out.zones.push(ZoneInstance {
            price0: p0.min(p1) as f32,
            price1: p0.max(p1) as f32,
            t0_rel: t0.min(t1),
            t1_rel: t0.max(t1),
            color,
        });
    }

    fn handle(&mut self, at: FigNode, color: [f32; 4]) {
        self.out.markers.push(MarkerInstance::at_price(
            self.to_rel(at.time_ms),
            at.price as f32,
            FIG_KNOT_SIZE,
            FIG_KNOT_THICKNESS,
            MARKER_SHAPE_KNOT,
            color,
        ));
    }

    fn label(&mut self, at: FigNode, place: LabelPlace, text: LabelText, color: [f32; 4]) {
        let permanent = !self.draft;
        let text = match text {
            LabelText::Price(p) => LabelValue::Price(p),
            LabelText::PctDelta { from, to } => LabelValue::PctDelta { from, to },
            LabelText::Level { ratio, price } => LabelValue::Ready(fmt_level(ratio, price).into()),
            // Two decimals: the number is compared against a habit ("at least two to one"), not
            // measured, and a third digit only makes it harder to read at a glance.
            LabelText::RiskReward(rr) => LabelValue::Ready(format!("R:R {rr:.2}").into()),
        };
        self.out.labels.push(FigureLabel {
            t_rel: self.to_rel(at.time_ms),
            price: at.price as f32,
            place,
            text,
            permanent,
            color: rgb_u32(color),
        });
    }
}

/// Builds chart figure geometry. `draft` is a preview of the figure being drawn; `hovered`/
/// `selected` figures are highlighted, and a selected knot-grabbed figure gains endpoint knots.
pub fn build_figure_geometry<'a>(
    figures: impl IntoIterator<Item = &'a Figure>,
    draft: Option<&Figure>,
    view: FigureView,
    out: &mut FigureBuffers<'_>,
) {
    let mut sink = Sink {
        epoch_ms: view.epoch_ms,
        fills: true,
        draft: false,
        out,
    };
    for fig in figures {
        let is_sel = view.selected == Some(fig.id);
        let is_hovered = view.hovered == Some(fig.id);
        let ctx = fig_ctx(fig, is_hovered, is_sel);
        sink.fills = view.dragging != Some(fig.id);
        build_figure(&fig.kind, &ctx, &mut sink);
    }
    if let Some(d) = draft {
        // Preview of the figure being drawn: brighter and thicker, using the style's base line
        // kind, and already showing its readout so a trend line can be aimed while drawing.
        let ctx = BuildCtx {
            stroke: Stroke {
                color: rgba(d.color, 1.0),
                thickness: d.thickness * FIG_ACTIVE_THICKNESS,
                kind: d.line_kind,
            },
            // A BAND being drawn paints its fill, in the style the finished figure will have: the
            // Zone and the rectangle ARE areas, and two bare lines do not show the area being
            // chosen. Every other tool keeps the old empty fill, so a Fibonacci preview does not
            // push ten moving bands through the zone signature.
            fill: rgba(d.fill, 1.0),
            hot: true,
            handles: false,
        };
        // Set, never inherited: the loop above leaves this false when the LAST figure was the one
        // being dragged, and the draft's own fill must not depend on which figure came last.
        //
        // The cost is real and deliberate: a moving fill re-enters the zone signature and re-bakes
        // the base cache of every pane at present rate for as long as the draft follows the cursor
        // (see `FigureView::dragging`, which is refused for exactly that reason). It is accepted
        // here only for the two tools whose whole point is the area, and only while a draft is
        // live — a draft ends on its finishing click, a click elsewhere, a tool change or leaving
        // drawing mode.
        sink.fills = d.kind.price_band().is_some();
        sink.draft = true;
        build_figure(&d.kind, &ctx, &mut sink);
    }
}

/// Build context for a figure by state:
/// - idle — thin base style from `fig.line_kind`;
/// - hovered/selected — thick SOLID line (sharply "comes alive"), plus knots when selected;
/// - armed (alert) — thick base style (thickness indicates that it is armed).
///
/// The READOUT follows the cursor, not the selection: selection is sticky, and a label that
/// outlives the pointer would keep the text pass formatting and re-shaping it every frame for as
/// long as a figure stays selected. Hover and the draft are transient by nature, so for every tool
/// that labels on `hot` an idle chart pays nothing. A ratio scale opts out and labels always — its
/// levels ARE the reading — and pays for that on every frame it is on screen.
fn fig_ctx(fig: &Figure, is_hovered: bool, is_selected: bool) -> BuildCtx {
    let is_hot = is_hovered || is_selected;
    let stroke = if is_hot {
        Stroke {
            color: rgba(fig.color, 1.0),
            thickness: fig.thickness * FIG_ACTIVE_THICKNESS,
            kind: LineKind::Solid,
        }
    } else if fig.alert {
        Stroke {
            color: rgba(fig.color, 1.0),
            thickness: fig.thickness * FIG_ARMED_THICKNESS,
            kind: fig.line_kind,
        }
    } else {
        Stroke {
            color: rgba(fig.color, FIG_IDLE_ALPHA),
            thickness: fig.thickness,
            kind: fig.line_kind,
        }
    };
    BuildCtx {
        stroke,
        // The figure's own fill, interaction state deliberately excluded: see the module doc —
        // a fill that changed under the cursor would re-bake the base cache.
        fill: rgba(fig.fill, 1.0),
        hot: is_hovered,
        handles: is_selected,
    }
}

#[cfg(test)]
mod tests;
