mod read;
mod refresh;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use moonproto::state::{
    LastPricePoint, MarkPricePoint, OrderBookKind, SeqRingCursor, SeqRingPriceRow, SeqRingReader,
    SeqRingTimedRow, TradeHistoryRow,
};
use moonproto::MoonTime;

use super::candles::{CandleSeries, ChartCandle};
use crate::feed::{MarketDirtyFlags, PricePoint, SharedMoonClient, Side, Tick};
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
    revisions: &mut HashMap<CoreId, HashMap<String, MarketRevisionCounters>>,
    provider: CoreId,
    market: &str,
    flags: MarketDirtyFlags,
) {
    let entry = revisions
        .entry(provider)
        .or_default()
        .entry(market.to_string())
        .or_default();
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

/// Снимок курса рынка для тикера шапки: последняя цена + знаковые дельты, %.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketTickerReadout {
    pub last: f64,
    pub delta_1h_pct: f64,
    pub delta_24h_pct: f64,
}

/// Замороженный снимок для карточки детекта (собирается ОДИН раз в момент детекта):
/// мини-чарт последних закрытых 5м-свечей + идентити биржи. Данные — ретейненный снимок
/// провайдера: `candles_5m` = собственная запись ядра (биржевой API не трогаем),
/// `server_info` = идентити подключения. Пустой — нет провайдера/снимка.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetectSnapshot {
    /// Последние 5м-свечи `(open, high, low, close)`, порядок старые→новые. Пусто — нет истории.
    /// Полный OHLC → мини-чарт рисует тело+фитиль (фолбэк `candles_5m` несёт только high/low →
    /// тело вырождено, но тень видна).
    pub bars: Vec<(f32, f32, f32, f32)>,
    /// Цены (close) для режима «линия» — порядок старые→новые, до ~24ч глубины. Пусто — нет.
    pub line: Vec<f32>,
    /// ФАКТИЧЕСКОЕ изменение цены за 24ч, % (сейчас vs close ~24ч назад из наших бакетов —
    /// совпадает со сдвигом линии; НЕ moonproto `coin_24h_delta`, то — отклонение от средней).
    pub delta_24h: f32,
    /// Фактическое изменение цены за 1ч, % (сейчас vs close ~1ч назад).
    pub delta_1h: f32,
    /// Человекочитаемое имя биржи из server_info (напр. «Binance Futures», «Bybit»). Пусто — нет.
    pub exchange_name: String,
    /// Короткий тип биржи из `exchange_type_mask` («Спот»/«Фьючи»/«DEX»/…). Пусто — не сообщён.
    pub exchange_kind: String,
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
    /// Слитная серия свечей (серверная база + локальный хвост из трейдов).
    /// Свой курсор по трейд-рингу (агрегация независима от зоны отображения крестов).
    candle_series: CandleSeries,
    candle_trades: Option<SeqRingCursor>,
    candle_trade_rows: Vec<TradeHistoryRow>,
    candle_ticks: Vec<Tick>,
    server_candle_rows: Vec<moonproto::state::Candle5mRow>,
    server_candles: Vec<ChartCandle>,
    /// Троттл неблокирующего запроса CoinCard deep history (честные OHLC).
    last_deep_request: Option<Instant>,
    /// Последний запрошенный kind — смена ТФ обходит троттл.
    last_deep_kind: Option<moonproto::DeepHistoryKind>,
    /// Бэкофф повторных coin-card запросов, СЕКУНДЫ (0 → стартовые 30). Deep history ядро
    /// тянет с БИРЖЕВОГО API (веса!) — ретрай без прогресса удваивает паузу до 10 мин,
    /// приход новых рядов сбрасывает. Иначе зависшее ядро/биржа = вечный 30с-шторм запросов
    /// со всех открытых чартов → «автостоп по превышению лимитов API» ядра.
    deep_retry_delay_s: u32,
    /// Префикс из локального kline-кэша (нативный kind панели): читается из sqlite один
    /// раз на (рынок, kind, левый край) и переживает series_reset'ы (ресеты частые —
    /// пан/зум, каждый раз ходить в БД нельзя).
    cache_rows: Vec<ChartCandle>,
    cache_kind: Option<u32>,
    /// ФАКТИЧЕСКИЙ kind прочитанных рядов: нативный kind панели либо фолбэк
    /// (5м регистратора → 1м deep-записей).
    cache_rows_kind: u32,
    cache_from_ms: i64,
    /// Крупные слои для дорисовки хвоста истории СТАРШИМИ ТФ («до упора»):
    /// 5м (снимок+регистратор) и 1д (бэкфилл/кэш). Читаются вместе с cache_rows.
    cache_rows_5m: Vec<ChartCandle>,
    cache_rows_1d: Vec<ChartCandle>,
    /// Сигнатура последних записанных в кэш deep-рядов — write-back только на изменение.
    cache_written_sig: u64,
    /// Троттл диагностики «разрыв свечи↔сейчас» (WARN раз в 30с на панель).
    last_gap_diag: Option<Instant>,
    /// То же для рядов НАТИВНОГО kind панели (урожай разового бэкфилла, когда
    /// эффективный kind ядра мельче нативного).
    cache_written_native_sig: u64,
    /// Сигнатура загруженных deep-строк на момент последней пересборки серии: их
    /// приход/обновление форсит rebuild (без этого история появлялась только после
    /// переоткрытия графика).
    last_deep_sig: u64,
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
        self.candle_series.invalidate();
        self.candle_trades = None;
        self.candle_trade_rows.clear();
        self.candle_ticks.clear();
        self.server_candle_rows.clear();
        self.server_candles.clear();
        // last_deep_request НЕ сбрасываем: троттл запросов переживает reset
        // (смена рынка пересоздаёт PaneRender → курсор свежий).
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
    /// Свечи серии (полный видимый ряд). Наполняется ТОЛЬКО когда ревизия серии отличается
    /// от `CandleReadParams::shipped_revision` (см. `ChartHistoryRead::candles_changed`).
    pub candles: Vec<ChartCandle>,
    /// ТФ каждой свечи `candles` (мс), параллельный массив: хвост истории дорисовывается
    /// СТАРШИМИ ТФ (5м-слой, затем 1д «до упора»), у таких свечей своя ширина.
    /// Пусто = все свечи ТФ серии.
    pub candle_tf_ms: Vec<f32>,
}

impl ChartHistoryBuffers {
    fn clear(&mut self) {
        self.ticks.clear();
        self.liquidations.clear();
        self.last_points.clear();
        self.mark_points.clear();
        self.candles.clear();
        self.candle_tf_ms.clear();
    }
}

/// Параметры чтения свечей/зоны трейдов для `read_chart_history_into`. `None` у вызова =
/// легаси-поведение (только кресты, без свечей).
#[derive(Debug, Clone, Copy)]
pub struct CandleReadParams {
    /// Таймфрейм серии, мс.
    pub tf_ms: i64,
    /// Нижняя граница ОТОБРАЖАЕМЫХ трейдов (rel ms от epoch) — зона «последних K свечей».
    /// `f32::INFINITY` = трейды не отображаем вовсе (K=0). На агрегацию свечей не влияет.
    pub trades_from_rel_ms: f32,
    /// Жёсткий лимит числа отображаемых трейдов.
    pub trades_limit: usize,
    /// Ревизия серии, уже доставленная рендеру: совпала — `out.candles` не наполняем.
    pub shipped_revision: u64,
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
    /// Серия свечей изменилась относительно `shipped_revision` — `out.candles` наполнен.
    pub candles_changed: bool,
    /// Текущая ревизия серии свечей (вернуть в следующий `CandleReadParams`).
    pub candles_revision: u64,
}

/// Состояние живой подписки ТФ-баров одного (провайдер, рынок) — см. `candle_subs`.
struct CandleSubState {
    kind_min: u32,
    last_want: Instant,
    subscribed: bool,
}

struct MarketDataSourceInner {
    store: SharedMarketStore,
    clients: HashMap<CoreId, SharedMoonClient>,
    core_provider: HashMap<CoreId, CoreId>,
    provider_orderbook_kind: HashMap<CoreId, OrderBookKind>,
    cursors: HashMap<(CoreId, String), MarketPullCursor>,
    market_revisions: HashMap<CoreId, HashMap<String, MarketRevisionCounters>>,
    provider_generations: HashMap<CoreId, u64>,
    started_at: Instant,
    /// ГЛОБАЛЬНЫЙ дедуп coin-card запросов (provider, market, kind_min → момент отправки):
    /// курсоры per-pane, и N окон одной монеты слали N одинаковых запросов; deep history
    /// стоит биржевых весов на ядре, поэтому одна пара (монета, ТФ) — не чаще раза в 30с
    /// на всё приложение (ответ ложится в общий retained-стейт, панели делят его).
    deep_req_gate: Mutex<HashMap<(CoreId, String, u32), Instant>>,
    /// Желаемые deep-kind'ы живых свечных панелей per провайдер (kind_min → последний спрос).
    /// ЯДРО ДЕРЖИТ ОДИН СВЕЧНОЙ ТФ НА ЯДРО (слова разработчика МБ 2026-07-12): каждый флип
    /// kind = ядро перекачивает историю С БИРЖИ заново → бан API при окнах с разными ТФ.
    /// Эффективный kind ядра = min живых желаний (kind'ы цепочкой делятся 1|5|30|60|240|1440),
    /// панели крупнее ресемплят из мелкой базы (ценой глубины). Запись живёт 30с без спроса.
    deep_kind_wants: Mutex<HashMap<CoreId, HashMap<u32, Instant>>>,
    /// Живые подписки ТФ-баров per (провайдер, рынок). Подписка ГЛОБАЛЬНА на клиенте
    /// (последний kind выигрывает) — per-pane управление дёргало ядро; здесь панели лишь
    /// «трогают» запись, протухшие (>60с без спроса: панель закрыта/суб-минутный ТФ)
    /// отписываются попутно при следующем свечном чтении этого провайдера.
    candle_subs: Mutex<HashMap<(CoreId, String), CandleSubState>>,
    /// Локальный kline-кэш (klines.sqlite) — None, пока терминал не задал путь.
    kline_cache: Option<crate::market::kline_cache::KlineCache>,
    /// Стабильная идентичность биржи провайдера — ключ kline-кэша (НЕ CoreId).
    provider_exchange: HashMap<CoreId, crate::feed::ExchangeId>,
    /// Разовые нативные бэкфиллы крупных ТФ за сессию: (провайдер, рынок, kind_min).
    /// Один осознанный флип ТФ-слота ядра на открытие крупного ТФ с пустым кэшем.
    native_backfill_done: Mutex<HashSet<(CoreId, String, u32)>>,
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

/// Общий конвертер строк last/mark price-line → точки чарта: тела были идентичны,
/// оба типа отдают время через `SeqRingTimedRow`, цену — вырожденным диапазоном
/// `SeqRingPriceRow` (`(p, p)`, `None` у этих строк не бывает).
fn price_rows_to_points<R: SeqRingTimedRow + SeqRingPriceRow>(
    rows: &[R],
    out: &mut Vec<PricePoint>,
) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().filter_map(|p| {
        let (price, _) = p.seq_ring_price_range()?;
        Some(PricePoint {
            time_ms: p.seq_ring_time_ms() as f64,
            price,
        })
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
