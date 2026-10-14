//! Nonblocking, incremental cache reads for the open candle's finer price path.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use crate::market::candles::ChartCandle;
use crate::market::kline_cache::{PendingPrefixRead, PrefixPoll, PrefixReadRequest};

use super::{MarketDataSource, SharedMoonClient};

/// Cached ranges delivered independently of the main candle series.
#[derive(Default, Clone)]
pub struct PathPrefixRows {
    /// `None` means no successful one-minute read; empty `Some` is authoritative empty.
    pub minutes: Option<Vec<ChartCandle>>,
    /// Five-minute fallback, with cached data overriding the end-stamped core ring.
    pub five: Vec<ChartCandle>,
}

/// One consumer's open-bucket prefix, preserved across camera resets.
#[derive(Default)]
pub(super) struct OpenPathHistory {
    key: Option<PathReadKey>,
    rows: PathPrefixRows,
    pending: Option<PendingPrefixRead>,
    /// The last minute requested; unavailable reads retry on the next minute, never per frame.
    requested_minute: Option<i64>,
    /// Previously covered frontier; incremental queries overlap one five-minute bucket.
    frontier: Option<i64>,
}

/// Identity and bucket that make retained path data safe to reuse.
#[derive(PartialEq, Eq)]
struct PathReadKey {
    provider: u64,
    exchange: Option<String>,
    market: String,
    tf_ms: i64,
    open_ms: i64,
}

impl OpenPathHistory {
    /// A historical cache merge can fill holes behind the frontier. Request that prefix again
    /// without discarding currently visible ranges while the queued read settles.
    pub(super) fn invalidate_cache(&mut self) {
        self.pending = None;
        self.requested_minute = None;
        self.frontier = None;
    }

    /// Polls one queued reply and requests at most one read per minute. A bucket/provider or
    /// archive change clears the prefix; ordinary live ticks and camera movement do no SQL.
    pub(super) fn sync(
        &mut self,
        source: &MarketDataSource,
        target: (u64, &str),
        tf_ms: i64,
        now_ms: i64,
        refresh: bool,
        done: Arc<AtomicU64>,
    ) -> bool {
        let (core, market) = target;
        let open = (now_ms / tf_ms.max(1)) * tf_ms.max(1);
        let inner = source.inner.read().expect("market source poisoned");
        let provider = inner.core_provider.get(&core).copied().unwrap_or(core);
        let exchange = inner
            .provider_exchange
            .get(&provider)
            .map(|e| format!("{}:{:08x}", e.code, e.dex));
        let key = PathReadKey {
            provider,
            exchange: exchange.clone(),
            market: market.to_owned(),
            tf_ms,
            open_ms: open,
        };
        let mut changed = false;
        if self.key.as_ref() != Some(&key) || refresh {
            self.key = Some(key);
            self.rows = PathPrefixRows::default();
            self.pending = None;
            self.requested_minute = None;
            self.frontier = None;
            changed = true;
        }
        if tf_ms <= 60_000 {
            return changed;
        }
        if let Some(pending) = self.pending.as_ref() {
            match pending.poll() {
                PrefixPoll::Pending => return changed,
                PrefixPoll::Ready(rows) => {
                    merge_rows(self.rows.minutes.get_or_insert_default(), rows.native);
                    merge_rows(&mut self.rows.five, rows.m5);
                    self.pending = None;
                    self.frontier = self.requested_minute.map(|m| m * 60_000);
                    changed = true;
                }
                PrefixPoll::Lost => self.pending = None,
            }
        }
        let minute = now_ms / 60_000;
        if self.requested_minute == Some(minute) {
            return changed;
        }
        self.requested_minute = Some(minute);
        let from_ms = self.frontier.map_or(open, |t| (t - 300_000).max(open));
        // The ring is end-stamped: include the bucket ending after our left edge.
        let client = inner.clients.get(&provider).and_then(SharedMoonClient::get);
        let snapshot = client.and_then(|c| c.snapshot_versioned());
        let ring = snapshot
            .as_ref()
            .and_then(|s| s.market_history_readers(market))
            .and_then(|r| r.candles_5m);
        if let Some(ring) = ring {
            let mut wire = Vec::new();
            ring.copy_time_range(
                moonproto::MoonTime::from_unix_millis(from_ms + 1),
                moonproto::MoonTime::from_unix_millis(now_ms),
                ring.capacity(),
                &mut wire,
            );
            let rows = wire
                .iter()
                .map(|r| ChartCandle {
                    t_open_ms: (r.time().unix_millis() - 300_000) as f64,
                    open: r.open(),
                    high: r.high(),
                    low: r.low(),
                    close: r.close(),
                    volume: 0.0,
                    quote_volume: 0.0,
                })
                .collect();
            // Existing cached rows win over the lower-resolution snapshot.
            let cached = std::mem::take(&mut self.rows.five);
            self.rows.five = rows;
            merge_rows(&mut self.rows.five, cached);
            changed = true;
        }
        if let (Some(cache), Some(exchange)) = (inner.kline_cache.as_ref(), exchange) {
            self.pending = cache.request_prefix(PrefixReadRequest {
                exchange,
                market: market.to_owned(),
                native_kind: 1,
                from_ms,
                now_ms,
                want_5m: true,
                want_1d: false,
                done,
            });
        }
        changed
    }

    /// Copies the composed prefix only when it changed, leaving ordinary ticks allocation-free.
    pub(super) fn copy_into(&self, out: &mut PathPrefixRows) {
        out.clone_from(&self.rows);
    }
}

/// Merges ordered cache replies, with the incoming rows replacing matching timestamps.
fn merge_rows(held: &mut Vec<ChartCandle>, incoming: Vec<ChartCandle>) {
    for row in incoming {
        match held.binary_search_by(|r| r.t_open_ms.total_cmp(&row.t_open_ms)) {
            Ok(i) => held[i] = row,
            Err(i) => held.insert(i, row),
        }
    }
}

#[cfg(test)]
mod tests;
