//! Frozen replay reads from matching cores, without advancing any live chart cursor.

use super::{MarketDataSource, ReplayAddress};
use crate::feed::{Side, Tick};
use crate::market::trade_replay::ReplayWindow;

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
    /// Called only on the replay worker. Identity includes the platform and DEX discriminator,
    /// so a spot core never donates to a futures replay of the same name; within one donor the
    /// ring is whichever the client filled. Missing or late archives fall through immediately
    /// to REST, whose candle stage gives the archive time to arrive.
    pub(crate) fn replay_core_ticks(
        &self,
        address: &ReplayAddress,
        market: &str,
        window: ReplayWindow,
    ) -> Option<CoreReplayTicks> {
        self.core_ticks(address, market, window, CoreSpanRule::BracketPosition)
    }

    /// Copy whatever a matching core's ring holds inside `[from_ms, to_ms]` — the capture a
    /// trade's close files while the ring still holds it.
    ///
    /// Unlike [`Self::replay_core_ticks`], the ring need not bracket anything: it is contiguous,
    /// so the stretch it holds inside the span is exhaustive whatever its edges, and the copy is
    /// clipped to that stretch. A ring lagging behind the span's end covers up to its own last
    /// print; the rest is another pass's or the venue's.
    ///
    /// Args:
    ///     address: The core's exchange addressing.
    ///     market: Exchange-native market name.
    ///     from_ms: Left edge, inclusive.
    ///     to_ms: Right edge, inclusive.
    pub(crate) fn capture_core_span(
        &self,
        address: &ReplayAddress,
        market: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Option<CoreReplayTicks> {
        let window = ReplayWindow {
            from_ms,
            to_ms,
            open_ms: from_ms,
            close_ms: to_ms,
            margin_ms: 0,
            long_position_ms: crate::market::trade_replay::long_position_ms(),
            over_budget: false,
        };
        self.core_ticks(address, market, window, CoreSpanRule::Overlap)
    }

    fn core_ticks(
        &self,
        address: &ReplayAddress,
        market: &str,
        window: ReplayWindow,
        rule: CoreSpanRule,
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
            // Whichever ring this client filled, as every other reader of the rings takes it
            // (`history.rs`, `read.rs`, `volume.rs`): the donor is ONE client of ONE venue, so
            // it holds this market's prints under one kind only, and picking strictly by the
            // venue's kind left every Hyperliquid spot market — filed by moonproto under
            // `futures_trades` — with no donor at all, in the replay and in the close-time
            // capture alike.
            let Some(reader) = readers.futures_trades.or(readers.spot_trades) else {
                continue;
            };
            // Inspect bounds across the retained ring, but copy only the narrow tick window.
            // Total ring length does not spend the replay's copy budget.
            let captured = reader.with_last(reader.capacity(), |view| {
                let mut capture = ReplayCapture::new(window, MAX_CORE_ROWS, rule);
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

/// What a ring must hold before its copy of a window counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoreSpanRule {
    /// The whole position, `open_ms..close_ms`: the rule for REPLACING a venue tick stage,
    /// where a ring holding half the trade must not win over a route that holds it all.
    BracketPosition,
    /// Any overlap with `from_ms..to_ms`: the rule for a CAPTURE, which keeps whatever the ring
    /// holds and leaves the rest to a later pass.
    Overlap,
}

/// Collect one requested range while retaining the ring's bounds from the same locked read.
struct ReplayCapture {
    window: ReplayWindow,
    budget: usize,
    rule: CoreSpanRule,
    first: i64,
    last: i64,
    ticks: Vec<Tick>,
    overflow: bool,
}

impl ReplayCapture {
    /// Start with no observed coverage; rows outside the range contribute bounds, not allocations.
    fn new(window: ReplayWindow, budget: usize, rule: CoreSpanRule) -> Self {
        Self {
            window,
            budget,
            rule,
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
        let covered = match self.rule {
            CoreSpanRule::BracketPosition => usable_span(self.first, self.last, self.window),
            CoreSpanRule::Overlap => overlap_span(self.first, self.last, self.window),
        };
        let Some(covered) = covered else {
            // The one fact a refused capture needs on record: where the ring's bounds sat
            // against the ask, or "the ring holds this market but nothing near the trade"
            // cannot be told apart from "the ring's stamps are on another clock".
            log::debug!(
                "[x] core archive refused {:?}: ring {}..{} vs ask {}..{} (position {}..{}), {} rows inside",
                self.rule,
                self.first,
                self.last,
                self.window.from_ms,
                self.window.to_ms,
                self.window.open_ms,
                self.window.close_ms,
                self.ticks.len()
            );
            return None;
        };
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

/// Whatever contiguous stretch of the ring falls inside the window, for a capture.
fn overlap_span(first: i64, last: i64, window: ReplayWindow) -> Option<(i64, i64)> {
    (first <= window.to_ms && last >= window.from_ms && first < last)
        .then_some((first.max(window.from_ms), last.min(window.to_ms)))
}

#[cfg(test)]
mod tests;
