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
//! captions that print fine today. A COLUMN is not a figure: its natural width is the longest line
//! it holds, which is routinely the whole plot, so a line of it yields to figures that actually
//! share its Y — and spends the rest of the zone once those figures have ended. It is not elastic
//! either — two wrapping bands skip the split, and a column is not prose.

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
    left_h: f32,
    centre_h: f32,
    right_h: f32,
}

impl Taken {
    /// Record what one band took. A width that came back negative or NaN is read as zero: the
    /// budget this ends up in is compared against [`MIN_LEGIBLE_W`] by the drawing pass, and a NaN
    /// compares false against it — it would let a caption through at an unknown width. Height zero
    /// means "still in the way at every Y" — the conservative reading, and what the width-only
    /// tests still ask for.
    #[cfg(test)]
    pub(super) fn set(&mut self, align: LabelAlign, w: f32) {
        self.set_extent(align, w, 0.0);
    }

    /// Record a band's width and how far it runs in the fill direction, in logical pixels.
    pub(super) fn set_extent(&mut self, align: LabelAlign, w: f32, h: f32) {
        let (width, height) = match align {
            LabelAlign::Left => (&mut self.left, &mut self.left_h),
            LabelAlign::Center => (&mut self.centre, &mut self.centre_h),
            LabelAlign::Right => (&mut self.right, &mut self.right_h),
        };
        *width = w.max(0.0);
        *height = h.max(0.0);
    }

    /// Neighbours that still occupy `travelled` pixels into the fill. A column line past that
    /// depth no longer shares Y with them, so [`free_width`] can spend the rest of the zone.
    ///
    /// A neighbour recorded with no height keeps blocking: the drawing pass always knows how tall
    /// a band it just drew, and a missing height must not be read as "out of the way".
    pub(super) fn blocking_at(self, travelled: f32) -> Self {
        let keep = |w: f32, h: f32| {
            if w <= 0.0 {
                0.0
            } else if h <= 0.0 || travelled < h {
                w
            } else {
                0.0
            }
        };
        Self {
            left: keep(self.left, self.left_h),
            centre: keep(self.centre, self.centre_h),
            right: keep(self.right, self.right_h),
            ..Self::default()
        }
    }

    /// How far into the fill the neighbours that actually take width still run.
    ///
    /// A wrapping skip list that would start inside this strip is moved past it rather than
    /// squeezed beside a one-line core name — squeezing wraps the sentence at half the plot and
    /// still reads as printing through the name.
    pub(super) fn blocking_height(self, align: LabelAlign) -> f32 {
        let h = |w: f32, h: f32| if w > 0.0 { h.max(0.0) } else { 0.0 };
        match align {
            LabelAlign::Left => h(self.centre, self.centre_h).max(h(self.right, self.right_h)),
            LabelAlign::Right => h(self.centre, self.centre_h).max(h(self.left, self.left_h)),
            LabelAlign::Center => h(self.left, self.left_h).max(h(self.right, self.right_h)),
        }
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
        ..
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

/// Width a column line may spend at this depth of the stack.
///
/// `travelled` is how far the line sits from the band's own edge, in the fill direction. Neighbours
/// shorter than that no longer share its Y, so the line takes whatever they left — the whole zone
/// when nothing else is in the way.
pub(super) fn line_budget(
    total: f32,
    align: LabelAlign,
    taken: Taken,
    travelled: f32,
    inset: f32,
) -> f32 {
    (free_width(total, align, taken.blocking_at(travelled)) - inset).max(0.0)
}

/// Draw order inside one zone: figures, then columns, then wrapping prose.
///
/// Packed into one key so the drawing pass can `sort_by_key` without a two-key tuple, and so the
/// rank that keeps a column from counting as a second elastic band is stated once.
pub(super) fn band_rank(elastic: bool, hungry: bool) -> u8 {
    u8::from(elastic) * 2 + u8::from(hungry)
}

/// Width one band may spend, given the zone split and what has already been drawn.
///
/// Figures keep the whole zone when nothing wraps. A column — not elastic — is drawn after them
/// and spends only what they left. When a wrapping band IS dividing the zone, the column takes the
/// same figure cap the detect line already reserved, rather than the leftover of an empty `taken`
/// (which would be the whole zone and print through the sentence).
pub(super) fn band_max_w(
    total: f32,
    cap: Option<f32>,
    elastic: bool,
    hungry: bool,
    align: LabelAlign,
    taken: Taken,
) -> f32 {
    match (cap, elastic, hungry) {
        (None, false, true) => free_width(total, align, taken),
        (None, _, _) => total,
        (Some(cap), false, _) => cap,
        (Some(_), true, _) => free_width(total, align, taken),
    }
}

#[cfg(test)]
mod tests;
