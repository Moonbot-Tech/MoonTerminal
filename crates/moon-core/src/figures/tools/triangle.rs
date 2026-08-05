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

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::Triangle,
    key: "triangle",
    locale_key: "alerts.fig.triangle",
    glyph: "△",
    clicks: 3,
    level_palette: false,
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
