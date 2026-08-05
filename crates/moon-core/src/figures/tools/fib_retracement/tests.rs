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
            LabelPlace::LineEnd {
                t0_ms: t0,
                t1_ms: t1
            },
            "a level label anchored anywhere but its own line vanishes with the box's end"
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

#[test]
fn neighbouring_bands_differ_so_they_do_not_merge_into_one_wash() {
    let f = fib();
    let mut sink = crate::figures::tools::tests::RecSink::default();
    let mut alphas = Vec::new();
    crate::figures::build_figure(
        &FigureKind::FibRetracement(f),
        &ctx(false, false),
        &mut sink,
    );
    for a in sink.band_alphas.drain(..) {
        alphas.push(a);
    }
    assert!(alphas.len() >= 2);
    assert!(
        alphas.windows(2).all(|w| w[0] != w[1]),
        "adjacent bands share an alpha: {alphas:?}"
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
