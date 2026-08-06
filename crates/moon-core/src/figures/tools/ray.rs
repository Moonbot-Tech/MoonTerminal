//! Ray: a line that starts at one node, passes through a second, and runs off the chart.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::node::FigNode;
use super::super::proj::{Proj, PxPoint};
use super::super::sink::{BuildCtx, GeomSink, LabelPlace, LabelText};
use super::{FigureTool, ToolDef, ToolShape};

/// Ray from `a` through `b`, continuing past `b` without end.
///
/// The two nodes are an ORIGIN and a DIRECTION, not two ends. `b` is a real, draggable point — it
/// is how the slope is aimed — but the drawn line does not stop there, and neither does the hit
/// test.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ray {
    /// Where the ray starts. The end that exists.
    pub a: FigNode,
    /// The point it is aimed through. Everything past it is drawn by extrapolation.
    pub b: FigNode,
}

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::Ray,
    key: "ray",
    locale_key: "alerts.fig.ray",
    glyph: "↗",
    clicks: 2,
    scale_swatch: None,
    fills: false,
    // The core has no chart-object type for a ray: types 1..5 are hline, segment, fibo, triangle
    // and zone. It is drawn here only, and the Alerts panel calls that kind "Terminal".
    alertable: false,
    make: |nodes| match nodes {
        [a, b, ..] => Some(FigureKind::Ray(Ray { a: *a, b: *b })),
        _ => None,
    },
    preview: |placed, cursor| {
        placed
            .first()
            .map(|a| FigureKind::Ray(Ray { a: *a, b: cursor }))
    },
};

/// Distance in pixels from `pos` to the ray `a -> b`, which ends only at `a`.
///
/// The segment's own helper cannot be reused: it clamps the projection to `0..=1`, which is exactly
/// the half this shape does not have. Clamping only at the origin is what makes the whole visible
/// line grabbable rather than just the stretch between the two nodes.
fn ray_dist(pos: PxPoint, a: PxPoint, b: PxPoint) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len_sq = dx * dx + dy * dy;
    if len_sq <= f32::EPSILON {
        // Aimed at itself — mid-draw, before the second point moves. It is a point, not a line.
        return (pos.0 - a.0).hypot(pos.1 - a.1);
    }
    let t = (((pos.0 - a.0) * dx + (pos.1 - a.1) * dy) / len_sq).max(0.0);
    (pos.0 - (a.0 + t * dx)).hypot(pos.1 - (a.1 + t * dy))
}

impl ToolShape for Ray {
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
        ray_dist(pos, proj.px_of(self.a), proj.px_of(self.b))
    }

    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        sink.ray(self.a, self.b, &ctx.stroke);
        if ctx.hot {
            // The move the ray describes so far, read at the point it is aimed through — the last
            // place on the line that means anything, since everything past it is extrapolation.
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
