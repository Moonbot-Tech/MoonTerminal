//! Per-pane bubble invalidation and atomic publication of background-prepared layouts.
//!
//! The last complete fold stays visible while its replacement is computed. Each pane has at
//! most one in-flight request; the frame path accepts it only while the fold settings, layout,
//! epoch and history generation still match and its captured reach covers the visible window.
//! Live appends remain dirty while a job runs, then follow the existing 200 ms data throttle.

use std::time::{Duration, Instant};

use super::super::pane_render::PaneRender;
use super::super::types::ChartViewGpu;
use worker::Source;
pub(in crate::chartdx) use worker::{Folded, Pending, Request};

#[cfg(test)]
mod tests;
mod worker;

/// Minimum interval between two re-folds driven by new trades alone.
const CLUSTER_DATA_THROTTLE: Duration = Duration::from_millis(200);

/// What a pane draws its bubbles with this sync.
pub(in crate::chartdx) struct ClusterInputs {
    pub enabled: bool,
    pub tick: Option<f64>,
    pub view: ChartViewGpu,
    pub epoch_ms: f64,
    pub ppp: f32,
    pub merge_ms: u16,
    pub small_pct: u8,
    pub buy: [f32; 3],
    pub sell: [f32; 3],
}

/// Settings that determine print membership and percentile scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::chartdx) struct FoldKey {
    merge_ms: u16,
    small_pct: u8,
    tick_bits: u64,
}

/// Inputs that determine disc merging, physical size and colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::chartdx) struct LayoutKey {
    time_to_px: f32,
    ppp: f32,
    colors: [[f32; 3]; 2],
}

/// Wake on a completed job even on a quiet market, or on throttled data with no job in flight.
pub(in crate::chartdx) fn refold_due(pr: &PaneRender, now: Instant) -> bool {
    if let Some(pending) = &pr.cluster_pending {
        return pending.is_ready();
    }
    pr.cluster_on && data_due(pr, now)
}

/// Whether the data throttle allows another snapshot; completion never clears newer appends.
fn data_due(pr: &PaneRender, now: Instant) -> bool {
    pr.cluster_dirty
        && pr
            .cluster_built_at
            .is_none_or(|at| now.duration_since(at) >= CLUSTER_DATA_THROTTLE)
}

/// Publish a matching completed snapshot and enqueue at most one replacement without blocking.
/// Returns whether the native cluster buffer changed and needs a presentation.
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
    let mut request = Request {
        fold: FoldKey {
            merge_ms: input.merge_ms,
            small_pct: input.small_pct,
            tick_bits: input.tick.map_or(0, f64::to_bits),
        },
        layout: LayoutKey {
            time_to_px: view.time_to_px,
            ppp: input.ppp,
            colors: [input.buy, input.sell],
        },
        range: (visible.0 - reach, visible.1 + reach),
        epoch: input.epoch_ms,
        generation: pr.cluster_generation,
    };
    let mut changed = false;
    if pr.cluster_pending.as_ref().is_some_and(Pending::is_ready) {
        match pr
            .cluster_pending
            .take()
            .expect("ready request exists")
            .take()
        {
            Ok(result) if result.request.matches(request, visible) => {
                pr.cluster_request = Some(result.request);
                let old_fold = pr.cluster_folded.replace(result.folded);
                let old_discs = std::mem::replace(&mut pr.cluster_discs, result.discs);
                super::super::text::note_cluster_discs_published(pr);
                let old_markers = pr.layers.set_clusters(result.markers);
                worker::release_later((old_fold, old_discs, old_markers));
                pr.cluster_on = true;
                pr.cluster_uploaded = true;
                changed = true;
            }
            result => {
                if let Err(ref err) = result {
                    log::error!("bubble preparation failed: {err}");
                }
                worker::release_later(result);
                // A discarded raw fold must not lose the data whose dirty bit it consumed.
                pr.cluster_dirty = true;
                pr.cluster_built_at = None;
            }
        }
    }
    if pr.cluster_pending.is_some() {
        return changed;
    }
    let refold = pr.cluster_request.is_none_or(|old| {
        old.fold != request.fold
            || old.epoch != request.epoch
            || old.generation != request.generation
            || visible.0 < old.range.0
            || visible.1 > old.range.1
    }) || data_due(pr, now);
    let relayout = pr
        .cluster_request
        .is_none_or(|old| old.layout != request.layout);
    if !refold && !relayout {
        return changed;
    }
    let source = if refold {
        // Keep the ring's f32 clock and backend candidate selection exactly as the synchronous
        // path did. No borrowed GPU state or mutable tape reaches the background executor.
        let samples = pr.layers.tick_snapshot(
            request.range.0 - request.epoch,
            request.range.1 - request.epoch,
        );
        let previous = pr
            .cluster_request
            .filter(|old| old.fold == request.fold)
            .and_then(|_| pr.cluster_folded.as_ref().and_then(|folded| folded.scale));
        Source::Tape(samples, previous)
    } else {
        request.range = pr.cluster_request.expect("published layout exists").range;
        Source::Fold(
            pr.cluster_folded
                .as_ref()
                .expect("published fold exists")
                .clone(),
        )
    };
    match Pending::spawn(request, source) {
        Ok(pending) => {
            pr.cluster_pending = Some(pending);
            if refold {
                pr.cluster_dirty = false;
                pr.cluster_built_at = Some(now);
            }
        }
        Err(err) => log::error!("cannot enqueue bubble preparation: {err}"),
    }
    changed
}

/// Disable bubbles immediately, invalidate in-flight work, and retain its slot until completion.
fn clear(pr: &mut PaneRender) -> bool {
    pr.cluster_generation = pr.cluster_generation.wrapping_add(1);
    if pr.cluster_pending.as_ref().is_some_and(Pending::is_ready) {
        worker::release_later(pr.cluster_pending.take());
    }
    pr.cluster_on = false;
    let old_fold = pr.cluster_folded.take();
    pr.cluster_request = None;
    let old_discs = std::mem::take(&mut pr.cluster_discs);
    super::super::text::note_cluster_discs_published(pr);
    pr.cluster_dirty = true;
    if !pr.cluster_uploaded {
        return false;
    }
    let old_markers = pr.layers.set_clusters(Vec::new());
    worker::release_later((old_fold, old_discs, old_markers));
    pr.cluster_uploaded = false;
    true
}
