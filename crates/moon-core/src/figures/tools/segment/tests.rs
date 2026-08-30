use super::*;
use crate::figures::kind::FigureKind;
use crate::figures::tools::tests::{TestProj, build, ctx};

fn seg() -> Segment {
    Segment {
        a: FigNode::new(TestProj::T0_MS, 100.0),
        b: FigNode::new(TestProj::T0_MS + 10_000.0, 120.0),
    }
}

#[test]
fn both_ends_are_handles_and_move_independently() {
    let mut s = seg();
    assert_eq!(s.handle_count(), 2);
    assert_eq!(s.handle(0), Some(s.a));
    assert_eq!(s.handle(2), None);
    let moved = FigNode::new(TestProj::T0_MS + 1.0, 101.0);
    assert!(s.move_handle(0, moved));
    assert_eq!(s.a, moved);
    assert_eq!(s.b, seg().b, "the far end must stay put");
    assert!(!s.move_handle(0, moved));
    assert!(!s.move_handle(5, moved));
}

#[test]
fn the_hit_distance_is_to_the_segment_not_to_its_infinite_line() {
    let s = seg();
    // Endpoints: a=(0,0), b=(10,-40). A point far beyond `b` along the line is measured to `b`.
    let beyond = (20.0, -80.0);
    let expected = ((20.0f32 - 10.0).powi(2) + (-80.0f32 + 40.0).powi(2)).sqrt();
    assert!((s.hit(beyond, &TestProj) - expected).abs() < 1e-3);
    // On the segment itself the distance collapses to zero.
    assert!(s.hit((5.0, -20.0), &TestProj) < 1e-3);
}

#[test]
fn it_draws_one_segment_and_reads_out_its_move_when_hot() {
    let s = seg();
    let kind = FigureKind::Segment(s);
    let idle = build(&kind, ctx(false, false));
    assert_eq!(idle.segs, vec![(s.a, s.b)]);
    assert!(idle.hlines.is_empty());

    let hot = build(&kind, ctx(true, false));
    assert_eq!(
        hot.labels,
        vec![(
            s.b,
            LabelPlace::Above,
            LabelText::PctDelta {
                from: 100.0,
                to: 120.0
            }
        )]
    );
}
