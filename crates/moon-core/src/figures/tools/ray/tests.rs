use super::*;
use crate::figures::kind::FigureKind;
use crate::figures::tools::tests::{TestProj, build, ctx};

/// A ray pointing up and to the right: origin at `T0`, aimed through a point 10 s later and 20
/// higher. In test pixels that is `(0, 0)` aimed through `(10, -40)`.
fn ray() -> Ray {
    Ray {
        a: FigNode::new(TestProj::T0_MS, 100.0),
        b: FigNode::new(TestProj::T0_MS + 10_000.0, 120.0),
    }
}

/// Both nodes are draggable, and dragging one must not drag the other. The second is the AIM, so
/// moving it turns the ray about its origin — that is the whole interaction.
#[test]
fn both_nodes_are_handles_and_move_independently() {
    let mut r = ray();
    assert_eq!(r.handle_count(), 2);
    assert_eq!(r.handle(0), Some(r.a));
    assert_eq!(r.handle(1), Some(r.b));
    assert_eq!(r.handle(2), None);
    let aim = FigNode::new(TestProj::T0_MS + 5_000.0, 90.0);
    assert!(r.move_handle(1, aim));
    assert_eq!(r.b, aim);
    assert_eq!(r.a, ray().a, "moving the aim must not move the origin");
    assert!(
        !r.move_handle(1, aim),
        "an unchanged drag reports no change"
    );
}

/// The whole VISIBLE line is grabbable, not just the stretch between the two nodes.
///
/// This is the one thing that makes a ray a ray rather than a segment, and it is exactly what the
/// segment's own distance helper cannot express: it clamps the projection to `0..=1`, so everything
/// past the second node would be dead to the cursor while still being drawn.
#[test]
fn the_line_past_the_aim_is_still_grabbable() {
    let r = ray();
    let proj = TestProj;
    // On the line, far beyond the second node: 100 s out, at the price the slope reaches there.
    let far = (100.0f32, -400.0f32);
    assert!(
        r.hit(far, &proj) < 0.01,
        "a point on the ray past its aim must be a hit"
    );
    // The same distance BEHIND the origin is not part of the shape: a ray has one end.
    let behind = (-10.0f32, 40.0f32);
    let dist = r.hit(behind, &proj);
    assert!(
        (dist - 41.23).abs() < 0.1,
        "behind the origin the distance is to the origin itself, got {dist}"
    );
}

/// Aiming a ray at its own origin has no direction to extrapolate. It must answer as the point it
/// is, not divide by zero — this is the state every ray passes through while being drawn.
#[test]
fn a_ray_aimed_at_its_own_origin_is_a_point() {
    let r = Ray {
        a: FigNode::new(TestProj::T0_MS, 100.0),
        b: FigNode::new(TestProj::T0_MS, 100.0),
    };
    let d = r.hit((3.0, 4.0), &TestProj);
    assert!(d.is_finite(), "a degenerate ray must not produce NaN");
    assert!((d - 5.0).abs() < 0.01, "distance to the origin point");
}

/// The tool must emit a RAY, not a segment. A segment would draw a line that stops at the second
/// node — visually a different figure, and nothing else in the pipeline would notice.
#[test]
fn it_emits_a_ray_and_no_segment() {
    let rec = build(&FigureKind::Ray(ray()), ctx(false, false));
    assert_eq!(rec.rays.len(), 1, "one ray");
    assert_eq!(rec.rays[0], (ray().a, ray().b));
    assert!(
        rec.segs.is_empty(),
        "a ray drawn as a segment would simply stop at its aim"
    );
}

/// Hovering reads out the move the ray describes so far, anchored at the aim — the last point on
/// the line that is real rather than extrapolated.
#[test]
fn hovering_labels_the_move_at_the_aim() {
    let rec = build(&FigureKind::Ray(ray()), ctx(true, false));
    assert_eq!(rec.labels.len(), 1);
    let (at, _, text) = rec.labels[0];
    assert_eq!(at, ray().b);
    assert_eq!(
        text,
        crate::figures::LabelText::PctDelta {
            from: 100.0,
            to: 120.0
        }
    );
}

/// Dragging the body moves both nodes by the same amount, or the ray would change direction while
/// being moved.
#[test]
fn a_body_drag_keeps_the_direction() {
    let mut r = ray();
    assert!(r.translate(1_000.0, 5.0));
    assert_eq!(r.a.time_ms, TestProj::T0_MS + 1_000.0);
    assert_eq!(r.a.price, 105.0);
    assert_eq!(r.b.time_ms, TestProj::T0_MS + 11_000.0);
    assert_eq!(r.b.price, 125.0);
    assert!(!r.translate(0.0, 0.0), "a zero drag reports no change");
}

/// The core has no chart-object type for a ray, so it must never be offered as an alert — the
/// blob encoder refuses it, and this is the flag every surface reads before offering the box.
#[test]
fn a_ray_is_not_alertable() {
    // Through the registry, not the const: this also proves the tool IS registered, which is what
    // every surface looks the flag up by.
    assert!(!FigureTool::Ray.def().alertable);
    assert!(
        crate::alert_blob::encode(
            &FigureKind::Ray(ray()),
            [1, 2, 3, 4],
            1.0,
            crate::figures::LineKind::Solid,
            0.0,
            0,
            1,
        )
        .is_none(),
        "encoding a ray would have the core draw something the user did not draw"
    );
}
