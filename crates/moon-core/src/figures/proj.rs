//! Data ↔ pixel projection a tool hit-tests through.
//!
//! The implementation lives in the UI (the chart pane's own mapping); this trait is the seam that
//! keeps figure math in this crate, where it can be tested against a hand-written projection
//! instead of a running window.

use super::node::FigNode;

/// Point in a pane's LOGICAL pixels, the same space mouse events arrive in.
pub type PxPoint = (f32, f32);

/// Maps figure coordinates to a chart pane's pixels and back.
///
/// Implementors must be consistent in both directions for the current view: `time_at_x(x_of_time(t))`
/// is expected to round-trip within a pixel. Tools rely on that only for dragging, where a small
/// deviation is invisible.
pub trait Proj {
    fn x_of_time(&self, time_ms: f64) -> f32;
    fn y_of_price(&self, price: f64) -> f32;
    fn time_at_x(&self, x: f32) -> f64;
    fn price_at_y(&self, y: f32) -> f64;

    /// Pixel position of a node.
    fn px_of(&self, n: FigNode) -> PxPoint {
        (self.x_of_time(n.time_ms), self.y_of_price(n.price))
    }

    /// Node under a pixel position.
    fn node_at(&self, p: PxPoint) -> FigNode {
        FigNode {
            time_ms: self.time_at_x(p.0),
            price: self.price_at_y(p.1),
        }
    }
}

/// Distance in pixels between two points.
pub fn pt_dist(a: PxPoint, b: PxPoint) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Distance in pixels from a point to a line SEGMENT (not to its infinite line).
pub fn seg_dist(p: PxPoint, a: PxPoint, b: PxPoint) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= 1e-6 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    pt_dist(p, (a.0 + t * dx, a.1 + t * dy))
}

/// Distance in pixels from a point to a horizontal line at `price`.
pub fn hline_dist(p: PxPoint, price: f64, proj: &dyn Proj) -> f32 {
    (p.1 - proj.y_of_price(price)).abs()
}
