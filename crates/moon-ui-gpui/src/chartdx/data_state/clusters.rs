//! The per-pane trade bubbles: when the prints are re-folded and when their discs are re-laid out
//! and uploaded.
//!
//! The arithmetic is `moon_chart::clusters`; this module owns only the invalidation. Prints are
//! folded from the backend's resident trade mirror over the visible window plus one window either
//! side, and the market's scale is measured from them. Re-folding follows a view that left that
//! reach, a new merge window, small threshold or tick, and new trades — those at most once per
//! [`CLUSTER_DATA_THROTTLE`], like the order book's own bake throttle. The discs are laid out
//! again only when the X zoom, the pixel density or the colours change: which discs of one price
//! cover each other depends on the pixels per millisecond and nothing else, so a pan or a price
//! zoom uploads nothing.

use std::time::{Duration, Instant};

use moon_chart::clusters::{self, DiscRadii};
use moon_chart::layers::{MARKER_SHAPE_KNOT, MarkerInstance};

use super::super::pane_render::PaneRender;
use super::super::types::ChartViewGpu;

/// Minimum interval between two re-folds driven by new trades alone.
const CLUSTER_DATA_THROTTLE: Duration = Duration::from_millis(200);

/// Opacity of a disc: a crowd of them still lets the candles show through.
const DISC_ALPHA: f32 = 0.85;

/// What a pane draws its bubbles with this sync.
pub(in crate::chartdx) struct ClusterInputs {
    /// Whether the tab draws its trades as bubbles at all.
    pub enabled: bool,
    /// The market's tick, `None` while unknown: prints then group by exact price.
    pub tick: Option<f64>,
    /// The plot's own transform, physical pixels.
    pub view: ChartViewGpu,
    /// The pane's time epoch, unix milliseconds: `time_rel` is relative to it.
    pub epoch_ms: f64,
    /// Physical pixels per logical pixel.
    pub ppp: f32,
    /// The tab's merge window, milliseconds.
    pub merge_ms: u16,
    /// The tab's small-print threshold, percent of the market's prints.
    pub small_pct: u8,
    /// sRGB colours of a disc that bought more and of one that sold more.
    pub buy: [f32; 3],
    pub sell: [f32; 3],
}

/// What the prints were folded with; any change re-folds them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::chartdx) struct FoldKey {
    merge_ms: u16,
    small_pct: u8,
    tick_bits: u64,
}

/// What the discs were laid out with; any change lays them out again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::chartdx) struct LayoutKey {
    time_to_px: f32,
    ppp: f32,
    colors: [[f32; 3]; 2],
}

/// Whether new trades wait to be folded and the throttle now lets them through.
///
/// Read by the sync's own trigger (`pull_market_source_if_visible`): a burst the throttle held
/// back must still be folded on a market that then goes quiet, when no new revision would ever
/// run the sync again.
pub(in crate::chartdx) fn refold_due(pr: &PaneRender, now: Instant) -> bool {
    pr.cluster_dirty
        && pr.cluster_on
        && pr
            .cluster_built_at
            .is_none_or(|at| now.duration_since(at) >= CLUSTER_DATA_THROTTLE)
}

/// Bring the pane's bubbles up to date.
///
/// Returns:
///     Whether the layer received new discs (or lost them), so the caller re-presents.
pub(in crate::chartdx) fn sync_clusters(
    pr: &mut PaneRender,
    input: &ClusterInputs,
    now: Instant,
) -> bool {
    let view = input.view;
    let time_to_px = f64::from(view.time_to_px);
    if !input.enabled || !(time_to_px.is_finite() && time_to_px > 0.0) {
        return clear(pr);
    }
    let visible = (
        input.epoch_ms + f64::from(view.view_time0),
        input.epoch_ms + f64::from(view.view_time0) + f64::from(view.bounds[2]) / time_to_px,
    );
    let reach = visible.1 - visible.0;
    let fold_key = FoldKey {
        merge_ms: input.merge_ms,
        small_pct: input.small_pct,
        tick_bits: input.tick.map_or(0, f64::to_bits),
    };
    let left_reach = !pr.cluster_on
        || pr.cluster_fold != Some(fold_key)
        || visible.0 < pr.cluster_range.0
        || visible.1 > pr.cluster_range.1;
    let data_due = pr.cluster_dirty
        && pr
            .cluster_built_at
            .is_none_or(|at| now.duration_since(at) >= CLUSTER_DATA_THROTTLE);
    let mut refolded = false;
    if left_reach || data_due {
        let range = (visible.0 - reach, visible.1 + reach);
        let epoch = input.epoch_ms;
        // The ring keeps times as f32 from the pane's epoch, as the crosses draw them: on a pane
        // open for days a short merge window groups on that same coarser clock.
        let samples = pr
            .layers
            .tick_samples(range.0 - epoch, range.1 - epoch)
            .filter(|c| c.side < 2)
            .map(|c| (epoch + f64::from(c.time_rel), c.price, c.qty, c.side == 0));
        clusters::aggregate_prints(
            samples,
            input.tick,
            f64::from(input.merge_ms),
            &mut pr.cluster_prints,
        );
        clusters::price_time_order(&pr.cluster_prints, &mut pr.cluster_order);
        let fresh =
            clusters::print_scale(&pr.cluster_prints, input.small_pct, &mut pr.cluster_scratch);
        // Same settings and a scale that barely moved: keep it, so the discs do not breathe. The
        // small end never stays above the fresh one, or a print the threshold now sizes — every
        // print, at 0% — would draw as a dot.
        let same_fold = pr.cluster_fold == Some(fold_key);
        pr.cluster_scale = match (pr.cluster_scale, fresh) {
            (Some(old), Some(new)) if same_fold && old.holds_against(&new) => {
                Some(moon_chart::clusters::PrintScale {
                    small: old.small.min(new.small),
                    ..old
                })
            }
            (_, fresh) => fresh,
        };
        pr.cluster_fold = Some(fold_key);
        pr.cluster_range = range;
        pr.cluster_dirty = false;
        pr.cluster_built_at = Some(now);
        pr.cluster_on = true;
        refolded = true;
    }
    let layout_key = LayoutKey {
        time_to_px: view.time_to_px,
        ppp: input.ppp,
        colors: [input.buy, input.sell],
    };
    if !refolded && pr.cluster_layout == Some(layout_key) {
        return false;
    }
    pr.cluster_layout = Some(layout_key);
    match pr.cluster_scale {
        Some(scale) => {
            let radii = DiscRadii::at(input.ppp);
            clusters::merge_overlapping(
                &pr.cluster_prints,
                &pr.cluster_order,
                time_to_px,
                |v| clusters::print_radius(v, scale, radii),
                &mut pr.cluster_discs,
            );
        }
        None => pr.cluster_discs.clear(),
    }
    let buy = [input.buy[0], input.buy[1], input.buy[2], DISC_ALPHA];
    let sell = [input.sell[0], input.sell[1], input.sell[2], DISC_ALPHA];
    pr.cluster_markers.clear();
    pr.cluster_markers.extend(pr.cluster_discs.iter().map(|d| {
        MarkerInstance::at_price(
            (d.time_ms - input.epoch_ms) as f32,
            d.price,
            d.radius,
            0.0,
            MARKER_SHAPE_KNOT,
            if d.buy >= d.sell { buy } else { sell },
        )
    }));
    // Small discs under big ones: a crowd of dots never hides a wall's disc.
    pr.cluster_markers.sort_by(|a, b| a.size.total_cmp(&b.size));
    pr.layers.set_clusters(&pr.cluster_markers);
    pr.cluster_uploaded = true;
    true
}

/// Take the pane's bubbles down: the tab draws crosses, or the view has no usable scale.
fn clear(pr: &mut PaneRender) -> bool {
    pr.cluster_on = false;
    pr.cluster_prints.clear();
    pr.cluster_order.clear();
    pr.cluster_discs.clear();
    pr.cluster_scale = None;
    pr.cluster_fold = None;
    pr.cluster_layout = None;
    pr.cluster_range = (f64::NAN, f64::NAN);
    if !pr.cluster_uploaded {
        return false;
    }
    pr.layers.set_clusters(&[]);
    pr.cluster_uploaded = false;
    true
}
