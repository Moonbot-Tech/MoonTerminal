//! Regression checks for drawing magnet distance, clipping, and deterministic selection.

use super::nearest_point;
use crate::view::Rect;
use moon_core::figures::{FigNode, Proj};

/// An independently specified linear projection, including a device scale factor.
struct Projection(f32);

impl Proj for Projection {
    /// Convert the scaled X coordinate into the test's time axis.
    fn time_at_x(&self, x: f32) -> f64 {
        (x / self.0) as f64
    }
    /// Project the test's time axis with the independent device scale.
    fn x_of_time(&self, time: f64) -> f32 {
        time as f32 * self.0
    }
    /// Convert downward Y pixels into an upward price axis.
    fn price_at_y(&self, y: f32) -> f64 {
        (100.0 - y / self.0) as f64
    }
    /// Project price with an explicitly reversed Y direction.
    fn y_of_price(&self, price: f64) -> f32 {
        (100.0 - price as f32) * self.0
    }
}

/// Exercise the public selector with a logical 100-pixel plot at the requested device scale.
fn snap(points: &[FigNode], pointer: (f32, f32), scale: f32) -> Option<FigNode> {
    nearest_point(
        points.iter().copied(),
        (pointer.0 * scale, pointer.1 * scale),
        Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0 * scale,
            h: 100.0 * scale,
        },
        &Projection(scale),
        8.0 * scale,
    )
}

/// Both axes determine proximity: time-nearest alone must not win over price-nearest.
#[test]
fn nearest_uses_two_dimensional_distance_and_exact_market_values() {
    let points = [FigNode::new(50.0, 70.0), FigNode::new(54.0, 48.0)];
    assert_eq!(snap(&points, (50.0, 50.0), 1.0), Some(points[1]));
    assert_eq!(snap(&points, (50.0, 50.0), 2.0), Some(points[1]));
    assert_eq!(snap(&points, (10.0, 50.0), 1.0), None);
}

/// A circular tolerance must reject diagonal points outside it, including on a scaled display.
#[test]
fn radius_is_inclusive_and_does_not_become_a_square() {
    let edge = FigNode::new(58.0, 50.0);
    assert_eq!(snap(&[edge], (50.0, 50.0), 1.0), Some(edge));
    assert_eq!(snap(&[FigNode::new(56.0, 44.0)], (50.0, 50.0), 2.0), None);
}

/// Upload order cannot cause a magnet to alternate between equidistant points.
#[test]
fn ties_use_time_then_price_regardless_of_input_order() {
    let a = FigNode::new(46.0, 50.0);
    let b = FigNode::new(54.0, 50.0);
    assert_eq!(snap(&[b, a], (50.0, 50.0), 1.0), Some(a));
    assert_eq!(snap(&[a, b], (50.0, 50.0), 1.0), Some(a));
    let c = FigNode::new(50.0, 46.0);
    let d = FigNode::new(50.0, 54.0);
    assert_eq!(snap(&[d, c], (50.0, 50.0), 1.0), Some(c));
}

/// Invisible prices, off-plot times and trading-zone pointers must not attract a figure.
#[test]
fn rejects_clipped_invalid_and_empty_candidates() {
    assert_eq!(snap(&[FigNode::new(-1.0, 50.0)], (1.0, 50.0), 1.0), None);
    assert_eq!(snap(&[FigNode::new(50.0, 101.0)], (50.0, 1.0), 1.0), None);
    assert_eq!(snap(&[FigNode::new(99.0, 50.0)], (101.0, 50.0), 1.0), None);
    assert_eq!(
        snap(
            &[FigNode::new(f64::NAN, 50.0), FigNode::new(50.0, 0.0)],
            (50.0, 99.0),
            1.0
        ),
        None
    );
    assert_eq!(snap(&[], (50.0, 50.0), 1.0), None);
}
