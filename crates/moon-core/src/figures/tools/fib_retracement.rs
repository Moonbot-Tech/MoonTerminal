//! Fibonacci retracement: a move between two nodes, read as a ratio scale.
//!
//! Two clicks mark the move — the first its start, the second its end — and the tool draws the
//! scale across the time span between them: a line per level, a faint band filling each gap, and
//! a `ratio (price)` readout at the right edge of the box.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::levels::{price_at, Emphasis, BAND_ALPHA, BAND_ALPHA_ALT, FIB_LEVELS};
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
            let a = proj.px_of(FigNode::new(t0, price));
            let b = proj.px_of(FigNode::new(t1, price));
            best = best.min(seg_dist(pos, a, b));
        }
        best
    }

    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        let (t0, t1) = self.span();
        let base = ctx.stroke.color;
        let tint = |alpha_mul: f32| {
            let mut c = base;
            c[3] *= alpha_mul;
            c
        };
        // A fill never reacts to hover or selection: it lives in the base cache (see `BuildCtx`).
        let fill = |alpha: f32| {
            let mut c = ctx.fill;
            c[3] = alpha;
            c
        };
        // The move itself, dotted and dimmed: it is the scale's definition, not one of its levels.
        sink.seg(
            self.a,
            self.b,
            &Stroke {
                color: tint(0.5),
                thickness: ctx.stroke.thickness,
                kind: super::super::style::LineKind::Dot,
            },
        );
        // A move with no height has no scale: every level would collapse onto one price, and
        // eleven lines, ten zero-height bands and eleven identical readouts would stack there.
        if self.a.price == self.b.price {
            return;
        }
        let mut prev: Option<f64> = None;
        let mut band_i = 0usize;
        for level in FIB_LEVELS {
            let price = self.price(level.ratio);
            // A level far enough past the move's start crosses zero. A negative price is not a
            // level, it is arithmetic: drop it rather than draw and label it.
            if price <= 0.0 {
                prev = None;
                continue;
            }
            let color = tint(level.emphasis.line_alpha());
            // Fill the gap to the previous level, under the lines that bound it. Alternating alpha
            // keeps neighbouring bands apart instead of merging them into one wash.
            if let Some(prev_price) = prev {
                let alpha = if band_i % 2 == 0 {
                    BAND_ALPHA
                } else {
                    BAND_ALPHA_ALT
                };
                sink.band(t0, t1, prev_price, price, fill(alpha));
                band_i += 1;
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
                FigNode::new(t1, price),
                LabelPlace::LineEnd {
                    t0_ms: t0,
                    t1_ms: t1,
                },
                LabelText::Level {
                    ratio: level.ratio,
                    price,
                },
                color,
            );
            prev = Some(price);
        }
    }
}

#[cfg(test)]
mod tests;
