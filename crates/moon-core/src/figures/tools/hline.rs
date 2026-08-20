//! Horizontal line: one price, spanning the whole plot in time.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::node::FigNode;
use super::super::proj::{hline_dist, Proj, PxPoint};
use super::super::sink::{BuildCtx, GeomSink, LabelPlace, LabelText};
use super::{FigureTool, GrabMode, ToolDef, ToolShape};

/// Horizontal line at a price, extending indefinitely in time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HLine {
    pub price: f64,
}

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::HLine,
    key: "hline",
    locale_key: "alerts.fig.hline",
    glyph: "─",
    clicks: 1,
    drag_rest: None,
    scale_swatch: None,
    fills: false,
    alertable: true,
    make: |nodes| {
        nodes
            .first()
            .map(|n| FigureKind::HLine(HLine { price: n.price }))
    },
    // A horizontal line follows the cursor from the first move: there is nothing to place first.
    preview: |_, cursor| {
        Some(FigureKind::HLine(HLine {
            price: cursor.price,
        }))
    },
};

impl ToolShape for HLine {
    fn def(&self) -> &'static ToolDef {
        &DEF
    }

    fn handle_count(&self) -> usize {
        1
    }

    fn handle(&self, i: usize) -> Option<FigNode> {
        // Time is meaningless for this tool; the node carries the price alone.
        (i == 0).then(|| FigNode::new(0.0, self.price))
    }

    fn move_handle(&mut self, i: usize, to: FigNode) -> bool {
        if i != 0 || self.price == to.price {
            return false;
        }
        self.price = to.price;
        true
    }

    fn translate(&mut self, _dt_ms: f64, dp: f64) -> bool {
        if dp == 0.0 {
            return false;
        }
        self.price += dp;
        true
    }

    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        hline_dist(pos, self.price, proj)
    }

    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        sink.hline(self.price, &ctx.stroke);
        if ctx.hot {
            sink.label(
                FigNode::new(0.0, self.price),
                LabelPlace::RightEdge,
                LabelText::Price(self.price),
                ctx.stroke.color,
            );
        }
    }

    fn grab_mode(&self) -> GrabMode {
        GrabMode::PriceLines
    }
}

#[cfg(test)]
mod tests;
