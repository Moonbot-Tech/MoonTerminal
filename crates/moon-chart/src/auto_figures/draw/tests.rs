use super::*;
use crate::auto_figures::{Broken, Touch};

/// A colour no default style carries, so a test can tell "stamped from the resolved style" from
/// "left at some default".
const RGB: [u8; 3] = [10, 20, 30];

fn style(extend: Extend) -> ResolvedStyle {
    ResolvedStyle {
        rgb: RGB,
        thickness: 2.5,
        line_kind: LineKind::Solid,
        broken_kind: LineKind::Dot,
        extend,
        zone_alpha: None,
    }
}

fn level(price: f64, first_ms: f64, last_ms: f64) -> Found {
    Found {
        shape: FoundShape::Horizontal { price },
        touches: vec![
            Touch {
                t_ms: first_ms,
                price: price * 0.999,
            },
            Touch {
                t_ms: last_ms,
                price: price * 1.002,
            },
        ],
        strength: 2.0,
        first_ms,
        last_ms,
        broken: None,
    }
}

fn draw(found: &Found, style: &ResolvedStyle, tol: f32) -> (Vec<Figure>, Vec<FigureBand>) {
    draw_at(found, style, tol, 10_000.0)
}

fn draw_at(
    found: &Found,
    style: &ResolvedStyle,
    tol: f32,
    ref_ms: f64,
) -> (Vec<Figure>, Vec<FigureBand>) {
    let (mut figures, mut bands) = (Vec::new(), Vec::new());
    append_shapes(found, style, tol, ref_ms, &mut figures, &mut bands);
    (figures, bands)
}

/// A support at 100 touched at 0.5 s and 1 s, with highs falling 110 → 106 over 1..3 s
/// (−2 per second): the line meets the level at 6 s.
fn falling_squeeze() -> Found {
    Found {
        shape: FoundShape::Squeeze {
            level: 100.0,
            t0_ms: 1_000.0,
            p0: 110.0,
            t1_ms: 3_000.0,
            p1: 106.0,
            pressed: false,
        },
        touches: vec![
            Touch {
                t_ms: 500.0,
                price: 100.1,
            },
            Touch {
                t_ms: 1_000.0,
                price: 110.0,
            },
            Touch {
                t_ms: 2_000.0,
                price: 99.9,
            },
            Touch {
                t_ms: 3_000.0,
                price: 106.0,
            },
        ],
        strength: 4.0,
        first_ms: 500.0,
        last_ms: 3_000.0,
        broken: None,
    }
}

/// The sloped half of a squeeze stops at the apex when the apex has come, at the reference time
/// before that — never a ray to the plot's edge.
///
/// Breakage: drawing the sloped half as a ray (the live defect: a line falling through the whole
/// chart, far below the level it was pressing), or ending it at the last touch.
#[test]
fn a_squeeze_slope_ends_at_the_apex_or_now() {
    let found = falling_squeeze();
    // Reference 4 s: before the apex (6 s), so the segment ends at 4 s, at 110 - 2·3 = 104.
    let (figs, _) = draw_at(&found, &style(Extend::RightEdge), 0.5, 4_000.0);
    assert_eq!(figs.len(), 2);
    assert_eq!(
        figs[1].kind,
        FigureKind::Segment(Segment {
            a: FigNode::new(1_000.0, 110.0),
            b: FigNode::new(4_000.0, 104.0),
        })
    );
    // Reference 9 s: past the apex, so it ends ON the level at 6 s.
    let (figs, _) = draw_at(&found, &style(Extend::BothEdges), 0.5, 9_000.0);
    assert_eq!(
        figs[1].kind,
        FigureKind::Segment(Segment {
            a: FigNode::new(1_000.0, 110.0),
            b: FigNode::new(6_000.0, 100.0),
        })
    );
    assert!(figs.iter().all(|f| !matches!(f.kind, FigureKind::Ray(_))));
}

/// A squeeze's zone covers its LEVEL only: its two level touches (100.1 and 99.9) are inside a
/// 0.5 % tolerance, its sloped touches (110, 106) are not and must not stretch the band.
///
/// Breakage: spanning every touch of the formation, which drags the band up to the converging
/// line.
#[test]
fn a_squeeze_zone_ignores_the_sloped_touches() {
    let mut s = style(Extend::RightEdge);
    s.zone_alpha = Some(0.3);
    let (_, bands) = draw_at(&falling_squeeze(), &s, 0.5, 4_000.0);
    assert_eq!(bands.len(), 1);
    // ±0.25 around 100 (half the tolerance) already contains 99.9 and 100.1.
    assert!((bands[0].p0 - 99.75).abs() < 1e-9, "{:?}", bands[0]);
    assert!((bands[0].p1 - 100.25).abs() < 1e-9, "{:?}", bands[0]);
}

#[test]
fn a_level_extends_to_the_right_edge_from_its_first_touch() {
    let (figs, bands) = draw(
        &level(100.0, 1_000.0, 5_000.0),
        &style(Extend::RightEdge),
        0.25,
    );
    assert_eq!(figs.len(), 1);
    assert_eq!(
        figs[0].kind,
        FigureKind::HorizontalRay(HorizontalRay {
            origin: FigNode::new(1_000.0, 100.0)
        })
    );
    assert_eq!(figs[0].color, [10, 20, 30, 255]);
    assert_eq!(figs[0].thickness, 2.5);
    assert_eq!(figs[0].line_kind, LineKind::Solid);
    assert!(bands.is_empty(), "no zone was asked for");
}

#[test]
fn both_edges_is_a_full_width_line_and_between_touches_a_segment() {
    let found = level(100.0, 1_000.0, 5_000.0);
    let (both, _) = draw(&found, &style(Extend::BothEdges), 0.25);
    assert_eq!(both[0].kind, FigureKind::HLine(HLine { price: 100.0 }));
    let (between, _) = draw(&found, &style(Extend::BetweenTouches), 0.25);
    assert_eq!(
        between[0].kind,
        FigureKind::Segment(Segment {
            a: FigNode::new(1_000.0, 100.0),
            b: FigNode::new(5_000.0, 100.0),
        })
    );
}

#[test]
fn a_kept_broken_level_takes_the_broken_pattern() {
    let mut found = level(100.0, 1_000.0, 5_000.0);
    found.broken = Some(Broken {
        at_ms: 6_000.0,
        bars_since: 1,
        returned: false,
    });
    let (figs, _) = draw(&found, &style(Extend::RightEdge), 0.25);
    assert_eq!(figs[0].line_kind, LineKind::Dot);
}

#[test]
fn a_zone_spans_the_touches_and_at_least_the_tolerance() {
    let mut s = style(Extend::RightEdge);
    s.zone_alpha = Some(0.2);
    // Touches at 99.9 and 100.2, both within a 0.3 % tolerance of 100 (a swing level's touches
    // always are). The band is ±0.15 around 100, widened up to the 100.2 touch.
    let (_, bands) = draw(&level(100.0, 1_000.0, 5_000.0), &s, 0.3);
    assert_eq!(bands.len(), 1);
    let b = bands[0];
    // The tolerance is an `f32` parameter: 0.3 % carries its float error of about 1e-8 of price.
    assert!(
        (b.p0 - 99.85).abs() < 1e-6 && (b.p1 - 100.2).abs() < 1e-9,
        "{b:?}"
    );
    assert_eq!((b.t0_ms, b.t1_ms), (1_000.0, f64::INFINITY));
    assert_eq!(b.color[3], 0.2);
    // A 2 % tolerance (±1) is wider than the touches and wins.
    let (_, wide) = draw(&level(100.0, 1_000.0, 5_000.0), &s, 2.0);
    assert!((wide[0].p0 - 99.0).abs() < 1e-9 && (wide[0].p1 - 101.0).abs() < 1e-9);
}

#[test]
fn a_sloped_line_is_a_ray_and_both_edges_adds_the_mirror_without_overlap() {
    let found = Found {
        shape: FoundShape::Sloped {
            t0_ms: 1_000.0,
            p0: 100.0,
            t1_ms: 3_000.0,
            p1: 110.0,
        },
        touches: Vec::new(),
        strength: 3.0,
        first_ms: 1_000.0,
        last_ms: 3_000.0,
        broken: None,
    };
    let a = FigNode::new(1_000.0, 100.0);
    let (right, _) = draw(&found, &style(Extend::RightEdge), 0.0);
    assert_eq!(
        right[0].kind,
        FigureKind::Ray(Ray {
            a,
            b: FigNode::new(3_000.0, 110.0)
        })
    );
    let (both, _) = draw(&found, &style(Extend::BothEdges), 0.0);
    assert_eq!(both.len(), 2);
    // The second ray starts at the SAME origin and points back along the same slope.
    assert_eq!(
        both[1].kind,
        FigureKind::Ray(Ray {
            a,
            b: FigNode::new(-1_000.0, 90.0)
        })
    );
}

#[test]
fn a_squeeze_draws_its_level_and_its_sloped_line() {
    let found = Found {
        shape: FoundShape::Squeeze {
            level: 120.0,
            t0_ms: 1_000.0,
            p0: 100.0,
            t1_ms: 3_000.0,
            p1: 110.0,
            pressed: true,
        },
        touches: Vec::new(),
        strength: 3.0,
        first_ms: 500.0,
        last_ms: 3_000.0,
        broken: None,
    };
    let (figs, _) = draw(&found, &style(Extend::BetweenTouches), 0.0);
    assert_eq!(figs.len(), 2);
    assert!(matches!(figs[0].kind, FigureKind::Segment(s) if s.a.price == 120.0));
    assert!(matches!(figs[1].kind, FigureKind::Segment(s) if s.a.price == 100.0));
}

#[test]
fn unusable_geometry_draws_nothing() {
    let (figs, bands) = draw(
        &level(f64::NAN, 1_000.0, 5_000.0),
        &style(Extend::RightEdge),
        0.25,
    );
    assert!(figs.is_empty() && bands.is_empty());
    let degenerate = Found {
        shape: FoundShape::Sloped {
            t0_ms: 3_000.0,
            p0: 100.0,
            t1_ms: 3_000.0,
            p1: 110.0,
        },
        touches: Vec::new(),
        strength: 0.0,
        first_ms: 3_000.0,
        last_ms: 3_000.0,
        broken: None,
    };
    assert!(
        draw(&degenerate, &style(Extend::RightEdge), 0.0)
            .0
            .is_empty()
    );
}
