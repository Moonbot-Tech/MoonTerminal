//! Bounded nearest-point selection for drawing gestures, independent of the renderer.

use moon_core::figures::{FigNode, Proj};

use crate::view::Rect;

/// Find the nearest visible market node within a pixel radius of the pointer.
///
/// The caller supplies only points actually plotted for the current market and mode. The
/// projection, plot and tolerance share one pixel coordinate system; device-pixel callers scale
/// their logical tolerance once. Equal distances resolve by time then price, independent of
/// upload order. A pointer outside the plot never attracts a point across a trading-zone edge.
pub fn nearest_point(
    points: impl IntoIterator<Item = FigNode>,
    pointer: (f32, f32),
    plot: Rect,
    projection: &dyn Proj,
    tolerance: f32,
) -> Option<FigNode> {
    if !tolerance.is_finite() || tolerance <= 0.0 || !inside(plot, pointer) {
        return None;
    }
    let mut best: Option<(FigNode, f32)> = None;
    for node in points {
        if !node.time_ms.is_finite() || !node.price.is_finite() || node.price <= 0.0 {
            continue;
        }
        let pixel = projection.px_of(node);
        if !inside(plot, pixel) {
            continue;
        }
        let distance = (pixel.0 - pointer.0).hypot(pixel.1 - pointer.1);
        if distance > tolerance {
            continue;
        }
        let replace = best.is_none_or(|(previous, previous_distance)| {
            distance
                .total_cmp(&previous_distance)
                .then(node.time_ms.total_cmp(&previous.time_ms))
                .then(node.price.total_cmp(&previous.price))
                .is_lt()
        });
        if replace {
            best = Some((node, distance));
        }
    }
    best.map(|(node, _)| node)
}

/// Reject non-finite geometry as well as clipped points before comparing distances.
fn inside(plot: Rect, point: (f32, f32)) -> bool {
    [plot.x, plot.y, plot.w, plot.h, point.0, point.1]
        .into_iter()
        .all(f32::is_finite)
        && plot.w > 0.0
        && plot.h > 0.0
        && point.0 >= plot.x
        && point.0 <= plot.x + plot.w
        && point.1 >= plot.y
        && point.1 <= plot.y + plot.h
}

#[cfg(test)]
mod tests;
