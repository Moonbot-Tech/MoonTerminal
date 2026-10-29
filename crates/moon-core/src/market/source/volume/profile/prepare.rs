//! Nonblocking profile publication; stale archive results never replace the current profile.

use super::super::prepare::{HistoryKey, Prepared};
use super::{PriceProfile, PriceProfileRow, ProfileWindow, rebuild_ttl_ms};
use moonproto::state::{MiniCandle, SeqRingReader, TradeHistoryRow};

/// A profile request's immutable key and clock, carried together into its reader job.
pub(in super::super) struct ProfileRequest {
    pub window: ProfileWindow,
    pub row_width: f64,
    pub source_key: HistoryKey,
    pub now: i64,
}

/// One requested profile's published rows and its exclusively owned background fold.
pub(in super::super) struct ProfileCache {
    state: Prepared<PriceProfile>,
    rows: Vec<PriceProfileRow>,
    revision: u64,
    built_ms: i64,
    source_key: HistoryKey,
    /// Last request time, independent of the reader's completion, for market pruning.
    pub(in super::super) used_ms: i64,
}

impl ProfileCache {
    /// Allocate metadata only; the first retained read is submitted by `read`.
    pub(in super::super) fn new(
        now: i64,
        completed: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        Self {
            state: Prepared::new(PriceProfile::new(), completed),
            rows: Vec::new(),
            revision: 0,
            built_ms: i64::MIN,
            source_key: HistoryKey::default(),
            used_ms: now,
        }
    }

    /// Poll a completed fold and schedule a due read without holding a ring or waiting for work.
    pub(in super::super) fn read(
        &mut self,
        trades: Option<SeqRingReader<TradeHistoryRow>>,
        minis: Option<SeqRingReader<MiniCandle>>,
        request: ProfileRequest,
        seen: u64,
        out: &mut Vec<PriceProfileRow>,
    ) -> Option<u64> {
        let ProfileRequest {
            window,
            row_width,
            source_key,
            now,
        } = request;
        self.used_ms = now;
        if self.state.poll() {
            let ready = self.state.ready.as_ref().expect("completed profile");
            if ready.source_key == source_key {
                self.rows.clear();
                self.rows.extend_from_slice(ready.rows());
                self.revision = ready.revision();
                self.built_ms = ready.built_ms;
                self.source_key = ready.source_key;
            }
        }
        if self.source_key != source_key
            || now.saturating_sub(self.built_ms) >= rebuild_ttl_ms(window)
        {
            self.state.start(move |profile| {
                profile.refresh(
                    trades.as_ref(),
                    minis.as_ref(),
                    window,
                    row_width,
                    source_key,
                    now,
                )
            });
        }
        if self.built_ms == i64::MIN {
            return None;
        }
        if self.revision != seen {
            out.clone_from(&self.rows);
        }
        Some(self.revision)
    }
}
