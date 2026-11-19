//! Source lifecycle for clients, providers, resets, and `refresh_market` order-book pulls.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use moonproto::state::OrderBookKind;

use crate::feed::{Level, MarketDirty, MarketDirtyFlags, OrderBook, SharedMoonClient};
use crate::market::SharedMarketStore;
use crate::session::CoreId;

mod ring;

use super::{
    MarketDataSource, MarketDataSourceInner, ORDERBOOK_PULL_PERIOD_MS, bump_generation,
    bump_market_revisions, cadence_phase_ms, cadence_slot, market_diag, market_diag_due,
    market_diag_enabled, market_diag_floor,
};

/// Convert and fold one retained trade row into the recorder's five-minute candles.
fn aggregate_recorder_trade(
    row: &moonproto::state::TradeHistoryRow,
    tf_ms: i64,
    candles: &mut Vec<crate::market::candles::ChartCandle>,
) {
    let time_ms = row.unix_millis();
    let tick = crate::feed::Tick {
        time_ms: time_ms as f64,
        price: row.price,
        qty: row.quantity(),
        side: if row.is_buy() {
            crate::feed::Side::Buy
        } else {
            crate::feed::Side::Sell
        },
    };
    crate::market::candles::aggregate_trade(&tick, tf_ms, candles);
}

impl MarketDataSource {
    /// Create an unrouted source; the coordinator publishes catalog and quote ownership together.
    pub fn new(store: SharedMarketStore) -> Self {
        Self {
            inner: Arc::new(RwLock::new(MarketDataSourceInner {
                store,
                clients: HashMap::new(),
                core_provider: HashMap::new(),
                market_provider_overrides: HashMap::new(),
                provider_orderbook_kind: HashMap::new(),
                cursors: HashMap::new(),
                market_revisions: HashMap::new(),
                provider_generations: HashMap::new(),
                started_at: Instant::now(),
                deep_req_gate: std::sync::Mutex::new(HashMap::new()),
                deep_kind_wants: std::sync::Mutex::new(HashMap::new()),
                candle_subs: std::sync::Mutex::new(HashMap::new()),
                kline_cache: None,
                provider_exchange: HashMap::new(),
                core_venue: HashMap::new(),
                arb_book: Default::default(),
                listing_cache: Default::default(),
                volume_book: Default::default(),
                native_backfill: Default::default(),
                archive: Default::default(),
            })),
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Open the local kline cache in `klines.sqlite`.
    ///
    /// Called once at startup. Charts retain their previous behavior when this optional cache is
    /// not initialized.
    pub fn init_kline_cache(&self, path: std::path::PathBuf) {
        let cache = crate::market::kline_cache::KlineCache::open(path);
        let opened = cache.is_some();
        {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.kline_cache = cache;
        }
        if opened {
            self.spawn_kline_recorder();
        }
    }

    /// Spawn the background 5-minute candle recorder for every provider market.
    ///
    /// Subscribed trades flow continuously, so sealed 5-minute bars are aggregated from trade rings
    /// and stored in the kline cache. The recorder deliberately uses 5 minutes rather than 1 minute:
    /// each merge rewrites a coin's entire daily chunk, and 1-minute data would rewrite roughly N
    /// markets every minute, growing the WAL by megabytes within minutes. Five-minute data writes
    /// five times less often with chunks about five times smaller, roughly 7 KB per coin per day.
    /// History therefore survives restarts and core outages even for coins never opened, while deep
    /// requests provide 1-minute precision for open coins through cache kind 1.
    ///
    /// The same pass keeps the core's own 5-minute ring in the cache where no real candle exists
    /// (`ring` module): the only source for the hours the core ran and the terminal did not.
    fn spawn_kline_recorder(&self) {
        let source = self.clone();
        let _ = std::thread::Builder::new()
            .name("kline-recorder".into())
            .spawn(move || {
                let mut last_written: HashMap<(CoreId, String), i64> = HashMap::new();
                let mut ring_marks: HashMap<(CoreId, String), ring::RingMark> = HashMap::new();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    // Before the recorder pass, not after: a first pass merges a day of buckets
                    // per market and can run for a long while, and the diagnostic line should not
                    // wait on it.
                    if crate::candles_5m_diag::enabled() {
                        source.sweep_candles_5m_diag();
                    }
                    source.record_sealed_5m(&mut last_written, &mut ring_marks);
                }
            });
    }

    /// Elected providers with their kline-cache exchange key, client slot, and whether the
    /// venue's wire `volume` is quote money (`crate::venue::deep_volume_is_quote`, which the
    /// chart's snapshot layer reads for the same ring rows).
    ///
    /// The one traversal both the recorder and the ring-depth channel walk, so the channel reports
    /// exactly the providers whose rings the recorder reads. Called under the caller's read lock.
    fn recorder_providers(
        inner: &MarketDataSourceInner,
    ) -> Vec<(CoreId, String, SharedMoonClient, bool)> {
        inner
            .provider_exchange
            .iter()
            .filter_map(|(p, e)| {
                inner.clients.get(p).map(|c| {
                    (
                        *p,
                        format!("{}:{:08x}", e.code, e.dex),
                        c.clone(),
                        crate::venue::deep_volume_is_quote(e.code),
                    )
                })
            })
            .collect()
    }

    /// Write one `channels.candles_5m` line per provider: how deep its 5-minute candle rings are.
    ///
    /// Reads the client's snapshot only; nothing is sent to the core. Each ring is folded under
    /// its own read lock by [`crate::candles_5m_diag::ring_facts`], and the source lock is held
    /// only for the provider list, never across the scan.
    fn sweep_candles_5m_diag(&self) {
        let providers: Vec<(CoreId, String, SharedMoonClient, String)> = {
            let inner = self.inner.read().expect("market source poisoned");
            Self::recorder_providers(&inner)
                .into_iter()
                .map(|(p, ex, slot, _)| {
                    let venue = inner
                        .core_venue
                        .get(&p)
                        .map(|venue| match venue.resolved() {
                            Some(v) => v.brand.display().to_string(),
                            None => venue.reported.clone(),
                        })
                        .unwrap_or_else(|| "unidentified".to_string());
                    (p, ex, slot, venue)
                })
                .collect()
        };
        for (provider, ex, slot, venue) in providers {
            // Switched off mid-sweep: the writer would discard the rest anyway.
            if !crate::candles_5m_diag::enabled() {
                return;
            }
            let Some(client) = slot.get() else { continue };
            let Some(snap) = client.snapshot_versioned() else {
                continue;
            };
            let mut depth = crate::candles_5m_diag::ProviderDepth::default();
            for handle in snap.markets().iter() {
                let name = handle.name();
                match snap
                    .market_history_readers(name)
                    .and_then(|readers| readers.candles_5m)
                {
                    Some(ring) => depth.add_ring(
                        name,
                        ring.capacity(),
                        crate::candles_5m_diag::ring_facts(&ring),
                    ),
                    None => depth.add_without_ring(),
                }
            }
            crate::candles_5m_diag::record(
                &format!("provider={provider} ({venue} · ex={ex})"),
                &depth,
                crate::util::time::now_unix_ms_i64(),
            );
        }
    }

    /// One recorder pass: every elected provider's markets, their sealed 5-minute buckets folded
    /// from the trade rings into the cache (`last_written` is each market's cursor), and the rows
    /// of the core's 5-minute ring kept where no real candle exists (`ring_marks`, see the `ring`
    /// module). Runs on the recorder's own thread; the cache writes block it, never the UI.
    fn record_sealed_5m(
        &self,
        last_written: &mut HashMap<(CoreId, String), i64>,
        ring_marks: &mut HashMap<(CoreId, String), ring::RingMark>,
    ) {
        const TF: i64 = 300_000;
        /// How far back the FIRST pass for a market reaches into the core's trade ring.
        ///
        /// A day covers the restart gaps actually observed (2 to 10 hours) without asking for
        /// depth no ring holds. The ring's own capacity bounds the read regardless, so a larger
        /// value would mostly buy an emptier copy.
        const FIRST_PASS_BACKFILL_MS: i64 = 24 * 60 * 60_000;
        /// Markets per cache transaction.
        ///
        /// The whole-pass batch is fine when each market carries three buckets and far too large
        /// when a first pass carries a day of them.
        const MERGE_CHUNK_MARKETS: usize = 256;
        // Snapshot providers under a short read lock and perform all work after releasing it.
        let (cache, providers) = {
            let inner = self.inner.read().expect("market source poisoned");
            let Some(cache) = inner.kline_cache.clone() else {
                return;
            };
            (cache, Self::recorder_providers(&inner))
        };
        let now_ms = crate::util::time::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let cur_bucket = (now_ms / TF) * TF;
        let mut candles: Vec<crate::market::candles::ChartCandle> = Vec::new();
        // Accumulate items so a run of markets reaches the cache in ONE transaction instead of one
        // per market. The run is capped at `MERGE_CHUNK_MARKETS` rather than covering the whole
        // pass: a first pass carries a day of buckets per market, and a chart read shares this
        // worker's queue.
        let mut batch: Vec<crate::market::kline_cache::MergeItem> = Vec::new();
        // The ring's rows go through their own batch: they are merged without ever replacing a
        // real candle, which the plain merge above would do.
        let mut ring_batch: Vec<crate::market::kline_cache::MergeItem> = Vec::new();
        let mut ring_fill = crate::market::kline_cache::RangeFill::default();
        let (mut ring_markets, mut ring_rows) = (0usize, 0usize);
        for (provider, ex, slot, volume_is_quote) in providers {
            let Some((client, epoch)) = slot.get_with_epoch() else {
                continue;
            };
            let Some(snap) = client.snapshot_versioned() else {
                continue;
            };
            let markets = snap.markets();
            for handle in markets.iter() {
                let name = handle.name();
                let Some(readers) = snap.market_history_readers(name) else {
                    continue;
                };
                if let Some(ring) = readers.candles_5m.as_ref() {
                    let key = (provider, name.to_string());
                    let (rows, mark) = fresh_ring_rows(
                        ring,
                        ring_marks.get(&key).copied(),
                        epoch,
                        volume_is_quote,
                        cur_bucket,
                    );
                    if let Some(mark) = mark {
                        ring_marks.insert(key, mark);
                    }
                    if !rows.is_empty() {
                        ring_markets += 1;
                        ring_rows += rows.len();
                        ring_batch.push(crate::market::kline_cache::MergeItem {
                            exchange: ex.clone(),
                            market: name.to_string(),
                            kind_min: 5,
                            rows,
                        });
                    }
                    if ring_batch.len() >= MERGE_CHUNK_MARKETS {
                        ring_fill
                            .add(cache.fill_range_batch_blocking(std::mem::take(&mut ring_batch)));
                    }
                }
                let Some(reader) = readers.futures_trades.or(readers.spot_trades) else {
                    continue;
                };
                let key = (provider, name.to_string());
                // The FIRST pass for a market reaches back over the whole retained trade ring,
                // bounded by FIRST_PASS_BACKFILL_MS. `last_written` lives in memory only, so every
                // restart starts a first pass — and the old 15-minute window meant a restart wrote
                // nothing for the hours the process was down, even though the core hands over a
                // ring that already covers them. Measured on a live cache: two coins on the same
                // provider had byte-identical kind-5 holes of 2 to 10 hours, which is exactly the
                // shape of process downtime rather than of quiet markets.
                let first_pass = !last_written.contains_key(&key);
                let from_ms = last_written
                    .get(&key)
                    .map(|t| t + TF)
                    .unwrap_or(cur_bucket - FIRST_PASS_BACKFILL_MS);
                if from_ms >= cur_bucket {
                    continue;
                }
                // Fold borrowed ring rows directly into candles. No full trade-ring or tick copy is
                // owned by this pass; only simple CPU aggregation runs under the ring's read lock.
                candles.clear();
                let cursor =
                    reader.cursor_at_or_after_time(moonproto::MoonTime::from_unix_millis(from_ms));
                let ((trade_count, oldest, _), _) = reader.scan_from_cursor(
                    cursor,
                    reader.capacity(),
                    (0usize, f64::INFINITY, &mut candles),
                    |(count, oldest, candles), row| {
                        let time_ms = row.unix_millis();
                        if time_ms >= from_ms && time_ms < cur_bucket {
                            aggregate_recorder_trade(row, TF, candles);
                            (count + 1, oldest.min(time_ms as f64), candles)
                        } else {
                            (count, oldest, candles)
                        }
                    },
                );
                // Advance the cursor even without trades so quiet coins are not reread forever.
                last_written.insert(key, cur_bucket - TF);
                if trade_count == 0 {
                    continue;
                }
                // Do not write the current unsealed bucket; the next pass will complete it.
                candles.retain(|c| (c.t_open_ms as i64) < cur_bucket);
                if first_pass {
                    // Drop the leading bucket when the ring begins inside it. A merge is
                    // INSERT OR REPLACE per `(kind, day, t_open)`, so a bucket built from the tail
                    // half of its trades would OVERWRITE a complete row cached in an earlier
                    // session — turning a restart into data loss instead of a gap. Only the first
                    // pass needs this: a later pass starts at a bucket boundary the ring already
                    // covers, where a thin bucket means a quiet market and is real.
                    // The MINIMUM timestamp, not the first row: the trade ring is only nearly
                    // time-sorted, because a late UDP resend enters an old bucket without
                    // preserving ring order (`candles::aggregate_trades` says so and handles it).
                    // Reading the front row instead would let one straggler place the boundary too
                    // early, keeping a sparse leading bucket — and since the cursor advances
                    // unconditionally below, no later pass would ever revisit and repair it.
                    if oldest.is_finite() {
                        let sealed_from = crate::market::candles::first_full_bucket_ms(oldest, TF);
                        candles.retain(|c| (c.t_open_ms as i64) >= sealed_from);
                    }
                }
                if !candles.is_empty() {
                    batch.push(crate::market::kline_cache::MergeItem {
                        exchange: ex.clone(),
                        market: name.to_string(),
                        kind_min: 5,
                        rows: std::mem::take(&mut candles),
                    });
                }
                // Flush in chunks rather than accumulating the whole pass. One transaction over
                // thousands of markets was affordable at 15 minutes of rows each; a first pass at
                // a day each is not, and the WAL budget in this module's docstring is sized
                // against the small case.
                if batch.len() >= MERGE_CHUNK_MARKETS {
                    // Blocking, so at most ONE chunk is ever queued ahead of a chart read. The
                    // cache has a single worker and a chart read gives up after 250 ms, so queueing
                    // every chunk at once would park the reads behind all of them — during exactly
                    // the cold start when charts are being opened.
                    cache.merge_batch_blocking(std::mem::take(&mut batch));
                }
            }
        }
        cache.merge_batch_blocking(batch);
        ring_fill.add(cache.fill_range_batch_blocking(ring_batch));
        crate::candles_5m_diag::record_ring_fill(ring_markets, ring_rows, ring_fill);
    }

    /// Set stable provider exchange identities used as kline-cache keys.
    ///
    /// The exchange identity lets cores on one exchange share the cache and keeps the cache address
    /// stable when provider election changes. `CoreId` itself is a stable uid since schema v11, but
    /// identifies one core. The coordinator calls this during reconciliation.
    pub fn set_provider_exchanges(
        &self,
        map: &HashMap<crate::session::CoreId, crate::feed::ExchangeId>,
    ) {
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.provider_exchange = map.clone();
    }

    pub fn store(&self) -> SharedMarketStore {
        self.inner
            .read()
            .expect("market source poisoned")
            .store
            .clone()
    }

    pub fn set_client(&self, core: CoreId, client: SharedMoonClient) {
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.clients.insert(core, client);
        inner.cursors.retain(|(provider, _), _| *provider != core);
        // A respawn hands over a whole new client SLOT, whose epoch starts from zero again — so the
        // archive state left by the old slot would collide with the new slot's first epoch and read
        // as "already asked" for a client that has never been asked. Epochs only order installs
        // WITHIN one slot; replacing the slot has to drop them.
        inner.archive.forget_provider(core);
        inner.native_backfill.forget_provider(core);
        inner
            .candle_subs
            .lock()
            .expect("candle subs poisoned")
            .remove(&core);
        inner
            .deep_req_gate
            .lock()
            .expect("deep req gate poisoned")
            .retain(|(provider, _, _), _| *provider != core);
        inner
            .deep_kind_wants
            .lock()
            .expect("deep kind wants poisoned")
            .remove(&core);
        // A new client slot is a new market universe: its arbitrage quotes, and the answers about
        // which market carries which coin on it, are about the slot that just went away.
        inner
            .arb_book
            .lock()
            .expect("arb book poisoned")
            .forget_core(core);
        // The listing book was read from those same slots, and the new client starts with none:
        // its warm-up has to start over, and the kept book has to be rebuilt.
        inner
            .listing_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .forget_core(core);
        // The traded amounts are about that same retained history, and it went away with the slot.
        inner
            .volume_book
            .lock()
            .expect("volume book poisoned")
            .forget_core(core);
        bump_generation(&mut inner.provider_generations, core);
    }

    /// Remove the client for a core whose server was removed from configuration.
    ///
    /// Also discard that core's cursors, revisions, order-book kind, and provider mapping. Existing
    /// provider views in `SharedMarketStore` remain until the separate `drop_provider` lifecycle
    /// operation removes them.
    pub fn remove_client(&self, core: CoreId) {
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.clients.remove(&core);
        inner.cursors.retain(|(provider, _), _| *provider != core);
        inner.market_revisions.remove(&core);
        inner.provider_orderbook_kind.remove(&core);
        inner.core_provider.remove(&core);
        inner.market_provider_overrides.remove(&core);
        for overrides in inner.market_provider_overrides.values_mut() {
            overrides.retain(|_, provider| *provider != core);
        }
        inner.core_venue.remove(&core);
        inner.archive.forget_provider(core);
        inner.native_backfill.forget_provider(core);
        inner
            .candle_subs
            .lock()
            .expect("candle subs poisoned")
            .remove(&core);
        inner
            .deep_req_gate
            .lock()
            .expect("deep req gate poisoned")
            .retain(|(provider, _, _), _| *provider != core);
        inner
            .deep_kind_wants
            .lock()
            .expect("deep kind wants poisoned")
            .remove(&core);
        inner
            .arb_book
            .lock()
            .expect("arb book poisoned")
            .forget_core(core);
        inner
            .listing_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .forget_core(core);
        // The traded amounts are about that same retained history, and it went away with the slot.
        inner
            .volume_book
            .lock()
            .expect("volume book poisoned")
            .forget_core(core);
        bump_generation(&mut inner.provider_generations, core);
    }

    /// Publish what each core is connected to, for the reads that need a venue rather than a
    /// provider.
    ///
    /// Separate from `set_provider_exchanges` beside it, which covers only elected providers: the
    /// arbitrage column asks about the venue of the core a PANE sits on. Cheap and called on the
    /// same reconciliation, so the map cannot lag an identity that just arrived.
    pub fn set_core_venues(&self, venues: &HashMap<CoreId, crate::venue::CoreVenue>) {
        let mut inner = self.inner.write().expect("market source poisoned");
        if inner.core_venue == *venues {
            return;
        }
        inner.core_venue = venues.clone();
    }

    /// Replace live provider ownership while retaining replay archive gates for connected cores.
    pub fn set_provider_map(&self, core_provider: &HashMap<CoreId, CoreId>) {
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.core_provider = core_provider.clone();

        let active_providers: HashSet<CoreId> = inner.core_provider.values().copied().collect();
        inner
            .cursors
            .retain(|(provider, _), _| active_providers.contains(provider));
        inner
            .market_revisions
            .retain(|provider, _| active_providers.contains(provider));
        inner
            .provider_orderbook_kind
            .retain(|provider, _| active_providers.contains(provider));
        // Replays also read archives from connected cores that are not the elected live source.
        // Keep the once-per-client gate until disconnect, not merely until a provider election.
        let connected_cores = inner.clients.keys().copied().collect();
        inner.archive.retain_providers(&connected_cores);
        inner.native_backfill.retain_providers(&active_providers);
    }

    /// Publish quote routing and report changes even when catalog ownership stays the same.
    pub fn set_market_provider_overrides(
        &self,
        overrides: &HashMap<CoreId, HashMap<Box<str>, CoreId>>,
    ) -> bool {
        let mut inner = self.inner.write().expect("market source poisoned");
        if inner.market_provider_overrides == *overrides {
            return false;
        }
        inner.market_provider_overrides = overrides.clone();
        true
    }

    pub fn set_orderbook_kind(&self, core: CoreId, kind: OrderBookKind) {
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.provider_orderbook_kind.insert(core, kind);
    }

    /// Invalidate shared state for `reset_market` and `drop_market`.
    ///
    /// Removes the history cursor, increments every market revision, and returns the store for the
    /// final operation.
    fn invalidate_market(&self, provider: CoreId, market: &str) -> SharedMarketStore {
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.cursors.remove(&(provider, market.to_string()));
        // The archive claim deliberately SURVIVES this. Invalidating a market here resets the
        // terminal's own view and cursors; it does not touch MoonProto's retained rings, which keep
        // the merged archive as long as the market is in the catalog. Forgetting the claim here also
        // raced the chart: `set_open` calls `reset_market` when a market becomes wanted, which lands
        // just AFTER the pane's first history read has already asked, and the erased claim made the
        // next pass ask a second time. The claim belongs to the client, so only a new client
        // (`set_client`) or a departed provider clears it.
        bump_market_revisions(
            &mut inner.market_revisions,
            provider,
            market,
            MarketDirtyFlags::ALL,
        );
        inner.store.clone()
    }

    pub fn reset_market(&self, provider: CoreId, market: &str) {
        let store = self.invalidate_market(provider, market);
        market_diag(format!("reset_market provider={provider} market={market}"));
        store
            .write()
            .expect("market store poisoned")
            .reset(provider, market);
    }

    /// Release one chart's live candle subscription and all of its deep-request gate entries.
    fn release_chart_history(&self, provider: CoreId, market: &str) {
        let (client, subscription) = {
            let inner = self.inner.read().expect("market source poisoned");
            let client = inner.clients.get(&provider).and_then(SharedMoonClient::get);
            let mut subs = inner.candle_subs.lock().expect("candle subs poisoned");
            let subscription = subs
                .get_mut(&provider)
                .and_then(|markets| markets.remove(market));
            if subs
                .get(&provider)
                .is_some_and(|markets| markets.is_empty())
            {
                subs.remove(&provider);
            }
            super::history::forget_deep_requests(
                &mut inner.deep_req_gate.lock().expect("deep req gate poisoned"),
                provider,
                market,
            );
            (client, subscription)
        };
        if let (Some(client), Some(subscription)) = (client, subscription) {
            if subscription.subscribed {
                let _ = client.streams().unsubscribe_candles([market]);
            }
        }
    }

    pub fn drop_market(&self, provider: CoreId, market: &str) {
        self.release_chart_history(provider, market);
        let store = self.invalidate_market(provider, market);
        // Symmetric with `reset_market`: this was the only market-lifecycle path that invalidated
        // silently, which made it the hardest one to rule out when reading a log.
        market_diag(format!("drop_market provider={provider} market={market}"));
        store
            .write()
            .expect("market store poisoned")
            .drop_market(provider, market);
    }

    pub fn drop_provider(&self, provider: CoreId) {
        let (client, subscriptions) = {
            let inner = self.inner.read().expect("market source poisoned");
            let client = inner.clients.get(&provider).and_then(SharedMoonClient::get);
            let subscriptions = inner
                .candle_subs
                .lock()
                .expect("candle subs poisoned")
                .remove(&provider)
                .unwrap_or_default();
            inner
                .deep_req_gate
                .lock()
                .expect("deep req gate poisoned")
                .retain(|(p, _, _), _| *p != provider);
            inner
                .deep_kind_wants
                .lock()
                .expect("deep kind wants poisoned")
                .remove(&provider);
            (client, subscriptions)
        };
        if let Some(client) = client {
            for (market, state) in subscriptions {
                if state.subscribed {
                    let _ = client.streams().unsubscribe_candles([market.as_str()]);
                }
            }
        }
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.cursors.retain(|(p, _), _| *p != provider);
            bump_generation(&mut inner.provider_generations, provider);
            inner.provider_orderbook_kind.remove(&provider);
            inner.market_revisions.remove(&provider);
            inner.archive.forget_provider(provider);
            inner.native_backfill.forget_provider(provider);
            inner.store.clone()
        };
        store
            .write()
            .expect("market store poisoned")
            .drop_provider(provider);
    }

    /// Clear catalog and quote routing together when the source mode changes.
    pub fn clear(&self) {
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.core_provider.clear();
            inner.market_provider_overrides.clear();
            inner.cursors.clear();
            inner.market_revisions.clear();
            inner.provider_generations.clear();
            inner.provider_orderbook_kind.clear();
            inner.archive.clear();
            inner.native_backfill.clear();
            inner
                .candle_subs
                .lock()
                .expect("candle subs poisoned")
                .clear();
            inner
                .deep_req_gate
                .lock()
                .expect("deep req gate poisoned")
                .clear();
            inner
                .deep_kind_wants
                .lock()
                .expect("deep kind wants poisoned")
                .clear();
            inner.store.clone()
        };
        store.write().expect("market store poisoned").clear();
    }

    pub fn mark_dirty(&self, provider: CoreId, dirty: &[MarketDirty]) {
        if dirty.is_empty() {
            return;
        }
        let mut inner = self.inner.write().expect("market source poisoned");
        for item in dirty {
            bump_market_revisions(
                &mut inner.market_revisions,
                provider,
                &item.market,
                item.flags,
            );
        }
    }

    /// `core`'s feed saw the core answer its chart-archive request for `market` — see
    /// [`super::archive::ArchiveGate::answered`]. Any core, not only an elected provider: the tape
    /// stage asks every connected core of the venue.
    pub fn archive_answered(&self, core: CoreId, market: &str, epoch: u64) {
        let archive = self
            .inner
            .read()
            .expect("market source poisoned")
            .archive
            .clone();
        archive.answered(core, market, epoch);
    }

    /// Pull this market's book from its routed tape, refusing an ownership change mid-read.
    pub fn refresh_market(&self, core: CoreId, market: &str) -> bool {
        let (provider, client, store, elapsed_ms, orderbook_kind) = {
            let inner = self.inner.read().expect("market source poisoned");
            let Some(provider) = inner.market_provider(core, market) else {
                if market_diag_enabled()
                    && market_diag_due(format!("no-provider:{core}:{market}"), market_diag_floor())
                {
                    market_diag(format!(
                        "refresh core={} market={market}: no provider",
                        crate::feed::core_label(core)
                    ));
                }
                return false;
            };
            let Some(client) = inner.clients.get(&provider).and_then(SharedMoonClient::get) else {
                if market_diag_enabled()
                    && market_diag_due(
                        format!("no-client:{provider}:{market}"),
                        market_diag_floor(),
                    )
                {
                    market_diag(format!(
                        "refresh core={} provider={} market={market}: no client",
                        crate::feed::core_label(core),
                        crate::feed::core_label(provider)
                    ));
                }
                return false;
            };
            (
                provider,
                client,
                inner.store.clone(),
                inner.started_at.elapsed().as_millis() as u64,
                inner
                    .provider_orderbook_kind
                    .get(&provider)
                    .copied()
                    .unwrap_or(OrderBookKind::Futures),
            )
        };

        let Some(snapshot) = client.snapshot_versioned() else {
            if market_diag_enabled()
                && market_diag_due(
                    format!("no-snapshot:{provider}:{market}"),
                    market_diag_floor(),
                )
            {
                market_diag(format!(
                    "refresh core={} provider={} market={market}: no snapshot",
                    crate::feed::core_label(core),
                    crate::feed::core_label(provider)
                ));
            }
            return false;
        };

        if store
            .read()
            .expect("market store poisoned")
            .view(provider, market)
            .is_none()
        {
            if market_diag_enabled()
                && market_diag_due(format!("no-view:{provider}:{market}"), market_diag_floor())
            {
                let price_known = snapshot.markets().price(market).is_some();
                market_diag(format!(
                    "refresh core={} provider={} market={market}: no store view \
                     kind={orderbook_kind:?} price_known={price_known} (cursor not advanced)",
                    crate::feed::core_label(core),
                    crate::feed::core_label(provider),
                ));
            }
            return false;
        }

        let key = (provider, market.to_string());
        let mut book_update: Option<OrderBook> = None;
        let mut has_book_snapshot = false;
        // Record which kind actually supplied the snapshot for diagnosing Hyperliquid/HIP-3 and
        // spot/futures mismatches between the core and engine. None means neither kind had a
        // snapshot, pointing to name resolution rather than kind selection.
        let mut book_kind_used: Option<OrderBookKind> = None;
        let book_dirty_revision: u64;
        let book_due: bool;

        {
            let mut inner = self.inner.write().expect("market source poisoned");
            if inner.market_provider(core, market) != Some(provider) {
                return false;
            }
            book_dirty_revision = inner
                .market_revisions
                .get(&provider)
                .and_then(|markets| markets.get(market))
                .map(|revs| revs.book)
                .unwrap_or(0);
            let cursor = inner.cursors.entry(key).or_default();

            let phase_ms = *cursor.book_phase_ms.get_or_insert_with(|| {
                cadence_phase_ms(provider, market, ORDERBOOK_PULL_PERIOD_MS)
            });
            let book_slot = cadence_slot(elapsed_ms, phase_ms, ORDERBOOK_PULL_PERIOD_MS);
            let book_dirty = cursor.last_book_dirty_revision != book_dirty_revision;
            book_due =
                book_dirty || book_slot.is_some_and(|slot| cursor.last_book_slot != Some(slot));
            if book_due {
                // Apply a compatibility fallback by kind. The engine tags the wire order book with
                // book_kind, where 0 is Futures and 1 is Spot, but the terminal's core classification
                // does not always match. The gs/bgs spot cores send their books as Futures, and
                // Hyperliquid HIP-3 perpetuals with prefixes such as `xyz:` or deployer codes may
                // also differ from the expected kind. Lookup uses `(market_index, kind)`, while one
                // market normally populates exactly one kind, so try the expected kind and then its
                // opposite. Correctly classified cores stop after the first successful lookup. If
                // both return None, name resolution rather than kind selection is the issue; see the
                // diagnostics below.
                let other_kind = match orderbook_kind {
                    OrderBookKind::Spot => OrderBookKind::Futures,
                    _ => OrderBookKind::Spot,
                };
                let book = snapshot
                    .order_book(market, orderbook_kind)
                    .map(|b| (orderbook_kind, b))
                    .or_else(|| {
                        snapshot
                            .order_book(market, other_kind)
                            .map(|b| (other_kind, b))
                    });
                if let Some((used_kind, book)) = book {
                    has_book_snapshot = true;
                    book_kind_used = Some(used_kind);
                    let revision = book.revision();
                    if cursor.last_book_revision != Some(revision) {
                        cursor.last_book_revision = Some(revision);
                        book_update = Some(OrderBook {
                            bids: book
                                .buys
                                .iter()
                                .map(|l| Level {
                                    price: l.rate as f32,
                                    qty: l.quantity as f32,
                                })
                                .collect(),
                            asks: book
                                .sells
                                .iter()
                                .map(|l| Level {
                                    price: l.rate as f32,
                                    qty: l.quantity as f32,
                                })
                                .collect(),
                        });
                    }
                }
                cursor.last_book_slot = book_slot;
                cursor.last_book_dirty_revision = book_dirty_revision;
            }
        }

        let mut store = store.write().expect("market store poisoned");
        if store.view(provider, market).is_none() {
            if market_diag_enabled()
                && market_diag_due(format!("no-view:{provider}:{market}"), market_diag_floor())
            {
                let price_known = snapshot.markets().price(market).is_some();
                market_diag(format!(
                    "refresh core={} provider={} market={market}: no store view \
                     kind={orderbook_kind:?} used_kind={book_kind_used:?} \
                     price_known={price_known} book_dirty_rev={book_dirty_revision} \
                     book_due={book_due} snapshot_book={has_book_snapshot} pulled_book={:?}",
                    crate::feed::core_label(core),
                    crate::feed::core_label(provider),
                    book_update.as_ref().map(|b| (b.bids.len(), b.asks.len()))
                ));
            }
            return false;
        }

        let pulled_book_shape = book_update.as_ref().map(|b| (b.bids.len(), b.asks.len()));
        let mut changed = false;
        if let Some(book) = book_update {
            store.apply_book(provider, market, &book);
            changed = true;
        }
        if market_diag_enabled()
            && market_diag_due(format!("refresh:{provider}:{market}"), market_diag_floor())
        {
            let book_len = store
                .view(provider, market)
                .map(|v| v.book.len())
                .unwrap_or(0);
            let price_known = snapshot.markets().price(market).is_some();
            market_diag(format!(
                "refresh core={} provider={} market={market}: changed={changed} \
                 kind={orderbook_kind:?} used_kind={book_kind_used:?} price_known={price_known} \
                 book_dirty_rev={book_dirty_revision} book_due={book_due} \
                 snapshot_book={has_book_snapshot} pulled_book={pulled_book_shape:?} \
                 view_book_len={book_len}",
                crate::feed::core_label(core),
                crate::feed::core_label(provider),
            ));
        }
        changed
    }
}

/// The rows of one market's 5-minute ring a recorder pass offers the cache, and the mark to keep
/// for the next pass (`None` when the ring holds no usable row, so the previous mark stands).
///
/// Two walks over the ring under ONE read lock, neither copying the ring: the first finds its
/// oldest and newest usable stamps, which decide the plan (`ring::plan`), the second converts only
/// the rows the plan takes — normally the one sealed since the last pass.
fn fresh_ring_rows(
    ring: &moonproto::state::SeqRingReader<moonproto::state::Candle5mRow>,
    prev: Option<ring::RingMark>,
    epoch: u64,
    volume_is_quote: bool,
    cur_bucket_ms: i64,
) -> (
    Vec<crate::market::candles::ChartCandle>,
    Option<ring::RingMark>,
) {
    ring.with_last(ring.capacity(), |view| {
        let (mut oldest, mut newest) = (i64::MAX, i64::MIN);
        view.for_each(|c| {
            let t = c.time().unix_millis();
            if ring::usable_stamp(t, cur_bucket_ms) {
                oldest = oldest.min(t);
                newest = newest.max(t);
            }
        });
        if oldest > newest {
            return (Vec::new(), None);
        }
        let plan = ring::plan(prev, epoch, oldest);
        let mut rows = Vec::new();
        view.for_each(|c| {
            let stamp_ms = c.time().unix_millis();
            if !plan.takes(stamp_ms) {
                return;
            }
            let wire = ring::WireRow {
                stamp_ms,
                open: c.open(),
                high: c.high(),
                low: c.low(),
                close: c.close(),
                volume: c.volume(),
            };
            rows.extend(ring::ring_candle(wire, volume_is_quote, cur_bucket_ms));
        });
        (
            rows,
            Some(ring::RingMark {
                epoch,
                oldest_ms: oldest,
                newest_ms: newest,
            }),
        )
    })
}

#[cfg(test)]
mod tests;
