//! Keeping the three bands of one zone off each other.
//!
//! A zone is laid out three times — once per alignment — and until this module existed each of
//! those passes was handed the WHOLE zone to spend. That is what let a centred detect line wrap
//! across the entire plot and print straight over the modules pinned to either edge: the three
//! passes never saw each other.
//!
//! They still do not measure each other, which is the point. The bands printing FIGURES are drawn
//! first and report what they took; the ELASTIC band — the one that wraps, whose width is its
//! budget rather than its content — is drawn last, into what is left. The figures are never
//! measured ahead of being drawn; the one measurement this costs is a single line of the elastic
//! band, and this pass runs on every presented frame.
//!
//! What is divided, and what is not: a zone holding exactly ONE elastic band, and only while the
//! division leaves the figures a legible width. Two figure bands that would meet in the middle are
//! left alone — they were never the defect, and narrowing figures against each other would truncate
//! captions that print fine today.

use moon_core::config::LabelAlign;

use super::{CAPTION_GAP, MIN_LEGIBLE_W};

/// Largest share of a zone the elastic band may take off the bands beside it.
///
/// It wraps into every pixel it is given, so a long detect line would push the modules on either
/// side out of the pane if nothing bounded it. Past this share the SENTENCE is the one that wraps
/// tighter.
const PROSE_MAX_FRAC: f32 = 0.4;

/// What each band of a zone has taken so far, by the edge it anchors to.
///
/// Named slots rather than a positional array: the drawing pass fills them one at a time, and three
/// numbers addressed by an alignment's index are one reordering away from quietly swapping the left
/// band for the right one.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub(super) struct Taken {
    left: f32,
    centre: f32,
    right: f32,
}

impl Taken {
    /// Record what one band took. A width that came back negative or NaN is read as zero: the
    /// budget this ends up in is compared against [`MIN_LEGIBLE_W`] by the drawing pass, and a NaN
    /// compares false against it — it would let a caption through at an unknown width.
    pub(super) fn set(&mut self, align: LabelAlign, w: f32) {
        let slot = match align {
            LabelAlign::Left => &mut self.left,
            LabelAlign::Center => &mut self.centre,
            LabelAlign::Right => &mut self.right,
        };
        *slot = w.max(0.0);
    }
}

/// How wide the figure bands of a zone may print when an elastic band shares it with them.
///
/// `None` means the zone is not divided at all: its bands keep the whole width and print over each
/// other as they did before. That is deliberate, and it mirrors the vertical axis — `draw_stack`
/// exempts the first line of a band from its own clamp for the same reason. Under
/// [`MIN_LEGIBLE_W`] a caption is not truncated but DROPPED, so the guard is where truncating a
/// figure band turns into losing it, and a pane that loses its coin and its core name is worse off
/// than one whose captions touch.
///
/// Args:
///     total: Width of the whole zone, in logical pixels.
///     prose_w: What the elastic band would take on one line; `0` — nothing to divide for.
///
/// Returns:
///     The budget every figure band of the zone is drawn at, or `None` to divide nothing.
///
/// What it leaves is exact for an elastic band in the CENTRE — the shipped shape, and the one the
/// tests pin: two edges capped here leave the centre precisely what it was owed. An elastic band on
/// an EDGE is bounded by the centred band beside it instead, which leaves it less and wraps it a
/// line sooner; a centred neighbour that draws a hair wider than it measured — a split prefix and
/// value shape as two runs — can still come within a pixel or two of it.
pub(super) fn edge_cap(total: f32, prose_w: f32) -> Option<f32> {
    let total = total.max(0.0);
    let owed = prose_owed(total, prose_w);
    if owed <= 0.0 {
        return None;
    }
    let cap = (total - owed) * 0.5 - CAPTION_GAP;
    (cap >= MIN_LEGIBLE_W).then_some(cap)
}

/// What the elastic band is owed: what it asks for, and never more than its share of the zone.
///
/// Asking is one measurement of one line — the wrapping itself needs a budget, which is what this
/// answers, so it cannot be the thing that decides it.
fn prose_owed(total: f32, prose_w: f32) -> f32 {
    (total * PROSE_MAX_FRAC).min(prose_w.max(0.0))
}

/// Width still free for one band, given what the bands drawn before it took.
///
/// A centred band stays centred on the zone: it loses the WIDER of its two neighbours on BOTH
/// sides. Fitting it into the free interval instead would win a few pixels and cost what those
/// pixels are for — the edges print figures that are re-rendered every tick, and a centre anchored
/// to them would slide sideways every time a price grew a digit. That is also why an EMPTY side is
/// not handed to the opposite edge: an edge grows toward the middle, so reaching the free half
/// means crossing the centred band, and the only way to spend it is to move the centre off centre.
///
/// Args:
///     total: Width of the whole zone, in logical pixels.
///     align: Which edge — or the middle — this band is anchored to.
///     taken: What the bands already drawn took.
///
/// Returns:
///     What this band may print at, never negative.
pub(super) fn free_width(total: f32, align: LabelAlign, taken: Taken) -> f32 {
    let Taken {
        left,
        centre,
        right,
    } = taken;
    // A band that drew nothing bounds nothing — not even by a gap. Charging for it would narrow
    // every caption on a pane holding one module, which is most of them.
    let bound = |taken: f32, free: f32| match taken > 0.0 {
        true => free - CAPTION_GAP,
        false => f32::INFINITY,
    };
    let free = match align {
        LabelAlign::Center => {
            let reserve = left.max(right);
            bound(reserve, total - 2.0 * reserve - CAPTION_GAP)
        }
        LabelAlign::Left => bound(centre, (total - centre) * 0.5).min(bound(right, total - right)),
        LabelAlign::Right => bound(centre, (total - centre) * 0.5).min(bound(left, total - left)),
    };
    free.min(total).max(0.0)
}

#[cfg(test)]
mod tests;
