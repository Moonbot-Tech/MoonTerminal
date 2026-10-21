//! The per-pane trade clusters: when the grid's cells are re-summed and when their discs are
//! re-sized and uploaded.
//!
//! The arithmetic is `moon_chart::clusters`; this module owns only the invalidation. Cells are summed
//! from the backend's resident trade mirror over the visible window plus one window either side, so a
//! pan within that reach re-sizes the discs without re-summing. Re-summing follows a new grid (a
//! zoom moved the time interval or the price row, the tick arrived), a view that left the summed
//! reach, and new trades — those at most once per [`CLUSTER_DATA_THROTTLE`], like the order book's
//! own bake throttle. The discs follow the biggest VISIBLE cluster, so a pan that changes it
//! re-sizes and re-uploads them.

use std::time::{Duration, Instant};

use moon_chart::clusters::{self, ClusterGrid};
use moon_chart::layers::{MARKER_SHAPE_KNOT, MarkerInstance};

use super::super::pane_render::PaneRender;
use super::super::types::ChartViewGpu;

/// Minimum interval between two re-sums driven by new trades alone.
const CLUSTER_DATA_THROTTLE: Duration = Duration::from_millis(200);

/// Opacity of a disc: a crowd of them still lets the candles show through.
const DISC_ALPHA: f32 = 0.85;

/// What a pane draws its clusters with this sync.
pub(in crate::chartdx) struct ClusterInputs {
    /// Whether the tab draws its trades as bubbles at all.
    pub enabled: bool,
    /// The market's tick, `None` while unknown.
    pub tick: Option<f64>,
    /// The plot's own transform, physical pixels.
    pub view: ChartViewGpu,
    /// The pane's time epoch, unix milliseconds: `time_rel` is relative to it.
    pub epoch_ms: f64,
    /// Physical pixels a cell needs on either axis.
    pub min_cell_px: f32,
    /// sRGB colours of a cell that bought more and of one that sold more.
    pub buy: [f32; 3],
    pub sell: [f32; 3],
}

/// Whether new trades wait to be summed and the throttle now lets them through.
///
/// Read by the sync's own trigger (`pull_market_source_if_visible`): a burst the throttle held
/// back must still be summed on a market that then goes quiet, when no new revision would ever
/// run the sync again.
pub(in crate::chartdx) fn resum_due(pr: &PaneRender, now: Instant) -> bool {
    pr.cluster_dirty
        && pr.cluster_grid.is_some()
        && pr
            .cluster_built_at
            .is_none_or(|at| now.duration_since(at) >= CLUSTER_DATA_THROTTLE)
}

/// Bring the pane's clusters up to date.
///
/// Returns:
///     Whether the layer received new discs (or lost them), so the caller re-presents.
pub(in crate::chartdx) fn sync_clusters(
    pr: &mut PaneRender,
    input: &ClusterInputs,
    now: Instant,
) -> bool {
    let view = input.view;
    let grid = input
        .enabled
        .then_some(())
        .and(input.tick)
        .and_then(|tick| {
            let time_step_ms = clusters::time_step_ms(view.time_to_px, input.min_cell_px)?;
            let price_step =
                moon_chart::ladder::row_step(tick, view.price_to_px, input.min_cell_px)?;
            Some(ClusterGrid {
                time_step_ms,
                tick,
                price_step,
            })
        });
    let Some(grid) = grid else {
        return clear(pr);
    };
    let time_to_px = f64::from(view.time_to_px);
    let visible = (
        input.epoch_ms + f64::from(view.view_time0),
        input.epoch_ms + f64::from(view.view_time0) + f64::from(view.bounds[2]) / time_to_px,
    );
    let reach = visible.1 - visible.0;
    let left_reach = pr.cluster_grid != Some(grid)
        || visible.0 < pr.cluster_range.0
        || visible.1 > pr.cluster_range.1;
    let data_due = pr.cluster_dirty
        && pr
            .cluster_built_at
            .is_none_or(|at| now.duration_since(at) >= CLUSTER_DATA_THROTTLE);
    let mut resummed = false;
    if left_reach || data_due {
        // The start lands on a whole number of merge spans: while the range keeps cutting the same
        // stretch, every re-sum merges it from the same intervals, so a follow or a small pan does
        // not make its discs hop. Once the stretch's real start enters the range it is merged from
        // that start instead — one re-chunk, accepted with chunking from a stretch's first interval.
        let span_ms = grid.time_step_ms as f64 * clusters::MERGE_SPAN as f64;
        let range = (
            ((visible.0 - reach) / span_ms).floor() * span_ms,
            visible.1 + reach,
        );
        let epoch = input.epoch_ms;
        let samples = pr
            .layers
            .tick_samples(range.0 - epoch, range.1 - epoch)
            .filter(|c| c.side < 2)
            .map(|c| (epoch + f64::from(c.time_rel), c.price, c.qty, c.side == 0));
        clusters::aggregate(samples, grid, &mut pr.cluster_cells);
        pr.cluster_grid = Some(grid);
        pr.cluster_range = range;
        pr.cluster_dirty = false;
        pr.cluster_built_at = Some(now);
        resummed = true;
    }
    // The discs scale against the biggest cluster on screen — any whose disc can still show, so a
    // centre just past the edge counts too, or its disc would draw at full size against a smaller
    // visible maximum.
    let row_h = grid.row_height();
    let step_ms = grid.time_step_ms as f64;
    let max_r = clusters::max_radius(
        (step_ms * time_to_px) as f32,
        (row_h * f64::from(view.price_to_px)) as f32,
    );
    let r_ms = f64::from(max_r) / time_to_px;
    let r_price = f64::from(max_r / view.price_to_px);
    let price_lo = f64::from(view.view_price0) - r_price;
    let price_hi =
        f64::from(view.view_price0) + f64::from(view.bounds[3] / view.price_to_px) + r_price;
    let max_total = pr
        .cluster_cells
        .iter()
        .filter(|c| {
            let p = c.price_idx as f64 * row_h;
            c.center_ms >= visible.0 - r_ms
                && c.center_ms <= visible.1 + r_ms
                && p >= price_lo
                && p <= price_hi
        })
        .map(|c| c.total())
        .fold(0.0_f32, f32::max);
    let colors = [input.buy, input.sell];
    if !resummed
        && pr.cluster_max == max_total
        && pr.cluster_max_r == max_r
        && pr.cluster_colors == colors
    {
        return false;
    }
    pr.cluster_max = max_total;
    pr.cluster_max_r = max_r;
    pr.cluster_colors = colors;
    let buy = [input.buy[0], input.buy[1], input.buy[2], DISC_ALPHA];
    let sell = [input.sell[0], input.sell[1], input.sell[2], DISC_ALPHA];
    pr.cluster_markers.clear();
    pr.cluster_markers.extend(pr.cluster_cells.iter().map(|c| {
        MarkerInstance::at_price(
            (c.center_ms - input.epoch_ms) as f32,
            (c.price_idx as f64 * row_h) as f32,
            clusters::disc_radius(c.total(), max_total, max_r),
            0.0,
            MARKER_SHAPE_KNOT,
            if c.buy >= c.sell { buy } else { sell },
        )
    }));
    pr.layers.set_clusters(&pr.cluster_markers);
    pr.cluster_uploaded = true;
    true
}

/// Take the pane's clusters down: no grid this sync, so nothing may stay drawn.
fn clear(pr: &mut PaneRender) -> bool {
    pr.cluster_grid = None;
    pr.cluster_cells.clear();
    pr.cluster_range = (f64::NAN, f64::NAN);
    pr.cluster_max = 0.0;
    if !pr.cluster_uploaded {
        return false;
    }
    pr.layers.set_clusters(&[]);
    pr.cluster_uploaded = false;
    true
}
