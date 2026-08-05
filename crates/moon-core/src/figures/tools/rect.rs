//! Rectangle: an area between two corners, outlined and filled.
//!
//! The general filled figure — a consolidation, a zone of interest, the box a setup lives in.
//! Two clicks place opposite corners, and the fill is the drawing style's, shared with the price
//! channel and the Fibonacci scale.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::node::FigNode;
use super::super::proj::{seg_dist, Proj, PxPoint};
use super::super::sink::{BuildCtx, GeomSink, LabelPlace, LabelText};
use super::{FigureTool, ToolDef, ToolShape};

/// Rectangle defined by two opposite corners.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub a: FigNode,
    pub b: FigNode,
}

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::Rect,
    key: "rect",
    locale_key: "alerts.fig.rect",
    glyph: "▭",
    clicks: 2,
    fills: true,
    // The core's chart-object blob has no rectangle type.
    alertable: false,
    make: |nodes| match nodes {
        [a, b, ..] => Some(FigureKind::Rect(Rect { a: *a, b: *b })),
        _ => None,
    },
    preview: |placed, cursor| {
        placed
            .first()
            .map(|a| FigureKind::Rect(Rect { a: *a, b: cursor }))
    },
};

impl Rect {
    /// The four corners, clockwise from `a`.
    fn corners(&self) -> [FigNode; 4] {
        [
            self.a,
            FigNode::new(self.b.time_ms, self.a.price),
            self.b,
            FigNode::new(self.a.time_ms, self.b.price),
        ]
    }
}

impl ToolShape for Rect {
    fn def(&self) -> &'static ToolDef {
        &DEF
    }

    fn handle_count(&self) -> usize {
        4
    }

    fn handle(&self, i: usize) -> Option<FigNode> {
        self.corners().get(i).copied()
    }

    /// Every drawn corner is grabbable, and dragging one moves the two stored corners it is made
    /// of — the neighbours follow, as a rectangle's corners must.
    fn move_handle(&mut self, i: usize, to: FigNode) -> bool {
        let before = (self.a, self.b);
        match i {
            0 => self.a = to,
            1 => {
                self.b.time_ms = to.time_ms;
                self.a.price = to.price;
            }
            2 => self.b = to,
            3 => {
                self.a.time_ms = to.time_ms;
                self.b.price = to.price;
            }
            _ => return false,
        }
        (self.a, self.b) != before
    }

    fn translate(&mut self, dt_ms: f64, dp: f64) -> bool {
        if dt_ms == 0.0 && dp == 0.0 {
            return false;
        }
        self.a = self.a.shifted(dt_ms, dp);
        self.b = self.b.shifted(dt_ms, dp);
        true
    }

    /// Distance to the nearest EDGE. The filled interior is deliberately not a hit: a rectangle
    /// usually covers price action the user still needs to click through.
    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        // Project each corner once: this runs per figure on the hover path.
        let c = self.corners().map(|n| proj.px_of(n));
        let mut best = f32::INFINITY;
        for i in 0..4 {
            best = best.min(seg_dist(pos, c[i], c[(i + 1) % 4]));
        }
        best
    }

    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        sink.band(
            self.a.time_ms,
            self.b.time_ms,
            self.a.price,
            self.b.price,
            ctx.fill,
        );
        let c = self.corners();
        for i in 0..4 {
            sink.seg(c[i], c[(i + 1) % 4], &ctx.stroke);
        }
        if ctx.hot {
            // What a box is drawn to answer: how far it reaches, in percent. Anchored to the TOP
            // edge whichever way the box was drawn, so the readout never lands inside the fill.
            sink.label(
                FigNode::new(
                    self.b.time_ms.max(self.a.time_ms),
                    self.a.price.max(self.b.price),
                ),
                LabelPlace::Above,
                LabelText::PctDelta {
                    from: self.a.price,
                    to: self.b.price,
                },
                ctx.stroke.color,
            );
        }
    }
}

#[cfg(test)]
mod tests;
