//! Trend line: a segment between two nodes.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::node::FigNode;
use super::super::proj::{seg_dist, Proj, PxPoint};
use super::super::sink::{BuildCtx, GeomSink, LabelPlace, LabelText};
use super::{FigureTool, ToolDef, ToolShape};

/// Segment between two nodes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub a: FigNode,
    pub b: FigNode,
}

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::Segment,
    key: "segment",
    locale_key: "alerts.fig.line",
    glyph: "╱",
    clicks: 2,
    level_palette: false,
    fills: false,
    alertable: true,
    drawable: true,
    make: |nodes| match nodes {
        [a, b, ..] => Some(FigureKind::Segment(Segment { a: *a, b: *b })),
        _ => None,
    },
    preview: |placed, cursor| {
        placed
            .first()
            .map(|a| FigureKind::Segment(Segment { a: *a, b: cursor }))
    },
};

impl ToolShape for Segment {
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

    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        seg_dist(pos, proj.px_of(self.a), proj.px_of(self.b))
    }

    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        sink.seg(self.a, self.b, &ctx.stroke);
        if ctx.hot {
            // The move a trend line describes, read at the end it points to.
            sink.label(
                self.b,
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
