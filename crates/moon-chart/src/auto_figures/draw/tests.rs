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
    let (mut figures, mut bands) = (Vec::new(), Vec::new());
    append_shapes(found, style, tol, &mut figures, &mut bands);
    (figures, bands)
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
    // Touches at 99.9 and 100.2; a 0.1 % tolerance is ±0.05 around 100, inside the touches.
    let (_, bands) = draw(&level(100.0, 1_000.0, 5_000.0), &s, 0.1);
    assert_eq!(bands.len(), 1);
    let b = bands[0];
    assert!((b.p0 - 99.9).abs() < 1e-9 && (b.p1 - 100.2).abs() < 1e-9);
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
