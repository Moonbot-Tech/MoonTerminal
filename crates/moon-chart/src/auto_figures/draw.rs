//! A detector's [`Found`] figure turned into what the chart draws: ordinary figure-layer figures
//! (`moon_core::figures`) plus the filled band of a level drawn as a zone.
//!
//! The auto figures have no renderer of their own (LinKvo, 18.11.2026: "drawing — existing
//! figures"). Each found shape maps onto a figure TOOL — a horizontal line, a horizontal ray, a
//! segment, a ray — so the three GPU backends draw it with code they already run for the user's
//! own figures, and a new shape here is a mapping, never a shader. The figures built here are
//! values the caller hands to `figures::build_passive_figure_geometry`; they never enter the
//! `FigureStore`, so they are never saved, hit-tested, dragged, listed as alerts or sent to a core.
//!
//! Colour arrives already resolved: picking it is a theme decision (the row's `LabelColor` against
//! the active palette), which belongs to the terminal, not to this backend-free crate.

use moon_core::config::auto_figures::{Extend, LineStyle};
use moon_core::figures::tools::{HLine, HorizontalRay, Ray, Segment};
use moon_core::figures::{DrawStyle, FigNode, Figure, FigureKind, LineKind};

use super::{Found, FoundShape, LineTag, PairSide, Seg, positive, within};
use crate::figures::FigureBand;

/// A row's line style with its colour already resolved, ready to stamp onto figures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedStyle {
    /// Line colour, `[r, g, b]`.
    pub rgb: [u8; 3],
    /// Factor on the alpha of everything drawn — the lines (on top of the figure layer's own idle
    /// alpha) and the zone bands: the configuration's one opacity for the block
    /// (`AutoFiguresCfg::opacity`). `1.0` from [`Self::new`], which is also what a pinned figure
    /// takes: once pinned it is the reader's own figure, drawn like every other.
    pub opacity: f32,
    pub thickness: f32,
    /// Dash pattern of a live line.
    pub line_kind: LineKind,
    /// Dash pattern of a line kept after a break (`BrokenPolicy::Keep`).
    pub broken_kind: LineKind,
    pub extend: Extend,
    /// Opacity of the tolerance band around a level, `None` for a bare line.
    pub zone_alpha: Option<f32>,
}

impl ResolvedStyle {
    /// `style` with its colour reference resolved to `rgb` by the caller.
    pub fn new(style: &LineStyle, rgb: [u8; 3]) -> Self {
        Self {
            rgb,
            opacity: 1.0,
            thickness: style.thickness,
            line_kind: style.line_kind,
            broken_kind: style.broken_kind,
            extend: style.extend,
            zone_alpha: style.zone,
        }
    }

    /// The same style drawn at `opacity` ([`Self::opacity`]); a value that is not a finite number
    /// in `0..=1` is clamped into it, NaN to fully opaque, so nothing here can poison a colour.
    pub fn with_opacity(self, opacity: f32) -> Self {
        Self {
            opacity: unit_alpha(opacity),
            ..self
        }
    }
}

/// `a` as an alpha factor in `0..=1`; NaN reads as `1.0`.
fn unit_alpha(a: f32) -> f32 {
    if a.is_nan() { 1.0 } else { a.clamp(0.0, 1.0) }
}

/// Append the figures and bands one found figure draws as.
///
/// Args:
///     found: The detector's figure.
///     style: Its row's resolved style.
///     ref_ms: The reference time (the detector run's "now"): where a squeeze's sloped half, and
///         each side of a converging formation (a triangle, a wedge, a flag or pennant), stops
///         when its apex lies further ahead. A channel's sides and a horizontal pair's levels run
///         by `extend` alone and do not read it.
///     figures: Receives the lines, in draw order.
///     bands: Receives the zone bands.
///
/// A zone band is as wide as the tolerance the detector grouped the touches by
/// ([`Found::tolerance_frac`]) — not one re-read from the row's parameters, which under an ATR
/// tolerance would be the wrong unit. A figure whose geometry is not finite and positive draws
/// nothing: these come from market data, and a NaN reaching the instance buffers would poison the
/// whole draw call, not just this line.
pub fn append_shapes(
    found: &Found,
    style: &ResolvedStyle,
    ref_ms: f64,
    figures: &mut Vec<Figure>,
    bands: &mut Vec<FigureBand>,
) {
    let draw = draw_style(found, style);
    let mut push = |kind: FigureKind| figures.push(Figure::new(kind, draw, 0.0));
    match found.shape {
        FoundShape::Horizontal { price } => {
            let Some(price) = positive(price) else {
                return;
            };
            horizontal(price, found, style.extend, &mut push);
            zone(price, found, style, bands);
        }
        FoundShape::Sloped {
            t0_ms,
            p0,
            t1_ms,
            p1,
        } => sloped(t0_ms, p0, t1_ms, p1, style.extend, &mut push),
        // One formation, one figure: the level and the line pressing into it, drawn in the same
        // style so the reader sees them as a pair.
        FoundShape::Squeeze {
            level,
            t0_ms,
            p0,
            t1_ms,
            p1,
            ..
        } => {
            let Some(level) = positive(level) else {
                return;
            };
            horizontal(level, found, style.extend, &mut push);
            squeeze_slope(
                (t0_ms, p0, t1_ms, p1),
                level,
                ref_ms,
                style.extend,
                &mut push,
            );
            zone(level, found, style, bands);
        }
        FoundShape::Pair { upper, lower } => {
            pair(&upper, &lower, found, style, ref_ms, push, bands)
        }
    }
}

/// The figures PINNING `found` copies into the figure store ("Pin as figure"): what the user then
/// owns, moves and arms as an alert.
///
/// A two-line formation pins as exactly TWO figures, one per side, each the figure that side is
/// DRAWN as ([`drawn_side`]): two levels for a pair drawn as levels (a double, a range), the capped
/// segment of a converging side, and a channel side's ray (`RightEdge`) or the segment between its
/// touches (`BetweenTouches`) — what the reader sees is what lands in the store. The one shape the
/// store cannot hold is a channel side drawn `BothEdges`, a line unbounded at both ends drawn as two
/// rays: it pins as its right-hand ray, the half through its touches and on into the future, so a
/// pair still pins as two figures, not four. Every other shape pins as the figures it draws; the
/// zone band is never pinned (it is a property of the auto line, not a figure).
///
/// Args:
///     found: The detector's figure.
///     style: Its row's resolved style.
///     ref_ms: The reference time the figure was drawn against.
///     figures: Receives the figures to pin, in draw order.
pub fn pin_figures(found: &Found, style: &ResolvedStyle, ref_ms: f64, figures: &mut Vec<Figure>) {
    if !matches!(found.shape, FoundShape::Pair { .. }) {
        let mut bands = Vec::new();
        append_shapes(found, style, ref_ms, figures, &mut bands);
        return;
    }
    let draw = draw_style(found, style);
    let mut push = |kind: FigureKind| figures.push(Figure::new(kind, draw, 0.0));
    for side in [PairSide::Upper, PairSide::Lower] {
        match drawn_side(found, side, style.extend, ref_ms) {
            Some(DrawnSide::Level { price, .. }) => {
                horizontal(price, found, style.extend, &mut push)
            }
            Some(DrawnSide::Free { seg, extend }) => {
                let a = FigNode::new(seg.t0_ms, seg.p0);
                let b = FigNode::new(seg.t1_ms, seg.p1);
                push(match extend {
                    Extend::BetweenTouches => FigureKind::Segment(Segment { a, b }),
                    Extend::RightEdge | Extend::BothEdges => FigureKind::Ray(Ray { a, b }),
                });
            }
            Some(DrawnSide::Capped { a, b }) => push(FigureKind::Segment(Segment { a, b })),
            None => {}
        }
    }
}

/// How ONE side of a two-line formation is drawn: the single decision [`append_shapes`] draws the
/// side by, [`pin_figures`] pins it by and the terminal places the side's caption by
/// ([`DrawnSide::span`]) — so a caption at the right end sits where the drawn line ends, not where
/// the line extrapolated past its apex would be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawnSide {
    /// A level at `price` over `span` (`±∞` = to that plot edge): a pair drawn as two levels.
    Level { price: f64, span: (f64, f64) },
    /// A channel side, drawn by `extend` alone through its two touches: a ray to the right edge, a
    /// ray each way, or the segment between the touches.
    Free { seg: Seg, extend: Extend },
    /// A converging side (a triangle, a wedge, a flag or pennant): the segment from its first touch
    /// `a` to `b`, the apex or the reference time, whichever comes first.
    Capped { a: FigNode, b: FigNode },
}

impl DrawnSide {
    /// The time span the side is drawn over, Unix ms, `±∞` meaning "to that plot edge".
    pub fn span(&self) -> (f64, f64) {
        match *self {
            DrawnSide::Level { span, .. } => span,
            DrawnSide::Free { seg, extend } => match extend {
                Extend::RightEdge => (seg.t0_ms, f64::INFINITY),
                Extend::BothEdges => (f64::NEG_INFINITY, f64::INFINITY),
                Extend::BetweenTouches => (seg.t0_ms, seg.t1_ms),
            },
            DrawnSide::Capped { a, b } => (a.time_ms, b.time_ms),
        }
    }
}

/// How `side` of `found` is drawn under `extend` against the reference time `ref_ms`; `None` for a
/// shape that is not a pair, and for a side whose geometry draws nothing (not finite, not positive,
/// or a converging side that would end where it starts).
///
/// - Both sides flat and the pattern one that IS two levels (a double, a range): a level over the
///   span `extend` gives the formation ([`time_span`]).
/// - A channel: the side's own line, run by `extend` — a channel runs on.
/// - Everything else (a triangle, a wedge, a flag or pennant): the segment from the side's first
///   touch to the apex where the two sides meet or `ref_ms`, whichever comes first — the
///   squeeze's rule, for its reason: past the apex the lines claim prices the pattern never did.
///   Never short of the side's last touch; `BetweenTouches` is the segment between its touches.
pub fn drawn_side(found: &Found, side: PairSide, extend: Extend, ref_ms: f64) -> Option<DrawnSide> {
    let FoundShape::Pair { upper, lower } = found.shape else {
        return None;
    };
    let s = match side {
        PairSide::Upper => upper,
        PairSide::Lower => lower,
    };
    if drawn_as_levels(found, &upper, &lower) {
        return positive(s.p0).map(|price| DrawnSide::Level {
            price,
            span: time_span(found, extend),
        });
    }
    if !(s.t0_ms.is_finite() && s.t1_ms.is_finite() && s.t1_ms > s.t0_ms) {
        return None;
    }
    let (p0, p1) = (positive(s.p0)?, positive(s.p1)?);
    if found.tag == Some(LineTag::Channel) {
        return Some(DrawnSide::Free {
            seg: Seg { p0, p1, ..s },
            extend,
        });
    }
    let end = capped_end(s.t0_ms, s.t1_ms, apex_ms(&upper, &lower), ref_ms, extend);
    capped_segment((s.t0_ms, p0, s.t1_ms, p1), end).map(|(a, b)| DrawnSide::Capped { a, b })
}

/// The line style a found figure is stamped with: the row's colour and thickness, and its broken
/// dash pattern once a close beyond has been seen.
fn draw_style(found: &Found, style: &ResolvedStyle) -> DrawStyle {
    let kind = if found.broken.is_some() {
        style.broken_kind
    } else {
        style.line_kind
    };
    DrawStyle {
        color: [
            style.rgb[0],
            style.rgb[1],
            style.rgb[2],
            (unit_alpha(style.opacity) * 255.0).round() as u8,
        ],
        thickness: style.thickness,
        kind,
        fill: [0; 4],
    }
}

/// Whether a pair is drawn as two levels and a box ([`pair`]): both sides flat, and the pattern is
/// one that IS two levels (a double, a range) — a flag or a triangle that happens to come out flat
/// is still a short pattern, capped like a sloped one.
fn drawn_as_levels(found: &Found, upper: &Seg, lower: &Seg) -> bool {
    let flat = |s: &Seg| s.p0 == s.p1;
    !matches!(
        found.tag,
        Some(LineTag::Triangle | LineTag::Wedge | LineTag::Flag)
    ) && flat(upper)
        && flat(lower)
}

/// A two-line formation: its two sides in the row's style, and for a horizontal pair the box
/// between them as its zone.
///
/// - Both sides horizontal (a double top or bottom, a range — the detector reports a range's sides
///   as exact levels), and not tagged a triangle, wedge or flag: each side is a level drawn like a
///   [`FoundShape::Horizontal`] over the span `extend` gives the formation, and the zone, when the
///   row draws one, is the box from the lower side to the upper over that span — the
///   "rectangle of the zone" the pattern is read as.
/// - A channel: each side like a [`FoundShape::Sloped`] line — a ray to the edge, its mirror, or
///   the segment between its own touches, as `extend` says. A channel runs on.
/// - Everything else (a triangle, a wedge, a flag or pennant): each side a segment from its first
///   touch to the apex where the sides meet or the reference time, whichever comes first — the
///   squeeze's rule, for its reason: past the apex the lines claim prices the pattern never did.
///   `BetweenTouches` keeps its meaning.
///
/// No zone for sloped sides: the zone layer draws axis-aligned boxes, and a box around a sloped
/// pair would shade prices outside it.
fn pair(
    upper: &Seg,
    lower: &Seg,
    found: &Found,
    style: &ResolvedStyle,
    ref_ms: f64,
    mut push: impl FnMut(FigureKind),
    bands: &mut Vec<FigureBand>,
) {
    let sides =
        [PairSide::Upper, PairSide::Lower].map(|s| drawn_side(found, s, style.extend, ref_ms));
    if drawn_as_levels(found, upper, lower) {
        let [
            Some(DrawnSide::Level { price: hi, .. }),
            Some(DrawnSide::Level { price: lo, .. }),
        ] = sides
        else {
            return;
        };
        horizontal(hi, found, style.extend, &mut push);
        horizontal(lo, found, style.extend, &mut push);
        zone_box(lo, hi, found, style, bands);
        return;
    }
    for side in sides.into_iter().flatten() {
        match side {
            DrawnSide::Free { seg, extend } => {
                sloped(seg.t0_ms, seg.p0, seg.t1_ms, seg.p1, extend, &mut push)
            }
            DrawnSide::Capped { a, b } => push(FigureKind::Segment(Segment { a, b })),
            // Only a pair drawn as levels has level sides, and it returned above.
            DrawnSide::Level { .. } => {}
        }
    }
}

/// The time where the two sides meet, when they converge (the upper side falling toward the lower,
/// relative to it); `NaN` for parallel or diverging sides, which `capped_slope` reads as "no apex".
fn apex_ms(upper: &Seg, lower: &Seg) -> f64 {
    let slope = |s: &Seg| (s.p1 - s.p0) / (s.t1_ms - s.t0_ms);
    let (su, sl) = (slope(upper), slope(lower));
    // upper(t) = p0u + su (t − t0u), lower(t) = p0l + sl (t − t0l); equal at the apex.
    let closing = sl - su;
    if !(closing > 0.0) {
        return f64::NAN;
    }
    (upper.p0 - su * upper.t0_ms - lower.p0 + sl * lower.t0_ms) / closing
}

/// The time span `extend` gives a figure's level, `±∞` meaning "to that plot edge". An unusable
/// anchor time widens to the edge rather than dropping the line.
///
/// Anchored to the figure's first and last touch ([`Found::first_ms`] / [`Found::last_ms`]) —
/// except a squeeze, whose level is anchored to its OWN touches (`level_t0_ms` / `level_t1_ms`):
/// the formation's first touch is often the sloped half's, and a level drawn from there claims to
/// have held where it did not. Its sloped half is drawn from its own touches as well, so each half
/// starts where it was found, as each side of a pair does.
///
/// Public because the terminal places a line's caption on the stretch that is DRAWN: a caption
/// at the left end of a level must sit where the level starts, not where the plot does.
pub fn time_span(found: &Found, extend: Extend) -> (f64, f64) {
    let (first_ms, last_ms) = match found.shape {
        FoundShape::Squeeze {
            level_t0_ms,
            level_t1_ms,
            ..
        } => (level_t0_ms, level_t1_ms),
        _ => (found.first_ms, found.last_ms),
    };
    let first = if first_ms.is_finite() && first_ms > 0.0 {
        first_ms
    } else {
        f64::NEG_INFINITY
    };
    match extend {
        Extend::RightEdge => (first, f64::INFINITY),
        Extend::BothEdges => (f64::NEG_INFINITY, f64::INFINITY),
        Extend::BetweenTouches if last_ms.is_finite() && last_ms > first => (first, last_ms),
        // A single touch has no stretch "between" — the level still exists, so it runs right.
        Extend::BetweenTouches => (first, f64::INFINITY),
    }
}

/// A level at `price` over the span `extend` gives it, as the figure tool that draws that span.
fn horizontal(price: f64, found: &Found, extend: Extend, push: &mut impl FnMut(FigureKind)) {
    let (t0, t1) = time_span(found, extend);
    let kind = match (t0.is_finite(), t1.is_finite()) {
        (true, false) => FigureKind::HorizontalRay(HorizontalRay {
            origin: FigNode::new(t0, price),
        }),
        (true, true) => FigureKind::Segment(Segment {
            a: FigNode::new(t0, price),
            b: FigNode::new(t1, price),
        }),
        // Unbounded on the left: the whole level. (A bounded right end with no left one is not a
        // tool's shape either, so it widens the same way.)
        (false, _) => FigureKind::HLine(HLine { price }),
    };
    push(kind);
}

/// A sloped line through `(t0, p0)` and `(t1, p1)`.
///
/// `RightEdge` is a ray from the first touch through the last; `BothEdges` adds a second ray from
/// the same origin pointing the other way, so the two halves meet without overlapping (an overlap
/// would double the idle alpha and break a dash pattern); `BetweenTouches` is the segment itself.
fn sloped(t0: f64, p0: f64, t1: f64, p1: f64, extend: Extend, push: &mut impl FnMut(FigureKind)) {
    let usable = t0.is_finite() && t1.is_finite() && t1 > t0;
    let (Some(p0), Some(p1)) = (positive(p0), positive(p1)) else {
        return;
    };
    if !usable {
        return;
    }
    let a = FigNode::new(t0, p0);
    let b = FigNode::new(t1, p1);
    match extend {
        Extend::RightEdge => push(FigureKind::Ray(Ray { a, b })),
        Extend::BothEdges => {
            push(FigureKind::Ray(Ray { a, b }));
            // Mirrored through the origin: the same slope, aimed into the past. Its price may go
            // non-positive far enough back, which is harmless — the shader only extrapolates.
            push(FigureKind::Ray(Ray {
                a,
                b: FigNode::new(t0 - (t1 - t0), p0 - (p1 - p0)),
            }));
        }
        Extend::BetweenTouches => push(FigureKind::Segment(Segment { a, b })),
    }
}

/// A squeeze's sloped half: a segment from its first touch to where it stops pressing — the apex,
/// where it meets the level, or the reference time `ref_ms`, whichever comes first.
///
/// Never a ray. A ray runs on past the apex to the plot's edge and, on a long chart, far through
/// prices the formation never claimed — that is the line a reader then takes for a trend that
/// price has "broken". `BetweenTouches` keeps its meaning: the segment between the two touches.
fn squeeze_slope(
    (t0, p0, t1, p1): (f64, f64, f64, f64),
    level: f64,
    ref_ms: f64,
    extend: Extend,
    push: &mut impl FnMut(FigureKind),
) {
    // Where the line meets the level; NaN (no apex) for a flat line, which `capped_slope` ignores.
    let apex = t0 + (level - p0) / (p1 - p0) * (t1 - t0);
    capped_slope((t0, p0, t1, p1), apex, ref_ms, extend, push);
}

/// A sloped line drawn as a segment from its first touch to `apex` or the reference time `ref_ms`,
/// whichever comes first ([`capped_end`]). A squeeze's sloped half; a converging formation's sides
/// take the same two helpers through [`drawn_side`].
fn capped_slope(
    (t0, p0, t1, p1): (f64, f64, f64, f64),
    apex: f64,
    ref_ms: f64,
    extend: Extend,
    push: &mut impl FnMut(FigureKind),
) {
    if !(t0.is_finite() && t1.is_finite() && t1 > t0) {
        return;
    }
    let (Some(p0), Some(p1)) = (positive(p0), positive(p1)) else {
        return;
    };
    let end = capped_end(t0, t1, apex, ref_ms, extend);
    if let Some((a, b)) = capped_segment((t0, p0, t1, p1), end) {
        push(FigureKind::Segment(Segment { a, b }));
    }
}

/// Where a capped sloped line through its first touch `t0` and last touch `t1` ends: at `apex` or
/// the reference time `ref_ms`, whichever comes first — never a ray, and never cut short of its
/// last touch by a stale reference. An `apex` that is not a finite time after the first touch is
/// no apex. `BetweenTouches` ends at the last touch.
fn capped_end(t0: f64, t1: f64, apex: f64, ref_ms: f64, extend: Extend) -> f64 {
    match extend {
        Extend::BetweenTouches => t1,
        Extend::RightEdge | Extend::BothEdges => {
            // Never short of the last touch: a stale reference must not cut off what was found.
            let now = if ref_ms.is_finite() {
                ref_ms.max(t1)
            } else {
                t1
            };
            if apex.is_finite() && apex > t0 {
                now.min(apex)
            } else {
                now
            }
        }
    }
}

/// The segment from the first touch `(t0, p0)` to the line's point at `end`; `None` when `end` is
/// not after `t0` or the line has no positive price there.
fn capped_segment((t0, p0, t1, p1): (f64, f64, f64, f64), end: f64) -> Option<(FigNode, FigNode)> {
    let p_end = super::line_price_at(t0, p0, t1, p1, end).filter(|_| end > t0)?;
    Some((FigNode::new(t0, p0), FigNode::new(end, p_end)))
}

/// The tolerance band around a level, when the row draws one.
///
/// It spans the level's own touches — those within the found tolerance (`Found::tolerance_frac`) of
/// it, which is every pivot the detector grouped into the level — and at least that tolerance of
/// the level centred on it: what "the level" meant to the detector. A squeeze's sloped-line pivots
/// lie outside that tolerance by construction and never stretch the band toward the converging
/// line. A tolerance of `0` (a detector without one) draws the band from the touches' spread
/// alone. Same time span as the line.
fn zone(price: f64, found: &Found, style: &ResolvedStyle, bands: &mut Vec<FigureBand>) {
    let Some(alpha) = style.zone_alpha.filter(|a| a.is_finite() && *a > 0.0) else {
        return;
    };
    // Not a positive finite fraction: no band beyond the touches, never a widened or NaN one.
    let frac = if found.tolerance_frac.is_finite() && found.tolerance_frac > 0.0 {
        found.tolerance_frac
    } else {
        0.0
    };
    let half = price * frac / 2.0;
    let (mut lo, mut hi) = (price - half, price + half);
    for t in &found.touches {
        if let Some(p) = positive(t.price).filter(|&p| within(p, price, frac)) {
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    let (t0_ms, t1_ms) = time_span(found, style.extend);
    bands.push(FigureBand {
        t0_ms,
        t1_ms,
        p0: lo,
        p1: hi,
        color: band_color(style, alpha),
    });
}

/// The box from `lo` to `hi` over the formation's drawn span, when the row draws a zone — a
/// horizontal formation's zone. Same colour and opacity rule as a level's band.
fn zone_box(lo: f64, hi: f64, found: &Found, style: &ResolvedStyle, bands: &mut Vec<FigureBand>) {
    let Some(alpha) = style.zone_alpha.filter(|a| a.is_finite() && *a > 0.0) else {
        return;
    };
    if !(lo < hi) {
        return;
    }
    let (t0_ms, t1_ms) = time_span(found, style.extend);
    bands.push(FigureBand {
        t0_ms,
        t1_ms,
        p0: lo,
        p1: hi,
        color: band_color(style, alpha),
    });
}

/// The row's colour at `alpha`, as the zone layer's `0..=1` RGBA.
fn band_color(style: &ResolvedStyle, alpha: f32) -> [f32; 4] {
    let c = |v: u8| f32::from(v) / 255.0;
    [
        c(style.rgb[0]),
        c(style.rgb[1]),
        c(style.rgb[2]),
        alpha.min(1.0) * unit_alpha(style.opacity),
    ]
}

#[cfg(test)]
mod tests;
