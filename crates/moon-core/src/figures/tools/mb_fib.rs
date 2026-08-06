//! Moonbot's own Fibonacci object (chart-object type 3), as Moonbot draws it.
//!
//! A SEPARATE tool from our [`super::FibRetracement`], deliberately, because it is a different
//! object and not a different rendering of the same one:
//!
//! - Moonbot stores seven finished level PRICES, not a scale of ratios. The set of ratios behind
//!   them is a user setting on that side — the samples sit at `0 .236 .382 .5 .618 .786 1.236`
//!   while a chart drawn later showed the seventh at `1.618` — so the prices are the only thing
//!   that survives it. They are read as given and never re-derived from a scale of ours.
//! - It spans the WHOLE chart: no start, no end, the levels run into the order book. The one time
//!   in the blob is where it was drawn from, not an edge.
//!
//! Converting one into our eleven-ratio tool would therefore draw something the user never drew.
//! This tool draws what Moonbot drew.
//!
//! Not drawable here ([`ToolDef::drawable`] is false): it exists to show — and to round-trip — an
//! object that arrives from the core. Ours is the tool the toolbar offers.

use serde::{Deserialize, Serialize};

use super::super::node::FigNode;
use super::super::proj::{hline_dist, Proj, PxPoint};
use super::super::sink::{BuildCtx, GeomSink, LabelPlace, LabelText};
use super::{FigureTool, GrabMode, ToolDef, ToolShape};

/// Levels a Moonbot Fibonacci object carries. Fixed by the format: every sampled blob is 145 bytes,
/// which is exactly this many `f64` prices after the header and the anchors.
pub const MB_FIB_LEVELS: usize = 7;

/// The largest ratio worth naming. Well past 4.236, the furthest extension any charting package
/// offers, and far short of what a degenerate span produces.
const MAX_NAMED_RATIO: f64 = 100.0;

/// A Fibonacci object drawn in Moonbot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MbFib {
    /// The price the scale calls ZERO. Deliberately not described as the move's start or end:
    /// which end Moonbot anchors is not visible in the bytes, and our own [`super::FibRetracement`]
    /// numbers a move from the opposite end. What the samples do show is that a level equal to this
    /// price is labelled 0, which is all a label needs.
    pub a: f64,
    /// The price the scale calls ONE. Neither anchor is necessarily one of [`Self::levels`]: the
    /// sampled sets contain a level at ratio 0 but none at ratio 1.
    pub b: f64,
    /// When it was drawn. Kept because the blob carries it and a re-upsert must give it back
    /// unchanged; it is NOT an edge, and nothing here draws with it.
    pub time_ms: f64,
    /// The drawn level prices, in the order Moonbot wrote them.
    pub levels: [f64; MB_FIB_LEVELS],
}

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::MbFib,
    key: "mb-fib",
    locale_key: "alerts.fig.mb_fib",
    glyph: "▤",
    clicks: 0,
    level_palette: false,
    fills: false,
    alertable: true,
    // Arrives from the core; the toolbar offers our own Fibonacci instead.
    drawable: false,
    make: |_| None,
    preview: |_, _| None,
};

impl MbFib {
    /// Where `price` sits on the move, as the ratio Moonbot would label it.
    ///
    /// Derived for the LABEL only — the price is what is drawn. `None` when the move has no height,
    /// which makes every ratio a division by zero: a level is then shown by its price alone rather
    /// than by an infinity or a NaN, both of which would reach the text layer as literal glyphs.
    pub fn ratio_of(&self, price: f64) -> Option<f64> {
        let span = self.b - self.a;
        if span == 0.0 || !span.is_finite() {
            return None;
        }
        let r = (price - self.a) / span;
        // A ratio no scale would name is not a name: a span that underflows makes the division huge,
        // and `{:.3}` of 1e300 is a three-hundred-character label that nothing downstream truncates
        // and every frame re-shapes. Past the bound the level is shown by its price instead.
        if !r.is_finite() || r.abs() > MAX_NAMED_RATIO {
            return None;
        }
        Some(super::super::levels::snap_ratio(r, price, span))
    }

    /// The levels worth drawing: finite and above zero.
    ///
    /// A price at or below zero is not a level on a price chart, and a non-finite one would put a
    /// line at an undefined height; both come off a wire this codec does not own.
    fn drawn(&self) -> impl Iterator<Item = f64> + '_ {
        self.levels.iter().copied().filter(|p| Self::is_price(*p))
    }

    /// Whether a wire value can be a price on this chart.
    ///
    /// Bounded above as well as below: the geometry buffers carry `f32`, so a finite `f64` past
    /// `f32::MAX` reaches the GPU as an infinity — the same check `moon-chart`'s fill sink makes
    /// for exactly this reason.
    fn is_price(p: f64) -> bool {
        p.is_finite() && p > 0.0 && p <= f32::MAX as f64
    }
}

impl ToolShape for MbFib {
    fn def(&self) -> &'static ToolDef {
        &DEF
    }

    /// No DRAGGABLE handles: the object belongs to Moonbot, and there is no gesture here that could
    /// edit it without inventing which of the seven prices a drag was supposed to move.
    fn handle_count(&self) -> usize {
        0
    }

    /// The move's start, which is the figure's anchor price for the alert list.
    ///
    /// Answered even though `handle_count` is zero: the two questions differ. Nothing iterates past
    /// the count — `pick_handle` and the knot pass both stop at it — while `FigureKind::anchor_price`
    /// asks index 0 directly, and a figure with no answer would sort and display as price zero.
    fn handle(&self, i: usize) -> Option<FigNode> {
        if i != 0 {
            return None;
        }
        // The move's start when it is a price at all; otherwise the first level that is one. The
        // anchor is what the alerts list prints in its Price column, and `a` comes off the wire
        // unchecked — a NaN or a negative would be printed there verbatim, and a zero would render
        // as an empty cell indistinguishable from "no price".
        let price = Self::is_price(self.a)
            .then_some(self.a)
            .or_else(|| self.drawn().next())?;
        Some(FigNode::new(self.time_ms, price))
    }

    fn move_handle(&mut self, _i: usize, _to: FigNode) -> bool {
        false
    }

    fn translate(&mut self, _dt_ms: f64, _dp: f64) -> bool {
        false
    }

    /// Distance to the nearest drawn level, measured vertically: every level spans the full width,
    /// so there is no X to aim at.
    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        self.drawn()
            .map(|p| hline_dist(pos, p, proj))
            .fold(f32::INFINITY, f32::min)
    }

    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        for price in self.drawn() {
            sink.hline(price, &ctx.stroke);
            // The readout is the point of a ratio scale, so it is drawn at rest rather than on
            // hover — the same rule our own Fibonacci follows.
            // A level whose ratio cannot be named draws its line and stays silent. A bare price in
            // a column of ratios reads as something else entirely, and it would also slip past the
            // per-tab switch that hides the rest of the column.
            //
            // The level sitting ON an anchor is silent for a different reason: it is the move's own
            // end rather than a retracement of it, and which end that is cannot be read from the
            // bytes. Measured across live samples: a fib drawn UP puts that level on `a`, one drawn
            // DOWN puts it on `b`, so the same slot is ratio 0 in one and ratio 1 in the other and
            // we would name one of them wrong. Moonbot leaves it unlabelled too — its own chart
            // names six levels and draws the seventh bare. A line with no name is never wrong; a
            // line with the wrong number is.
            if let Some(ratio) = self.ratio_of(price).filter(|r| *r != 0.0 && *r != 1.0) {
                sink.label(
                    FigNode::new(0.0, price),
                    LabelPlace::RightEdge,
                    LabelText::Level { ratio, price },
                    ctx.stroke.color,
                );
            }
        }
    }

    fn grab_mode(&self) -> GrabMode {
        GrabMode::PriceLines
    }
}

#[cfg(test)]
mod tests;
