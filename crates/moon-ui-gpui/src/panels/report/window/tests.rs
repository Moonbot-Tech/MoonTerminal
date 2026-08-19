//! Regression tests for standalone Report initial geometry.

use gpui::{Bounds, point, px, size};

use super::{initial_report_bounds, report_title, restored_report_bounds};
use moon_core::config::GeomRect;

/// Restoring the old 1240x720 constants must fail this assertion and recreate the cramped window
/// reported by the user.
///
/// Returns:
///     Nothing; the preferred large geometry is asserted.
#[test]
fn report_prefers_the_reviewed_large_geometry() {
    let bounds = initial_report_bounds(Some(Bounds {
        origin: point(px(0.0), px(0.0)),
        size: size(px(1880.0), px(1328.0)),
    }));

    assert_eq!(f32::from(bounds.origin.x), 120.0);
    assert_eq!(f32::from(bounds.origin.y), 180.0);
    assert_eq!(f32::from(bounds.size.width), 1640.0);
    assert_eq!(f32::from(bounds.size.height), 1100.0);
}

/// Dropping visible-bounds clamping must fail on the secondary-display origin or bottom edge and
/// would reopen Reports partly outside a smaller monitor.
///
/// Returns:
///     Nothing; global origin and safe edge clamping are asserted.
#[test]
fn report_clamps_inside_a_smaller_secondary_display() {
    let bounds = initial_report_bounds(Some(Bounds {
        origin: point(px(1920.0), px(40.0)),
        size: size(px(1280.0), px(720.0)),
    }));

    assert_eq!(f32::from(bounds.origin.x), 1944.0);
    assert_eq!(f32::from(bounds.origin.y), 64.0);
    assert_eq!(f32::from(bounds.size.width), 1232.0);
    assert_eq!(f32::from(bounds.size.height), 672.0);
}

/// Reusing saved coordinates after their monitor disappears must fail this assertion and would
/// reopen the standalone Report outside the current desktop.
///
/// Returns:
///     Nothing; stale geometry falls back to safe bounds on the selected display.
#[test]
fn report_discards_geometry_from_a_disconnected_display() {
    let visible = Bounds {
        origin: point(px(0.0), px(0.0)),
        size: size(px(1920.0), px(1080.0)),
    };
    let bounds = restored_report_bounds(
        Some(GeomRect {
            x: 2400,
            y: 180,
            w: 1200,
            h: 800,
            display_uuid: None,
        }),
        Some(visible),
        false,
    );

    assert_eq!(f32::from(bounds.origin.x), 140.0);
    assert_eq!(f32::from(bounds.origin.y), 24.0);
    assert_eq!(f32::from(bounds.size.width), 1640.0);
    assert_eq!(f32::from(bounds.size.height), 1032.0);
}

/// Returning saved bounds unchanged after a display shrinks must fail these edge assertions and
/// would reopen a reachable Report partly outside the usable desktop.
///
/// Returns:
///     Nothing; saved size and origin are clamped together to the current work area.
#[test]
fn report_clamps_saved_geometry_after_display_changes() {
    let bounds = restored_report_bounds(
        Some(GeomRect {
            x: 2500,
            y: 600,
            w: 1400,
            h: 900,
            display_uuid: None,
        }),
        Some(Bounds {
            origin: point(px(1920.0), px(40.0)),
            size: size(px(1280.0), px(720.0)),
        }),
        true,
    );

    assert_eq!(f32::from(bounds.origin.x), 1920.0);
    assert_eq!(f32::from(bounds.origin.y), 40.0);
    assert_eq!(f32::from(bounds.size.width), 1280.0);
    assert_eq!(f32::from(bounds.size.height), 720.0);
}

/// Accepting a hand-edited 1x1 saved rectangle must fail this assertion and would make the Report
/// unusable even though its origin still belongs to an attached display.
///
/// Returns:
///     Nothing; unusable persisted dimensions restore the safe initial geometry.
#[test]
fn report_rejects_unusable_saved_dimensions() {
    let bounds = restored_report_bounds(
        Some(GeomRect {
            x: 400,
            y: 300,
            w: 1,
            h: 1,
            display_uuid: None,
        }),
        Some(Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(1920.0), px(1080.0)),
        }),
        true,
    );

    assert_eq!(f32::from(bounds.origin.x), 140.0);
    assert_eq!(f32::from(bounds.origin.y), 24.0);
    assert_eq!(f32::from(bounds.size.width), 1640.0);
    assert_eq!(f32::from(bounds.size.height), 1032.0);
}

/// Dropping the sole-core suffix in `report::window::report_title` must fail the middle assertion;
/// appending it for implicit All or multi-select must fail the surrounding assertions.
#[test]
fn report_title_names_only_an_explicit_sole_core() {
    let cores = vec![(1, "CORE-A".to_string()), (2, "CORE-B".to_string())];
    assert_eq!(report_title(&Default::default(), &cores), "Report");
    assert_eq!(
        report_title(&[2].into_iter().collect(), &cores),
        "Report — CORE-B"
    );
    assert_eq!(
        report_title(&[1, 2].into_iter().collect(), &cores),
        "Report"
    );
    assert_eq!(report_title(&[99].into_iter().collect(), &cores), "Report");
}
