//! Long/short position: an entry, a target and a stop, with the two zones they enclose.
//!
//! The trader's arithmetic drawn on the chart. Two clicks place the entry and the target; the stop
//! starts at half the reward and is dragged from there. Which side is profit and which is risk is
//! not a setting — it is READ from the geometry: a target above the entry is a long, below it a
//! short, and dragging the target through the entry flips the position with it.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::node::FigNode;
use super::super::proj::{Proj, PxPoint, seg_dist};
use super::super::sink::{BuildCtx, GeomSink, LabelPlace, LabelText};
use super::{FigureTool, ToolDef, ToolShape};

/// Fill of the half that pays: green in every charting package there is.
const PROFIT_RGB: [u8; 3] = [0x26, 0xA6, 0x69];
/// Fill of the half that costs.
const RISK_RGB: [u8; 3] = [0xE0, 0x4D, 0x4D];

/// Reward the default stop risks, as a fraction: the box opens at two to one, which is the ratio a
/// setup is usually judged against before it is adjusted.
const DEFAULT_RISK_FRACTION: f64 = 0.5;

/// A planned position: where it is opened, where it is taken, where it is cut.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    /// Where the box starts in time.
    pub t0_ms: f64,
    /// Where it ends. Both zones and all three lines stop here.
    pub t1_ms: f64,
    /// The price the position is opened at.
    pub entry: f64,
    /// The price it is closed at in profit. Above `entry` for a long, below for a short.
    pub target: f64,
    /// The price it is cut at. On the opposite side of `entry` from `target`.
    pub stop: f64,
}

impl Position {
    /// Builds the box two clicks describe, placing a stop that has not been aimed yet.
    ///
    /// The stop is not left ON the entry: a zero-height risk zone would draw nothing, read as a
    /// position that cannot lose, and give a ratio of infinity.
    fn placed(entry: FigNode, target: FigNode) -> Self {
        let reward = target.price - entry.price;
        Self {
            t0_ms: entry.time_ms,
            t1_ms: target.time_ms,
            entry: entry.price,
            target: target.price,
            stop: entry.price - reward * DEFAULT_RISK_FRACTION,
        }
    }

    /// Whether this is a LONG: the target sits above the entry.
    ///
    /// Read from the geometry rather than stored, so the figure cannot disagree with itself after a
    /// drag — and so dragging the target through the entry turns a long into a short, which is what
    /// the two coloured zones then show without any further bookkeeping.
    pub fn is_long(&self) -> bool {
        self.target >= self.entry
    }

    /// Reward divided by risk, or `None` when the stop sits exactly on the entry and the question
    /// has no answer.
    pub fn risk_reward(&self) -> Option<f64> {
        let risk = (self.entry - self.stop).abs();
        (risk > 0.0).then(|| (self.target - self.entry).abs() / risk)
    }
}

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::Position,
    key: "position",
    locale_key: "alerts.fig.position",
    glyph: "⇅",
    clicks: 2,
    // The two zones stand for profit and loss, so their colours are the tool's and not the style's;
    // the swatch the settings panel offers is the profit green.
    scale_swatch: Some(|| PROFIT_RGB),
    fills: true,
    // The core's chart-object blob has no position type.
    alertable: false,
    make: |nodes| match nodes {
        [entry, target, ..] => Some(FigureKind::Position(Position::placed(*entry, *target))),
        _ => None,
    },
    preview: |placed, cursor| {
        placed
            .first()
            .map(|entry| FigureKind::Position(Position::placed(*entry, cursor)))
    },
};

impl ToolShape for Position {
    fn def(&self) -> &'static ToolDef {
        &DEF
    }

    fn handle_count(&self) -> usize {
        3
    }

    fn handle(&self, i: usize) -> Option<FigNode> {
        match i {
            0 => Some(FigNode::new(self.t0_ms, self.entry)),
            1 => Some(FigNode::new(self.t1_ms, self.target)),
            2 => Some(FigNode::new(self.t1_ms, self.stop)),
            _ => None,
        }
    }

    /// The entry handle carries the box's START in time, the target handle its END, and the stop
    /// handle only a price.
    ///
    /// The stop deliberately does not move the box's end: it sits on the same edge as the target,
    /// and letting both drag that edge would make the width jump depending on which of two
    /// overlapping handles was grabbed.
    fn move_handle(&mut self, i: usize, to: FigNode) -> bool {
        let before = *self;
        match i {
            0 => {
                self.t0_ms = to.time_ms;
                self.entry = to.price;
            }
            1 => {
                self.t1_ms = to.time_ms;
                self.target = to.price;
            }
            2 => self.stop = to.price,
            _ => return false,
        }
        *self != before
    }

    fn translate(&mut self, dt_ms: f64, dp: f64) -> bool {
        if dt_ms == 0.0 && dp == 0.0 {
            return false;
        }
        self.t0_ms += dt_ms;
        self.t1_ms += dt_ms;
        self.entry += dp;
        self.target += dp;
        self.stop += dp;
        true
    }

    /// Grabbed by any of its three lines. The zones between them are not hit targets: a box tall
    /// enough to plan a trade in covers a good part of the pane, and grabbing it by the middle
    /// would take every click meant for the chart underneath.
    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        [self.entry, self.target, self.stop]
            .into_iter()
            .map(|price| {
                seg_dist(
                    pos,
                    proj.px_of(FigNode::new(self.t0_ms, price)),
                    proj.px_of(FigNode::new(self.t1_ms, price)),
                )
            })
            .fold(f32::INFINITY, f32::min)
    }

    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        // The zones first, so the three lines are drawn over their own fill.
        let alpha = ctx.fill[3];
        let zone = |rgb: [u8; 3]| {
            [
                rgb[0] as f32 / 255.0,
                rgb[1] as f32 / 255.0,
                rgb[2] as f32 / 255.0,
                alpha,
            ]
        };
        sink.band(self.t0_ms, self.t1_ms, self.entry, self.target, zone(PROFIT_RGB));
        sink.band(self.t0_ms, self.t1_ms, self.entry, self.stop, zone(RISK_RGB));
        for price in [self.entry, self.target, self.stop] {
            sink.seg(
                FigNode::new(self.t0_ms, price),
                FigNode::new(self.t1_ms, price),
                &ctx.stroke,
            );
        }
        if !ctx.hot {
            return;
        }
        // What the box is FOR, read at the edge the numbers belong to: how far each exit is, and
        // what the trade pays for what it risks.
        let at = |price: f64| FigNode::new(self.t1_ms, price);
        sink.label(
            at(self.target),
            LabelPlace::Above,
            LabelText::PctDelta {
                from: self.entry,
                to: self.target,
            },
            ctx.stroke.color,
        );
        sink.label(
            at(self.stop),
            LabelPlace::Above,
            LabelText::PctDelta {
                from: self.entry,
                to: self.stop,
            },
            ctx.stroke.color,
        );
        if let Some(rr) = self.risk_reward() {
            sink.label(
                at(self.entry),
                LabelPlace::Above,
                LabelText::RiskReward(rr),
                ctx.stroke.color,
            );
        }
    }
}

#[cfg(test)]
mod tests;
