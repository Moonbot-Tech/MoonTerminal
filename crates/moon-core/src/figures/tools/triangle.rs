//! Triangle: three vertices, three edges (Moonbot's chart-object type 4).

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::node::FigNode;
use super::super::proj::{seg_dist, Proj, PxPoint};
use super::super::sink::{BuildCtx, GeomSink};
use super::segment::Segment;
use super::{FigureTool, ToolDef, ToolShape};

/// Triangle defined by three vertices.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Triangle {
    pub a: FigNode,
    pub b: FigNode,
    pub c: FigNode,
}

/// The apex Moonbot derives when a triangle is dragged rather than clicked: the perpendicular
/// raised from the middle of the dragged BASE, as long as the base itself.
///
/// The perpendicular's sign is fixed instead of "always upwards". Dragging right to left flips the
/// base vector, so the apex flips with it and the triangle points down — which is what Moonbot
/// does, and what a hand-written "up" rule would have to special-case.
fn drag_apex(a: PxPoint, b: PxPoint) -> PxPoint {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let mid = ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
    // Rotation by -90° in screen space, where y grows downward: a left-to-right base lifts its
    // apex above itself. Rotating the base vector also carries its LENGTH, so the height follows
    // the base without a second constant.
    (mid.0 + dy, mid.1 - dx)
}

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::Triangle,
    key: "triangle",
    locale_key: "alerts.fig.triangle",
    glyph: "△",
    clicks: 3,
    // Dragged, a triangle is its base and nothing else; the third vertex comes from the two.
    drag_rest: Some(|a, b| vec![drag_apex(a, b)]),
    scale_swatch: None,
    fills: false,
    alertable: true,
    make: |nodes| match nodes {
        [a, b, c, ..] => Some(FigureKind::Triangle(Triangle {
            a: *a,
            b: *b,
            c: *c,
        })),
        _ => None,
    },
    // One vertex placed previews the first EDGE; two preview the whole triangle.
    preview: |placed, cursor| match placed {
        [a] => Some(FigureKind::Segment(Segment { a: *a, b: cursor })),
        [a, b, ..] => Some(FigureKind::Triangle(Triangle {
            a: *a,
            b: *b,
            c: cursor,
        })),
        _ => None,
    },
};

impl ToolShape for Triangle {
    fn def(&self) -> &'static ToolDef {
        &DEF
    }

    fn handle_count(&self) -> usize {
        3
    }

    fn handle(&self, i: usize) -> Option<FigNode> {
        match i {
            0 => Some(self.a),
            1 => Some(self.b),
            2 => Some(self.c),
            _ => None,
        }
    }

    fn move_handle(&mut self, i: usize, to: FigNode) -> bool {
        let n = match i {
            0 => &mut self.a,
            1 => &mut self.b,
            2 => &mut self.c,
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
        for n in [&mut self.a, &mut self.b, &mut self.c] {
            *n = n.shifted(dt_ms, dp);
        }
        true
    }

    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        let (pa, pb, pc) = (proj.px_of(self.a), proj.px_of(self.b), proj.px_of(self.c));
        seg_dist(pos, pa, pb)
            .min(seg_dist(pos, pb, pc))
            .min(seg_dist(pos, pc, pa))
    }

    /// No readout: a triangle marks a shape, not a level or a move, so there is no one number to
    /// put beside it. Its vertices carry the prices, and each is readable from the price axis.
    ///
    /// No fill either, though it encloses an area: the fill primitive paints an axis-aligned band,
    /// and a triangle needs a polygon. It joins the filled tools when that primitive lands.
    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        // Three edges: a-b, b-c, c-a.
        sink.seg(self.a, self.b, &ctx.stroke);
        sink.seg(self.b, self.c, &ctx.stroke);
        sink.seg(self.c, self.a, &ctx.stroke);
    }
}

#[cfg(test)]
mod tests;
