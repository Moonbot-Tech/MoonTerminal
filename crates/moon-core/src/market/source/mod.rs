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

/// Market price snapshot for the header ticker: last price and signed percentage deltas.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketTickerReadout {
    pub last: f64,
    pub delta_1h_pct: f64,
    pub delta_24h_pct: f64,
}

/// Frozen snapshot for a detection card, built exactly once when the detection occurs.
///
/// The mini-chart combines recent 5-minute candles from the provider's retained
/// `candles_5m`, the local kline cache, and the live trade-ring tail without calling the exchange
/// API. `server_info` supplies the connection identity. Missing history leaves the chart data
/// empty while exchange metadata may still be present; a missing provider, client, or client
/// snapshot returns the fully empty default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetectSnapshot {
    /// Recent 5-minute candles as `(open, high, low, close)`, ordered oldest to newest.
    ///
    /// Full OHLC lets the mini-chart draw bodies and wicks. The retained `candles_5m` fallback is
    /// range-only, so its candle orientation is synthesized. Empty when no history is available.
    pub bars: Vec<(f32, f32, f32, f32)>,
    /// Close prices for line mode, ordered oldest to newest, with up to about 24 hours of history.
    pub line: Vec<f32>,
    /// Actual 24-hour price change in percent, comparing now with a close from about 24 hours ago.
    ///
    /// This is derived from our buckets so it matches the line movement. It is not MoonProto's
    /// `coin_24h_delta`, which measures deviation from a retained average.
    pub delta_24h: f32,
    /// Actual 1-hour price change in percent, comparing now with a close from about one hour ago.
    pub delta_1h: f32,
    /// Human-readable exchange name from `server_info`, such as `Binance Futures` or `Bybit`.
    pub exchange_name: String,
    /// Short Russian-language exchange type label derived from `exchange_type_mask`.
    ///
    /// The label distinguishes spot, futures, DEX, and combined connections. Empty when the type
    /// was not reported.
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
    /// Merged candle series consisting of the server base and a local tail built from trades.
    /// Uses its own trade-ring cursor so aggregation is independent of the cross display range.
    candle_series: CandleSeries,
    candle_trades: Option<SeqRingCursor>,
    candle_trade_rows: Vec<TradeHistoryRow>,
    candle_ticks: Vec<Tick>,
    server_candle_rows: Vec<moonproto::state::Candle5mRow>,
    server_candles: Vec<ChartCandle>,
    /// Throttle for non-blocking CoinCard deep-history requests that provide authoritative OHLC.
    last_deep_request: Option<Instant>,
    /// Most recently requested kind; a timeframe change bypasses the throttle.
    last_deep_kind: Option<moonproto::DeepHistoryKind>,
    /// Coin-card retry backoff in seconds, where zero starts at 30 seconds.
    ///
    /// The core fetches deep history from the exchange API and consumes request weight. A retry
    /// without progress doubles the delay up to 10 minutes, while new rows reset it. Without this
    /// backoff, a stalled core or exchange would receive a request storm from every open chart and
    /// trigger the core's API-limit auto-stop.
    deep_retry_delay_s: u32,
    /// Prefix loaded from the local kline cache using the panel's native kind.
    ///
    /// It is read from SQLite once per `(market, kind, left edge)` and survives series resets.
    /// Resets are frequent during pan and zoom, so each reset must not query the database.
    cache_rows: Vec<ChartCandle>,
    cache_kind: Option<u32>,
    /// Actual kind of the loaded rows: the panel's native kind or a fallback.
    ///
    /// Fallback order is the recorder's 5-minute rows followed by 1-minute deep-history rows.
    cache_rows_kind: u32,
    cache_from_ms: i64,
    /// Cache-only coarser layers used to extend the historical prefix as far back as possible.
    ///
    /// The 5-minute layer contains kind-5 cache rows from the recorder and possible deep-history
    /// writeback; the retained 5-minute snapshot separately feeds the main series through
    /// `snap_part`. The 1-day layer comes from backfill and cache.
    cache_rows_5m: Vec<ChartCandle>,
    cache_rows_1d: Vec<ChartCandle>,
    /// Signature of the last deep rows written to the cache; write back only after a change.
    cache_written_sig: u64,
    /// Throttle for candle-to-now gap diagnostics: at most one warning per panel every 30 seconds.
    last_gap_diag: Option<Instant>,
    /// Equivalent signature for rows of the panel's native kind.
    ///
    /// These rows are produced by a one-time backfill when the core's effective kind is finer than
    /// the panel's native kind.
    cache_written_native_sig: u64,
    /// Low-cost fingerprint of loaded deep rows at the last series rebuild.
    ///
    /// It hashes only the row count and final timestamp. An in-place OHLC update at the same final
    /// timestamp therefore does not itself trigger a rebuild or cache writeback. A trade-tail or
    /// explicit reset can independently rebuild the series; writeback waits for the fingerprint to
    /// advance, normally when a new bucket arrives.
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
        // Preserve last_deep_request so request throttling survives a reset. Changing markets
        // recreates PaneRender and therefore starts with a fresh cursor.
    }
}

#[derive(Default)]
pub struct ChartHistoryBuffers {
    pub ticks: Vec<Tick>,
    /// Liquidation trades from the separate `readers.liquidations` ring.
    ///
    /// A reset returns the full visible range; otherwise only new live-edge rows are returned, as
    /// with `ticks`. The quantity sign carries the side, but the renderer assigns `side=2` and
    /// draws all liquidations with one color.
    pub liquidations: Vec<Tick>,
    pub last_points: Vec<PricePoint>,
    pub mark_points: Vec<PricePoint>,
    /// Complete visible candle series.
    ///
    /// Populated only when the series revision differs from
    /// `CandleReadParams::shipped_revision`; see `ChartHistoryRead::candles_changed`.
    pub candles: Vec<ChartCandle>,
    /// Timeframe in milliseconds for each entry in `candles`, stored as a parallel array.
    ///
    /// The historical prefix is extended with coarser timeframes, first 5-minute and then 1-day,
    /// whose candles require distinct widths. Empty means every candle uses the series timeframe.
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

/// Candle and trade-zone parameters for `read_chart_history_into`.
///
/// Passing `None` preserves legacy behavior: trade crosses only, without candles.
#[derive(Debug, Clone, Copy)]
pub struct CandleReadParams {
    /// Series timeframe in milliseconds.
    pub tf_ms: i64,
    /// Lower bound for displayed trades in milliseconds relative to the epoch.
    ///
    /// This defines the last-K-candles display zone. `f32::INFINITY` hides trades entirely when
    /// K is zero. It does not limit candle aggregation.
    pub trades_from_rel_ms: f32,
    /// Hard limit on the number of displayed trades.
    pub trades_limit: usize,
    /// Series revision already delivered to the renderer.
    ///
    /// When it matches the current revision, `out.candles` remains empty.
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
    /// Whether the candle series changed from `shipped_revision`, populating `out.candles`.
    pub candles_changed: bool,
    /// Current candle-series revision to return in the next `CandleReadParams`.
    pub candles_revision: u64,
}

/// Live timeframe-bar subscription state for one `(provider, market)`; see `candle_subs`.
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
    /// Global deduplication gate for coin-card requests, mapping request keys to send times.
    ///
    /// Cursors are per panel, so N windows for one coin would otherwise send N identical requests.
    /// Deep history consumes exchange request weight in the core, so the application sends at most
    /// one request per `(provider, market, kind_min)` every 30 seconds. The response enters shared
    /// retained state used by all panels.
    deep_req_gate: Mutex<HashMap<(CoreId, String, u32), Instant>>,
    /// Requested deep kinds for live candle panels, grouped by provider.
    ///
    /// Each `kind_min` maps to its most recent demand. The core holds one candle timeframe per
    /// core, according to the MoonBot developer on 2026-07-12, and each kind change refetches
    /// history from the exchange. Alternating kinds across windows can therefore trigger API
    /// limits. The effective core kind is the minimum live request because the supported kinds
    /// divide into the chain `1|5|30|60|240|1440`; coarser panels resample the finer base at the
    /// cost of depth. A demand entry remains live for 30 seconds.
    deep_kind_wants: Mutex<HashMap<CoreId, HashMap<u32, Instant>>>,
    /// Live timeframe-bar subscriptions keyed by `(provider, market)`.
    ///
    /// A subscription is global to the client and the most recent kind wins, so per-panel control
    /// repeatedly disturbed the core. Panels now only refresh demand. Entries stale for more than
    /// 60 seconds, such as closed panels or sub-minute timeframes, are unsubscribed during the next
    /// candle read for that provider.
    candle_subs: Mutex<HashMap<(CoreId, String), CandleSubState>>,
    /// Local kline cache in `klines.sqlite`; `None` until the terminal supplies its path.
    kline_cache: Option<crate::market::kline_cache::KlineCache>,
    /// Exchange identity used to share kline-cache rows across cores on the same exchange and to
    /// survive selected-provider changes. `CoreId` itself is a stable uid since schema v11, but it
    /// identifies one core rather than the exchange.
    provider_exchange: HashMap<CoreId, crate::feed::ExchangeId>,
    /// One-time native backfills for coarse timeframes during a session.
    ///
    /// Each `(provider, market, kind_min)` records one deliberate core timeframe-slot change when
    /// opening a coarse timeframe with an empty cache.
    native_backfill_done: Mutex<HashSet<(CoreId, String, u32)>>,
}

impl MarketDataSourceInner {
    /// The naming family of a market-data provider, for callers already holding the lock.
    ///
    /// Exists so a caller that needs the provider's client AND its naming family reads both under
    /// one guard: taken separately, a provider election between the two would spell markets for
    /// one exchange and price them against another's catalog.
    fn exchange_of_provider(&self, provider: CoreId) -> crate::symbol::Exchange {
        self.provider_exchange
            .get(&provider)
            .map(|id| crate::symbol::Exchange::from_code(id.code))
            .unwrap_or_default()
    }
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

/// Drain a last-price or mark-price line through their shared control flow.
///
/// The branches differ only in cursor, buffer, output, and converter. A reset or first call places
/// the cursor at now; subsequent calls drain new rows and accumulate `clipped` and `caught_up` in
/// `read`. After a change, the visible range is copied and converted to points. Call only when the
/// reader exists.
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

/// Convert last-price or mark-price line rows into chart points.
///
/// Both row types expose time through `SeqRingTimedRow` and price as the degenerate `(p, p)` range
/// through `SeqRingPriceRow`; these rows never return `None`.
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
