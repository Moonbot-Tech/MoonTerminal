use super::*;
use crate::auto_figures::{Broken, Touch};

/// A colour no default style carries, so a test can tell "stamped from the resolved style" from
/// "left at some default".
const RGB: [u8; 3] = [10, 20, 30];

fn style(extend: Extend) -> ResolvedStyle {
    ResolvedStyle {
        rgb: RGB,
        opacity: 1.0,
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
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: None,
        origin: None,
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
    // The band is drawn from the tolerance the detector reports, as a fraction of price; these
    // tests state it in % of price, the unit the rows are written in.
    let mut found = found.clone();
    found.tolerance_frac = f64::from(tol) / 100.0;
    let (mut figures, mut bands) = (Vec::new(), Vec::new());
    append_shapes(&found, style, ref_ms, &mut figures, &mut bands);
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
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: None,
        origin: None,
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
        longest_bars: 1,
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
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: None,
        origin: None,
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
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: None,
        origin: None,
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
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: None,
        origin: None,
    };
    assert!(
        draw(&degenerate, &style(Extend::RightEdge), 0.0)
            .0
            .is_empty()
    );
}

/// A two-line formation of `tag` with the sides `upper` and `lower`, `(t0, p0, t1, p1)` each.
fn formation(tag: LineTag, upper: (f64, f64, f64, f64), lower: (f64, f64, f64, f64)) -> Found {
    let seg = |(t0_ms, p0, t1_ms, p1)| Seg {
        t0_ms,
        p0,
        t1_ms,
        p1,
    };
    Found {
        shape: FoundShape::Pair {
            upper: seg(upper),
            lower: seg(lower),
        },
        tag: Some(tag),
        first_ms: upper.0.min(lower.0),
        last_ms: upper.2.max(lower.2),
        ..level(100.0, 0.0, 0.0)
    }
}

/// A triangle's sides stop where they meet, or at the reference time before that — never rays.
///
/// By hand: the top falls 110 → 106 over 1..3 s (−2/s), the bottom rises 90 → 92 over 2..4 s
/// (+1/s): they meet where 110 − 2(t − 1) = 90 + (t − 2), at t = 8 s, price 96. At a reference of
/// 5 s both stop at 5 s (102 and 93); at 20 s both stop at the apex.
///
/// Breakage: a converging side drawn as a ray past the apex, or the apex solved with the wrong
/// signs.
#[test]
fn a_triangle_stops_at_its_apex_or_now() {
    let f = formation(
        LineTag::Triangle,
        (1_000.0, 110.0, 3_000.0, 106.0),
        (2_000.0, 90.0, 4_000.0, 92.0),
    );
    let (figs, bands) = draw_at(&f, &style(Extend::RightEdge), 0.5, 5_000.0);
    assert!(bands.is_empty(), "no zone for sloped sides");
    let segs: Vec<_> = figs.iter().map(|f| f.kind.clone()).collect();
    assert_eq!(
        segs,
        [
            FigureKind::Segment(Segment {
                a: FigNode::new(1_000.0, 110.0),
                b: FigNode::new(5_000.0, 102.0),
            }),
            FigureKind::Segment(Segment {
                a: FigNode::new(2_000.0, 90.0),
                b: FigNode::new(5_000.0, 93.0),
            }),
        ]
    );
    let (figs, _) = draw_at(&f, &style(Extend::RightEdge), 0.5, 20_000.0);
    let ends: Vec<_> = figs
        .iter()
        .map(|f| match f.kind {
            FigureKind::Segment(s) => (s.b.time_ms, s.b.price),
            ref k => panic!("{k:?}"),
        })
        .collect();
    assert_eq!(ends.len(), 2);
    for (t, p) in ends {
        assert!(
            (t - 8_000.0).abs() < 1e-6 && (p - 96.0).abs() < 1e-9,
            "{t} {p}"
        );
    }
}

/// A sloped channel runs on as two rays; a horizontal pair (a double top) is two levels from the
/// formation's first touch, with its zone the box between them.
#[test]
fn a_channel_runs_on_and_a_double_is_a_boxed_pair_of_levels() {
    let channel = formation(
        LineTag::Channel,
        (1_000.0, 110.0, 3_000.0, 108.0),
        (2_000.0, 99.0, 4_000.0, 97.0),
    );
    let (figs, _) = draw(&channel, &style(Extend::RightEdge), 0.5);
    assert_eq!(figs.len(), 2);
    assert!(figs.iter().all(|f| matches!(f.kind, FigureKind::Ray(_))));

    let double = formation(
        LineTag::DoubleTop,
        (1_000.0, 110.0, 3_000.0, 110.0),
        (1_000.0, 104.0, 3_000.0, 104.0),
    );
    let mut boxed = style(Extend::RightEdge);
    boxed.zone_alpha = Some(0.2);
    let (figs, bands) = draw(&double, &boxed, 0.5);
    let kinds: Vec<_> = figs.iter().map(|f| f.kind.clone()).collect();
    assert_eq!(
        kinds,
        [
            FigureKind::HorizontalRay(HorizontalRay {
                origin: FigNode::new(1_000.0, 110.0),
            }),
            FigureKind::HorizontalRay(HorizontalRay {
                origin: FigNode::new(1_000.0, 104.0),
            }),
        ]
    );
    assert_eq!(bands.len(), 1);
    let b = bands[0];
    assert_eq!(
        (b.p0, b.p1, b.t0_ms, b.t1_ms),
        (104.0, 110.0, 1_000.0, f64::INFINITY)
    );
    assert!((b.color[3] - 0.2).abs() < 1e-6);
}

/// A flag that comes out exactly flat is still a short pattern: its sides stop at the reference
/// time, never run to the edge as levels, and draw no box.
///
/// Breakage: the level-and-box branch taken on float equality before the tag is read.
#[test]
fn a_flat_flag_is_capped_not_a_pair_of_levels() {
    let flag = formation(
        LineTag::Flag,
        (1_000.0, 110.0, 3_000.0, 110.0),
        (1_500.0, 108.0, 3_500.0, 108.0),
    );
    let mut boxed = style(Extend::RightEdge);
    boxed.zone_alpha = Some(0.2);
    let (figs, bands) = draw_at(&flag, &boxed, 0.5, 5_000.0);
    assert!(bands.is_empty());
    assert_eq!(figs.len(), 2);
    for f in &figs {
        let FigureKind::Segment(s) = f.kind else {
            panic!("{:?}", f.kind);
        };
        assert_eq!(s.b.time_ms, 5_000.0);
    }
}

/// Pinning a formation gives exactly two figures, one per side, each the figure the side is drawn
/// as: a channel drawn `RightEdge` pins as its two rays, drawn `BetweenTouches` as its two
/// segments, and drawn `BothEdges` (four rays) as the right-hand ray of each side; a double top
/// pins as its two levels; a triangle as the two capped segments it is drawn as. A level pins as
/// what it draws, without its zone.
///
/// Breakage: a channel side pinned as a segment to the reference time while the screen shows a
/// ray (the pinned copy stops where the drawn line runs on — review finding F7), pinning the drawn
/// figures of a pair (four rays for a `BothEdges` channel), or one side pinned twice.
#[test]
fn a_formation_pins_as_one_figure_per_side() {
    let pins = |f: &Found, extend: Extend| {
        let mut out = Vec::new();
        pin_figures(f, &style(extend), 5_000.0, &mut out);
        out.into_iter().map(|f| f.kind).collect::<Vec<_>>()
    };
    let drawn = |f: &Found, extend: Extend| {
        draw_at(f, &style(extend), 0.5, 5_000.0)
            .0
            .into_iter()
            .map(|f| f.kind)
            .collect::<Vec<_>>()
    };
    let channel = formation(
        LineTag::Channel,
        (1_000.0, 110.0, 3_000.0, 108.0),
        (2_000.0, 99.0, 4_000.0, 97.0),
    );
    let rays = [
        FigureKind::Ray(Ray {
            a: FigNode::new(1_000.0, 110.0),
            b: FigNode::new(3_000.0, 108.0),
        }),
        FigureKind::Ray(Ray {
            a: FigNode::new(2_000.0, 99.0),
            b: FigNode::new(4_000.0, 97.0),
        }),
    ];
    assert_eq!(drawn(&channel, Extend::RightEdge), rays);
    assert_eq!(pins(&channel, Extend::RightEdge), rays);
    assert_eq!(
        pins(&channel, Extend::BetweenTouches),
        drawn(&channel, Extend::BetweenTouches)
    );
    assert_eq!(drawn(&channel, Extend::BothEdges).len(), 4);
    assert_eq!(pins(&channel, Extend::BothEdges), rays);

    let double = formation(
        LineTag::DoubleTop,
        (1_000.0, 110.0, 3_000.0, 110.0),
        (1_000.0, 104.0, 3_000.0, 104.0),
    );
    assert_eq!(
        pins(&double, Extend::BothEdges),
        [
            FigureKind::HLine(HLine { price: 110.0 }),
            FigureKind::HLine(HLine { price: 104.0 }),
        ]
    );

    let triangle = formation(
        LineTag::Triangle,
        (1_000.0, 110.0, 3_000.0, 106.0),
        (2_000.0, 90.0, 4_000.0, 92.0),
    );
    assert_eq!(
        pins(&triangle, Extend::RightEdge),
        drawn(&triangle, Extend::RightEdge)
    );

    let lvl = level(100.0, 1_000.0, 2_000.0);
    assert_eq!(pins(&lvl, Extend::RightEdge).len(), 1);
}

/// Where each side of a formation is drawn — the span its caption is placed on.
///
/// By hand (the triangle of `a_triangle_stops_at_its_apex_or_now`): the sides meet at 8 s. At a
/// reference of 20 s both sides end at the apex, so each spans its first touch to 8 s — not the
/// right edge; at 5 s both end at 5 s. A channel side drawn `RightEdge` runs from its first touch
/// to the edge, `BetweenTouches` between its touches. A double's sides span the formation. Not a
/// pair: no drawn side.
///
/// Breakage: a caption of a capped side placed by the row's `extend` alone (`RightEdge` → the plot
/// edge, past the apex, at an extrapolated price — review finding F1), or the drawn span and the
/// drawn segment computed by two rules that drift apart.
#[test]
fn a_formation_side_is_drawn_over_the_span_its_caption_uses() {
    let triangle = formation(
        LineTag::Triangle,
        (1_000.0, 110.0, 3_000.0, 106.0),
        (2_000.0, 90.0, 4_000.0, 92.0),
    );
    for (side, t0) in [(PairSide::Upper, 1_000.0), (PairSide::Lower, 2_000.0)] {
        let (a, b) = drawn_side(&triangle, side, Extend::RightEdge, 20_000.0)
            .unwrap()
            .span();
        assert_eq!(a, t0);
        assert!((b - 8_000.0).abs() < 1e-6, "{b}");
        assert_eq!(
            drawn_side(&triangle, side, Extend::RightEdge, 5_000.0)
                .unwrap()
                .span(),
            (t0, 5_000.0)
        );
    }
    // The span and the segment drawn come from the one decision.
    let (figs, _) = draw_at(&triangle, &style(Extend::RightEdge), 0.5, 20_000.0);
    let FigureKind::Segment(s) = figs[1].kind else {
        panic!("{:?}", figs[1].kind);
    };
    assert_eq!(
        drawn_side(&triangle, PairSide::Lower, Extend::RightEdge, 20_000.0)
            .unwrap()
            .span(),
        (s.a.time_ms, s.b.time_ms)
    );

    let channel = formation(
        LineTag::Channel,
        (1_000.0, 110.0, 3_000.0, 108.0),
        (2_000.0, 99.0, 4_000.0, 97.0),
    );
    let span = |extend| {
        drawn_side(&channel, PairSide::Lower, extend, 5_000.0)
            .unwrap()
            .span()
    };
    assert_eq!(span(Extend::RightEdge), (2_000.0, f64::INFINITY));
    assert_eq!(span(Extend::BetweenTouches), (2_000.0, 4_000.0));

    let double = formation(
        LineTag::DoubleTop,
        (1_000.0, 110.0, 3_000.0, 110.0),
        (1_500.0, 104.0, 3_000.0, 104.0),
    );
    assert_eq!(
        drawn_side(&double, PairSide::Lower, Extend::RightEdge, 5_000.0),
        Some(DrawnSide::Level {
            price: 104.0,
            span: (1_000.0, f64::INFINITY),
        })
    );
    assert_eq!(
        drawn_side(
            &level(100.0, 0.0, 1.0),
            PairSide::Upper,
            Extend::RightEdge,
            5.0
        ),
        None
    );
}

/// The block's opacity scales the alpha of the line and of its zone band, and nothing else; a NaN
/// opacity draws opaque and an out-of-range one is clamped, so a broken setting never poisons a
/// colour.
///
/// By hand: at half opacity the line's alpha is 255 · 0.5 = 127.5 → 128, and a zone of 0.4 is
/// drawn at 0.2; the colour itself stays the row's.
///
/// Breakage: the factor applied to the line only (zones stay loud) or to the RGB instead of the
/// alpha, or `with_opacity` letting a NaN through.
#[test]
fn the_block_opacity_scales_line_and_zone_alpha() {
    let mut s = style(Extend::RightEdge);
    s.zone_alpha = Some(0.4);
    let found = level(100.0, 1_000.0, 2_000.0);
    let (figs, bands) = draw(&found, &s.with_opacity(0.5), 0.5);
    assert!(!figs.is_empty() && !bands.is_empty());
    assert!(figs.iter().all(|f| f.color == [10, 20, 30, 128]));
    assert!(bands.iter().all(|b| (b.color[3] - 0.2).abs() < 1e-6));
    assert_eq!(
        bands[0].color[..3],
        [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0]
    );

    let (figs, _) = draw(&found, &s.with_opacity(f32::NAN), 0.5);
    assert!(figs.iter().all(|f| f.color[3] == 255));
    let (figs, _) = draw(&found, &s.with_opacity(3.0), 0.5);
    assert!(figs.iter().all(|f| f.color[3] == 255));
    assert_eq!(s.with_opacity(-1.0).opacity, 0.0);
}
