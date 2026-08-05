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
//! therefore resolved from the figure alone and never from hover or selection: a fill that
//! brightened under the cursor would re-bake the background, grid, candles and book of every pane.
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
    LineInstance, MarkerInstance, SegInstance, ZoneInstance, MARKER_SHAPE_KNOT,
    SEG_PATTERN_DASH_DOT_DOT, SEG_PATTERN_DOT, SEG_PATTERN_SOLID, TIME_UNBOUNDED,
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
    /// by [`LabelPlace::LineEnd`], which carries its own span — in ABSOLUTE milliseconds, as every
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
    /// Filled bands. They ride the ORDER-ZONE layer, which draws below the grid, so a fill sits
    /// behind the candles instead of over them.
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
    pub dragging: Option<u64>,
}

/// Segment pattern (shader: 0 solid / 1 dashdotdot / 2 dot) based on the Moonbot line kind.
/// The shader has fewer than five dashed variants → Dash/DashDot/DashDotDot share one pattern.
fn seg_pattern(kind: LineKind) -> f32 {
    match kind {
        LineKind::Solid => SEG_PATTERN_SOLID,
        LineKind::Dot => SEG_PATTERN_DOT,
        _ => SEG_PATTERN_DASH_DOT_DOT,
    }
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
    // `+ 0.0` folds -0.0, which would otherwise print as "-0".
    let r = format!("{:.3}", ratio + 0.0);
    let r = r.trim_end_matches('0').trim_end_matches('.');
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
    /// Off for the DRAFT and for the figure being DRAGGED: both follow the cursor, and a fill
    /// entering the base-cache signature on every mouse move would re-bake the background, grid,
    /// candles and book of every pane while the user is still aiming. Lines and readouts are what
    /// aiming needs; the fill returns on release, in one bake.
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
            style: if stroke.kind.is_solid() { 0.0 } else { 1.0 },
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
            extend: 0.0,
            color: stroke.color,
        });
    }

    fn band(&mut self, t0_ms: f64, t1_ms: f64, p0: f64, p1: f64, color: [f32; 4]) {
        if !self.fills {
            return;
        }
        // The order path refuses a non-finite or non-positive band (`build_order_geometry`), and
        // figure fills share ONE draw call with it: a NaN from a hand-edited file would take the
        // whole call down, not just this band.
        // No epsilon on the height: an absolute one drops a band that is many pixels tall on a
        // market priced at 1e-8 (the pitfall `view.rs` documents), and a truly flat band simply
        // draws nothing.
        if !(p0.is_finite() && p1.is_finite() && p0 > 0.0 && p1 > 0.0) {
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
            fill: rgba(d.fill, 1.0),
            hot: true,
            handles: false,
        };
        sink.fills = false;
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
