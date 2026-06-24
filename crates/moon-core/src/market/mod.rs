//! Market-плоскость: ОБЩИЕ рыночные данные (крестики + стакан), дедуплицированные
//! по ядру-провайдеру. Рынок `BTCUSDT@Binance Futures` идентичен у всех ядер этой
//! биржи, поэтому его тянет одно избранное ядро (провайдер), а не все 200.
//!
//! Ключ хранилища — id ЯДРА-ПРОВАЙДЕРА (того, что реально несёт подписку). Этот же
//! механизм покрывает оба режима: в dedup один провайдер на биржу, в per-core каждое
//! ядро провайдер самому себе (см. `MarketDataMode`).

mod source;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::data::OrderBookModel;
use crate::feed::{OrderBook, Tick};
use crate::session::CoreId;

pub use source::{
    ChartHistoryBuffers, ChartHistoryCursor, ChartHistoryRead, LatestPriceError, MarketDataSource,
    MarketRevisions,
};

/// Shared market buffer owned by moon-core, not by a GPUI entity. Live feeds only wake
/// consumers; `SessionManager` pulls provider snapshots into this buffer for visible
/// charts. Synthetic/compat feed messages can still publish here directly.
pub type SharedMarketStore = Arc<RwLock<MarketStore>>;

/// Режим источника рыночных данных (рубильник из настроек). Хранится в settings.toml
/// кодом ("dedup"/"percore"); неизвестный код откатывается на дефолт.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarketDataMode {
    /// Один провайдер на биржу: все ядра биржи читают рынок с него. По умолчанию.
    #[default]
    Dedup,
    /// Без дедупа: каждый чарт берёт рынок со своего ядра (ядро = свой провайдер).
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
        // Неизвестный код не роняет разбор файла — откатываемся на дефолт.
        let s = String::deserialize(d)?;
        Ok(MarketDataMode::from_code(&s).unwrap_or_default())
    }
}

/// Внутренний legacy/synth store одного рынка от одного провайдера.
///
/// Production UI не должен читать отсюда latest price/history напрямую: история и
/// latest-price идут через `MarketDataSource::read_chart_history_into/latest_price`,
/// стакан — через `MarketDataSource::with_orderbook_view`.
pub struct MarketView {
    book: OrderBookModel,
    last_price: Option<f32>,
    /// Время последнего тика (unix ms) — правый край графика следует за ним.
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

/// Рыночные данные всех провайдеров: провайдер → (рынок → данные).
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

    /// Данные рынка от конкретного провайдера (None, пока провайдер их не прислал).
    pub fn view(&self, provider: CoreId, market: &str) -> Option<&MarketView> {
        self.by_provider.get(&provider)?.get(market)
    }

    /// Сбросить рынок провайдера на чистый: новое открытие или смена провайдера
    /// (провайдер заново выгрузит retained-историю с начала кольца).
    pub fn reset(&mut self, provider: CoreId, market: &str) {
        self.by_provider
            .entry(provider)
            .or_default()
            .insert(market.to_string(), MarketView::new());
    }

    /// Рынок больше никто не смотрит — освобождаем (после linger-задержки).
    pub fn drop_market(&mut self, provider: CoreId, market: &str) {
        if let Some(m) = self.by_provider.get_mut(&provider) {
            m.remove(market);
        }
    }

    /// Провайдер сменился/отвалился — выкидываем все его рынки.
    pub fn drop_provider(&mut self, provider: CoreId) {
        self.by_provider.remove(&provider);
    }

    /// Полный сброс (смена режима источника): провайдеры/рынки переизберутся заново.
    pub fn clear(&mut self) {
        self.by_provider.clear();
    }

    /// Тики от провайдера (применяются, только если view для рынка существует —
    /// его создаёт координатор через `reset` при попадании рынка в wanted).
    pub fn apply_ticks(&mut self, provider: CoreId, market: &str, ticks: &[Tick]) {
        if let Some(v) = self
            .by_provider
            .get_mut(&provider)
            .and_then(|m| m.get_mut(market))
        {
            v.push_ticks(ticks);
        }
    }

    /// Снимок стакана от провайдера (как и тики — только при существующем view).
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
