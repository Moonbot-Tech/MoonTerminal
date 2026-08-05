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
fn every_drawn_corner_is_grabbable_and_drags_its_neighbours_with_it() {
    let r = rect();
    assert_eq!(
        r.handle_count(),
        4,
        "a corner the user can see must be grabbable"
    );
    for i in 0..4 {
        assert_eq!(r.handle(i), Some(r.corners()[i]));
    }
    assert_eq!(r.handle(4), None);

    // Corner 2 is `b` itself: it moves alone.
    let mut far = rect();
    let moved = FigNode::new(TestProj::T0_MS + 20_000.0, 70.0);
    assert!(far.move_handle(2, moved));
    assert_eq!(far.a, rect().a);
    assert_eq!(far.b, moved);
    assert!(!far.move_handle(2, moved), "no change reports none");
    assert!(!far.move_handle(7, moved));

    // Corner 1 is made of b's time and a's price: dragging it must move exactly those, so the
    // rectangle stays a rectangle instead of shearing.
    let mut mixed = rect();
    assert!(mixed.move_handle(1, FigNode::new(TestProj::T0_MS + 50_000.0, 111.0)));
    assert_eq!(mixed.a.price, 111.0);
    assert_eq!(mixed.b.time_ms, TestProj::T0_MS + 50_000.0);
    assert_eq!(mixed.a.time_ms, rect().a.time_ms, "the far side stays put");
    assert_eq!(mixed.b.price, rect().b.price);
}

#[test]
fn a_box_with_no_height_still_draws_its_edges() {
    // The user is mid-gesture and must see what they are placing. Whether the degenerate BAND
    // reaches the GPU is the renderer's call, and its own tests pin it.
    let flat = Rect {
        a: FigNode::new(TestProj::T0_MS, 100.0),
        b: FigNode::new(TestProj::T0_MS + 10_000.0, 100.0),
    };
    let sink = build(&FigureKind::Rect(flat), ctx(false, false));
    assert_eq!(sink.segs.len(), 4);
}

#[test]
fn a_body_drag_keeps_the_rectangle_rigid() {
    let mut r = rect();
    assert!(r.translate(1_000.0, 2.0));
    assert_eq!(r.a, rect().a.shifted(1_000.0, 2.0));
    assert_eq!(r.b, rect().b.shifted(1_000.0, 2.0));
    assert!(!r.translate(0.0, 0.0));
}
