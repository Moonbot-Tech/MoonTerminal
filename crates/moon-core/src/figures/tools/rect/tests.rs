use super::*;
use crate::figures::kind::FigureKind;
use crate::figures::tools::tests::{build, ctx, TestProj};

fn rect() -> Rect {
    Rect {
        a: FigNode::new(TestProj::T0_MS, 100.0),
        b: FigNode::new(TestProj::T0_MS + 10_000.0, 80.0),
    }
}

#[test]
fn it_closes_its_outline_and_fills_the_area_between_the_corners() {
    let r = rect();
    let sink = build(&FigureKind::Rect(r), ctx(false, false));
    assert_eq!(sink.segs.len(), 4, "a rectangle is four edges");
    assert_eq!(sink.bands.len(), 1);
    assert_eq!(
        sink.bands[0],
        (r.a.time_ms, r.b.time_ms, r.a.price, r.b.price)
    );
    // Every edge is either horizontal or vertical: the corners must not be joined diagonally.
    for (p, q) in &sink.segs {
        assert!(
            p.time_ms == q.time_ms || p.price == q.price,
            "diagonal edge {p:?}..{q:?}"
        );
    }
}

#[test]
fn a_rectangle_drawn_in_any_direction_covers_the_same_area() {
    let forward = build(&FigureKind::Rect(rect()), ctx(false, false));
    let backward = build(
        &FigureKind::Rect(Rect {
            a: FigNode::new(TestProj::T0_MS + 10_000.0, 80.0),
            b: FigNode::new(TestProj::T0_MS, 100.0),
        }),
        ctx(false, false),
    );
    let norm = |b: &(f64, f64, f64, f64)| (b.0.min(b.1), b.0.max(b.1), b.2.min(b.3), b.2.max(b.3));
    assert_eq!(norm(&forward.bands[0]), norm(&backward.bands[0]));
}

#[test]
fn it_is_grabbed_by_an_edge_and_not_by_its_middle() {
    // A box wide enough that its middle is far from every edge: 100 px across, 40 px tall in the
    // test projection.
    let r = Rect {
        a: FigNode::new(TestProj::T0_MS, 100.0),
        b: FigNode::new(TestProj::T0_MS + 100_000.0, 80.0),
    };
    assert!(r.hit((50.0, 0.5), &TestProj) < 1.0, "the top edge is a hit");
    assert!(
        r.hit((50.0, 20.0), &TestProj) >= 20.0,
        "the filled interior must stay clickable for the chart underneath"
    );
}

#[test]
fn dragging_a_corner_moves_only_that_corner() {
    let mut r = rect();
    let moved = FigNode::new(TestProj::T0_MS + 20_000.0, 70.0);
    assert!(r.move_handle(1, moved));
    assert_eq!(r.a, rect().a);
    assert_eq!(r.b, moved);
    assert!(!r.move_handle(1, moved));
    assert!(!r.move_handle(7, moved));
}

#[test]
fn a_body_drag_keeps_the_rectangle_rigid() {
    let mut r = rect();
    assert!(r.translate(1_000.0, 2.0));
    assert_eq!(r.a, rect().a.shifted(1_000.0, 2.0));
    assert_eq!(r.b, rect().b.shifted(1_000.0, 2.0));
    assert!(!r.translate(0.0, 0.0));
}
