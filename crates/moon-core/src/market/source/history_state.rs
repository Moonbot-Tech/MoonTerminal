//! Per-consumer chart history cursors and buffers.

use super::*;

#[derive(Default)]
pub struct ChartHistoryCursor {
    pub(super) trades: Option<SeqRingCursor>,
    pub(super) liquidations: Option<SeqRingCursor>,
    pub(super) last_prices: Option<SeqRingCursor>,
    pub(super) mark_prices: Option<SeqRingCursor>,
    pub(super) last_price: Option<f32>,
    pub(super) trade_rows: Vec<TradeHistoryRow>,
    /// Oldest trade Unix-ms uploaded by the last full cross copy.
    ///
    /// Max uses it to decide when eviction or a backfill has moved the zone's left
    /// bucket. `None` means that copy held no trade. Cleared with the cursor.
    pub(super) displayed_oldest_ms: Option<i64>,
    pub(super) scan_trade_rows: Vec<TradeHistoryRow>,
    /// Every trade copied or drained since the last full copy, blocked for the auto-Y price fit.
    pub(super) price_fit: price_fit::PriceFitIndex,
    /// Fixture candles last uploaded by this pane, retained for viewport-only fitting without SQL.
    pub(super) fixture_candles: Vec<ChartCandle>,
    pub(super) liq_rows: Vec<TradeHistoryRow>,
    pub(super) last_price_rows: Vec<LastPricePoint>,
    pub(super) mark_price_rows: Vec<MarkPricePoint>,
    /// Merged candle series consisting of the server base and a local tail built from trades.
    /// Uses its own trade-ring cursor so aggregation is independent of the cross display range.
    pub(super) candle_series: CandleSeries,
    pub(super) candle_trades: Option<SeqRingCursor>,
    pub(super) candle_trade_rows: Vec<TradeHistoryRow>,
    pub(super) candle_ticks: Vec<Tick>,
    pub(super) server_candle_rows: Vec<moonproto::state::Candle5mRow>,
    pub(super) server_candles: Vec<ChartCandle>,
    /// Throttle for non-blocking CoinCard deep-history requests that provide authoritative OHLC.
    pub(super) last_deep_request: Option<Instant>,
    /// Most recently requested kind; a timeframe change bypasses the throttle.
    pub(super) last_deep_kind: Option<moonproto::DeepHistoryKind>,
    /// Coin-card retry backoff in seconds, where zero starts at 30 seconds.
    ///
    /// The core fetches deep history from the exchange API and consumes request weight. A retry
    /// without progress doubles the delay up to 10 minutes, while new rows reset it. Without this
    /// backoff, a stalled core or exchange would receive a request storm from every open chart and
    /// trigger the core's API-limit auto-stop.
    pub(super) deep_retry_delay_s: u32,
    /// Prefix loaded from the local kline cache using the panel's native kind.
    ///
    /// It is read from SQLite once per `(market, kind, left edge)` and survives series resets.
    /// Resets are frequent during pan and zoom, so each reset must not query the database.
    pub(super) cache_rows: Vec<ChartCandle>,
    pub(super) cache_kind: Option<u32>,
    /// The native kind `cache_rows` and `cache_rows_finer` were READ at.
    ///
    /// Set in the same pass as the rows, so it stays right when a later pass computes another
    /// native kind while the retry gate keeps the read from running: the merge tags the rows with
    /// this, not with the pass's own kind, or kind-5 rows would go into a 1-minute series as
    /// 1-minute candles for the length of that gate.
    pub(super) cache_rows_kind: u32,
    /// The native kind's holes, filled from the cached kinds finer than it — kind 1 for a
    /// 5-minute chart, kinds 1 and 5 for 30 minutes and up — already aggregated to the native
    /// kind's timeframe (`cache_rows_kind`), coarser finer kind winning a bucket.
    ///
    /// Read in the same pass as `cache_rows` and over the same window, but only when `cache_rows`
    /// leaves a hole in it; aggregated once here rather than at every series reset, which during
    /// a pan would re-walk up to 30 days of 1-minute rows. At merge time these rank below the
    /// native rows and above the range-only snapshot, instead of the old fallback that consulted
    /// the finer kinds only when the native kind had no rows at all (#634).
    pub(super) cache_rows_finer: Vec<ChartCandle>,
    pub(super) cache_from_ms: i64,
    /// Cache-only coarser layers used to extend the historical prefix as far back as possible.
    ///
    /// The 5-minute layer contains kind-5 cache rows from the recorder and possible deep-history
    /// writeback; the retained 5-minute snapshot separately feeds the main series through
    /// `snap_part`. The 1-day layer comes from backfill and cache.
    pub(super) cache_rows_5m: Vec<ChartCandle>,
    pub(super) cache_rows_1d: Vec<ChartCandle>,
    /// The core's OWN retained 5-minute ring, kept as a coarse fill layer for sub-5m timeframes.
    ///
    /// `snap_part` already merges this ring into the series at 5 minutes and coarser, where it can
    /// be resampled. A 1-minute series cannot resample it — but it can still DRAW it in a hole, and
    /// this is the only layer that covers a stretch during which the CORE was up and the terminal
    /// was not. Rows arrive end-stamped and range-only, so they are shifted and oriented on the way
    /// in and stored ready to use.
    pub(super) ring_rows_5m: Vec<ChartCandle>,
    /// Bumped whenever the coarse layers above are reread, so the composed fill below can tell a
    /// stale cache from a stale series without comparing the row vectors themselves.
    pub(super) cache_generation: u64,
    /// When the last cache read TIMED OUT, so the retry is not attempted on every frame.
    ///
    /// `None` means the last attempt completed, whatever it found.
    pub(super) cache_retry_at: Option<Instant>,
    /// The one kline-cache prefix read in flight, picked up by polling on a later frame.
    pub(super) cache_pending: Option<history::PendingCacheRead>,
    /// Leftmost edge wanted while a read is in flight, asked for by the next read; `None` = none.
    pub(super) cache_want_from: Option<i64>,
    /// `(exchange key, market)` the held cache rows belong to; a change drops them at once.
    pub(super) cache_identity: Option<(String, String)>,
    /// Bumped by the cache worker whenever a prefix read this cursor asked for is answered.
    pub(super) cache_done: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// The series plus its coarse gap fillers, each tagged with the timeframe it is drawn at.
    ///
    /// Retained rather than rebuilt per block because the two consumers fire INDEPENDENTLY: the
    /// upload runs only when the series revision moved, while the auto-Y scan runs every frame.
    /// Deriving the fill twice is how the price scale and the drawn candles came to disagree about
    /// which coarse rows exist; one vector makes that unrepresentable.
    pub(super) coarse_fill: Vec<(ChartCandle, f32)>,
    /// Widest timeframe `coarse_fill` was composed with, so a window scan can bound its start
    /// without a pass over the fill. `None` means unknown: scan from the first row.
    pub(super) coarse_fill_max_tf: Option<f64>,
    /// `(series layout revision, cache generation)` the retained fill was composed from; a
    /// same-layout change is patched into the fill's tail instead of recomposing it.
    pub(super) coarse_fill_key: Option<(u64, u64)>,
    /// What the next candle emission must ship.
    pub(super) candle_emit: CandleEmit,
    /// Series revision this reader last emitted candles at; a tail patch is only valid against it.
    pub(super) candles_emitted_rev: Option<u64>,
    /// Signature of the last deep rows written to the cache; write back only after a change.
    pub(super) cache_written_sig: u64,
    /// `(exchange key, deep kind minutes)` the deep-row writeback last wrote for.
    pub(super) cache_written_key: Option<(String, u32)>,
    /// Open time of the first deep row at the last writeback; `i64::MIN` = nothing written.
    pub(super) cache_written_first_ms: i64,
    /// Open time of the last deep row at the last writeback; `i64::MIN` = nothing written.
    pub(super) cache_written_last_ms: i64,
    /// Deep row count at the last writeback.
    pub(super) cache_written_len: usize,
    /// Tail-only writebacks since the last full one; a full rewrite every
    /// `DEEP_FULL_WRITEBACK_EVERY` heals middle rows a tail write cannot see.
    pub(super) cache_writebacks_since_full: u32,
    /// Left edge the current candle series was built from; one window span left of the edge that
    /// was needed at build time, so a leftward pan within that span does not rebuild. `i64::MAX` =
    /// no series built. The derived default 0 is re-anchored by `series_floor` the same way.
    pub(super) series_from_base_ms: i64,
    /// `cache_generation` the current series was built with; a different value means kline-cache
    /// rows landed after the build and only a rebuild merges them into the base.
    pub(super) series_cache_generation: u64,
    /// Throttle for candle-to-now gap diagnostics: at most one warning per panel every 30 seconds.
    pub(super) last_gap_diag: Option<Instant>,
    /// When the O(N) internal-gap scan last ran; it runs at most every 30 s whether or not a
    /// warning fired.
    pub(super) last_gap_scan: Option<Instant>,
    /// Equivalent signature for rows of the panel's native kind.
    ///
    /// These rows are produced by native backfill attempts when the core's effective kind is finer
    /// than the panel's native kind.
    pub(super) cache_written_native_sig: u64,
    /// Low-cost fingerprint of loaded deep rows at the last series rebuild.
    ///
    /// It hashes only the row count and final timestamp. An in-place OHLC update at the same final
    /// timestamp therefore does not itself trigger a rebuild or cache writeback. A trade-tail or
    /// explicit reset can independently rebuild the series; writeback waits for the fingerprint to
    /// advance, normally when a new bucket arrives.
    pub(super) last_deep_sig: u64,
    /// The resolved `exchange_key` (`"{code}:{dex}"`, `history.rs`'s cache address) from the last
    /// pass, `None` before the provider's `ExchangeId` was ever resolved.
    ///
    /// Keyed on the ADDRESS rather than on the derived `deep_quote: bool` a first cut of this fix
    /// used: neither `cache_stale` nor `series_reset` otherwise tracks the exchange key at all, so
    /// a bool derived from it cannot see every transition the key itself can. Two cases that a
    /// bool misses and the key catches:
    /// - A **SPOT** provider's `deep_quote` is `false` before its identity resolves and `false`
    ///   after — the bool never changes, so a bool-keyed version never re-reads the now-addressable
    ///   cache and that core's history stays empty for the whole session.
    /// - A provider **ELECTION** that changes which core answers for a market changes the exchange
    ///   key (and therefore the cache address) without necessarily changing `deep_quote` — e.g. two
    ///   futures cores on the same brand. `cache_stale` never tracked the key either, so this was a
    ///   pre-existing bug; keying invalidation on the key fixes it for free, which is why it is now
    ///   IN scope rather than the adjacent bug batch 2 left alone.
    ///
    /// Stores the owned `String` rather than a hash: the key is a handful of bytes
    /// (`"{code}:{08x-dex-hash}"`), resolved once per pass per panel rather than in a hot loop, so
    /// a hash would trade a real equality check for a collision risk to save a copy nobody
    /// measured as expensive.
    ///
    /// `Option` rather than a bare key so "never resolved" is representable — but note what that
    /// actually buys: `cursor.last_exchange_key != exchange_key` is `true` on the very first pass
    /// too (`None != Some(_)`), so the first pass IS technically read as "changed". That is
    /// harmless rather than incorrect: on the first pass `cache_kind` is already `None` and
    /// `series_reset` is already `true` via `!candle_series.is_valid()`, so the extra transition
    /// signals nothing new. The `Option` earns its keep on RE-entry instead — a market whose
    /// provider identity is known, then goes unknown, then resolves again — where `None` still
    /// reads as a fresh unknown rather than colliding with any real key string.
    pub(super) last_exchange_key: Option<String>,
}

impl ChartHistoryCursor {
    /// Prefix reads answered for this cursor so far; a change means its reply can be picked up.
    pub fn cache_completions(&self) -> u64 {
        self.cache_done.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Forces a fresh prefix read. The read in flight is dropped too: the worker answers in FIFO
    /// order, so a read queued before a merge would return the pre-merge rows and mark the prefix
    /// fresh.
    pub(crate) fn invalidate_cache_prefix(&mut self) {
        self.cache_kind = None;
        self.cache_pending = None;
    }

    /// Drops every held cache layer and bumps the generation so the composed fill recomputes.
    pub(super) fn drop_cache_rows(&mut self) {
        self.cache_rows.clear();
        self.cache_rows_finer.clear();
        self.cache_rows_5m.clear();
        self.cache_rows_1d.clear();
        self.cache_generation = self.cache_generation.wrapping_add(1);
    }

    pub fn reset(&mut self) {
        self.trades = None;
        self.liquidations = None;
        self.last_prices = None;
        self.mark_prices = None;
        self.last_price = None;
        self.trade_rows.clear();
        self.displayed_oldest_ms = None;
        self.scan_trade_rows.clear();
        self.price_fit.clear();
        self.liq_rows.clear();
        self.last_price_rows.clear();
        self.mark_price_rows.clear();
        self.candle_series.invalidate();
        self.candle_trades = None;
        self.candle_trade_rows.clear();
        self.candle_ticks.clear();
        self.server_candle_rows.clear();
        self.server_candles.clear();
        // The composed fill is derived from the series, so it cannot outlive an invalidated one.
        self.ring_rows_5m.clear();
        self.coarse_fill.clear();
        self.coarse_fill_key = None;
        self.coarse_fill_max_tf = None;
        self.series_from_base_ms = i64::MAX;
        self.series_cache_generation = 0;
        self.candle_emit = CandleEmit::Full;
        self.candles_emitted_rev = None;
        self.cache_pending = None;
        self.cache_want_from = None;
        self.cache_written_key = None;
        self.cache_written_first_ms = i64::MIN;
        self.cache_written_last_ms = i64::MIN;
        self.cache_written_len = 0;
        self.cache_writebacks_since_full = 0;
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
    /// Composed candle list: the whole of it, or only its changed tail when
    /// `ChartHistoryRead::candles_patch_from` is `Some`.
    ///
    /// Populated only when `ChartHistoryRead::candles_changed` is set.
    pub candles: Vec<ChartCandle>,
    /// Timeframe in milliseconds for each entry in `candles`, stored as a parallel array.
    ///
    /// The historical prefix is extended with coarser timeframes, first 5-minute and then 1-day,
    /// whose candles require distinct widths. Empty means every candle uses the series timeframe.
    pub candle_tf_ms: Vec<f32>,
}

impl ChartHistoryBuffers {
    pub(super) fn clear(&mut self) {
        self.ticks.clear();
        self.liquidations.clear();
        self.last_points.clear();
        self.mark_points.clear();
        self.candles.clear();
        self.candle_tf_ms.clear();
    }
}

impl ChartHistoryRead {
    /// Marks both price lines as replaced by what the read carries (possibly nothing), so a bench
    /// or a replay clears the live lines instead of leaving them drawn.
    pub(crate) fn replace_price_lines(&mut self) {
        self.last_line = PriceLineUpdate::Replace;
        self.mark_line = PriceLineUpdate::Replace;
        self.price_lines_changed = true;
    }
}

/// What one read carries for a price line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PriceLineUpdate {
    /// Nothing new; the output is empty.
    #[default]
    None,
    /// The output holds only the newly drained rows, to be appended to what the receiver holds.
    Append,
    /// The output holds the whole window, replacing what the receiver holds.
    Replace,
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
    /// K is zero. [`crate::market::candles::TRADES_FROM_UNBOUNDED`] asks for every retained
    /// trade and lets the ring capacity clamp the copy. It does not limit candle aggregation.
    pub trades_from_rel_ms: f32,
    /// Hard limit on the number of displayed trades.
    pub trades_limit: usize,
    /// Series revision already delivered to the renderer.
    ///
    /// When it matches the current revision, `out.candles` remains empty.
    pub shipped_revision: u64,
    /// Series-relevant reset reasons only (source generation or archive change, candle config
    /// change, first read). A camera pan or a trade/combo reset must NOT set it: the candle series
    /// is anchored on its own history floor, not on the camera.
    pub series_reset: bool,
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
    /// What `out.last_points` carries for the last-price line.
    pub last_line: PriceLineUpdate,
    /// What `out.mark_points` carries for the mark-price line.
    pub mark_line: PriceLineUpdate,
    /// `None`: `out.candles` is the whole composed list. `Some(i)`: `out.candles` replaces the
    /// composed entries from index `i` on; the list's new length is `i + out.candles.len()`. Only
    /// sent when the receiver's `shipped_revision` equals the revision this reader last emitted.
    pub candles_patch_from: Option<usize>,
}
