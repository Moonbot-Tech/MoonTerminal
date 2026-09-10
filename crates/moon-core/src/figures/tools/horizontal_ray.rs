//! A single-origin horizontal ray extending toward later chart times.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::node::FigNode;
use super::super::proj::{Proj, PxPoint};
use super::super::sink::{BuildCtx, GeomSink};
use super::{FigureTool, ToolDef, ToolShape};

/// A fixed-price ray with one draggable origin; its direction is never stored or edited.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HorizontalRay {
    /// The earliest time and the price included in the ray.
    pub origin: FigNode,
}

/// One click completes this terminal-only tool, including an immediate cursor preview.
pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::HorizontalRay,
    key: "horizontal_ray",
    locale_key: "alerts.fig.horizontal_ray",
    glyph: "→",
    clicks: 1,
    drag_rest: None,
    scale_swatch: None,
    fills: false,
    alertable: false,
    make: |nodes| {
        nodes
            .first()
            .map(|origin| FigureKind::HorizontalRay(HorizontalRay { origin: *origin }))
    },
    preview: |_, cursor| Some(FigureKind::HorizontalRay(HorizontalRay { origin: cursor })),
};

impl ToolShape for HorizontalRay {
    /// Returns the registry metadata shared by the picker and persistence defaults.
    fn def(&self) -> &'static ToolDef {
        &DEF
    }

    /// Only the origin is editable; there is no slope handle.
    fn handle_count(&self) -> usize {
        1
    }

    /// Exposes the origin to the generic knot picker.
    fn handle(&self, i: usize) -> Option<FigNode> {
        (i == 0).then_some(self.origin)
    }

    /// Moves the origin in both coordinates without introducing a second price.
    fn move_handle(&mut self, i: usize, to: FigNode) -> bool {
        if i != 0 || self.origin == to {
            return false;
        }
        self.origin = to;
        true
    }

    /// Body dragging has the same geometry as translating its only origin.
    fn translate(&mut self, dt_ms: f64, dp: f64) -> bool {
        self.move_handle(0, self.origin.shifted(dt_ms, dp))
    }

    /// Measures the right half-line, clamping the nearest point at the origin.
    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        let origin = proj.px_of(self.origin);
        (origin.0 - pos.0).max(0.0).hypot(pos.1 - origin.1)
    }

    /// Emits the existing ray primitive with equal prices and a strictly later aim.
    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        // A day keeps the direction distinct after GPU f32 time conversion at wide zooms.
        // This is an aim, not an endpoint: the renderer extends it to the right plot edge.
        let aim = self.origin.shifted(86_400_000.0, 0.0);
        sink.ray(self.origin, aim, &ctx.stroke);
    }
}

#[cfg(test)]
mod tests;
