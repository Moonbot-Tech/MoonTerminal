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

/// A dragged base raises its apex above itself, as tall as the base is long.
///
/// The height is not a free constant: it comes from rotating the base vector, so a test that only
/// checked "the apex is above" would pass for a degenerate one-pixel triangle.
#[test]
fn a_dragged_base_raises_an_apex_as_tall_as_itself() {
    let apex = drag_apex((10.0, 200.0), (110.0, 200.0));
    // Screen space: y grows downward, so 100 px above a base at y=200 is y=100.
    assert_eq!(apex, (60.0, 100.0));
}

/// Dragging the other way flips the apex below the base, with no "which side is up" rule anywhere.
#[test]
fn dragging_right_to_left_points_the_triangle_down() {
    let apex = drag_apex((110.0, 200.0), (10.0, 200.0));
    assert_eq!(apex, (60.0, 300.0));
}

/// On a sloped base the apex stays perpendicular to it and keeps the base's length.
///
/// Plausible breakage: an apex written as "the middle, minus the base's width" — which is right on
/// a level base and visibly wrong on every sloped one.
#[test]
fn a_sloped_base_keeps_the_apex_perpendicular_and_the_height_equal() {
    let (a, b) = ((0.0f32, 0.0f32), (60.0f32, 80.0f32));
    let apex = drag_apex(a, b);
    let mid = ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
    let (base, up) = ((b.0 - a.0, b.1 - a.1), (apex.0 - mid.0, apex.1 - mid.1));
    assert!((base.0 * up.0 + base.1 * up.1).abs() < 1e-3, "apex is not perpendicular to the base");
    let (base_len, up_len) = (base.0.hypot(base.1), up.0.hypot(up.1));
    assert!((base_len - up_len).abs() < 1e-3, "height {up_len} does not match base {base_len}");
}
