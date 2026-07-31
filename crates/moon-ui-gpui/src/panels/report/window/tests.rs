//! Regression tests for standalone Report initial geometry.

use gpui::{Bounds, point, px, size};

use super::initial_report_bounds;

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
