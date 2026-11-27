//! Where this chart's auto lines sit on screen, for the private filming overlay.

use super::super::*;

impl ChartPanel {
    /// Every auto line this panel last drew, placed where it sits on screen.
    ///
    /// The chart paints itself, so the inspector's element map sees the whole plot as one
    /// rectangle and cannot say where a line is. Tooling that must point at one — the private
    /// filming overlay moves a pointer to a level the detector just found — asks here. It reads
    /// what the panes DREW ([`crate::chartdx::ChartEngine::with_auto_lines`]) through the same
    /// projection the hover hit test uses ([`Self::pane_map`]), so a point given here is a point the
    /// hover would also find the line at.
    ///
    /// Empty before the first paint (no slot geometry yet), for a pane without a valid view, and
    /// while the panel is not in the rendered scene: the slot geometry and the drawn lines are
    /// kept from the last paint, so a hidden chart would answer with places some other UI now
    /// covers.
    ///
    /// Returns:
    ///     One entry per drawn line, its points in window-relative CONTENT pixels: the space event
    ///     positions arrive in after the window divided them by its content zoom.
    pub(crate) fn screen_auto_lines(&self) -> Vec<ScreenAutoLine> {
        if !self.scene_visible {
            return Vec::new();
        }
        let Some((bounds, _, _)) = self.chart.slot_geometry() else {
            return Vec::new();
        };
        // The factor `chart_local_from_window_pos` multiplies by, so this is its exact inverse.
        let scale = self.chart.slot_scale_factor();
        if !(scale > 0.0) {
            return Vec::new();
        }
        let origin = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        let to_window =
            |(x, y): (f32, f32)| point(px(origin.0 + x / scale), px(origin.1 + y / scale));
        let mut out = Vec::new();
        for pane in 0..self.chart.pane_count() {
            let Some(map) = self.pane_map(pane) else {
                continue;
            };
            let plot_left_ms = {
                use moon_core::figures::Proj;
                map.time_at_x(map.plot.x)
            };
            let plot_top = px(origin.1 + map.plot.y / scale);
            let plot_bottom = px(origin.1 + (map.plot.y + map.plot.h) / scale);
            self.chart.with_auto_lines(pane, |lines| {
                out.extend(lines.iter().filter_map(|line| {
                    let points: Vec<Point<Pixels>> =
                        sample_line(&line.shape, line.side, line.span, &map, SCREEN_LINE_SAMPLES)
                            .into_iter()
                            .map(to_window)
                            .collect();
                    let slope: Vec<Point<Pixels>> = sample_squeeze_slope(
                        &line.shape,
                        line.extend,
                        line.ref_ms,
                        &map,
                        SCREEN_LINE_SAMPLES,
                    )
                    .into_iter()
                    .map(to_window)
                    .collect();
                    (!points.is_empty()).then(|| ScreenAutoLine {
                        pane,
                        row_ix: line.key.row_ix,
                        title: line.title.text(),
                        shape: shape_name(&line.shape),
                        caption: line.text.clone(),
                        points,
                        slope,
                        first_ms: line.first_ms,
                        touches: line.touches,
                        plot_left_ms,
                        plot_top,
                        plot_bottom,
                    })
                }));
            });
        }
        out
    }
}

/// How many points [`ChartPanel::screen_auto_lines`] samples across a pane's plot per line.
///
/// Enough to aim a pointer along a sloped line and to tell its visible stretch from its hidden
/// one; a straight line needs no more, and the call is rare (a filming step, never a frame).
const SCREEN_LINE_SAMPLES: usize = 24;

/// One drawn auto line as it sits on screen.
#[derive(Debug, Clone)]
pub(crate) struct ScreenAutoLine {
    /// The pane it is drawn on.
    pub pane: usize,
    /// Its row's index in its kind's configuration.
    pub row_ix: usize,
    /// The row's title as the reader sees it in the current language.
    pub title: String,
    /// Its geometry's kind: `horizontal`, `sloped`, `squeeze` or `pair`.
    pub shape: &'static str,
    /// The caption printed on it, when it prints one.
    pub caption: Option<std::sync::Arc<str>>,
    /// Points along the drawn, visible part, window-relative content pixels, left to right.
    pub points: Vec<Point<Pixels>>,
    /// A squeeze's sloped half, sampled like [`Self::points`] (whose points are its level), so a
    /// film can point the viewer at the line pressing into the level; empty for every other shape
    /// and for a sloped half off the plot.
    pub slope: Vec<Point<Pixels>>,
    /// The line's first anchor, Unix ms (`Found::first_ms`): its earliest touch for the shapes
    /// built from touches (levels, sloped lines, squeezes, formations); the period's first bar for
    /// a period extreme or a volume profile; `0.0` for a round price.
    pub first_ms: f64,
    /// How many touches the line was found from; `0` for a shape with none (a profile, a round
    /// price).
    pub touches: usize,
    /// The time at the left edge of its pane's plot, Unix ms: a line whose first touch lies before
    /// it was found from candles off screen, so a viewer cannot see why it is drawn.
    pub plot_left_ms: f64,
    /// The top and bottom of its pane's plot, window-relative content pixels: a level hugging
    /// either edge is drawn, but reads as part of the axis rather than a level (21.11, video 13).
    pub plot_top: Pixels,
    pub plot_bottom: Pixels,
}

/// The name [`ScreenAutoLine::shape`] gives a geometry.
fn shape_name(shape: &moon_chart::auto_figures::FoundShape) -> &'static str {
    use moon_chart::auto_figures::FoundShape;
    match shape {
        FoundShape::Horizontal { .. } => "horizontal",
        FoundShape::Sloped { .. } => "sloped",
        FoundShape::Squeeze { .. } => "squeeze",
        FoundShape::Pair { .. } => "pair",
    }
}

/// Sample a line at `samples` evenly spaced x across the plot of `map`, keeping only the points
/// inside the line's drawn stretch `span` (Unix ms) and inside the plot.
///
/// Why the plot test and not only the span: a level far above the price is drawn, but clipped off
/// the top, and a pointer aimed at it would land on the axis or outside the chart.
///
/// Args:
///     shape: The line's geometry.
///     side: The side a two-line formation was kept by; `None` for every other shape.
///     span: The drawn stretch, `±∞` for "to that plot edge".
///     map: The pane's projection.
///     samples: How many x positions to try; fewer than two yields nothing.
///
/// Returns:
///     The kept points in panel-local device pixels, left to right.
fn sample_line(
    shape: &moon_chart::auto_figures::FoundShape,
    side: Option<moon_chart::auto_figures::PairSide>,
    span: (f64, f64),
    map: &super::super::geom::PaneMap,
    samples: usize,
) -> Vec<(f32, f32)> {
    use moon_core::figures::Proj;
    if samples < 2 {
        return Vec::new();
    }
    let plot = map.plot;
    let step = plot.w / (samples - 1) as f32;
    (0..samples)
        .filter_map(|i| {
            let x = plot.x + step * i as f32;
            let t = map.time_at_x(x);
            if t < span.0 || t > span.1 {
                return None;
            }
            let y = map.y_of_price(shape.side_price_at(t, side)?);
            (y.is_finite() && y >= plot.y && y <= plot.y + plot.h).then_some((x, y))
        })
        .collect()
}

/// Sample the sloped half of a squeeze over the stretch it is drawn: from its first touch to where
/// it meets the level or to the run's reference time `ref_ms`, whichever comes first, never short
/// of its last touch; under `BetweenTouches`, to its last touch — the rule of
/// `moon_chart::auto_figures::draw::capped_end`, fed the same `extend` and `ref_ms` the line was
/// drawn with (`AutoLineRec`).
///
/// Both ends are sampled exactly, not on the plot's grid: a film marks the apex with a ring at the
/// last point and shades the zone up to it, and a grid step is up to a 23rd of the plot short.
/// The ends are clipped to the plot's left and right edges; a point above or below the plot is
/// dropped, as [`sample_line`] drops it, so a half starting off the plot vertically begins at
/// the first grid column inside it.
///
/// Args:
///     shape: The line's geometry; anything but a squeeze yields nothing.
///     extend: The row's `extend`.
///     ref_ms: The run's reference time, Unix ms.
///     map: The pane's projection.
///     samples: Grid columns across the plot between the two ends, as for [`sample_line`].
///
/// Returns:
///     The kept points in panel-local device pixels, left to right.
fn sample_squeeze_slope(
    shape: &moon_chart::auto_figures::FoundShape,
    extend: moon_core::config::auto_figures::Extend,
    ref_ms: f64,
    map: &super::super::geom::PaneMap,
    samples: usize,
) -> Vec<(f32, f32)> {
    use moon_chart::auto_figures::{FoundShape, line_price_at};
    use moon_core::config::auto_figures::Extend;
    use moon_core::figures::Proj;
    let FoundShape::Squeeze {
        level,
        t0_ms,
        p0,
        t1_ms,
        p1,
        ..
    } = *shape
    else {
        return Vec::new();
    };
    // The chart draws no squeeze whose level or touches are not finite and positive
    // (`draw::append_shapes`), so the probe reports no slope for one either.
    let drawable = |v: f64| v.is_finite() && v > 0.0;
    if !(t0_ms.is_finite() && t1_ms.is_finite() && t1_ms > t0_ms)
        || !(drawable(level) && drawable(p0) && drawable(p1))
        || samples < 2
    {
        return Vec::new();
    }
    let end = match extend {
        Extend::BetweenTouches => t1_ms,
        Extend::RightEdge | Extend::BothEdges => {
            let now = if ref_ms.is_finite() {
                ref_ms.max(t1_ms)
            } else {
                t1_ms
            };
            // Where the line meets the level; a flat line gives ±inf (NaN when it lies on the
            // level), which is no apex.
            let apex = t0_ms + (level - p0) / (p1 - p0) * (t1_ms - t0_ms);
            if apex.is_finite() && apex > t0_ms {
                now.min(apex)
            } else {
                now
            }
        }
    };
    let plot = map.plot;
    let from = t0_ms.max(map.time_at_x(plot.x));
    let to = end.min(map.time_at_x(plot.x + plot.w));
    if !(to > from) {
        return Vec::new();
    }
    let step = plot.w / (samples - 1) as f32;
    let grid = (0..samples)
        .map(|i| map.time_at_x(plot.x + step * i as f32))
        .filter(|&t| t > from && t < to);
    std::iter::once(from)
        .chain(grid)
        .chain(std::iter::once(to))
        .filter_map(|t| {
            let y = map.y_of_price(line_price_at(t0_ms, p0, t1_ms, p1, t)?);
            (y.is_finite() && y >= plot.y && y <= plot.y + plot.h).then_some((map.x_of_time(t), y))
        })
        .collect()
}

#[cfg(test)]
mod tests;
