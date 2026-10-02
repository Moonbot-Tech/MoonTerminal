use super::*;
use crate::figures::kind::FigureKind;
use crate::figures::tools::tests::{TestProj, build, ctx};

fn channel() -> Channel {
    Channel {
        price1: 100.0,
        price2: 110.0,
    }
}

#[test]
fn each_corridor_price_is_its_own_handle() {
    let mut c = channel();
    assert_eq!(c.handle(0), Some(FigNode::new(0.0, 100.0)));
    assert_eq!(c.handle(1), Some(FigNode::new(0.0, 110.0)));
    assert_eq!(c.handle(2), None);
    assert!(c.move_handle(1, FigNode::new(123.0, 115.0)));
    assert_eq!((c.price1, c.price2), (100.0, 115.0));
    assert!(!c.move_handle(1, FigNode::new(0.0, 115.0)));
}

#[test]
fn a_body_drag_keeps_the_corridor_width() {
    let mut c = channel();
    assert!(c.translate(99_000.0, 5.0));
    assert_eq!(
        (c.price1, c.price2),
        (105.0, 115.0),
        "both lines move together and time is ignored"
    );
}

#[test]
fn the_hit_distance_takes_the_nearer_line() {
    let c = channel();
    // price 100 → y=0, price 110 → y=-20.
    assert_eq!(c.hit((0.0, -18.0), &TestProj), 2.0);
    assert_eq!(c.hit((0.0, 1.0), &TestProj), 1.0);
}

#[test]
fn it_draws_both_lines_and_labels_both_prices_when_hot() {
    let c = channel();
    let kind = FigureKind::Channel(c);
    let idle = build(&kind, ctx(false, false));
    assert_eq!(idle.hlines, vec![100.0, 110.0]);
    assert_eq!(
        idle.bands.len(),
        1,
        "a price corridor encloses an area and must fill it"
    );
    let (t0, t1, p0, p1) = idle.bands[0];
    assert!(
        t0.is_infinite() && t1.is_infinite(),
        "the corridor spans the plot"
    );
    assert_eq!((p0.min(p1), p0.max(p1)), (100.0, 110.0));

    let hot = build(&kind, ctx(true, false));
    let prices: Vec<LabelText> = hot
        .labels
        .iter()
        .filter(|(_, p, _)| *p == LabelPlace::RightEdge)
        .map(|(_, _, t)| *t)
        .collect();
    assert_eq!(
        prices,
        vec![LabelText::Price(100.0), LabelText::Price(110.0)]
    );
}

const FULL_WIDTH: LabelPlace = LabelPlace::LineSpan {
    t0_ms: f64::NEG_INFINITY,
    t1_ms: f64::INFINITY,
};

fn span_readout(c: Channel, hot: bool) -> Vec<(f64, LabelText)> {
    build(&FigureKind::Channel(c), ctx(hot, false))
        .labels
        .into_iter()
        .filter(|(_, p, _)| *p == FULL_WIDTH)
        .map(|(at, _, t)| (at.price, t))
        .collect()
}

/// Moonbot's zone is a ruler: its width is printed at both lines with nothing under the pointer.
#[test]
fn the_width_is_printed_at_both_lines_at_rest() {
    let expected = vec![
        (
            110.0,
            LabelText::PctDelta {
                from: 100.0,
                to: 110.0,
            },
        ),
        (
            100.0,
            LabelText::PctDelta {
                from: 110.0,
                to: 100.0,
            },
        ),
    ];
    assert_eq!(span_readout(channel(), false), expected);
    assert_eq!(
        span_readout(channel(), true),
        expected,
        "hover adds the prices at the right edge, it does not change the readout"
    );
}

/// The sign follows the geometry, not the order the lines were placed in.
#[test]
fn a_zone_drawn_downward_reads_the_same() {
    let down = Channel {
        price1: 110.0,
        price2: 100.0,
    };
    assert_eq!(span_readout(down, false), span_readout(channel(), false));
}

/// A zone dragged flat prints its price once instead of `+0.00%` twice.
#[test]
fn a_thin_zone_prints_its_price() {
    let thin = Channel {
        price1: 100.0,
        price2: 100.0005,
    };
    assert_eq!(
        span_readout(thin, false),
        vec![(100.0005, LabelText::Price(100.0005))]
    );
    // 0.004% would still print `+0.00%`: below the two-decimal print it is a price.
    let rounds_to_zero = Channel {
        price1: 100.0,
        price2: 100.004,
    };
    assert_eq!(span_readout(rounds_to_zero, false).len(), 1);
    let wide_enough = Channel {
        price1: 100.0,
        price2: 100.01,
    };
    assert_eq!(span_readout(wide_enough, false).len(), 2);
}

/// A corridor with no positive base has no percentage to print, but must not print a wrong one.
#[test]
fn a_zone_at_zero_or_below_prints_no_readout() {
    for (price1, price2) in [
        (0.0, 10.0),
        (-100.0, -90.0),
        (f64::NAN, 10.0),
        (1.0, f64::INFINITY),
    ] {
        assert!(
            span_readout(Channel { price1, price2 }, false).is_empty(),
            "{price1} .. {price2}"
        );
    }
}
