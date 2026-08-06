//! Moonbot's own Fibonacci object (chart-object type 3), as Moonbot draws it.
//!
//! A SEPARATE tool from our [`super::FibRetracement`], deliberately, because it is a different
//! object and not a different rendering of the same one:
//!
//! - Moonbot stores seven finished level PRICES, not a scale of ratios. The set of ratios behind
//!   them is a user setting on that side — the samples sit at `0 .236 .382 .5 .618 .786 1.236`
//!   while a chart drawn later showed the seventh at `1.618` — so the prices are the only thing
//!   that survives it. They are read as given and never re-derived from a scale of ours.
//! - It spans the WHOLE chart: no start, no end, the levels run into the order book. The one time
//!   in the blob is where it was drawn from, not an edge.
//!
//! Converting one into our eleven-ratio tool would therefore draw something the user never drew.
//! This tool draws what Moonbot drew.
//!
//! Both tools are offered. This one draws what Moonbot draws — Moonbot's own set of ratios, no
//! switches to turn a level off, one level draggable on its own as Moonbot allows, sent to the core
//! as a type-3 object. [`super::FibRetracement`] is the one that is ours to change: as many levels
//! as we like, each switchable, and local.

use serde::{Deserialize, Serialize};

use super::super::kind::FigureKind;
use super::super::node::FigNode;
use super::super::proj::{hline_dist, Proj, PxPoint};
use super::super::sink::{BuildCtx, GeomSink, LabelPlace, LabelText, Stroke};
use super::{FigureTool, GrabMode, ToolDef, ToolShape};

/// Levels a Moonbot Fibonacci object carries. Fixed by the format: every sampled blob is 145 bytes,
/// which is exactly this many `f64` prices after the header and the anchors.
pub const MB_FIB_LEVELS: usize = 7;

/// The ratios a fib drawn HERE places its levels at.
///
/// Measured off the wire, not chosen: every fib captured from Moonbot divides to exactly these.
/// The seventh is a setting on that side — a chart drawn later showed 1.618 where these show
/// 1.236 — so this is Moonbot's default rather than a law. It costs nothing when it differs: the
/// object stores PRICES, so a level placed here keeps its price whatever Moonbot's own set is, and
/// Moonbot labels it with its own names.
pub const MB_FIB_RATIOS: [f64; MB_FIB_LEVELS] = [0.0, 0.236, 0.382, 0.5, 0.618, 0.786, 1.236];

/// The slot Moonbot lets the user drag on its own, initially the 0.618.
///
/// This is why the object stores prices rather than a ratio set: that one level is free, so its
/// ratio is whatever the user dragged it to — a live sample reads 0.604 where the scale started at
/// 0.618. Every other slot keeps its ratio when the scale is stretched.
const MB_FIB_FREE_LEVEL: usize = 4;

/// The largest ratio worth naming. Well past 4.236, the furthest extension any charting package
/// offers, and far short of what a degenerate span produces.
const MAX_NAMED_RATIO: f64 = 100.0;

/// A Fibonacci object drawn in Moonbot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MbFib {
    /// The price the scale calls ZERO. Deliberately not described as the move's start or end:
    /// which end Moonbot anchors is not visible in the bytes, and our own [`super::FibRetracement`]
    /// numbers a move from the opposite end. What the samples do show is that a level equal to this
    /// price is labelled 0, which is all a label needs.
    pub a: f64,
    /// The price the scale calls ONE. Neither anchor is necessarily one of [`Self::levels`]: the
    /// sampled sets contain a level at ratio 0 but none at ratio 1.
    pub b: f64,
    /// When it was drawn. Kept because the blob carries it and nothing here has cause to change it
    /// — dragging the figure moves prices, not this — so a re-upsert gives it back as it arrived.
    /// It is NOT an edge: the levels run the whole width of the chart.
    pub time_ms: f64,
    /// The drawn level prices, in the order Moonbot wrote them.
    pub levels: [f64; MB_FIB_LEVELS],
}

pub(super) const DEF: ToolDef = ToolDef {
    tool: FigureTool::MbFib,
    key: "mb-fib",
    locale_key: "alerts.fig.mb_fib",
    glyph: "▤",
    clicks: 2,
    // Coloured by the scale, and filled between its levels, exactly as our own Fibonacci is: the
    // blob carries one line colour and no fill at all — every sampled length is accounted for by
    // the header and the geometry — so the bands Moonbot draws are its own level palette, and ours
    // are the same palette read by the ratio each level recovers to.
    level_palette: true,
    fills: true,
    alertable: true,
    // A move with no height is not a scale: every level would collapse onto one price, and the
    // object would be encodable and invisible at once.
    make: |nodes| match nodes {
        [a, b, ..] if a.price != b.price => Some(FigureKind::MbFib(MbFib::spanning(*a, *b))),
        _ => None,
    },
    preview: |placed, cursor| {
        placed
            .first()
            .filter(|a| a.price != cursor.price)
            .map(|a| FigureKind::MbFib(MbFib::spanning(*a, cursor)))
    },
};

impl MbFib {
    /// Builds the object a move from `a` to `b` produces, with Moonbot's own ratios.
    ///
    /// The level PRICES are computed once, here, and are what the figure carries from then on —
    /// exactly as an object drawn in Moonbot does. Nothing recomputes them later, so a set that
    /// differs on Moonbot's side changes nothing about a figure already placed.
    pub fn spanning(a: FigNode, b: FigNode) -> Self {
        let span = b.price - a.price;
        let mut levels = MB_FIB_RATIOS.map(|r| a.price + r * span);
        // Slot 0 holds the LOWER anchor, whichever way the move was drawn. Measured, and it is the
        // one rule both live samples obey: an upward fib's first slot equals `a` and a downward
        // one's equals `b`, which is ratio 0 in one and ratio 1 in the other. Placing it by ratio
        // alone would put a line where Moonbot has none on every fib drawn downward.
        levels[0] = a.price.min(b.price);
        Self {
            a: a.price,
            b: b.price,
            // Where it was drawn from, which is the one time the object carries. Not an edge: the
            // levels run the whole width of the chart.
            time_ms: a.time_ms,
            levels,
        }
    }

    /// Where `price` sits on the move, as the ratio Moonbot would label it.
    ///
    /// Derived for the LABEL only — the price is what is drawn. `None` when the move has no height,
    /// which makes every ratio a division by zero: a level is then shown by its price alone rather
    /// than by an infinity or a NaN, both of which would reach the text layer as literal glyphs.
    pub fn ratio_of(&self, price: f64) -> Option<f64> {
        let span = self.b - self.a;
        if span == 0.0 || !span.is_finite() {
            return None;
        }
        let r = (price - self.a) / span;
        // A ratio no scale would name is not a name: a span that underflows makes the division huge,
        // and `{:.3}` of 1e300 is a three-hundred-character label that nothing downstream truncates
        // and every frame re-shapes. Past the bound the level is shown by its price instead.
        if !r.is_finite() || r.abs() > MAX_NAMED_RATIO {
            return None;
        }
        Some(super::super::levels::snap_ratio(r, price, span))
    }

    /// The scale's two ends — ratio 0 and ratio 1 — ordered low to high.
    ///
    /// These are the anchors, not the outermost LEVELS: Moonbot draws a line at each anchor whether
    /// or not the level set contains one, and an extension above 1 is a projection rather than an
    /// edge. Both must be prices for the pair to mean anything.
    fn bounds(&self) -> Option<(f64, f64)> {
        (Self::is_price(self.a) && Self::is_price(self.b))
            .then(|| (self.a.min(self.b), self.a.max(self.b)))
    }

    /// The price of the level Moonbot lets the user drag, when it is one.
    fn free_level(&self) -> Option<f64> {
        self.levels
            .get(MB_FIB_FREE_LEVEL)
            .copied()
            .filter(|p| Self::is_price(*p))
    }

    /// Every level's ratio as it stands now, or `None` when the move has no height to measure by.
    fn ratios(&self) -> Option<[f64; MB_FIB_LEVELS]> {
        let span = self.b - self.a;
        if span == 0.0 || !span.is_finite() {
            return None;
        }
        Some(self.levels.map(|p| (p - self.a) / span))
    }

    /// The levels worth drawing: finite and above zero.
    ///
    /// A price at or below zero is not a level on a price chart, and a non-finite one would put a
    /// line at an undefined height; both come off a wire this codec does not own.
    fn drawn(&self) -> impl Iterator<Item = f64> + '_ {
        self.levels.iter().copied().filter(|p| Self::is_price(*p))
    }

    /// Whether a wire value can be a price on this chart.
    ///
    /// Bounded above as well as below: the geometry buffers carry `f32`, so a finite `f64` past
    /// `f32::MAX` reaches the GPU as an infinity — the same check `moon-chart`'s fill sink makes
    /// for exactly this reason.
    fn is_price(p: f64) -> bool {
        p.is_finite() && p > 0.0 && p <= f32::MAX as f64
    }
}

impl ToolShape for MbFib {
    fn def(&self) -> &'static ToolDef {
        &DEF
    }

    /// Three, as in Moonbot: the line at each end of the scale, which stretch it, and the one level
    /// that slides on its own.
    fn handle_count(&self) -> usize {
        3
    }

    /// Handle 0 is the scale's bottom line, 1 its top, 2 the free level.
    ///
    /// The ends are the ANCHORS — ratio 0 and ratio 1 — which Moonbot draws whether or not the level
    /// set contains them, and which are what bounds the scale: the extension above 1 is a
    /// projection, not an edge. Index 0 doubles as the figure's anchor price for the alerts list.
    fn handle(&self, i: usize) -> Option<FigNode> {
        let (lo, hi) = self.bounds()?;
        let price = match i {
            0 => lo,
            1 => hi,
            2 => self.free_level()?,
            _ => return None,
        };
        Some(FigNode::new(self.time_ms, price))
    }

    /// Stretches the scale from one end, or slides the free level alone.
    ///
    /// Dragging an END moves that anchor and leaves the other where it is; every level is then
    /// recomputed from the ratio it ALREADY had, which is what keeps a level the user had dragged
    /// where they put it. Dragging the free level moves that price and nothing else — its ratio is
    /// then whatever it now measures, exactly as Moonbot's own does.
    fn move_handle(&mut self, i: usize, to: FigNode) -> bool {
        if !Self::is_price(to.price) || i > 2 {
            return false;
        }
        if i == 2 {
            let Some(slot) = self.levels.get_mut(MB_FIB_FREE_LEVEL) else {
                return false;
            };
            if *slot == to.price {
                return false;
            }
            *slot = to.price;
            return true;
        }
        let Some(ratios) = self.ratios() else {
            return false;
        };
        // Which stored anchor is the end being dragged: `a` and `b` are the first and second click,
        // in either order, while the handles are the bottom and the top.
        let moving_is_a = (self.a < self.b) == (i == 0);
        let end = match moving_is_a {
            true => &mut self.a,
            false => &mut self.b,
        };
        if *end == to.price {
            return false;
        }
        *end = to.price;
        let (a, span) = (self.a, self.b - self.a);
        for (level, ratio) in self.levels.iter_mut().zip(ratios) {
            *level = a + ratio * span;
        }
        true
    }

    /// Moved as a whole, by PRICE, with every stored level carried along.
    ///
    /// Shifting the prices rather than recomputing them from a ratio set is what makes this safe
    /// for an object drawn in MOONBOT, whose set may not be ours: the levels keep whatever spacing
    /// they arrived with.
    ///
    /// Time is untouched, and a purely horizontal drag is not a move. The figure spans the whole
    /// chart, so sideways travel changes nothing you can see — while `@64` is the instant the
    /// object was DRAWN, and rewriting that in another program's object on every drag would be a
    /// change nobody asked for that the core would keep.
    fn translate(&mut self, _dt_ms: f64, dp: f64) -> bool {
        if dp == 0.0 {
            return false;
        }
        self.a += dp;
        self.b += dp;
        for level in &mut self.levels {
            *level += dp;
        }
        true
    }

    /// Distance to the nearest drawn level, measured vertically: every level spans the full width,
    /// so there is no X to aim at.
    fn hit(&self, pos: PxPoint, proj: &dyn Proj) -> f32 {
        self.drawn()
            .map(|p| hline_dist(pos, p, proj))
            .fold(f32::INFINITY, f32::min)
    }

    fn build(&self, ctx: &BuildCtx, sink: &mut dyn GeomSink) {
        // What Moonbot draws, in Moonbot's order: a line at each END of the scale carrying the
        // percentage between them instead of a name, the levels between carrying their ratios, and
        // an extension above 1 as a bare line. The fill stops at the ends — a projection past the
        // move is not part of the move, and filling it was the one thing our picture had that
        // Moonbot's did not.
        let Some((lo, hi)) = self.bounds() else {
            return;
        };
        // Every line, ordered by price and deduplicated: a level sitting exactly on an anchor is
        // one line, not two, and it is the ANCHOR — that is why it reads as a percentage.
        let mut lines: Vec<(f64, Option<f64>)> = self
            .drawn()
            .filter(|p| *p != lo && *p != hi)
            .map(|p| (p, self.ratio_of(p)))
            .chain([(lo, None), (hi, None)])
            .collect();
        lines.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));

        // The bands, under the lines that bound them, and only INSIDE the move.
        for pair in lines.windows(2) {
            let [(p0, r0), (p1, _)] = [pair[0], pair[1]];
            if p0 < lo || p1 > hi {
                continue;
            }
            // The band takes the hue of the level below it; against the bottom anchor there is no
            // ratio to look up, so the scale's own zero names it.
            let hue = super::super::levels::color_of_ratio(r0.unwrap_or(0.0));
            if let Some([r, g, b]) = hue {
                sink.band(
                    f64::NEG_INFINITY,
                    f64::INFINITY,
                    p0,
                    p1,
                    [
                        r as f32 / 255.0,
                        g as f32 / 255.0,
                        b as f32 / 255.0,
                        ctx.fill[3],
                    ],
                );
            }
        }

        for (price, ratio) in &lines {
            // Each line in its LEVEL's hue, like our own scale: the reader recognises 0.618 by its
            // teal without reading the number. The ends and any ratio our palette has no entry for
            // fall back to the figure's own colour.
            let hue = ratio.and_then(super::super::levels::color_of_ratio);
            let stroke = match hue {
                Some([r, g, b]) => Stroke {
                    color: [
                        r as f32 / 255.0,
                        g as f32 / 255.0,
                        b as f32 / 255.0,
                        ctx.stroke.color[3],
                    ],
                    ..ctx.stroke
                },
                None => ctx.stroke,
            };
            sink.hline(*price, &stroke);
            // The readout, read as a column at the left edge where Moonbot puts it. An END carries
            // the move itself as a percentage — up from the bottom, down from the top — which is
            // what Moonbot shows there instead of "0" and "1".
            let text = match ratio {
                Some(ratio) => LabelText::Level {
                    ratio: *ratio,
                    price: *price,
                },
                None if *price == hi => LabelText::PctDelta { from: lo, to: hi },
                None if *price == lo => LabelText::PctDelta { from: hi, to: lo },
                // A level whose ratio cannot be measured draws its line and stays silent: a bare
                // price in a column of ratios reads as something else entirely.
                None => continue,
            };
            sink.label(
                FigNode::new(0.0, *price),
                LabelPlace::LineSpan {
                    t0_ms: f64::NEG_INFINITY,
                    t1_ms: f64::INFINITY,
                },
                text,
                stroke.color,
            );
        }
    }

    fn grab_mode(&self) -> GrabMode {
        GrabMode::PriceLines
    }
}

#[cfg(test)]
mod tests;
