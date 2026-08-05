use super::*;
use crate::figures::kind::FigureKind;
use crate::figures::tools::tests::{build, ctx, TestProj};

fn line() -> HLine {
    HLine { price: 100.0 }
}

#[test]
fn the_handle_carries_the_price_and_a_drag_ignores_time() {
    let mut l = line();
    assert_eq!(l.handle(0), Some(FigNode::new(0.0, 100.0)));
    assert_eq!(l.handle(1), None);
    assert!(l.move_handle(0, FigNode::new(9_999.0, 105.0)));
    assert_eq!(l.price, 105.0);
    assert!(!l.move_handle(0, FigNode::new(0.0, 105.0)), "same price");
    assert!(l.translate(60_000.0, -5.0));
    assert_eq!(l.price, 100.0, "a horizontal line does not move in time");
    assert!(!l.translate(60_000.0, 0.0));
}

#[test]
fn the_hit_distance_is_vertical_and_holds_across_the_whole_plot() {
    let l = line();
    // price 100 sits at y=0 in the test projection.
    assert_eq!(l.hit((0.0, 3.0), &TestProj), 3.0);
    assert_eq!(
        l.hit((10_000.0, 3.0), &TestProj),
        3.0,
        "distance must not depend on x"
    );
}

#[test]
fn it_draws_one_line_and_labels_its_price_only_when_hot() {
    let kind = FigureKind::HLine(line());
    let idle = build(&kind, ctx(false, false));
    assert_eq!(idle.hlines, vec![100.0]);
    assert!(idle.segs.is_empty());

    let hot = build(&kind, ctx(true, false));
    assert_eq!(hot.labels.len(), 1);
    let (at, place, text) = &hot.labels[0];
    assert_eq!(at.price, 100.0);
    assert_eq!(*place, LabelPlace::RightEdge);
    assert_eq!(*text, LabelText::Price(100.0));
}
