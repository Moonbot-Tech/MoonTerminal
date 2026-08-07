//! Source lifecycle for clients, providers, resets, and `refresh_market` order-book pulls.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use moonproto::state::OrderBookKind;

use crate::feed::{Level, MarketDirty, MarketDirtyFlags, OrderBook, SharedMoonClient};
use crate::market::SharedMarketStore;
use crate::session::CoreId;

use super::{
    bump_generation, bump_market_revisions, cadence_phase_ms, cadence_slot, market_diag,
    market_diag_due, market_diag_enabled, MarketDataSource, MarketDataSourceInner,
    MARKET_DIAG_FLOOR, ORDERBOOK_PULL_PERIOD_MS,
};

impl MarketDataSource {
    pub fn new(store: SharedMarketStore) -> Self {
        Self {
            inner: Arc::new(RwLock::new(MarketDataSourceInner {
                store,
                clients: HashMap::new(),
                core_provider: HashMap::new(),
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
                native_backfill_done: std::sync::Mutex::new(HashSet::new()),
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
    fn spawn_kline_recorder(&self) {
        let source = self.clone();
        let _ = std::thread::Builder::new()
            .name("kline-recorder".into())
            .spawn(move || {
                let mut last_written: HashMap<(CoreId, String), i64> = HashMap::new();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    source.record_sealed_5m(&mut last_written);
                }
            });
    }

    fn record_sealed_5m(&self, last_written: &mut HashMap<(CoreId, String), i64>) {
        const TF: i64 = 300_000;
        // Snapshot providers under a short read lock and perform all work after releasing it.
        let (cache, providers) = {
            let inner = self.inner.read().expect("market source poisoned");
            let Some(cache) = inner.kline_cache.clone() else {
                return;
            };
            let providers: Vec<(CoreId, String, SharedMoonClient)> = inner
                .provider_exchange
                .iter()
                .filter_map(|(p, e)| {
                    inner
                        .clients
                        .get(p)
                        .map(|c| (*p, format!("{}:{:08x}", e.code, e.dex), c.clone()))
                })
                .collect();
            (cache, providers)
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let cur_bucket = (now_ms / TF) * TF;
        let mut ticks: Vec<crate::feed::Tick> = Vec::new();
        let mut candles: Vec<crate::market::candles::ChartCandle> = Vec::new();
        // Accumulate items so the entire pass across thousands of markets reaches the cache in one
        // transaction.
        let mut batch: Vec<crate::market::kline_cache::MergeItem> = Vec::new();
        for (provider, ex, slot) in providers {
            let Some(client) = slot.get() else { continue };
            let Some(snap) = client.snapshot_versioned() else {
                continue;
            };
            let markets = snap.markets();
            for handle in markets.iter() {
                let name = handle.name();
                let Some(readers) = snap.market_history_readers(name) else {
                    continue;
                };
                let Some(reader) = readers.futures_trades.or(readers.spot_trades) else {
                    continue;
                };
                let key = (provider, name.to_string());
                // Start at the bucket after the last written one; the first pass covers 15 minutes.
                let from_ms = last_written
                    .get(&key)
                    .map(|t| t + TF)
                    .unwrap_or(cur_bucket - 3 * TF);
                if from_ms >= cur_bucket {
                    continue;
                }
                let mut trade_rows = Vec::new();
                reader.copy_time_range(
                    moonproto::MoonTime::from_unix_millis(from_ms),
                    moonproto::MoonTime::from_unix_millis(cur_bucket - 1),
                    reader.capacity(),
                    &mut trade_rows,
                );
                // Advance the cursor even without trades so quiet coins are not reread forever.
                last_written.insert(key, cur_bucket - TF);
                if trade_rows.is_empty() {
                    continue;
                }
                super::rows_to_ticks(&trade_rows, &mut ticks);
                crate::market::candles::aggregate_trades(&ticks, TF, &mut candles);
                // Do not write the current unsealed bucket; the next pass will complete it.
                candles.retain(|c| (c.t_open_ms as i64) < cur_bucket);
                if !candles.is_empty() {
                    batch.push(crate::market::kline_cache::MergeItem {
                        exchange: ex.clone(),
                        market: name.to_string(),
                        kind_min: 5,
                        rows: std::mem::take(&mut candles),
                    });
                }
            }
        }
        cache.merge_batch(batch);
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
        inner.archive.forget_provider(core);
        bump_generation(&mut inner.provider_generations, core);
    }

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
        inner.archive.retain_providers(&active_providers);
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
        // Drop the archive claim too. `MarketDirtyFlags::ALL` below deliberately excludes the
        // archive bit, so nothing else here would, and a market that comes back would keep saying
        // "already asked" over rings that no longer hold the merged history.
        inner.archive.forget_market(provider, market);
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

    pub fn drop_market(&self, provider: CoreId, market: &str) {
        let store = self.invalidate_market(provider, market);
        store
            .write()
            .expect("market store poisoned")
            .drop_market(provider, market);
    }

    pub fn drop_provider(&self, provider: CoreId) {
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.cursors.retain(|(p, _), _| *p != provider);
            bump_generation(&mut inner.provider_generations, provider);
            inner.provider_orderbook_kind.remove(&provider);
            inner.market_revisions.remove(&provider);
            inner.archive.forget_provider(provider);
            inner.store.clone()
        };
        store
            .write()
            .expect("market store poisoned")
            .drop_provider(provider);
    }

    pub fn clear(&self) {
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.core_provider.clear();
            inner.cursors.clear();
            inner.market_revisions.clear();
            inner.provider_generations.clear();
            inner.provider_orderbook_kind.clear();
            inner.archive.clear();
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

    pub fn refresh_market(&self, core: CoreId, market: &str) -> bool {
        let (provider, client, store, elapsed_ms, orderbook_kind) = {
            let inner = self.inner.read().expect("market source poisoned");
            let Some(provider) = inner.core_provider.get(&core).copied() else {
                if market_diag_enabled()
                    && market_diag_due(format!("no-provider:{core}:{market}"), MARKET_DIAG_FLOOR)
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
                    && market_diag_due(format!("no-client:{provider}:{market}"), MARKET_DIAG_FLOOR)
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
                    MARKET_DIAG_FLOOR,
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
            if inner.core_provider.get(&core).copied() != Some(provider) {
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
                && market_diag_due(format!("no-view:{provider}:{market}"), MARKET_DIAG_FLOOR)
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
            && market_diag_due(format!("refresh:{provider}:{market}"), MARKET_DIAG_FLOOR)
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
