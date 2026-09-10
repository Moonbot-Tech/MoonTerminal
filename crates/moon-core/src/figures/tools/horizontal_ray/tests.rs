//! Regression coverage for one-click creation, right-only geometry, and generic dragging.

use super::*;
use crate::figures::tools::tests::{TestProj, build, ctx};
use crate::figures::tools::{Grab, drag_figure, pick_handle};
use crate::figures::{DrawStyle, Figure};

/// A visible origin with nonzero time and price offsets in the test projection.
fn ray() -> FigureKind {
    FigureKind::HorizontalRay(HorizontalRay {
        origin: FigNode::new(TestProj::T0_MS + 10_000.0, 110.0),
    })
}

/// A tool accidentally registered as two-click or as HLine cannot complete this flow.
#[test]
fn one_click_finishes_the_cursor_preview() {
    let def = FigureTool::HorizontalRay.def();
    let cursor = FigNode::new(1_700_000_123_456.0, 42.125);
    assert_eq!(def.clicks, 1);
    assert!((def.make)(&[]).is_none());
    let made = (def.make)(&[cursor]).expect("one click finishes the ray");
    assert!(matches!(made, FigureKind::HorizontalRay(_)));
    assert_eq!((def.preview)(&[], cursor), Some(made.clone()));
    assert_eq!(made.shape().handle(0), Some(cursor));
    assert_eq!(made.shape().handle_count(), 1);
    assert_eq!(made.shape().handle(1), None);
}

/// Infinite-line and leftward-ray substitutions both hit the forbidden left side.
#[test]
fn hits_only_the_right_half_line_at_fixed_price() {
    let kind = ray();
    let shape = kind.shape();
    assert_eq!(shape.hit((1_000.0, -20.0), &TestProj), 0.0);
    assert_eq!(shape.hit((1_000.0, -17.0), &TestProj), 3.0);
    assert_eq!(shape.hit((-10.0, -20.0), &TestProj), 20.0);
    assert_eq!(shape.hit((7.0, -16.0), &TestProj), 5.0);
}

/// The renderer must receive a rightward ray, with no full-width line or finite segment.
#[test]
fn emits_rightward_horizontal_ray_and_one_origin_knot() {
    let rec = build(&ray(), ctx(true, true));
    assert_eq!(rec.rays.len(), 1);
    let (origin, aim) = rec.rays[0];
    assert_eq!(origin, FigNode::new(TestProj::T0_MS + 10_000.0, 110.0));
    assert!(aim.time_ms > origin.time_ms);
    assert_eq!(aim.price, origin.price);
    assert_eq!(rec.handles, vec![origin]);
    assert!(rec.hlines.is_empty());
    assert!(rec.segs.is_empty());
}

/// Generic handle and body drags must move time and price while keeping a single horizontal ray.
#[test]
fn generic_anchor_and_body_drag_preserve_horizontal_geometry() {
    let mut figure = Figure::new(ray(), DrawStyle::default(), 0.0);
    assert_eq!(
        pick_handle(&figure.kind, (10.0, -20.0), &TestProj, 4.0),
        Some(0)
    );
    assert_eq!(
        pick_handle(&figure.kind, (500.0, -20.0), &TestProj, 4.0),
        None
    );
    for grab in [Grab::Handle(0), Grab::Body] {
        assert!(drag_figure(&mut figure, grab, 5_000.0, -2.5));
        let rec = build(&figure.kind, ctx(false, true));
        assert_eq!(rec.rays.len(), 1);
        assert!(rec.rays[0].1.time_ms > rec.rays[0].0.time_ms);
        assert_eq!(rec.rays[0].1.price, rec.rays[0].0.price);
        assert_eq!(rec.handles, vec![rec.rays[0].0]);
    }
    assert_eq!(
        figure.kind.shape().handle(0),
        Some(FigNode::new(TestProj::T0_MS + 20_000.0, 105.0))
    );
    assert!(!drag_figure(&mut figure, Grab::Handle(1), 5_000.0, 2.0));
    assert!(!drag_figure(&mut figure, Grab::Body, 0.0, 0.0));
}

/// The registry makes the tool reachable through the cycle and respects its persisted exclusion.
#[test]
fn cycle_includes_horizontal_ray_and_can_skip_it() {
    let config = crate::config::HotkeysConfig::default();
    let allowed = |tool: FigureTool| {
        !config
            .switch_figure_skip
            .iter()
            .any(|key| key == tool.def().key)
    };
    assert_eq!(
        FigureTool::Ray.next_allowed(allowed),
        FigureTool::HorizontalRay
    );
    let excluded: crate::config::HotkeysConfig =
        toml::from_str("switch_figure_skip = ['horizontal_ray']").unwrap();
    assert_eq!(
        FigureTool::Ray.next_allowed(|tool| {
            !excluded
                .switch_figure_skip
                .iter()
                .any(|key| key == tool.def().key)
        }),
        FigureTool::Position
    );
    assert_eq!(
        FigureTool::HorizontalRay.next_allowed(|_| true),
        FigureTool::Position
    );
}

/// There is no compatible chart object on the core; encoding must refuse this shape.
#[test]
fn remains_terminal_only() {
    assert!(!FigureTool::HorizontalRay.def().alertable);
    assert!(
        crate::alert_blob::encode(
            &ray(),
            [1, 2, 3, 4],
            1.0,
            crate::figures::LineKind::Solid,
            0.0,
            0,
            1,
        )
        .is_none()
    );
}
