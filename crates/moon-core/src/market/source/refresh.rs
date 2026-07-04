//! Жизненный цикл источника (клиенты/провайдеры/сбросы) + pull стакана `refresh_market`.

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
            })),
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
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
        bump_generation(&mut inner.provider_generations, core);
    }

    /// Убрать клиента удалённого ядра (сервер исключён из конфига). Курсоры/ревизии,
    /// где оно было провайдером, тоже снимаем, чтобы не держать мёртвые рынки.
    pub fn remove_client(&self, core: CoreId) {
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.clients.remove(&core);
        inner.cursors.retain(|(provider, _), _| *provider != core);
        inner.market_revisions.remove(&core);
        inner.provider_orderbook_kind.remove(&core);
        inner.core_provider.remove(&core);
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
    }

    pub fn set_orderbook_kind(&self, core: CoreId, kind: OrderBookKind) {
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.provider_orderbook_kind.insert(core, kind);
    }

    pub fn reset_market(&self, provider: CoreId, market: &str) {
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.cursors.remove(&(provider, market.to_string()));
            bump_market_revisions(
                &mut inner.market_revisions,
                provider,
                market,
                MarketDirtyFlags::ALL,
            );
            inner.store.clone()
        };
        market_diag(format!("reset_market provider={provider} market={market}"));
        store
            .write()
            .expect("market store poisoned")
            .reset(provider, market);
    }

    pub fn drop_market(&self, provider: CoreId, market: &str) {
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.cursors.remove(&(provider, market.to_string()));
            bump_market_revisions(
                &mut inner.market_revisions,
                provider,
                market,
                MarketDirtyFlags::ALL,
            );
            inner.store.clone()
        };
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
                    market_diag(format!("refresh core={core} market={market}: no provider"));
                }
                return false;
            };
            let Some(client) = inner.clients.get(&provider).and_then(SharedMoonClient::get) else {
                if market_diag_enabled()
                    && market_diag_due(format!("no-client:{provider}:{market}"), MARKET_DIAG_FLOOR)
                {
                    market_diag(format!(
                        "refresh core={core} provider={provider} market={market}: no client"
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
                    "refresh core={core} provider={provider} market={market}: no snapshot"
                ));
            }
            return false;
        };

        let key = (provider, market.to_string());
        let mut book_update: Option<OrderBook> = None;
        let mut has_book_snapshot = false;
        // Какой kind фактически отдал снимок (для диагностики Hyperliquid/HIP-3 и
        // spot/futures-расхождений между ядром и движком). None — снимка нет ни под
        // одним kind (тогда вопрос в резолве имени, а не в kind).
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
                // Compat fallback по kind. Движок штампует стакан на проводе флагом
                // book_kind (0=Futures/1=Spot), и классификация ядра в терминале не
                // всегда совпадает: spot-ядра gs/bgs шлют книгу как Futures; перпы
                // Hyperliquid HIP-3 (префикс «xyz:…», deployer-коды) тоже могут не
                // совпасть с ожидаемым kind. Lookup идёт по (market_index, kind), а у
                // одного рынка реально заполнен ровно один kind — поэтому пробуем
                // ожидаемый, затем противоположный. На корректных ядрах противоположный
                // запрос не делается (первый уже Some). Если оба None — дело не в kind,
                // а в резолве имени (см. диагностику ниже).
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
                    "refresh core={core} provider={provider} market={market}: no store view \
                     kind={orderbook_kind:?} used_kind={book_kind_used:?} \
                     price_known={price_known} book_dirty_rev={book_dirty_revision} \
                     book_due={book_due} snapshot_book={has_book_snapshot} pulled_book={:?}",
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
                "refresh core={core} provider={provider} market={market}: changed={changed} \
                 kind={orderbook_kind:?} used_kind={book_kind_used:?} price_known={price_known} \
                 book_dirty_rev={book_dirty_revision} book_due={book_due} \
                 snapshot_book={has_book_snapshot} pulled_book={pulled_book_shape:?} \
                 view_book_len={book_len}",
            ));
        }
        changed
    }
}
