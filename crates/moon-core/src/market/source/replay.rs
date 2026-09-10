//! Frozen replay reads from matching cores, without advancing any live chart cursor.

use super::{MarketDataSource, ReplayAddress};
use crate::feed::{Side, Tick};
use crate::market::trade_replay::ReplayWindow;
use crate::venue::MarketKind;

/// Bound the requested interval copy; unrelated retained history must not reject a donor.
const MAX_CORE_ROWS: usize = 250_000;

/// One contiguous retained span from a single core; never assembled across unrelated donors.
pub(crate) struct CoreReplayTicks {
    /// Owned points, independent of future ring eviction or reconnects.
    pub ticks: Vec<Tick>,
    /// Observed retained bounds clipped to the requested window.
    pub covered: (i64, i64),
}

impl MarketDataSource {
    /// Request archives through the shared gate and copy the best currently retained trade span.
    ///
    /// Called only on the replay worker. Identity includes the platform and DEX discriminator;
    /// spot and futures readers are never substituted for one another. Missing or late archives
    /// fall through immediately to REST, whose candle stage gives the archive time to arrive.
    pub(crate) fn replay_core_ticks(
        &self,
        address: &ReplayAddress,
        market: &str,
        window: ReplayWindow,
    ) -> Option<CoreReplayTicks> {
        let (donors, archive) = {
            let inner = self.inner.read().expect("market source poisoned");
            let donors: Vec<_> = inner
                .core_venue
                .iter()
                .filter_map(|(id, venue)| {
                    let exchange = venue.id;
                    (format!("{}:{:08x}", exchange.code, exchange.dex) == address.exchange_key)
                        .then(|| inner.clients.get(id).map(|slot| (*id, slot.clone())))
                        .flatten()
                })
                .collect();
            (donors, inner.archive.clone())
        };
        let mut best: Option<CoreReplayTicks> = None;
        for (provider, slot) in donors {
            let Some((client, epoch)) = slot.get_with_epoch() else {
                continue;
            };
            let Some(snapshot) = client.snapshot_versioned() else {
                continue;
            };
            // A queued job can outlive a provider election or slot replacement. Recheck the
            // actual client's identity before accepting its catalog or issuing an archive ask.
            let info = snapshot.server_info();
            let Some(code) = info.exchange_code else {
                continue;
            };
            let exchange = crate::feed::ExchangeId::with_dex(
                code.stable_id(),
                info.dex_name.as_deref().unwrap_or_default(),
            );
            if format!("{}:{:08x}", exchange.code, exchange.dex) != address.exchange_key {
                continue;
            }
            let Some(readers) = snapshot.market_history_readers(market) else {
                continue;
            };
            archive.request(provider, market, &client, epoch);
            let reader = match address.venue.kind {
                MarketKind::Spot => readers.spot_trades,
                MarketKind::Futures | MarketKind::Quarterly => readers.futures_trades,
            };
            let Some(reader) = reader else { continue };
            // Inspect bounds across the retained ring, but copy only the narrow tick window.
            // Total ring length does not spend the replay's copy budget.
            let captured = reader.with_last(reader.capacity(), |view| {
                let mut capture = ReplayCapture::new(window, MAX_CORE_ROWS);
                view.for_each(|row| {
                    capture.push(Tick {
                        time_ms: row.unix_millis() as f64,
                        price: row.price,
                        qty: row.quantity(),
                        side: if row.is_buy() { Side::Buy } else { Side::Sell },
                    })
                });
                capture.finish()
            });
            let Some(candidate) = captured else { continue };
            let covered = candidate.covered;
            if best
                .as_ref()
                .is_some_and(|old| old.covered.1 - old.covered.0 >= covered.1 - covered.0)
            {
                continue;
            }
            best = Some(candidate);
            if covered == (window.from_ms, window.to_ms) {
                break;
            }
        }
        best
    }
}

/// Collect one requested range while retaining the ring's bounds from the same locked read.
struct ReplayCapture {
    window: ReplayWindow,
    budget: usize,
    first: i64,
    last: i64,
    ticks: Vec<Tick>,
    overflow: bool,
}

impl ReplayCapture {
    /// Start with no observed coverage; rows outside the range contribute bounds, not allocations.
    fn new(window: ReplayWindow, budget: usize) -> Self {
        Self {
            window,
            budget,
            first: i64::MAX,
            last: i64::MIN,
            ticks: Vec::new(),
            overflow: false,
        }
    }

    /// Count only requested points against the copy budget, including out-of-order resend rows.
    fn push(&mut self, tick: Tick) {
        if !tick.time_ms.is_finite() || !tick.price.is_finite() || tick.price <= 0.0 {
            return;
        }
        let stamp = tick.time_ms as i64;
        self.first = self.first.min(stamp);
        self.last = self.last.max(stamp);
        if stamp < self.window.from_ms || stamp > self.window.to_ms {
            return;
        }
        if self.ticks.len() == self.budget {
            self.overflow = true;
        } else {
            self.ticks.push(tick);
        }
    }

    /// Refuse a truncated interval; otherwise freeze sorted points and their retained span.
    fn finish(mut self) -> Option<CoreReplayTicks> {
        if self.overflow || self.ticks.is_empty() {
            return None;
        }
        let covered = usable_span(self.first, self.last, self.window)?;
        self.ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
        Some(CoreReplayTicks {
            ticks: self.ticks,
            covered,
        })
    }
}

/// A donor must bracket the whole position before it can replace the exchange tick stage.
fn usable_span(first: i64, last: i64, window: ReplayWindow) -> Option<(i64, i64)> {
    (first <= window.open_ms && last >= window.close_ms && first < last)
        .then_some((first.max(window.from_ms), last.min(window.to_ms)))
}

#[cfg(test)]
mod tests;
