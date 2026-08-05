//! Rectangle: an area between two corners, outlined and filled.
//!
//! The general filled figure — a consolidation, a zone of interest, the box a setup lives in.
//! Two clicks place opposite corners; the fill is the drawing style's, like every other tool with
//! an area.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::node::FigNode;
use super::super::proj::{seg_dist, Proj, PxPoint};
use super::super::sink::{BuildCtx, GeomSink};
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

    /// Distance to the nearest EDGE. The filled interior is deliberately not a hit: a rectangle
    /// usually covers price action the user still needs to click through.
    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        let c = self.corners();
        let mut best = f32::INFINITY;
        for i in 0..4 {
            let (p, q) = (proj.px_of(c[i]), proj.px_of(c[(i + 1) % 4]));
            best = best.min(seg_dist(pos, p, q));
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
    }
}

#[cfg(test)]
mod tests;
