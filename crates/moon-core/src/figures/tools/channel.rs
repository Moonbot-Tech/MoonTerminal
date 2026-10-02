//! Horizontal channel: a price corridor of two horizontal lines (Moonbot's chart-object type 5,
//! which stores two prices and no time).
//!
//! Shown to the user as **Zone**, which is what Moonbot calls it. The Rust type keeps the name
//! `Channel` on purpose: it is the tag `figures.json` is written with, so renaming it would stop
//! every saved corridor from loading.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::node::FigNode;
use super::super::proj::{Proj, PxPoint, hline_dist};
use super::super::sink::{BuildCtx, GeomSink, LabelPlace, LabelText};
use super::{FigureTool, GrabMode, ToolDef, ToolShape};

/// Horizontal channel defined by two prices.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub price1: f64,
    pub price2: f64,
}

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::Channel,
    key: "channel",
    locale_key: "alerts.fig.channel",
    glyph: "☰",
    clicks: 2,
    drag_rest: None,
    scale_swatch: None,
    fills: true,
    alertable: true,
    make: |nodes| match nodes {
        [a, b, ..] => Some(FigureKind::Channel(Channel {
            price1: a.price,
            price2: b.price,
        })),
        _ => None,
    },
    preview: |placed, cursor| {
        placed.first().map(|a| {
            FigureKind::Channel(Channel {
                price1: a.price,
                price2: cursor.price,
            })
        })
    },
};

impl ToolShape for Channel {
    fn def(&self) -> &'static ToolDef {
        &DEF
    }

    /// The corridor IS a band: its two prices are exactly what it stores.
    fn price_band(&self) -> Option<(f64, f64)> {
        Some((self.price1, self.price2))
    }

    fn handle_count(&self) -> usize {
        2
    }

    fn handle(&self, i: usize) -> Option<FigNode> {
        // Time is meaningless for this tool; each node carries one corridor price.
        match i {
            0 => Some(FigNode::new(0.0, self.price1)),
            1 => Some(FigNode::new(0.0, self.price2)),
            _ => None,
        }
    }

    fn move_handle(&mut self, i: usize, to: FigNode) -> bool {
        let p = match i {
            0 => &mut self.price1,
            1 => &mut self.price2,
            _ => return false,
        };
        if *p == to.price {
            return false;
        }
        *p = to.price;
        true
    }

    fn translate(&mut self, _dt_ms: f64, dp: f64) -> bool {
        if dp == 0.0 {
            return false;
        }
        self.price1 += dp;
        self.price2 += dp;
        true
    }

    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        hline_dist(pos, self.price1, proj).min(hline_dist(pos, self.price2, proj))
    }

    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        // The corridor's body, spanning the whole plot like the lines that bound it. `band` orders
        // its own prices, so a channel drawn upward fills the same as one drawn downward.
        sink.band(
            f64::NEG_INFINITY,
            f64::INFINITY,
            self.price1,
            self.price2,
            ctx.fill,
        );
        sink.hline(self.price1, &ctx.stroke);
        sink.hline(self.price2, &ctx.stroke);
        self.build_span_readout(ctx, sink);
        if ctx.hot {
            for price in [self.price1, self.price2] {
                sink.label(
                    FigNode::new(0.0, price),
                    LabelPlace::RightEdge,
                    LabelText::Price(price),
                    ctx.stroke.color,
                );
            }
        }
    }

    fn grab_mode(&self) -> GrabMode {
        GrabMode::PriceLines
    }
}

/// Below this move, in percent, the corridor's width prints as `0.00%` at the chart's two
/// decimals, so the price is printed instead — Moonbot's `IsThinLine` fallback. Moonbot's own
/// threshold is 0.001%, finer than its two-decimal print, which leaves a band of `+0.00%`
/// readouts; this one sits at about where the print rounds to zero (the text pass goes through
/// `f32`, so the exact edge is a hair either side).
const THIN_PCT: f64 = 0.005;

impl Channel {
    /// The corridor's width as a percentage at the left edge of each line, drawn always rather than
    /// on hover: the readout is what makes the zone a ruler, and Moonbot's `TPriceRect` prints it
    /// unconditionally too. Up from the bottom at the upper line, down from the top at the lower
    /// one, so the sign follows the geometry rather than which line was placed first.
    fn build_span_readout(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        let (lo, hi) = if self.price1 <= self.price2 {
            (self.price1, self.price2)
        } else {
            (self.price2, self.price1)
        };
        // `LineSpan` over the whole time axis: the label rides the plot's left edge, where the
        // corridor's lines start, and the per-tab "line labels" switch hides it like a scale's.
        let full_width = LabelPlace::LineSpan {
            t0_ms: f64::NEG_INFINITY,
            t1_ms: f64::INFINITY,
        };
        let label = |sink: &mut dyn GeomSink, price: f64, text: LabelText| {
            sink.label(FigNode::new(0.0, price), full_width, text, ctx.stroke.color);
        };
        // A percentage needs a positive base and a finite ratio: a corridor at zero or below, or
        // one whose ratio overflows (a malformed alert from a core), keeps its lines and prints no
        // readout rather than `-100%`, `inf%` or `NaN%`.
        if !(lo > 0.0 && (hi / lo).is_finite()) {
            return;
        }
        // Judged on the DOWN move, the smaller of the two: both readouts must clear the print.
        if (1.0 - lo / hi) * 100.0 < THIN_PCT {
            // Too thin to print as a percentage: one price, not two near-identical copies of it.
            label(sink, hi, LabelText::Price(hi));
            return;
        }
        label(sink, hi, LabelText::PctDelta { from: lo, to: hi });
        label(sink, lo, LabelText::PctDelta { from: hi, to: lo });
    }
}

#[cfg(test)]
mod tests;
