use super::*;
use crate::figures::kind::FigureKind;
use crate::figures::tools::tests::{build, ctx, TestProj};

fn tri() -> Triangle {
    Triangle {
        a: FigNode::new(TestProj::T0_MS, 100.0),
        b: FigNode::new(TestProj::T0_MS + 10_000.0, 120.0),
        c: FigNode::new(TestProj::T0_MS + 20_000.0, 100.0),
    }
}

#[test]
fn it_closes_its_outline_with_three_edges() {
    let t = tri();
    let sink = build(&FigureKind::Triangle(t), ctx(false, false));
    assert_eq!(sink.segs, vec![(t.a, t.b), (t.b, t.c), (t.c, t.a)]);
}

#[test]
fn a_body_drag_keeps_the_triangle_rigid() {
    let mut t = tri();
    let before = [t.a, t.b, t.c];
    assert!(t.translate(1_000.0, 2.0));
    for (n, was) in [t.a, t.b, t.c].iter().zip(before) {
        assert_eq!(*n, was.shifted(1_000.0, 2.0));
    }
    assert!(!t.translate(0.0, 0.0));
}

#[test]
fn every_vertex_is_a_handle_and_the_selected_figure_shows_all_three() {
    let t = tri();
    assert_eq!(t.handle_count(), 3);
    assert_eq!(t.handle(2), Some(t.c));
    assert_eq!(t.handle(3), None);
    let sink = build(&FigureKind::Triangle(t), ctx(false, true));
    assert_eq!(sink.handles, vec![t.a, t.b, t.c]);
}

#[test]
fn the_hit_distance_takes_the_nearest_edge() {
    let t = tri();
    // Midpoint of edge c-a lies on the line price=100 → y=0, x between 0 and 20.
    assert!(t.hit((15.0, 2.0), &TestProj) <= 2.0 + 1e-3);
}

#[test]
fn the_first_click_previews_an_edge_rather_than_a_triangle() {
    let a = FigNode::new(TestProj::T0_MS, 100.0);
    let cursor = FigNode::new(TestProj::T0_MS + 5_000.0, 110.0);
    let one = (DEF.preview)(&[a], cursor).expect("one placed vertex previews something");
    assert!(matches!(one, FigureKind::Segment(_)));
    let two = (DEF.preview)(&[a, cursor], cursor).expect("two placed vertices preview a triangle");
    assert!(matches!(two, FigureKind::Triangle(_)));
}
