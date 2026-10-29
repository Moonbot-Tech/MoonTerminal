//! Nonblocking retained side-series seeds, with synchronous bounded live advances.

use super::super::prepare::{HistoryKey, Prepared};
use super::{SideSeries, SideVolumeBucket};
use moonproto::state::{MiniCandle, SeqRingReader, TradeHistoryRow};

/// One shared series; cold, clipped and archive seeds leave the last chart upload resident.
pub(in super::super) struct SeriesCache {
    state: Prepared<SideSeries>,
    /// Last request time, independent of background seed completion, for pruning.
    pub(in super::super) used_ms: i64,
}

impl SeriesCache {
    /// Allocate an empty fold without reading any retained history.
    pub(in super::super) fn new(
        now: i64,
        completed: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        Self {
            state: Prepared::new(SideSeries::new(), completed),
            used_ms: now,
        }
    }

    /// Poll a seed and fold only the bounded new batch; false asks the chart to retry next frame.
    pub(in super::super) fn advance(
        &mut self,
        trades: Option<SeqRingReader<TradeHistoryRow>>,
        minis: Option<SeqRingReader<MiniCandle>>,
        source_key: HistoryKey,
        now: i64,
    ) -> bool {
        self.used_ms = now;
        self.state.poll();
        if self
            .state
            .ready
            .as_mut()
            .is_some_and(|series| series.advance_live(trades.as_ref(), source_key))
        {
            return true;
        }
        self.state
            .start(move |series| series.advance(trades.as_ref(), minis.as_ref(), source_key));
        false
    }

    /// Sample only a completely seeded series; a pending seed never publishes a partial band.
    pub(in super::super) fn rolling_into(
        &self,
        tf_ms: i64,
        step_ms: i64,
        from: i64,
        to: i64,
        out: &mut Vec<SideVolumeBucket>,
    ) {
        if let Some(series) = self.state.ready.as_ref() {
            series.rolling_into(tf_ms, step_ms, from, to, out);
        }
    }
}
