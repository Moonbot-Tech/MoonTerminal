//! Caption period resolution, volume reads and retained sample bounds.

use super::*;

/// Turn one configured period into the pair the history is asked for.
///
/// `None` when the caption measures around the pointer and there is no pointer on this pane: there
/// is nothing to read, and reading the live edge instead would answer a different question under a
/// heading that says "cursor".
pub(super) fn resolve_span_key(
    key: &VolumeSpanKey,
    cursor_ms: Option<i64>,
) -> Option<(VolumeSpan, VolumeAt)> {
    let span = VolumeSpan::from_label(key.span, key.window);
    let at = match key.anchor {
        LabelAnchor::Now => VolumeAt::Now,
        LabelAnchor::Cursor => VolumeAt::Around(cursor_ms?),
    };
    Some((span, at))
}

/// The same for a whole set, dropping the ones that cannot be read right now.
pub(super) fn resolve_span_keys(
    keys: &[VolumeSpanKey],
    cursor_ms: Option<i64>,
) -> Vec<(VolumeSpan, VolumeAt)> {
    let mut out: Vec<(VolumeSpan, VolumeAt)> = Vec::new();
    for key in keys {
        let Some(resolved) = resolve_span_key(key, cursor_ms) else {
            continue;
        };
        // One read per PERIOD: the traded figures and the liquidation one share it.
        if !out.contains(&resolved) {
            out.push(resolved);
        }
    }
    out
}

/// Read every figure one pane's volume captions ask for, in one place.
///
/// Called from TWO paths that must not drift: the market revision, which refreshes a live-edge
/// block as trades arrive, and the pointer moving, which is the only thing that changes a measuring
/// one. Both cost the same and both are measured — `volume_read_us` is the counter that says whether
/// this is what a reader felt.
///
/// Args:
///     source: Shared market source; its own cache absorbs repeated asks.
///     core: Consumer core the pane sits on.
///     market: Data-key market name.
///     keys: Periods the configuration asks for, already deduplicated.
///     cursor_ms: Quantized moment under the pointer, or `None` when it is off this pane.
///
/// Returns:
///     The traded figures and the liquidation ones, each keyed by the period it answers.
pub(in crate::chartdx) fn read_volume_sets(
    source: &moon_core::market::MarketDataSource,
    core: moon_core::session::CoreId,
    market: &str,
    keys: &[VolumeSpanKey],
    cursor_ms: Option<i64>,
) -> (
    Vec<((VolumeSpan, VolumeAt), VolumeSpanReadout)>,
    Vec<((VolumeSpan, VolumeAt), LiqSpanReadout)>,
) {
    let started = std::time::Instant::now();
    let rows: Vec<((VolumeSpan, VolumeAt), VolumeSpanReadout)> = resolve_span_keys(keys, cursor_ms)
        .into_iter()
        .filter_map(|(span, at)| {
            source
                .market_volume_span(core, market, span, at)
                .map(|readout| ((span, at), readout))
        })
        .collect();
    // Only the periods something prints the liquidation figure over: that ring is its own read, and
    // a block showing volume alone must not order it.
    let liq: Vec<((VolumeSpan, VolumeAt), LiqSpanReadout)> = keys
        .iter()
        .filter(|key| key.liquidations)
        .filter_map(|key| resolve_span_key(key, cursor_ms))
        .filter_map(|(span, at)| {
            source
                .market_liq_span(core, market, span, at)
                .map(|readout| ((span, at), readout))
        })
        .collect();
    crate::diag::bump(&crate::diag::CHART_VOLUME_READS);
    crate::diag::bump_by(
        &crate::diag::CHART_VOLUME_READ_US,
        started.elapsed().as_micros() as u64,
    );
    (rows, liq)
}

/// Replace the CURSOR-anchored entries of a readout set, leaving the live-edge ones alone.
///
/// The two anchors are refreshed on different clocks — the market's and the pointer's — so each
/// path may only touch its own entries. Replacing the set wholesale is what would blank a live-edge
/// caption every time the mouse moved.
pub(super) fn merge_readouts<T>(
    held: &mut Vec<((VolumeSpan, VolumeAt), T)>,
    fresh: Vec<((VolumeSpan, VolumeAt), T)>,
) {
    held.retain(|((_, at), _)| matches!(at, VolumeAt::Now));
    held.extend(
        fresh
            .into_iter()
            .filter(|((_, at), _)| !matches!(at, VolumeAt::Now)),
    );
}

/// Whether two period sets hold the same entries, whatever order they are in.
///
/// Both sides are a handful of entries, so this is a pair of linear passes rather than a hash: a
/// chart prints one or two periods, and the comparison runs per pane per market revision.
pub(super) fn same_period_set(
    held: &[(VolumeSpan, VolumeAt)],
    want: &[(VolumeSpan, VolumeAt)],
) -> bool {
    held.len() == want.len() && want.iter().all(|key| held.contains(key))
}

/// Conservative upper bound on sample timeframes after one candle apply.
///
/// A full replacement scans every sample from zero. A patch scans only the new
/// suffix and starts from the previous bound, so removing the widest row cannot
/// shrink it. A rejected apply leaves the previous bound unchanged.
///
/// Args:
///     samples: Retained samples after the apply.
///     applied: How the read landed.
///     previous: Bound before this apply.
///
/// Returns:
///     A value at least as large as every surviving sample's `tf_ms`.
pub(super) fn volume_sample_timeframe_bound(
    samples: &[moon_chart::VolumeSample],
    applied: &CandleApply,
    previous: f64,
) -> f64 {
    match applied {
        CandleApply::Rejected => previous,
        CandleApply::Full => samples
            .iter()
            .fold(0.0, |max, sample| max.max(sample.tf_ms)),
        CandleApply::Patch(from) => samples[*from..]
            .iter()
            .fold(previous, |max, sample| max.max(sample.tf_ms)),
    }
}
