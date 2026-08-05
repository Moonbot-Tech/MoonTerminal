//! Fibonacci retracement: a move between two nodes, read as a ratio scale.
//!
//! Two clicks mark the move — the first its start, the second its end — and the tool draws the
//! scale across the time span between them: a line per level, a faint band filling each gap, and
//! a `ratio (price)` readout at the LEFT edge of the box, where a column of numbers is read from.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::levels::{price_at, Emphasis, Level, FIB_LEVELS};
use super::super::node::FigNode;
use super::super::proj::{seg_dist, Proj, PxPoint};
use super::super::sink::{BuildCtx, GeomSink, LabelPlace, LabelText, Stroke};
use super::{FigureTool, ToolDef, ToolShape};

/// A move whose retracement levels are drawn: `a` is where it started, `b` where it ended.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FibRetracement {
    pub a: FigNode,
    pub b: FigNode,
}

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::FibRetracement,
    key: "fib-retracement",
    locale_key: "alerts.fig.fib_retracement",
    glyph: "≣",
    clicks: 2,
    level_palette: true,
    fills: true,
    // The core's chart-object blob has a Fibo type (3), but its payload is not decoded yet, so
    // this tool stays local until it is; sending a blob we cannot read back would be a guess.
    alertable: false,
    make: |nodes| match nodes {
        [a, b, ..] => Some(FigureKind::FibRetracement(FibRetracement { a: *a, b: *b })),
        _ => None,
    },
    preview: |placed, cursor| {
        placed
            .first()
            .map(|a| FigureKind::FibRetracement(FibRetracement { a: *a, b: cursor }))
    },
};

/// A level's own hue as a vertex colour, at `alpha`.
///
/// The scale stores `u8` because that is what a colour is everywhere else in the figure model; the
/// sink speaks `f32`. One conversion, so a line, a band and a readout of the same level can never
/// drift apart by a rounding difference.
fn hue(level: &Level, alpha: f32) -> [f32; 4] {
    let [r, g, b] = level.color;
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, alpha]
}

impl FibRetracement {
    /// Time span the scale is drawn across, ordered.
    fn span(&self) -> (f64, f64) {
        (
            self.a.time_ms.min(self.b.time_ms),
            self.a.time_ms.max(self.b.time_ms),
        )
    }

    /// Price of one level of the scale.
    fn price(&self, ratio: f64) -> f64 {
        price_at(self.a.price, self.b.price, ratio)
    }
}

impl ToolShape for FibRetracement {
    fn def(&self) -> &'static ToolDef {
        &DEF
    }

    fn handle_count(&self) -> usize {
        2
    }

    fn handle(&self, i: usize) -> Option<FigNode> {
        match i {
            0 => Some(self.a),
            1 => Some(self.b),
            _ => None,
        }
    }

    fn move_handle(&mut self, i: usize, to: FigNode) -> bool {
        let n = match i {
            0 => &mut self.a,
            1 => &mut self.b,
            _ => return false,
        };
        if *n == to {
            return false;
        }
        *n = to;
        true
    }

    fn translate(&mut self, dt_ms: f64, dp: f64) -> bool {
        if dt_ms == 0.0 && dp == 0.0 {
            return false;
        }
        self.a = self.a.shifted(dt_ms, dp);
        self.b = self.b.shifted(dt_ms, dp);
        true
    }

    /// Distance to the nearest LEVEL, not to the move that defines them: the levels are what the
    /// figure is, and a retracement is grabbed by the line the cursor is reading.
    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        let (t0, t1) = self.span();
        let mut best = f32::INFINITY;
        for level in FIB_LEVELS {
            let price = self.price(level.ratio);
            // Skipped by `build` as well: a level that crossed zero is not drawn, so it must not
            // be grabbable either. `is_finite` also catches a NaN, which no comparison would.
            if !price.is_finite() || price <= 0.0 {
                continue;
            }
            let a = proj.px_of(FigNode::new(t0, price));
            let b = proj.px_of(FigNode::new(t1, price));
            best = best.min(seg_dist(pos, a, b));
        }
        best
    }

    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        let (t0, t1) = self.span();
        // Each level wears its OWN hue; the figure's colour keeps only what a state can change —
        // the opacity, which hover and selection raise. Hence hue from the level, alpha from the
        // figure. The pencil's colour therefore paints the move line and nothing else on this tool,
        // exactly as a coloured ratio scale works everywhere else.
        let level_line =
            |level: &Level| hue(level, ctx.stroke.color[3] * level.emphasis.line_alpha());
        // A fill never reacts to hover or selection: it lives in the base cache (see `BuildCtx`).
        // A band takes the hue of the level with the SMALLER RATIO of the two it joins — by ratio,
        // never by price: 0 sits at the END of the move, so on a rise the smaller ratio is the
        // HIGHER price. The opacity stays the user's, chosen once in the pencil popup; the scale
        // only says which colour spends it.
        let level_fill = |level: &Level| hue(level, ctx.fill[3]);
        // The move itself, dotted and dimmed: it is the scale's definition, not one of its levels,
        // so this is the one stroke that stays the figure's own colour.
        let mut move_color = ctx.stroke.color;
        move_color[3] *= 0.5;
        sink.seg(
            self.a,
            self.b,
            &Stroke {
                color: move_color,
                thickness: ctx.stroke.thickness,
                kind: super::super::style::LineKind::Dot,
            },
        );
        // A move with no height has no scale: every level would collapse onto one price, and
        // eleven lines, ten zero-height bands and eleven identical readouts would stack there.
        if self.a.price == self.b.price {
            return;
        }
        // The previous level in RATIO order, carried forward so the band can take ITS hue.
        let mut prev: Option<(f64, &Level)> = None;
        for level in FIB_LEVELS {
            let price = self.price(level.ratio);
            // A level far enough past the move's start crosses zero. A negative price is not a
            // level, it is arithmetic: drop it rather than draw and label it. `is_finite` also
            // catches a NaN, which no comparison would.
            if !price.is_finite() || price <= 0.0 {
                prev = None;
                continue;
            }
            let color = level_line(level);
            // Fill the gap to the previous level, under the lines that bound it, in the colour of
            // the level with the smaller ratio. Hue is what separates neighbouring bands now, so
            // they no longer alternate opacity to tell themselves apart.
            if let Some((prev_price, prev_level)) = prev {
                sink.band(t0, t1, prev_price, price, level_fill(prev_level));
            }
            sink.seg(
                FigNode::new(t0, price),
                FigNode::new(t1, price),
                &Stroke {
                    color,
                    thickness: ctx.stroke.thickness,
                    kind: if level.emphasis == Emphasis::Key {
                        super::super::style::LineKind::Solid
                    } else {
                        ctx.stroke.kind
                    },
                },
            );
            // The readout is the point of the tool, so it is drawn always rather than on hover:
            // a scale whose prices appear only under the cursor cannot be read at a glance.
            sink.label(
                // The anchor END, matching where `LineSpan` places the text. The renderer reads
                // the span rather than this node today, but a node pointing at the far end would
                // mislead the first reader that does — a de-overlap pass, say.
                FigNode::new(t0, price),
                LabelPlace::LineSpan {
                    t0_ms: t0,
                    t1_ms: t1,
                },
                LabelText::Level {
                    ratio: level.ratio,
                    price,
                },
                color,
            );
            prev = Some((price, level));
        }
    }
}

#[cfg(test)]
mod tests;
