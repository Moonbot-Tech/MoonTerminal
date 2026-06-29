use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use moonproto::state::{
    LastPricePoint, MarkPricePoint, OrderBookKind, SeqRingCursor, SeqRingReader, SeqRingTimedRow,
    TradeHistoryRow,
};
use moonproto::MoonTime;

use crate::data::OrderBookModel;
use crate::feed::{
    Level, MarketDirty, MarketDirtyFlags, OrderBook, PricePoint, SharedMoonClient, Side, Tick,
};
use crate::session::CoreId;

use super::SharedMarketStore;

const ORDERBOOK_PULL_PERIOD_MS: u64 = 200;
const MARKET_DIAG_FLOOR: Duration = Duration::from_millis(1000);

fn market_diag_enabled() -> bool {
    std::env::var_os("MOON_MARKET_DIAG").is_some() || std::env::var_os("MOON_RENDER_DIAG").is_some()
}

fn market_diag_due(key: impl Into<String>, floor: Duration) -> bool {
    if !market_diag_enabled() {
        return false;
    }
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let key = key.into();
    let now = Instant::now();
    let mut last = LAST
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("market diag lock poisoned");
    match last.get(&key).copied() {
        Some(prev) if now.duration_since(prev) < floor => false,
        _ => {
            last.insert(key, now);
            true
        }
    }
}

fn market_diag(msg: impl std::fmt::Display) {
    if market_diag_enabled() {
        log::info!("[market_diag] {msg}");
    }
}

fn bump_generation(revisions: &mut HashMap<CoreId, u64>, provider: CoreId) {
    let entry = revisions.entry(provider).or_insert(0);
    *entry = entry.wrapping_add(1);
}

fn bump_market_revisions(
    revisions: &mut HashMap<(CoreId, String), MarketRevisionCounters>,
    provider: CoreId,
    market: &str,
    flags: MarketDirtyFlags,
) {
    let entry = revisions.entry((provider, market.to_string())).or_default();
    if flags.contains(MarketDirtyFlags::HISTORY) {
        entry.history = entry.history.wrapping_add(1);
    }
    if flags.contains(MarketDirtyFlags::ORDERBOOK) {
        entry.book = entry.book.wrapping_add(1);
    }
    if flags.contains(MarketDirtyFlags::MARKET_META) {
        entry.meta = entry.meta.wrapping_add(1);
    }
}

fn mix_pair(a: u64, b: u64) -> u64 {
    a.wrapping_mul(0x9e37_79b1_85eb_ca87).rotate_left(17) ^ b
}

#[derive(Default)]
struct MarketPullCursor {
    book_phase_ms: Option<u64>,
    last_book_slot: Option<u64>,
    last_book_dirty_revision: u64,
    last_book_revision: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MarketRevisionCounters {
    history: u64,
    book: u64,
    meta: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarketRevisions {
    pub provider: CoreId,
    pub generation: u64,
    pub history: u64,
    pub book: u64,
    pub meta: u64,
}

impl MarketRevisions {
    pub fn combined_signature(self) -> u64 {
        let mut sig = 0xcbf29ce4_84222325u64;
        sig = mix_pair(sig, self.provider);
        sig = mix_pair(sig, self.generation);
        sig = mix_pair(sig, self.history);
        sig = mix_pair(sig, self.book);
        mix_pair(sig, self.meta)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LatestPriceError {
    NoProvider,
    NoClient,
    NoSnapshot,
    NoHistoryReaders,
    NoPrice,
}

impl std::fmt::Display for LatestPriceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProvider => f.write_str("no provider"),
            Self::NoClient => f.write_str("no client"),
            Self::NoSnapshot => f.write_str("no snapshot"),
            Self::NoHistoryReaders => f.write_str("no history readers"),
            Self::NoPrice => f.write_str("no price"),
        }
    }
}

#[derive(Default)]
pub struct ChartHistoryCursor {
    trades: Option<SeqRingCursor>,
    liquidations: Option<SeqRingCursor>,
    last_prices: Option<SeqRingCursor>,
    mark_prices: Option<SeqRingCursor>,
    last_price: Option<f32>,
    trade_rows: Vec<TradeHistoryRow>,
    scan_trade_rows: Vec<TradeHistoryRow>,
    liq_rows: Vec<TradeHistoryRow>,
    last_price_rows: Vec<LastPricePoint>,
    mark_price_rows: Vec<MarkPricePoint>,
}

impl ChartHistoryCursor {
    pub fn reset(&mut self) {
        self.trades = None;
        self.liquidations = None;
        self.last_prices = None;
        self.mark_prices = None;
        self.last_price = None;
        self.trade_rows.clear();
        self.scan_trade_rows.clear();
        self.liq_rows.clear();
        self.last_price_rows.clear();
        self.mark_price_rows.clear();
    }
}

#[derive(Default)]
pub struct ChartHistoryBuffers {
    pub ticks: Vec<Tick>,
    /// Трейды ликвидаций (отдельный ring `readers.liquidations`). На reset — полный видимый
    /// диапазон; иначе — только новые строки (живой край), как `ticks`. Сторона есть (знак qty),
    /// но рисуются единым цветом — рендер тегирует их `side=2`.
    pub liquidations: Vec<Tick>,
    pub last_points: Vec<PricePoint>,
    pub mark_points: Vec<PricePoint>,
}

impl ChartHistoryBuffers {
    fn clear(&mut self) {
        self.ticks.clear();
        self.liquidations.clear();
        self.last_points.clear();
        self.mark_points.clear();
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ChartHistoryRead {
    pub provider: CoreId,
    pub revision: u64,
    pub combo_capacity: usize,
    pub price_line_capacity: usize,
    pub combo_left_rel_ms: Option<f32>,
    pub combo_reset: bool,
    pub price_lines_changed: bool,
    pub clipped: bool,
    pub caught_up: bool,
    pub tick_price_range: Option<(f32, f32)>,
    pub last_price: Option<f32>,
}

struct MarketDataSourceInner {
    store: SharedMarketStore,
    clients: HashMap<CoreId, SharedMoonClient>,
    core_provider: HashMap<CoreId, CoreId>,
    provider_orderbook_kind: HashMap<CoreId, OrderBookKind>,
    cursors: HashMap<(CoreId, String), MarketPullCursor>,
    market_revisions: HashMap<(CoreId, String), MarketRevisionCounters>,
    provider_generations: HashMap<CoreId, u64>,
    started_at: Instant,
}

/// UI-agnostic market read-model bridge.
///
/// Feed threads publish only `SharedMoonClient` slots and lightweight wakes.
/// Consumers call this source when they are about to render: it pulls retained
/// MoonProto snapshot rows through per-consumer cursors into the shared
/// `MarketStore`, then exposes a read-only view by consumer core/market.
#[derive(Clone)]
pub struct MarketDataSource {
    inner: Arc<RwLock<MarketDataSourceInner>>,
}

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
        inner
            .market_revisions
            .retain(|(provider, _), _| *provider != core);
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
            .retain(|(provider, _), _| active_providers.contains(provider));
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
                .get(&key)
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
                    .or_else(|| snapshot.order_book(market, other_kind).map(|b| (other_kind, b)));
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

    /// Cheap hot-path revision for a consumer core. This reads one monotonic
    /// MoonProto snapshot number and does not clone the snapshot or drain rings.
    pub fn snapshot_revision(&self, core: CoreId) -> Option<(CoreId, u64)> {
        let (provider, client) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            let client = inner.clients.get(&provider)?.get()?;
            (provider, client)
        };
        Some((provider, client.snapshot_revision().unwrap_or(0)))
    }

    /// Cheap per-market wake revision for a consumer core.
    ///
    /// This is terminal-owned causality, not a MoonProto storage policy:
    /// feed threads mark the markets touched by domain events, and visible
    /// charts compare this one number before pulling retained rows or books.
    pub fn market_revisions(&self, core: CoreId, market: &str) -> Option<MarketRevisions> {
        let inner = self.inner.read().expect("market source poisoned");
        let provider = inner.core_provider.get(&core).copied()?;
        let generation = inner
            .provider_generations
            .get(&provider)
            .copied()
            .unwrap_or(0);
        let counters = inner
            .market_revisions
            .get(&(provider, market.to_string()))
            .copied()
            .unwrap_or_default();
        Some(MarketRevisions {
            provider,
            generation,
            history: counters.history,
            book: counters.book,
            meta: counters.meta,
        })
    }

    pub fn latest_price(&self, core: CoreId, market: &str) -> Result<f32, LatestPriceError> {
        let (provider, client) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner
                .core_provider
                .get(&core)
                .copied()
                .ok_or(LatestPriceError::NoProvider)?;
            let client = inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)
                .ok_or(LatestPriceError::NoClient)?;
            (provider, client)
        };
        let _ = provider;
        let snapshot = client
            .snapshot_versioned()
            .ok_or(LatestPriceError::NoSnapshot)?;
        let readers = snapshot
            .market_history_readers(market)
            .ok_or(LatestPriceError::NoHistoryReaders)?;

        let mut trades = Vec::new();
        if let Some(reader) = readers.futures_trades.or(readers.spot_trades) {
            reader.copy_last(1, &mut trades);
            if let Some(row) = trades.last() {
                if row.price.is_finite() && row.price > 0.0 {
                    return Ok(row.price);
                }
            }
        }

        let mut last_prices = Vec::new();
        if let Some(reader) = readers.last_prices {
            reader.copy_last(1, &mut last_prices);
            if let Some(row) = last_prices.last() {
                let price = row.price();
                if price.is_finite() && price > 0.0 {
                    return Ok(price);
                }
            }
        }

        let price = snapshot
            .markets()
            .price(market)
            .map(|p| p.p_last as f32)
            .filter(|p| p.is_finite() && *p > 0.0)
            .ok_or(LatestPriceError::NoPrice)?;
        Ok(price)
    }

    /// Курс валюты `currency` в USD: USD-стейбл → 1; иначе `p_last` рынка `<currency>USDT`
    /// (напр. BTC → BTCUSDT). `None` — курс неизвестен (нет провайдера/снимка/рынка).
    /// Та же линейная модель, что у `feed::assets` (без контрактных множителей).
    pub fn currency_usd_rate(&self, core: CoreId, currency: &str) -> Option<f64> {
        if currency.is_empty() {
            return None;
        }
        if crate::symbol::is_usd_stable(currency) {
            return Some(1.0);
        }
        let client = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            inner.clients.get(&provider).and_then(SharedMoonClient::get)?
        };
        let snapshot = client.snapshot_versioned()?;
        let market = format!("{}USDT", currency.to_ascii_uppercase());
        let p = snapshot.markets().price(&market)?;
        (p.p_last.is_finite() && p.p_last > 0.0).then_some(p.p_last)
    }

    /// Курс котировки рынка `market` в USD (для пересчёта ноционала qty·price в $).
    /// USDT-котировка → 1; BTC-котировка → курс BTC/USDT. `None` — неизвестен.
    pub fn quote_usd_rate(&self, core: CoreId, market: &str) -> Option<f64> {
        let quote = crate::symbol::resolve_quote(market);
        if quote.is_empty() {
            // HL/HIP-3 dex-перпы именуются как «xyz:BIRD» (dex-префикс + монета) — котировка
            // (USDC) в имени НЕ присутствует, поэтому суффикс-парсер её не находит. Но эти рынки
            // котируются в USDC (USD-стейбл, курс ≈1). Без этого `quote_usd` был None и подпись
            // размера падала в количество монет (показывала qty «11.8» вместо $-номинала «$50»).
            return Some(1.0);
        }
        self.currency_usd_rate(core, &quote)
    }

    /// Search the provider's market universe for a terminal coin-search box.
    ///
    /// Returns canonical market names (e.g. `"BTCUSDT"`) ranked by MoonProto's
    /// built-in search (exact → prefix → contains). Empty when the core has no
    /// provider/client/snapshot yet or the query is blank. The terminal pairs
    /// each name with the core's server name for the `"BTC - Bybit1"` display.
    pub fn search_markets(&self, core: CoreId, query: &str, limit: usize) -> Vec<String> {
        let client = {
            let inner = self.inner.read().expect("market source poisoned");
            let Some(provider) = inner.core_provider.get(&core).copied() else {
                return Vec::new();
            };
            match inner.clients.get(&provider).and_then(SharedMoonClient::get) {
                Some(client) => client,
                None => return Vec::new(),
            }
        };
        let Some(snapshot) = client.snapshot_versioned() else {
            return Vec::new();
        };
        snapshot
            .markets()
            .search(query, limit)
            .into_iter()
            .map(|handle| handle.name().to_string())
            .collect()
    }

    pub fn read_chart_history_into(
        &self,
        core: CoreId,
        market: &str,
        epoch_ms: f64,
        from_rel_ms: f32,
        to_rel_ms: f32,
        force_reset: bool,
        scan_price: bool,
        cursor: &mut ChartHistoryCursor,
        out: &mut ChartHistoryBuffers,
    ) -> Option<ChartHistoryRead> {
        out.clear();
        let (provider, client) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            let client = inner.clients.get(&provider)?.get()?;
            (provider, client)
        };
        let snapshot = client.snapshot_versioned()?;
        let revision = client.snapshot_revision().unwrap_or(0);
        let readers = snapshot.market_history_readers(market)?;
        let from_time = moon_time_from_rel_ms(epoch_ms, from_rel_ms);
        let to_time = moon_time_from_rel_ms(epoch_ms, to_rel_ms.max(from_rel_ms + 1.0));
        let mut read = ChartHistoryRead {
            provider,
            revision,
            caught_up: true,
            ..ChartHistoryRead::default()
        };

        let trade_reader = readers.futures_trades.or(readers.spot_trades);
        if let Some(reader) = trade_reader {
            read.combo_capacity = reader.capacity();
            let reset = force_reset || cursor.trades.is_none();
            if reset {
                reader.copy_time_range(
                    from_time,
                    to_time,
                    reader.capacity(),
                    &mut cursor.trade_rows,
                );
                cursor.trades = Some(reader.cursor_from_now());
                read.combo_reset = true;
                read.caught_up = true;
            } else if let Some(cur) = cursor.trades.as_mut() {
                let meta = reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.trade_rows);
                read.clipped |= meta.clipped;
                read.caught_up &= meta.caught_up;
                if meta.clipped {
                    reader.copy_time_range(
                        from_time,
                        to_time,
                        reader.capacity(),
                        &mut cursor.trade_rows,
                    );
                    cursor.trades = Some(reader.cursor_from_now());
                    read.combo_reset = true;
                }
            }
            rows_to_ticks(&cursor.trade_rows, &mut out.ticks);
            read.combo_left_rel_ms = out
                .ticks
                .first()
                .map(|tick| (tick.time_ms - epoch_ms) as f32);
            if let Some(t) = out.ticks.last() {
                cursor.last_price = Some(t.price);
            } else if cursor.last_price.is_none() {
                cursor.trade_rows.clear();
                reader.copy_last(1, &mut cursor.trade_rows);
                if let Some(row) = cursor.trade_rows.last() {
                    cursor.last_price = Some(row.price);
                }
            }
            if scan_price {
                reader.copy_time_range(
                    from_time,
                    to_time,
                    reader.capacity(),
                    &mut cursor.scan_trade_rows,
                );
                read.tick_price_range = trade_price_range(&cursor.scan_trade_rows);
            }
        } else {
            cursor.trades = None;
            cursor.last_price = None;
        }

        // Трейды ликвидаций — отдельный ring того же типа. Синхронны с combo: на полном
        // reset combo (или первом проходе) перечитываем весь видимый диапазон, иначе тянем
        // только новый живой край. Рендер тегирует их единым цветом (side=2).
        if let Some(reader) = readers.liquidations {
            let reset = read.combo_reset || cursor.liquidations.is_none();
            if reset {
                reader.copy_time_range(from_time, to_time, reader.capacity(), &mut cursor.liq_rows);
                cursor.liquidations = Some(reader.cursor_from_now());
            } else if let Some(cur) = cursor.liquidations.as_mut() {
                let meta = reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.liq_rows);
                if meta.clipped {
                    reader.copy_time_range(
                        from_time,
                        to_time,
                        reader.capacity(),
                        &mut cursor.liq_rows,
                    );
                    cursor.liquidations = Some(reader.cursor_from_now());
                }
            }
            rows_to_ticks(&cursor.liq_rows, &mut out.liquidations);
        } else {
            cursor.liquidations = None;
        }

        if let Some(reader) = readers.last_prices {
            drain_price_line(
                &reader,
                from_time,
                to_time,
                force_reset,
                &mut cursor.last_prices,
                &mut cursor.last_price_rows,
                &mut out.last_points,
                &mut read,
                last_rows_to_points,
            );
        } else {
            cursor.last_prices = None;
        }

        if let Some(reader) = readers.mark_prices {
            drain_price_line(
                &reader,
                from_time,
                to_time,
                force_reset,
                &mut cursor.mark_prices,
                &mut cursor.mark_price_rows,
                &mut out.mark_points,
                &mut read,
                mark_rows_to_points,
            );
        } else {
            cursor.mark_prices = None;
        }

        read.last_price = cursor.last_price;
        Some(read)
    }

    pub fn with_orderbook_view<R>(
        &self,
        core: CoreId,
        market: &str,
        f: impl FnOnce(Option<(&OrderBookModel, u64)>) -> R,
    ) -> R {
        let (provider, store) = {
            let inner = self.inner.read().expect("market source poisoned");
            (inner.core_provider.get(&core).copied(), inner.store.clone())
        };
        let store = store.read().expect("market store poisoned");
        f(provider
            .and_then(|p| store.view(p, market))
            .map(|view| (&view.book, view.book_rev)))
    }
}

fn moon_time_from_rel_ms(epoch_ms: f64, rel_ms: f32) -> MoonTime {
    MoonTime::from_unix_millis((epoch_ms + rel_ms as f64).round() as i64)
}

/// Дренаж линии цены (last/mark) — обе ветви идентичны по структуре, отличаются лишь
/// курсором/буфером/выходом/конвертером. reset|первый вызов → ставим курсор «от сейчас»;
/// иначе тянем новое (clipped/caught_up копятся в `read`); при изменении — копируем видимый
/// диапазон и конвертируем в точки. Вызывается только когда ридер существует.
#[allow(clippy::too_many_arguments)]
fn drain_price_line<R: SeqRingTimedRow>(
    reader: &SeqRingReader<R>,
    from_time: MoonTime,
    to_time: MoonTime,
    force_reset: bool,
    cursor_slot: &mut Option<SeqRingCursor>,
    rows: &mut Vec<R>,
    out: &mut Vec<PricePoint>,
    read: &mut ChartHistoryRead,
    convert: impl Fn(&[R], &mut Vec<PricePoint>),
) {
    read.price_line_capacity = read.price_line_capacity.max(reader.capacity());
    let reset = force_reset || cursor_slot.is_none();
    let mut changed = reset;
    if reset {
        *cursor_slot = Some(reader.cursor_from_now());
    } else if let Some(cur) = cursor_slot.as_mut() {
        let meta = reader.drain_new_bounded(cur, reader.capacity(), rows);
        read.clipped |= meta.clipped;
        read.caught_up &= meta.caught_up;
        changed = meta.copied > 0 || meta.clipped;
    }
    if changed {
        reader.copy_time_range(from_time, to_time, reader.capacity(), rows);
        convert(rows, out);
        read.price_lines_changed = true;
    }
}

fn rows_to_ticks(rows: &[TradeHistoryRow], out: &mut Vec<Tick>) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().map(|r| Tick {
        time_ms: r.unix_millis() as f64,
        price: r.price,
        qty: r.quantity(),
        side: if r.is_buy() { Side::Buy } else { Side::Sell },
    }));
}

fn last_rows_to_points(rows: &[LastPricePoint], out: &mut Vec<PricePoint>) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().map(|p| PricePoint {
        time_ms: p.unix_millis() as f64,
        price: p.price(),
    }));
}

fn mark_rows_to_points(rows: &[MarkPricePoint], out: &mut Vec<PricePoint>) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().map(|p| PricePoint {
        time_ms: p.unix_millis() as f64,
        price: p.price(),
    }));
}

fn trade_price_range(rows: &[TradeHistoryRow]) -> Option<(f32, f32)> {
    if rows.is_empty() {
        return None;
    }
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for r in rows {
        lo = lo.min(r.price);
        hi = hi.max(r.price);
    }
    Some((lo, hi))
}

fn cadence_phase_ms(provider: CoreId, market: &str, period_ms: u64) -> u64 {
    let mut sig = 0xcbf29ce484222325u64;
    sig ^= provider;
    sig = sig.wrapping_mul(0x100000001b3);
    for b in market.bytes() {
        sig ^= b as u64;
        sig = sig.wrapping_mul(0x100000001b3);
    }
    sig % period_ms.max(1)
}

fn cadence_slot(elapsed_ms: u64, phase_ms: u64, period_ms: u64) -> Option<u64> {
    if elapsed_ms < phase_ms {
        None
    } else {
        Some((elapsed_ms - phase_ms) / period_ms.max(1))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::market::MarketStore;

    use super::*;

    #[test]
    fn orderbook_cadence_phase_is_stable_and_bounded() {
        let a = cadence_phase_ms(1, "BTCUSDT", ORDERBOOK_PULL_PERIOD_MS);
        let b = cadence_phase_ms(1, "BTCUSDT", ORDERBOOK_PULL_PERIOD_MS);
        let c = cadence_phase_ms(1, "ETHUSDT", ORDERBOOK_PULL_PERIOD_MS);

        assert_eq!(a, b);
        assert!(a < ORDERBOOK_PULL_PERIOD_MS);
        assert!(c < ORDERBOOK_PULL_PERIOD_MS);
    }

    #[test]
    fn cadence_slot_waits_until_phase_then_advances_by_period() {
        assert_eq!(cadence_slot(99, 100, 200), None);
        assert_eq!(cadence_slot(100, 100, 200), Some(0));
        assert_eq!(cadence_slot(299, 100, 200), Some(0));
        assert_eq!(cadence_slot(300, 100, 200), Some(1));
    }

    #[test]
    fn market_dirty_flags_bump_only_their_slice_revisions() {
        let source = MarketDataSource::new(MarketStore::shared(0.0));
        let mut providers = HashMap::new();
        providers.insert(7, 42);
        source.set_provider_map(&providers);

        let initial = source.market_revisions(7, "BTCUSDT").unwrap();
        assert_eq!((initial.history, initial.book, initial.meta), (0, 0, 0));

        source.mark_dirty(
            42,
            &[MarketDirty::new("BTCUSDT", MarketDirtyFlags::ORDERBOOK)],
        );
        let after_book = source.market_revisions(7, "BTCUSDT").unwrap();
        assert_eq!(
            (after_book.history, after_book.book, after_book.meta),
            (0, 1, 0)
        );

        source.mark_dirty(
            42,
            &[MarketDirty::new(
                "BTCUSDT",
                MarketDirtyFlags::HISTORY | MarketDirtyFlags::MARKET_META,
            )],
        );
        let after_history_meta = source.market_revisions(7, "BTCUSDT").unwrap();
        assert_eq!(
            (
                after_history_meta.history,
                after_history_meta.book,
                after_history_meta.meta
            ),
            (1, 1, 1)
        );
    }
}
