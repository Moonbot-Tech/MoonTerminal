use super::*;
use crate::figures::kind::FigureKind;
use crate::figures::levels::FIB_LEVELS;
use crate::figures::tools::tests::{build, ctx, TestProj};

/// A fall from 100 at T0 to 80 ten seconds later.
fn fib() -> FibRetracement {
    FibRetracement {
        a: FigNode::new(TestProj::T0_MS, 100.0),
        b: FigNode::new(TestProj::T0_MS + 10_000.0, 80.0),
    }
}

#[test]
fn it_draws_a_line_and_a_readout_per_level_plus_the_move_itself() {
    let f = fib();
    let sink = build(&FigureKind::FibRetracement(f), ctx(false, false));
    let n = FIB_LEVELS.len();
    assert_eq!(sink.segs.len(), n + 1, "one line per level, plus the move");
    assert_eq!(
        sink.labels.len(),
        n,
        "a level with no readout cannot be read"
    );
    assert_eq!(
        sink.bands.len(),
        n - 1,
        "a band fills each gap between neighbouring levels"
    );
    assert_eq!(
        sink.segs[0],
        (f.a, f.b),
        "the move is drawn first, underneath"
    );
}

#[test]
fn every_level_spans_the_move_in_time_and_sits_at_its_own_price() {
    let f = fib();
    let sink = build(&FigureKind::FibRetracement(f), ctx(false, false));
    let (t0, t1) = (f.a.time_ms, f.b.time_ms);
    for (i, level) in FIB_LEVELS.iter().enumerate() {
        let (a, b) = sink.segs[i + 1];
        assert_eq!(
            (a.time_ms, b.time_ms),
            (t0, t1),
            "level {} spans wrong",
            level.ratio
        );
        assert_eq!(a.price, b.price, "a level must be horizontal");
        let expected = crate::figures::levels::price_at(f.a.price, f.b.price, level.ratio);
        assert!((a.price - expected).abs() < 1e-9);
    }
}

#[test]
fn every_readout_rides_the_line_it_names() {
    let f = fib();
    let sink = build(&FigureKind::FibRetracement(f), ctx(false, false));
    let (t0, t1) = (f.a.time_ms, f.b.time_ms);
    for (_, place, _) in &sink.labels {
        assert_eq!(
            *place,
            LabelPlace::LineSpan {
                t0_ms: t0,
                t1_ms: t1
            },
            "a level label anchored anywhere but its own line vanishes with the box's edge"
        );
    }
}

#[test]
fn a_level_that_crosses_zero_is_dropped_rather_than_drawn() {
    // A rise from 10 to 100 puts every extension below zero: 4.236 lands at -281.
    let f = FibRetracement {
        a: FigNode::new(TestProj::T0_MS, 10.0),
        b: FigNode::new(TestProj::T0_MS + 10_000.0, 100.0),
    };
    assert!(
        crate::figures::levels::price_at(10.0, 100.0, 4.236) < 0.0,
        "the fixture no longer produces a negative level"
    );
    let sink = build(&FigureKind::FibRetracement(f), ctx(false, false));
    for (_, _, text) in &sink.labels {
        match text {
            LabelText::Level { price, .. } => {
                assert!(*price > 0.0, "negative level drawn: {price}")
            }
            other => panic!("unexpected label {other:?}"),
        }
    }
    for band in &sink.bands {
        assert!(band.2 > 0.0 && band.3 > 0.0, "band crosses zero: {band:?}");
    }
    assert!(
        sink.labels.len() < FIB_LEVELS.len(),
        "nothing was dropped, so the guard did not run"
    );
}

#[test]
fn a_move_with_no_height_draws_no_scale_at_all() {
    let flat = FibRetracement {
        a: FigNode::new(TestProj::T0_MS, 100.0),
        b: FigNode::new(TestProj::T0_MS + 10_000.0, 100.0),
    };
    let sink = build(&FigureKind::FibRetracement(flat), ctx(false, false));
    assert!(
        sink.labels.is_empty(),
        "eleven readouts stacked on one price"
    );
    assert!(sink.bands.is_empty(), "zero-height bands");
}

#[test]
fn the_readout_names_the_level_and_the_price_it_sits_at() {
    let f = fib();
    let sink = build(&FigureKind::FibRetracement(f), ctx(false, false));
    let (_, _, text) = sink.labels[0];
    assert_eq!(
        text,
        LabelText::Level {
            ratio: 0.0,
            price: 80.0
        },
        "level 0 must read the END of the move"
    );
    let (_, _, last) = sink.labels[FIB_LEVELS.len() - 1];
    match last {
        LabelText::Level { ratio, price } => {
            assert!(ratio > 1.0);
            assert!(price > 100.0, "an extension of a fall sits above its start");
        }
        other => panic!("a level label must name its level: {other:?}"),
    }
}

/// Expected `[f32; 4]` for a level's own hue at `alpha`.
///
/// A deliberate COPY that shadows the production `hue` this module glob-imports: what the
/// assertions below are for is the PAIRING of a level with its line, its band and its readout, and
/// a shared conversion would make them pass by construction. The arithmetic itself is pinned once,
/// against literals, by [`the_scale_reaches_the_sink_in_the_colour_it_declares`].
fn hue(level: &crate::figures::levels::Level, alpha: f32) -> [f32; 4] {
    let [r, g, b] = level.color;
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, alpha]
}

#[test]
fn the_scale_reaches_the_sink_in_the_colour_it_declares() {
    // The one assertion whose oracle is NOT the production conversion: 0.618 is teal #089981, and
    // these are the floats a channel swap or a division by 256 would break while every pairing
    // assertion below still passed.
    let level = FIB_LEVELS
        .iter()
        .find(|l| l.ratio == 0.618)
        .expect("0.618 left the scale");
    assert_eq!(
        level.color,
        [0x08, 0x99, 0x81],
        "0.618 is not teal any more"
    );
    let sink = build(&FigureKind::FibRetracement(fib()), ctx(false, false));
    let line = sink.seg_colors[1 + FIB_LEVELS.iter().position(|l| l.ratio == 0.618).unwrap()];
    assert!(
        (line[0] - 8.0 / 255.0).abs() < 1e-6,
        "red channel: {line:?}"
    );
    assert!(
        (line[1] - 153.0 / 255.0).abs() < 1e-6,
        "green channel: {line:?}"
    );
    assert!(
        (line[2] - 129.0 / 255.0).abs() < 1e-6,
        "blue channel: {line:?}"
    );
}

#[test]
fn every_level_wears_its_own_hue_in_both_its_line_and_its_readout() {
    let c = ctx(false, false);
    let sink = build(&FigureKind::FibRetracement(fib()), c);
    for (i, level) in FIB_LEVELS.iter().enumerate() {
        // `+ 1`: the move itself is emitted first and keeps the FIGURE's colour, which is what
        // makes the pencil's colour still visible on this tool.
        let expected = hue(level, c.stroke.color[3] * level.emphasis.line_alpha());
        assert_eq!(
            sink.seg_colors[i + 1],
            expected,
            "level {} line is not its own hue",
            level.ratio
        );
        assert_eq!(
            sink.label_colors[i], expected,
            "level {} readout does not match the line it names",
            level.ratio
        );
    }
    assert_ne!(
        sink.seg_colors[0], sink.seg_colors[1],
        "the move must not be repainted in the first level's hue"
    );
}

#[test]
fn a_band_takes_the_hue_of_the_smaller_ratio_it_joins_and_the_users_own_opacity() {
    let c = ctx(false, false);
    let sink = build(&FigureKind::FibRetracement(fib()), c);
    // Band `i` joins levels `i` and `i + 1` only while every level is drawn; a fixture that
    // dropped one would shift the mapping and quietly weaken every assertion below.
    assert_eq!(
        sink.band_colors.len(),
        FIB_LEVELS.len() - 1,
        "the fixture skipped a level, so band i no longer starts at level i"
    );
    for (i, band_color) in sink.band_colors.iter().enumerate() {
        assert_eq!(
            *band_color,
            hue(&FIB_LEVELS[i], c.fill[3]),
            "band {i} does not take the hue of the smaller of the two ratios it joins"
        );
    }
    assert!(
        sink.band_colors.windows(2).all(|w| w[0] != w[1]),
        "neighbouring bands share a colour and merge into one wash"
    );
}

#[test]
fn the_bands_stack_without_gaps_or_overlaps() {
    let f = fib();
    let sink = build(&FigureKind::FibRetracement(f), ctx(false, false));
    for (i, band) in sink.bands.iter().enumerate() {
        let lo = crate::figures::levels::price_at(f.a.price, f.b.price, FIB_LEVELS[i].ratio);
        let hi = crate::figures::levels::price_at(f.a.price, f.b.price, FIB_LEVELS[i + 1].ratio);
        assert_eq!(
            (band.2.min(band.3), band.2.max(band.3)),
            (lo.min(hi), lo.max(hi)),
            "band {i} does not join its two levels"
        );
        assert_eq!((band.0, band.1), (f.a.time_ms, f.b.time_ms));
    }
}

#[test]
fn a_figure_drawn_right_to_left_still_spans_a_forward_time_range() {
    // The second click may land BEFORE the first; a negative span would collapse every band.
    let f = FibRetracement {
        a: FigNode::new(TestProj::T0_MS + 10_000.0, 100.0),
        b: FigNode::new(TestProj::T0_MS, 80.0),
    };
    let sink = build(&FigureKind::FibRetracement(f), ctx(false, false));
    for band in &sink.bands {
        assert!(band.0 < band.1, "band spans backwards: {band:?}");
    }
    let (a, b) = sink.segs[1];
    assert!(a.time_ms < b.time_ms);
}

#[test]
fn it_is_grabbed_by_a_level_line_not_only_by_the_move() {
    let f = fib();
    // Extension 2.618 of the fall sits at 152.36 → y = (100 - 152.36) * 2 = -104.72, far above
    // the move itself, which runs between y=0 and y=40.
    let on_level = (5.0, -104.72);
    assert!(
        f.hit(on_level, &TestProj) < 0.5,
        "{}",
        f.hit(on_level, &TestProj)
    );
    // Well outside the price range of every level.
    let far = (5.0, 400.0);
    assert!(f.hit(far, &TestProj) > 100.0);
}

#[test]
fn dragging_an_end_moves_the_whole_scale_with_it() {
    let mut f = fib();
    assert!(f.move_handle(1, FigNode::new(TestProj::T0_MS + 10_000.0, 60.0)));
    assert_eq!(f.a, fib().a, "the other end must stay put");
    let sink = build(&FigureKind::FibRetracement(f), ctx(false, false));
    let (a, _) = sink.segs[1];
    assert_eq!(a.price, 60.0, "level 0 follows the end it is anchored to");
}

#[test]
fn the_first_click_previews_the_scale_under_the_cursor() {
    let a = FigNode::new(TestProj::T0_MS, 100.0);
    let cursor = FigNode::new(TestProj::T0_MS + 5_000.0, 90.0);
    let preview = (DEF.preview)(&[a], cursor).expect("one placed node previews the scale");
    assert!(matches!(preview, FigureKind::FibRetracement(_)));
    assert!(
        (DEF.make)(&[a]).is_none(),
        "one node cannot finish the figure"
    );
}
