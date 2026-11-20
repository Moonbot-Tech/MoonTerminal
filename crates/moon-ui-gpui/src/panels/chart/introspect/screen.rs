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
        let mut out = Vec::new();
        for pane in 0..self.chart.pane_count() {
            let Some(map) = self.pane_map(pane) else {
                continue;
            };
            self.chart.with_auto_lines(pane, |lines| {
                out.extend(lines.iter().filter_map(|line| {
                    let points: Vec<Point<Pixels>> =
                        sample_line(&line.shape, line.side, line.span, &map, SCREEN_LINE_SAMPLES)
                            .into_iter()
                            .map(|(x, y)| point(px(origin.0 + x / scale), px(origin.1 + y / scale)))
                            .collect();
                    (!points.is_empty()).then(|| ScreenAutoLine {
                        pane,
                        row_ix: line.key.row_ix,
                        title: line.title.text(),
                        shape: shape_name(&line.shape),
                        caption: line.text.clone(),
                        points,
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

#[cfg(test)]
mod tests;
