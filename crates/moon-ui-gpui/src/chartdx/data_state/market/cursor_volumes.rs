//! Cursor-anchored volume and liquidation caption refresh.

use super::*;

impl ChartDataState {
    /// Re-read what the MEASURING captions show, because the pointer moved.
    ///
    /// The ordinary captions are refreshed on a data revision — the market changing is what changes
    /// them. A measuring caption is the other way round: its period is anchored to the pointer, so
    /// the market can be perfectly still and the figure still has to move. This is that second path,
    /// and it exists only for the panes that actually carry such a caption.
    ///
    /// Two guards keep it from becoming the cost the ordinary path avoids: nothing runs unless a
    /// drawn caption is cursor-anchored, and a pane whose quantized moment has not changed is
    /// skipped entirely — one comparison, no read.
    ///
    /// Args:
    ///     source: Shared market source.
    ///
    /// Returns:
    ///     Whether any caption changed, so the caller can repaint only when it did.
    pub(in crate::chartdx) fn sync_cursor_volumes(
        &mut self,
        source: &moon_core::market::MarketDataSource,
    ) -> bool {
        // A frozen replay reads no live history at all — see the `live` gate in
        // `sync_from_market_source`. Without this the pointer path would keep filling the
        // measuring captions the market path has stopped answering, and the window would print
        // what traded in the last ten seconds beside a trade that closed hours ago.
        if !self.draws_live_market() {
            return false;
        }
        let mut st = self.render.borrow_mut();
        if !st.chart_labels.any_cursor_anchored() {
            return false;
        }
        // Only the measuring periods. Passing the whole set would read every live-edge period as
        // well — on the mouse-move path, at the pointer's rate — and then throw the results away in
        // `merge_readouts`, which keeps them for the market revision that owns them.
        let keys: Vec<VolumeSpanKey> = st
            .chart_labels
            .volume_spans()
            .into_iter()
            .filter(|key| key.anchor == LabelAnchor::Cursor)
            .collect();
        if keys.is_empty() {
            return false;
        }
        let mut changed = false;
        for idx in 0..st.panes.len() {
            let cursor_ms = st.pane_cursor_unix_ms(idx);
            let Some(pr) = st.panes.get(idx) else {
                continue;
            };
            // The pointer is still on the same moment, or was never on this pane: nothing a
            // measuring caption prints can have changed.
            if pr.label_cursor_ms == cursor_ms {
                continue;
            }
            let target = pr.core.map(|core| (core, pr.market.clone()));
            // Kept before the readings are handed over, so the pane can record WHICH periods it
            // now holds without walking them again.
            let (rows, liq) = match (&target, cursor_ms) {
                (Some((core, market)), Some(_)) => {
                    read_volume_sets(source, *core, market, &keys, cursor_ms)
                }
                // The pointer left the plot. The figures are dropped rather than kept: a measuring
                // caption with nowhere to measure prints its dash, and holding the last reading
                // would keep stating a place the reader has left.
                _ => (Vec::new(), Vec::new()),
            };
            let rows_keys: Vec<(VolumeSpan, VolumeAt)> = rows.iter().map(|(key, _)| *key).collect();
            if let Some(pr) = st.panes.get_mut(idx) {
                pr.label_cursor_ms = cursor_ms;
                // The set is kept ACCURATE rather than cleared: the live-edge half was not read
                // here and keeps its own clock, while the measuring half just was. Dropping the
                // measuring entries would make the next market revision see a changed set, skip its
                // throttle and read everything again — at the pointer's rate.
                pr.label_volume_spans
                    .retain(|(_, at)| matches!(at, VolumeAt::Now));
                pr.label_volume_spans.extend(rows_keys.iter().copied());
                // The LIVE-EDGE entries are re-read on the ordinary path; replacing the whole set
                // here would drop them until the next market revision, which on a quiet coin is a
                // visible blank.
                merge_readouts(&mut pr.label_volumes, rows);
                merge_readouts(&mut pr.label_liquidations, liq);
            }
            if st.refresh_pane_labels(idx) {
                changed = true;
            }
        }
        changed
    }
}
