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

use super::{Found, FoundShape, pct_frac, positive};
use crate::figures::FigureBand;

/// A row's line style with its colour already resolved, ready to stamp onto figures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedStyle {
    /// Line colour, `[r, g, b]`, drawn opaque (the figure layer applies its own idle alpha).
    pub rgb: [u8; 3],
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
            thickness: style.thickness,
            line_kind: style.line_kind,
            broken_kind: style.broken_kind,
            extend: style.extend,
            zone_alpha: style.zone,
        }
    }
}

/// Append the figures and bands one found figure draws as.
///
/// Args:
///     found: The detector's figure.
///     style: Its row's resolved style.
///     tolerance_pct: The row's touch tolerance, in % of price — the band a zone shows. `0` draws
///         the band from the touches' spread alone.
///     figures: Receives the lines, in draw order.
///     bands: Receives the zone bands.
///
/// A figure whose geometry is not finite and positive draws nothing: these come from market data,
/// and a NaN reaching the instance buffers would poison the whole draw call, not just this line.
pub fn append_shapes(
    found: &Found,
    style: &ResolvedStyle,
    tolerance_pct: f32,
    figures: &mut Vec<Figure>,
    bands: &mut Vec<FigureBand>,
) {
    let kind = if found.broken.is_some() {
        style.broken_kind
    } else {
        style.line_kind
    };
    let draw = DrawStyle {
        color: [style.rgb[0], style.rgb[1], style.rgb[2], u8::MAX],
        thickness: style.thickness,
        kind,
        fill: [0; 4],
    };
    let mut push = |kind: FigureKind| figures.push(Figure::new(kind, draw, 0.0));
    match found.shape {
        FoundShape::Horizontal { price } => {
            let Some(price) = positive(price) else {
                return;
            };
            horizontal(price, found, style.extend, &mut push);
            zone(price, found, style, tolerance_pct, bands);
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
            sloped(t0_ms, p0, t1_ms, p1, style.extend, &mut push);
            zone(level, found, style, tolerance_pct, bands);
        }
    }
}

/// The time span `extend` gives a figure anchored to its first and last touch, `±∞` meaning "to
/// that plot edge". An unusable anchor time widens to the edge rather than dropping the line.
fn span(found: &Found, extend: Extend) -> (f64, f64) {
    let first = if found.first_ms.is_finite() && found.first_ms > 0.0 {
        found.first_ms
    } else {
        f64::NEG_INFINITY
    };
    match extend {
        Extend::RightEdge => (first, f64::INFINITY),
        Extend::BothEdges => (f64::NEG_INFINITY, f64::INFINITY),
        Extend::BetweenTouches if found.last_ms.is_finite() && found.last_ms > first => {
            (first, found.last_ms)
        }
        // A single touch has no stretch "between" — the level still exists, so it runs right.
        Extend::BetweenTouches => (first, f64::INFINITY),
    }
}

/// A level at `price` over the span `extend` gives it, as the figure tool that draws that span.
fn horizontal(price: f64, found: &Found, extend: Extend, push: &mut impl FnMut(FigureKind)) {
    let (t0, t1) = span(found, extend);
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

/// The tolerance band around a level, when the row draws one.
///
/// It spans every touch, and at least `tolerance_pct` of the level centred on it: what "the level"
/// meant to the detector, since a pivot anywhere in it counted. Same time span as the line.
fn zone(
    price: f64,
    found: &Found,
    style: &ResolvedStyle,
    tolerance_pct: f32,
    bands: &mut Vec<FigureBand>,
) {
    let Some(alpha) = style.zone_alpha.filter(|a| a.is_finite() && *a > 0.0) else {
        return;
    };
    let half = price * pct_frac(tolerance_pct) / 2.0;
    let (mut lo, mut hi) = (price - half, price + half);
    for t in &found.touches {
        if let Some(p) = positive(t.price) {
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    let (t0_ms, t1_ms) = span(found, style.extend);
    let c = |v: u8| f32::from(v) / 255.0;
    bands.push(FigureBand {
        t0_ms,
        t1_ms,
        p0: lo,
        p1: hi,
        color: [
            c(style.rgb[0]),
            c(style.rgb[1]),
            c(style.rgb[2]),
            alpha.min(1.0),
        ],
    });
}

#[cfg(test)]
mod tests;
