//! Market plane: SHARED market data (ticks + order book), deduplicated by provider core.
//! The `BTCUSDT@Binance Futures` market is identical across all cores on that exchange,
//! so one elected core (the provider) fetches it instead of all 200 cores.
//!
//! The store key is the PROVIDER CORE id (the core that actually owns the subscription).
//! The same mechanism supports both modes: dedup uses one provider per exchange, while
//! per-core makes each core its own provider (see `MarketDataMode`).

pub mod candles;
pub mod kline_cache;
mod screener;
mod source;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::data::OrderBookModel;
use crate::feed::{OrderBook, Tick};
use crate::session::CoreId;

pub use candles::{CandleViewCfg, ChartCandle};
pub use screener::ScreenerRow;
pub use source::{
    pick_market_for_coin, CandleReadParams, ChartHistoryBuffers, ChartHistoryCursor,
    ChartHistoryRead, DetectSnapshot, LatestPriceError, MarketDataSource, MarketLabel,
    MarketLimits, MarketRevisions, MarketTickerReadout, MaxOrder, MaxOrderSource,
};

/// Shared market buffer owned by moon-core, not by a GPUI entity. Live feeds only wake
/// consumers; retained chart history is read directly through `ChartHistoryCursor`, while
/// order-book views are pulled into this buffer. Synthetic/compat feed messages can still
/// publish here directly.
pub type SharedMarketStore = Arc<RwLock<MarketStore>>;

/// Market-data source mode selected in settings. Stored in `settings.toml` as
/// `"dedup"` or `"percore"`; an unknown code falls back to the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarketDataMode {
    /// One provider per exchange: all cores on the exchange read market data from it. Default.
    #[default]
    Dedup,
    /// No deduplication: each chart reads market data from its own core.
    PerCore,
}

impl MarketDataMode {
    pub fn code(self) -> &'static str {
        match self {
            MarketDataMode::Dedup => "dedup",
            MarketDataMode::PerCore => "percore",
        }
    }

    fn from_code(s: &str) -> Option<Self> {
        match s {
            "dedup" => Some(MarketDataMode::Dedup),
            "percore" => Some(MarketDataMode::PerCore),
            _ => None,
        }
    }
}

impl Serialize for MarketDataMode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for MarketDataMode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Keep an unknown code from failing file parsing by falling back to the default.
        let s = String::deserialize(d)?;
        Ok(MarketDataMode::from_code(&s).unwrap_or_default())
    }
}

/// Internal legacy/synthetic store for one market from one provider.
///
/// Production UI must not read the latest price or history directly from this store. History
/// and the latest price go through `MarketDataSource::read_chart_history_into/latest_price`,
/// and the order book goes through `MarketDataSource::with_orderbook_view`.
pub struct MarketView {
    book: OrderBookModel,
    last_price: Option<f32>,
    /// Retained timestamp metadata for legacy/synthetic ticks; chart reads do not use it here.
    last_tick_ms: Option<f64>,
    ticks_rev: u64,
    book_rev: u64,
}

impl MarketView {
    fn new() -> Self {
        Self {
            book: OrderBookModel::default(),
            last_price: None,
            last_tick_ms: None,
            ticks_rev: 0,
            book_rev: 0,
        }
    }

    fn push_ticks(&mut self, ticks: &[Tick]) {
        if let Some(t) = ticks.last() {
            self.last_price = Some(t.price);
            self.last_tick_ms = Some(t.time_ms);
        }
        self.ticks_rev = self.ticks_rev.wrapping_add(1);
    }

    fn set_book(&mut self, book: &OrderBook) {
        self.book.update(book);
        self.book_rev = self.book_rev.wrapping_add(1);
    }
}

/// Market data for all providers: provider -> (market -> data).
pub struct MarketStore {
    by_provider: HashMap<CoreId, HashMap<String, MarketView>>,
}

impl MarketStore {
    pub fn shared(epoch_ms: f64) -> SharedMarketStore {
        Arc::new(RwLock::new(Self::new(epoch_ms)))
    }

    pub fn new(_epoch_ms: f64) -> Self {
        Self {
            by_provider: HashMap::new(),
        }
    }

    /// Returns an installed market view for a provider.
    ///
    /// `reset` installs an empty view immediately, so `Some` does not mean provider data arrived.
    pub fn view(&self, provider: CoreId, market: &str) -> Option<&MarketView> {
        self.by_provider.get(&provider)?.get(market)
    }

    /// Installs an empty provider view for a new open or provider change.
    ///
    /// This does not reload retained history into the store. Charts read retained rows directly
    /// through their `ChartHistoryCursor` via `MarketDataSource`.
    pub fn reset(&mut self, provider: CoreId, market: &str) {
        self.by_provider
            .entry(provider)
            .or_default()
            .insert(market.to_string(), MarketView::new());
    }

    /// Releases a market after its linger delay once no consumer is watching it.
    pub fn drop_market(&mut self, provider: CoreId, market: &str) {
        if let Some(m) = self.by_provider.get_mut(&provider) {
            m.remove(market);
        }
    }

    /// Drops all markets when their provider changes or disconnects.
    pub fn drop_provider(&mut self, provider: CoreId) {
        self.by_provider.remove(&provider);
    }

    /// Clears everything after a source-mode change so providers and markets can be re-elected.
    pub fn clear(&mut self) {
        self.by_provider.clear();
    }

    /// Applies provider ticks only when the market view exists. The coordinator creates the
    /// view through `reset` when the market enters the wanted set.
    pub fn apply_ticks(&mut self, provider: CoreId, market: &str, ticks: &[Tick]) {
        if let Some(v) = self
            .by_provider
            .get_mut(&provider)
            .and_then(|m| m.get_mut(market))
        {
            v.push_ticks(ticks);
        }
    }

    /// Applies a provider order-book snapshot only when the market view exists.
    pub fn apply_book(&mut self, provider: CoreId, market: &str, book: &OrderBook) {
        if let Some(v) = self
            .by_provider
            .get_mut(&provider)
            .and_then(|m| m.get_mut(market))
        {
            v.set_book(book);
        }
    }
}
