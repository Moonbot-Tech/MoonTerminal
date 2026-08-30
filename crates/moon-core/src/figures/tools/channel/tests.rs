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
    let prices: Vec<LabelText> = hot.labels.iter().map(|(_, _, t)| *t).collect();
    assert_eq!(
        prices,
        vec![LabelText::Price(100.0), LabelText::Price(110.0)]
    );
    assert!(
        hot.labels
            .iter()
            .all(|(_, p, _)| *p == LabelPlace::RightEdge)
    );
}
