//! Source read plane for revisions, prices, ticker data, search, and chart-history draining.

use crate::data::OrderBookModel;
use crate::feed::SharedMoonClient;
use crate::session::CoreId;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use moonproto::DeepHistoryKind;

/// Convert a retained CoinCard row into a chart candle, normalizing shuffled wire fields.
fn deep_row_candle(r: &moonproto::DeepPrice) -> crate::market::candles::ChartCandle {
    let (open, high, low, close) =
        crate::market::candles::normalize_ohlc(r.open(), r.high(), r.low(), r.close());
    crate::market::candles::ChartCandle {
        t_open_ms: r.unix_millis() as f64,
        open,
        high,
        low,
        close,
        volume: r.volume(),
    }
}

use super::{
    drain_price_line, moon_time_from_rel_ms, price_rows_to_points, rows_to_ticks,
    trade_price_range, CandleReadParams, ChartHistoryBuffers, ChartHistoryCursor, ChartHistoryRead,
    DetectSnapshot, LatestPriceError, MarketDataSource, MarketRevisions, MarketTickerReadout,
};
use crate::market::candles::ChartCandle;

/// Return a short exchange-type label from `server_info.exchange_type_mask`.
///
/// The core is i18n-agnostic, so spot and futures labels are plain Russian strings that the UI may
/// relocalize. The mask represents connection capabilities; a single connection usually sets one
/// trading bit.
fn exchange_kind_label(info: &moonproto::ServerInfo) -> String {
    use moonproto::ExchangeTypeMask as M;
    let spot = info.supports(M::SPOT);
    let fut = info.supports(M::FUTURES);
    let dex = info.supports(M::DEX);
    match (dex, fut, spot) {
        (true, _, _) => "DEX".to_string(),
        (false, true, false) => "Фьючи".to_string(),
        (false, false, true) => "Спот".to_string(),
        (false, true, true) => "Спот/Фьючи".to_string(),
        _ => String::new(),
    }
}

/// Map a CoinCard history timeframe in minutes to its MoonProto wire kind.
fn deep_history_kind(tf_min: u32) -> DeepHistoryKind {
    match tf_min {
        1 => DeepHistoryKind::Min1,
        30 => DeepHistoryKind::Min30,
        60 => DeepHistoryKind::Hour1,
        240 => DeepHistoryKind::Hour4,
        1440 => DeepHistoryKind::Day1,
        _ => DeepHistoryKind::Min5,
    }
}

impl MarketDataSource {
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
            .get(&provider)
            .and_then(|markets| markets.get(market))
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

    /// Return the USD rate for `currency`.
    ///
    /// A USD stablecoin maps to 1; otherwise this uses `p_last` for `<currency>USDT`, such as
    /// `BTCUSDT` for BTC. `None` means the provider, snapshot, market, or rate is unavailable. This
    /// uses the same linear model as `feed::assets`, without contract multipliers.
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
            inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)?
        };
        let snapshot = client.snapshot_versioned()?;
        let market = format!("{}USDT", currency.to_ascii_uppercase());
        let p = snapshot.markets().price(&market)?;
        (p.p_last.is_finite() && p.p_last > 0.0).then_some(p.p_last)
    }

    /// Return the USD rate for the quote currency of `market`.
    ///
    /// This converts `quantity * price` notional into USD. A USDT quote maps to 1, while a BTC quote
    /// uses the BTC/USDT rate. `None` means the rate is unknown.
    pub fn quote_usd_rate(&self, core: CoreId, market: &str) -> Option<f64> {
        let quote = crate::symbol::resolve_quote(market);
        if quote.is_empty() {
            // HL/HIP-3 DEX perpetuals use names such as `xyz:BIRD`, consisting of a DEX prefix and
            // coin. Their USDC quote is absent from the name, so the suffix parser cannot find it.
            // These markets are nevertheless quoted in USDC, a USD stablecoin with a rate near 1.
            // Without this fallback, `quote_usd` was None and the size label fell back from a USD
            // notional such as `$50` to a coin quantity such as `11.8`.
            return Some(1.0);
        }
        self.currency_usd_rate(core, &quote)
    }

    /// Return the market-data provider core for a consumer core.
    ///
    /// This is the exchange deduplication key: cores on the same exchange share a provider. The
    /// screener groups cores by this value to avoid duplicate coins.
    pub fn provider_of(&self, core: CoreId) -> Option<CoreId> {
        self.inner
            .read()
            .expect("market source poisoned")
            .core_provider
            .get(&core)
            .copied()
    }

    /// Return the live MoonProto client for the specific core, not its market-data provider.
    ///
    /// The screener uses this for account fields that belong to the individual core.
    pub(crate) fn core_client(
        &self,
        core: CoreId,
    ) -> Option<std::sync::Arc<moonproto::MoonClient>> {
        let inner = self.inner.read().expect("market source poisoned");
        inner.clients.get(&core).and_then(SharedMoonClient::get)
    }

    /// Return trimmed display exchange names for live cores whose own clients reported one.
    ///
    /// This reads each core's direct client rather than its deduplicated market-data provider, so
    /// connection rows retain their own identity. Missing clients, snapshots, and blank names are
    /// omitted from the returned map.
    pub fn core_exchange_names(&self) -> HashMap<CoreId, String> {
        let clients: Vec<(CoreId, std::sync::Arc<moonproto::MoonClient>)> = {
            let inner = self.inner.read().expect("market source poisoned");
            inner
                .clients
                .iter()
                .filter_map(|(&core, client)| client.get().map(|client| (core, client)))
                .collect()
        };
        clients
            .into_iter()
            .filter_map(|(core, client)| {
                let name = client.snapshot()?.server_info().exchange_name.clone()?;
                let name = name.trim();
                (!name.is_empty()).then(|| (core, name.to_string()))
            })
            .collect()
    }

    /// Return the market price step from MoonProto's `chart_price_step`.
    ///
    /// This is the keyboard increment for `shift_buy/sell_up/down`. `None` means the provider,
    /// snapshot, or market is unavailable, or the step is non-positive. In that case orders are not
    /// shifted because the terminal must not invent an increment.
    pub fn price_step(&self, core: CoreId, market: &str) -> Option<f64> {
        let client = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)?
        };
        let snapshot = client.snapshot_versioned()?;
        let step = snapshot.markets().price(market)?.chart_price_step;
        (step.is_finite() && step > 0.0).then_some(step)
    }

    /// Return the last price and signed 1-hour and 24-hour percentage deltas for a market.
    ///
    /// MoonProto's `MarketDeltaState` defines `coin_1h_delta` and `coin_24h_delta` as deviations
    /// from retained averages, matching MoonBot's Coin1hDelta semantics. This feeds the header
    /// ticker and a future screener. `None` means the provider, snapshot, or market is unavailable.
    pub fn market_ticker(&self, core: CoreId, market: &str) -> Option<MarketTickerReadout> {
        let client = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)?
        };
        let snapshot = client.snapshot_versioned()?;
        let last = snapshot
            .markets()
            .price(market)
            .map(|p| p.p_last)
            .filter(|p| p.is_finite() && *p > 0.0)?;
        let delta = snapshot.markets().delta_state(market).unwrap_or_default();
        Some(MarketTickerReadout {
            last,
            delta_1h_pct: delta.coin_1h_delta,
            delta_24h_pct: delta.coin_24h_delta,
        })
    }

    /// Build a frozen snapshot for a detection card.
    ///
    /// The snapshot contains the latest `bars` 5-minute OHLC candles, ordered oldest to newest,
    /// plus the exchange name and type. History combines the provider's retained 5-minute snapshot,
    /// the local kline cache populated for every market by the background recorder, and the live
    /// trade-ring tail. None of these sources calls the exchange API here. Data belongs to the
    /// exchange's deduplicated provider and is shared by cores on the same exchange and market.
    /// Build this once when the detection occurs and retain it in the card. Missing provider,
    /// client, or snapshot yields the empty default. Missing history leaves `bars` and `line` empty
    /// while exchange metadata can remain populated.
    pub fn detect_snapshot(&self, core: CoreId, market: &str, bars: usize) -> DetectSnapshot {
        let t0 = std::time::Instant::now();
        let (client, kline_cache, exchange_key) = {
            let inner = self.inner.read().expect("market source poisoned");
            let Some(provider) = inner.core_provider.get(&core).copied() else {
                return DetectSnapshot::default();
            };
            let Some(client) = inner.clients.get(&provider).and_then(SharedMoonClient::get) else {
                return DetectSnapshot::default();
            };
            // Use the provider's stable exchange key for the kline cache, as in
            // read_chart_history_into.
            let exchange_key = inner
                .provider_exchange
                .get(&provider)
                .map(|e| format!("{}:{:08x}", e.code, e.dex));
            (client, inner.kline_cache.clone(), exchange_key)
        };
        let Some(snapshot) = client.snapshot_versioned() else {
            return DetectSnapshot::default();
        };
        let mut out = DetectSnapshot::default();
        // Read the exchange name and type from the connection identity supplied by BaseCheck's
        // server_info.
        let info = snapshot.server_info();
        if let Some(name) = &info.exchange_name {
            out.exchange_name = name.clone();
        }
        out.exchange_kind = exchange_kind_label(info);

        let tf_ms: i64 = 300_000; // 5 minutes
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        // Candle mode takes its latest `bars` buckets, roughly two hours, from a 24-hour window;
        // line mode uses every close in that window. `line_cap` is the maximum number of 5-minute
        // buckets in 24 hours: 288.
        let from_ms = now_ms - 24 * 3_600_000;
        let line_cap = (24 * 3_600_000 / tf_ms) as usize;

        // Build 5-minute candle buckets keyed by `t_open`, floored to the timeframe so sources
        // align. Base 1 is the core's retained 5-minute startup snapshot, which supplies depth;
        // base 2 is the local kline cache with real OHLC and overrides the snapshot; the trade ring
        // is the live tail and overrides recent buckets. Insertion order defines freshness priority:
        // snapshot < cache < ring. The snapshot used to be only an empty-data fallback, so the ring
        // always supplied a few bars and caused the snapshot's depth to be ignored.
        let mut buckets: std::collections::BTreeMap<i64, (f32, f32, f32, f32)> =
            std::collections::BTreeMap::new();
        let bucket_key = |t_ms: i64| (t_ms.max(0) / tf_ms) * tf_ms;
        let mut snap5_n = 0usize;
        let mut cache_n = 0usize;

        if let Some(readers) = snapshot.market_history_readers(market) {
            // Base 1 is the core's 5-minute snapshot. It carries only high and low, represented as
            // open == high and close == low, so the body spans the range. Rows are stamped at the
            // end of the period; shift them back one timeframe to align their open with the cache.
            // As in the chart, normalize_ohlc and orient_range_rows orient these range-only candles
            // by the average-price trend. Otherwise body-only rendering would color the entire
            // history in one direction because close < open.
            if let Some(candles) = readers.candles_5m {
                let mut snap: Vec<ChartCandle> = Vec::new();
                candles.with_last(line_cap, |view| {
                    view.for_each(|c| {
                        let (o, h, l, cl) = crate::market::candles::normalize_ohlc(
                            c.open(),
                            c.high(),
                            c.low(),
                            c.close(),
                        );
                        if h.is_finite() && l.is_finite() && h > 0.0 {
                            snap.push(ChartCandle {
                                t_open_ms: (c.time().unix_millis() - tf_ms) as f64,
                                open: o,
                                high: h,
                                low: l,
                                close: cl,
                                volume: 0.0,
                            });
                        }
                    });
                });
                crate::market::candles::orient_range_rows(&mut snap);
                for c in &snap {
                    buckets.insert(
                        bucket_key(c.t_open_ms as i64),
                        (c.open, c.high, c.low, c.close),
                    );
                }
                snap5_n = buckets.len();
            }
            // Base 2 is the kline cache: real OHLC from prior minutes and sessions at kind_min 5.
            if let (Some(cache), Some(ex)) = (kline_cache.as_ref(), exchange_key.as_ref()) {
                for c in cache.read_range(ex, market, 5, from_ms, now_ms) {
                    if c.high.is_finite() && c.low.is_finite() && c.high > 0.0 {
                        buckets.insert(
                            bucket_key(c.t_open_ms as i64),
                            (c.open, c.high, c.low, c.close),
                        );
                    }
                }
            }
            cache_n = buckets.len();
            // Aggregate the provider trade-ring tail into 5-minute candles that override recent
            // buckets.
            if let Some(reader) = readers.futures_trades.or(readers.spot_trades) {
                let from_t = moonproto::MoonTime::from_unix_millis(from_ms);
                let to_t = moonproto::MoonTime::from_unix_millis(now_ms);
                let mut rows = Vec::new();
                reader.copy_time_range(from_t, to_t, reader.capacity(), &mut rows);
                let mut ticks = Vec::new();
                rows_to_ticks(&rows, &mut ticks);
                let mut candles = Vec::new();
                crate::market::candles::aggregate_trades(&ticks, tf_ms, &mut candles);
                for c in &candles {
                    if c.high.is_finite() && c.low.is_finite() && c.high > 0.0 {
                        buckets.insert(
                            bucket_key(c.t_open_ms as i64),
                            (c.open, c.high, c.low, c.close),
                        );
                    }
                }
            }
        }

        // With MOON_DETECT_DIAG=1, report actual bucket structure: candle count, total span, maximum
        // time gap, and price range. Index-based rendering hides time gaps, but they reveal sparse
        // data; a price outlier explains flattening. This avoids guessing about discontinuities.
        if std::env::var_os("MOON_DETECT_DIAG").is_some() {
            let keys: Vec<i64> = buckets.keys().copied().collect();
            let max_gap = keys.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0);
            let (mut pmin, mut pmax) = (f32::INFINITY, f32::NEG_INFINITY);
            for &(_, bh, bl, _) in buckets.values() {
                pmax = pmax.max(bh);
                pmin = pmin.min(bl);
            }
            let span_ms = keys.last().copied().unwrap_or(0) - keys.first().copied().unwrap_or(0);
            log::info!(
                "detect_thumb {market}: n={} (snap5={} cache+={} ex_key={}) span_ms={} \
                 max_gap_ms={} (tf={}) price=[{:.6},{:.6}] data={:.1}мкс",
                keys.len(),
                snap5_n,
                cache_n.saturating_sub(snap5_n),
                exchange_key.as_deref().unwrap_or("<none>"),
                span_ms,
                max_gap,
                tf_ms,
                pmin,
                pmax,
                t0.elapsed().as_nanos() as f64 / 1000.0
            );
        }

        // Line mode uses every close in the 24-hour window, ordered oldest to newest.
        out.line = buckets.values().map(|&(_, _, _, c)| c).collect();
        // Candle mode uses the latest `bars` buckets, roughly two hours, oldest to newest.
        let start = buckets.len().saturating_sub(bars);
        out.bars = buckets.values().skip(start).copied().collect();
        // Derive actual 1-hour and 24-hour price changes from our buckets by comparing now with the
        // earlier price, so the deltas match the line's movement. MoonProto's coin_*_delta measures
        // deviation from the period average instead and remains the header ticker metric.
        if let Some((_, &(_, _, _, last))) = buckets.iter().next_back() {
            let close_at = |ago_ms: i64| -> Option<f32> {
                let target = now_ms - ago_ms;
                buckets
                    .range(..=target)
                    .next_back()
                    .map(|(_, &(_, _, _, c))| c)
                    .or_else(|| buckets.values().next().map(|&(_, _, _, c)| c))
            };
            if let Some(c1) = close_at(3_600_000).filter(|v| *v > 0.0) {
                out.delta_1h = (last - c1) / c1 * 100.0;
            }
            if let Some(c24) = close_at(24 * 3_600_000).filter(|v| *v > 0.0) {
                out.delta_24h = (last - c24) / c24 * 100.0;
            }
        }
        out
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

    #[allow(clippy::too_many_arguments)]
    pub fn read_chart_history_into(
        &self,
        core: CoreId,
        market: &str,
        epoch_ms: f64,
        from_rel_ms: f32,
        to_rel_ms: f32,
        force_reset: bool,
        scan_price: bool,
        candle_params: Option<&CandleReadParams>,
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
        // Read trade crosses and scans only from the last-K-candles display zone. INFINITY hides
        // trades entirely when K is zero; it does not constrain candle aggregation.
        let display_trades = candle_params.map_or(true, |cp| cp.trades_from_rel_ms.is_finite());
        let trades_from_rel = candle_params
            .map(|cp| cp.trades_from_rel_ms.max(from_rel_ms))
            .filter(|v| v.is_finite())
            .unwrap_or(from_rel_ms);
        let trades_from_time = moon_time_from_rel_ms(epoch_ms, trades_from_rel);
        let trades_limit = candle_params.map_or(usize::MAX, |cp| cp.trades_limit.max(1));
        let mut read = ChartHistoryRead {
            provider,
            revision,
            caught_up: true,
            ..ChartHistoryRead::default()
        };

        let trade_reader = readers.futures_trades.or(readers.spot_trades);
        if let Some(reader) = trade_reader.as_ref().filter(|_| display_trades) {
            read.combo_capacity = reader.capacity();
            let display_cap = reader.capacity().min(trades_limit);
            let reset = force_reset || cursor.trades.is_none();
            if reset {
                reader.copy_time_range(
                    trades_from_time,
                    to_time,
                    display_cap,
                    &mut cursor.trade_rows,
                );
                cursor.trades = Some(reader.cursor_from_now());
                read.combo_reset = true;
                read.caught_up = true;
            } else if let Some(cur) = cursor.trades.as_mut() {
                let meta = reader.drain_new_bounded(cur, display_cap, &mut cursor.trade_rows);
                read.clipped |= meta.clipped;
                read.caught_up &= meta.caught_up;
                if meta.clipped {
                    reader.copy_time_range(
                        trades_from_time,
                        to_time,
                        display_cap,
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
                    trades_from_time,
                    to_time,
                    display_cap,
                    &mut cursor.scan_trade_rows,
                );
                read.tick_price_range = trade_price_range(&cursor.scan_trade_rows);
            }
        } else {
            cursor.trades = None;
            cursor.last_price = None;
            // Trades are hidden when K is zero, but the ring still supplies last_price. On reset,
            // combo_reset instructs the layer to clear its cross ring.
            if let Some(reader) = trade_reader.as_ref() {
                read.combo_capacity = reader.capacity();
                if force_reset {
                    read.combo_reset = true;
                }
                cursor.trade_rows.clear();
                reader.copy_last(1, &mut cursor.trade_rows);
                if let Some(row) = cursor.trade_rows.last() {
                    cursor.last_price = Some(row.price);
                }
            }
        }

        // Liquidations use a separate ring of the same type and stay synchronized with combo. A
        // full combo reset or first pass rereads the entire visible range; otherwise only the new
        // live edge is drained. The renderer tags them with side=2 for one shared color. Their
        // window matches normal trades: the last-K-candles zone.
        if let Some(reader) = readers.liquidations.as_ref().filter(|_| display_trades) {
            let reset = read.combo_reset || cursor.liquidations.is_none();
            if reset {
                reader.copy_time_range(
                    trades_from_time,
                    to_time,
                    reader.capacity(),
                    &mut cursor.liq_rows,
                );
                cursor.liquidations = Some(reader.cursor_from_now());
            } else if let Some(cur) = cursor.liquidations.as_mut() {
                let meta = reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.liq_rows);
                if meta.clipped {
                    reader.copy_time_range(
                        trades_from_time,
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

        // The candle series combines the server's automatic 5-minute MoonProto snapshot, which
        // covers history before connection, with a local tail built from trades. Its own trade-ring
        // cursor keeps aggregation independent of the cross display zone. Only reset or timeframe
        // changes require a full rebuild; the live edge cheaply drains new rows.
        if let Some(cp) = candle_params {
            // CoinCard deep history applies only to timeframes of at least one minute. Sub-minute
            // candles are built from trades.
            let use_deep = cp.tf_ms >= 60_000;
            let native_kind_min =
                crate::market::candles::deep_kind_min_for_tf((cp.tf_ms / 60_000) as u32);
            // The core holds one candle timeframe per core, according to the MoonBot developer on
            // 2026-07-12. Alternating kinds between windows with different timeframes made the core
            // refetch exchange history for every request or subscription and could trigger API
            // limits. The provider's effective kind is the minimum live request because the kinds
            // divide into the chain 1|5|30|60|240|1440. Coarser panels resample the finer base at the
            // cost of depth; the core ring holds about 10,000 rows of the base timeframe.
            let deep_kind_min = if use_deep {
                let inner = self.inner.read().expect("market source poisoned");
                let mut wants = inner
                    .deep_kind_wants
                    .lock()
                    .expect("deep kind wants poisoned");
                let now_i = Instant::now();
                let m = wants.entry(provider).or_default();
                m.insert(native_kind_min, now_i);
                m.retain(|_, t| now_i.duration_since(*t) < Duration::from_secs(30));
                m.keys().copied().min().unwrap_or(native_kind_min)
            } else {
                native_kind_min
            };
            let deep_kind = deep_history_kind(deep_kind_min);
            // Pair the local kline-cache handle with the exchange key so cores on one exchange share
            // cached rows and provider election can change without changing the cache address.
            // CoreId itself is a stable uid since schema v11, but it identifies one core.
            let (kline_cache, exchange_key) = {
                let inner = self.inner.read().expect("market source poisoned");
                (
                    inner.kline_cache.clone(),
                    inner
                        .provider_exchange
                        .get(&provider)
                        .map(|e| format!("{}:{:08x}", e.code, e.dex)),
                )
            };
            // Subscribe to the core's live timeframe bars. Event::LiveCandle appends or replaces
            // the last retained tf_candles row. Without it, deep rows freeze at response time and
            // coarse-timeframe series can lag by hours. The subscription is global to the client
            // and the most recent kind wins, so a shared `(provider, market)` registry lets panels
            // refresh demand while entries stale for more than 60 seconds are unsubscribed.
            {
                let inner = self.inner.read().expect("market source poisoned");
                let mut subs = inner.candle_subs.lock().expect("candle subs poisoned");
                let now_i = Instant::now();
                if use_deep {
                    let entry = subs.entry((provider, market.to_string())).or_insert(
                        super::CandleSubState {
                            kind_min: deep_kind_min,
                            last_want: now_i,
                            subscribed: false,
                        },
                    );
                    if !entry.subscribed || entry.kind_min != deep_kind_min {
                        if client
                            .streams()
                            .subscribe_candles([market], deep_kind)
                            .is_ok()
                        {
                            entry.subscribed = true;
                            entry.kind_min = deep_kind_min;
                        }
                    }
                    entry.last_want = now_i;
                }
                // Remove stale subscriptions for this provider while its client is available.
                let stale: Vec<String> = subs
                    .iter()
                    .filter(|((p, _), s)| {
                        *p == provider
                            && s.subscribed
                            && now_i.duration_since(s.last_want) > Duration::from_secs(60)
                    })
                    .map(|((_, m), _)| m.clone())
                    .collect();
                for m in stale {
                    let _ = client.streams().unsubscribe_candles([m.as_str()]);
                    subs.remove(&(provider, m));
                }
            }
            // Load authoritative native klines from prior sessions as a local-cache prefix. Read
            // SQLite once per `(market, kind, left edge)` because pan and zoom reset frequently;
            // extending the window to the left triggers another read.
            if use_deep {
                let need_from = (epoch_ms + (from_rel_ms - cp.tf_ms.max(0) as f32) as f64) as i64;
                let cache_stale =
                    cursor.cache_kind != Some(native_kind_min) || need_from < cursor.cache_from_ms;
                if cache_stale {
                    cursor.cache_rows.clear();
                    cursor.cache_rows_5m.clear();
                    cursor.cache_rows_1d.clear();
                    cursor.cache_kind = Some(native_kind_min);
                    cursor.cache_from_ms = need_from;
                    if let (Some(cache), Some(ex)) = (kline_cache.as_ref(), exchange_key.as_deref())
                    {
                        cursor.cache_rows =
                            cache.read_range(ex, market, native_kind_min, need_from, i64::MAX);
                        cursor.cache_rows_kind = native_kind_min;
                        // If the native kind is absent, fall back first to the background recorder's
                        // 5-minute rows and then to 1-minute deep-history rows. Every supported
                        // timeframe is divisible by both, so the merge can resample them.
                        for fb in [5u32, 1] {
                            if !cursor.cache_rows.is_empty() || native_kind_min <= fb {
                                break;
                            }
                            cursor.cache_rows =
                                cache.read_range(ex, market, fb, need_from, i64::MAX);
                            cursor.cache_rows_kind = fb;
                        }
                        // Load cache-only coarser layers used to extend the historical prefix. Kind-5
                        // rows come from the recorder and possible deep-history writeback; the
                        // retained 5-minute snapshot is merged separately through `snap_part`.
                        if cp.tf_ms < 300_000 {
                            cursor.cache_rows_5m =
                                cache.read_range(ex, market, 5, need_from, i64::MAX);
                        }
                        if cp.tf_ms < 86_400_000 {
                            cursor.cache_rows_1d =
                                cache.read_range(ex, market, 1440, need_from, i64::MAX);
                        }
                        if !cursor.cache_rows.is_empty() {
                            log::info!(
                                "kline cache: префикс {market} kind{}: {} рядов",
                                cursor.cache_rows_kind,
                                cursor.cache_rows.len()
                            );
                        }
                    }
                }
            }
            // Perform a one-time native backfill for a coarse timeframe when the panel requests a
            // kind coarser than the effective one-core timeframe and neither retained state nor the
            // cache has native depth. Send one deliberate native-kind request per session for each
            // `(provider, market, kind)`. The core slot changes there and back as the freshness
            // guard restores the effective kind with backoff, while the response settles into the
            // cache and avoids future changes. Skip backfill without a cache because its result
            // would be lost on every restart while the slot changes remained.
            if use_deep && native_kind_min > deep_kind_min && kline_cache.is_some() {
                let native_kind = deep_history_kind(native_kind_min);
                let have_native = snapshot
                    .tf_candles(market, native_kind)
                    .map_or(false, |r| !r.is_empty());
                let cache_covers =
                    cursor.cache_rows_kind == native_kind_min && cursor.cache_rows.len() >= 30;
                if !have_native && !cache_covers {
                    let inner = self.inner.read().expect("market source poisoned");
                    let mut done = inner
                        .native_backfill_done
                        .lock()
                        .expect("native backfill set poisoned");
                    let key = (provider, market.to_string(), native_kind_min);
                    if !done.contains(&key) {
                        done.insert(key);
                        match client.candles().request_coin_card(market, native_kind) {
                            Ok(_) => log::info!(
                                "kline cache: разовый нативный бэкфилл {market} kind={native_kind:?}"
                            ),
                            Err(e) => super::market_diag(format!(
                                "native backfill request failed {market} kind={native_kind:?}: {e}"
                            )),
                        }
                    }
                }
            }
            // Native-backfill rows use the panel's native kind, which is coarser than the effective
            // kind, so they bypass the deep signature that tracks only the effective kind. Cache
            // them under a separate signature and force a prefix reread plus series rebuild so the
            // added depth appears immediately instead of in the next session.
            if use_deep && native_kind_min > deep_kind_min {
                if let (Some(cache), Some(ex)) = (kline_cache.as_ref(), exchange_key.as_ref()) {
                    let native_kind = deep_history_kind(native_kind_min);
                    if let Some(rows) = snapshot.tf_candles(market, native_kind) {
                        if !rows.is_empty() {
                            let sig = (rows.len() as u64).wrapping_mul(0x9e37_79b1)
                                ^ (rows.last().map_or(0, |r| r.unix_millis()) as u64);
                            if sig != cursor.cache_written_native_sig {
                                cursor.cache_written_native_sig = sig;
                                cache.merge(
                                    ex.clone(),
                                    market.to_string(),
                                    native_kind_min,
                                    rows.iter().map(deep_row_candle).collect(),
                                );
                                // The merge enters the FIFO queue before the future read, so the
                                // prefix reread sees the new rows.
                                cursor.cache_kind = None;
                                cursor.candle_series.invalidate();
                            }
                        }
                    }
                }
            }
            // Check base freshness on every pass, not only reset. When K is zero and the trade zone
            // is disabled, no resets occur between configuration changes, so a lost or expired
            // response used to freeze the series forever. Stale means the current bucket has no
            // row; the live subscription must maintain it, and a gap means a missed response or
            // disconnect. The core fetches deep history from the exchange API and consumes request
            // weight, so retries without progress use exponential backoff from 30 seconds to 10
            // minutes. New rows or a kind change reset the delay. A fixed 30-second retry against a
            // silent core or exchange previously triggered the core's API-limit auto-stop.
            if use_deep {
                let base_tf_native_ms = deep_kind_min as i64 * 60_000;
                let rows = snapshot.tf_candles(market, deep_kind);
                let now_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_millis() as f64);
                let cur_bucket_ms =
                    crate::market::candles::bucket_open_ms(now_unix_ms, base_tf_native_ms);
                let deep_stale = rows
                    .and_then(|r| r.last())
                    .map_or(true, |r| (r.unix_millis() as f64) < cur_bucket_ms);
                let kind_changed = cursor.last_deep_kind != Some(deep_kind);
                let retry_delay = Duration::from_secs(cursor.deep_retry_delay_s.max(30) as u64);
                if deep_stale
                    && (kind_changed
                        || cursor
                            .last_deep_request
                            .map_or(true, |t| t.elapsed() > retry_delay))
                {
                    cursor.last_deep_request = Some(Instant::now());
                    cursor.last_deep_kind = Some(deep_kind);
                    // Add global deduplication above per-panel backoff. N panels for one coin share
                    // the retained response, so the application sends one `(coin, kind)` request
                    // every 30 seconds. A gated panel does not increase its backoff when another
                    // panel sent the request; its last_deep_request was already advanced above.
                    let gate_open = {
                        let inner = self.inner.read().expect("market source poisoned");
                        let mut gate = inner.deep_req_gate.lock().expect("deep req gate poisoned");
                        let key = (provider, market.to_string(), deep_kind_min);
                        let now_i = Instant::now();
                        match gate.get(&key) {
                            Some(t) if now_i.duration_since(*t) < Duration::from_secs(30) => false,
                            _ => {
                                gate.insert(key, now_i);
                                true
                            }
                        }
                    };
                    if gate_open {
                        cursor.deep_retry_delay_s = if kind_changed {
                            30
                        } else {
                            (cursor.deep_retry_delay_s.max(30) * 2).min(600)
                        };
                        if let Err(e) = client.candles().request_coin_card(market, deep_kind) {
                            super::market_diag(format!(
                                "coin-card request failed {market} kind={deep_kind:?}: {e}"
                            ));
                        }
                    }
                }
            }
            // Track a cheap deep-row fingerprint: row count plus the final timestamp. It detects a
            // new or advanced bucket, but not an in-place OHLC replacement at the same timestamp.
            // Such a replacement does not itself trigger rebuild or writeback; a trade-tail or
            // explicit reset can rebuild independently, while writeback waits for a later fingerprint
            // advance, normally a new bucket.
            let deep_rows_sig = if use_deep {
                snapshot.tf_candles(market, deep_kind).map_or(0u64, |rows| {
                    let last_ms = rows.last().map_or(0, |r| r.unix_millis());
                    (rows.len() as u64).wrapping_mul(0x9e37_79b1) ^ (last_ms as u64)
                })
            } else {
                0
            };
            if deep_rows_sig != cursor.last_deep_sig {
                // A response or live bar advanced the deep rows, so reset the request backoff.
                cursor.deep_retry_delay_s = 30;
            }
            let series_reset = force_reset
                || read.combo_reset
                || !cursor.candle_series.is_valid()
                || cursor.candle_series.tf_ms() != cp.tf_ms
                || (cursor.candle_trades.is_none() && trade_reader.is_some())
                || deep_rows_sig != cursor.last_deep_sig;
            if series_reset {
                cursor.last_deep_sig = deep_rows_sig;
                cursor.server_candle_rows.clear();
                cursor.server_candles.clear();
                let from_base_ms =
                    (epoch_ms + (from_rel_ms - cp.tf_ms.max(0) as f32) as f64) as i64;
                // Keep the base's right edge at least at now rather than at the window's right edge.
                // A reset while scrolling into the past used to truncate the base at that window.
                // Returning live does not reset because it only extends left, leaving a gap in the
                // middle from the historical position to today's live bucket until another pan.
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                let to_ms = ((epoch_ms + to_rel_ms as f64) as i64).max(now_unix);
                // Base 1 is CoinCard deep history with authoritative OHLC for the effective
                // one-core timeframe. Base 2 is the free 5-minute range-only snapshot, used as a
                // prefix older than the deep part or as the entire base until deep history arrives.
                // The composite therefore has older range candles and newer authoritative candles.
                // Timeframes below five minutes cannot downsample the snapshot and do not use it.
                // One merge always converts the resulting base to the series timeframe.
                let base_tf_ms = cp.tf_ms;
                let mut deep_part: Vec<ChartCandle> = Vec::new();
                if let Some(rows) = snapshot.tf_candles(market, deep_kind).filter(|_| use_deep) {
                    deep_part.extend(
                        rows.iter()
                            .filter(|r| {
                                let t = r.unix_millis();
                                t >= from_base_ms && t <= to_ms
                            })
                            .map(|r| {
                                let (open, high, low, close) =
                                    crate::market::candles::normalize_ohlc(
                                        r.open(),
                                        r.high(),
                                        r.low(),
                                        r.close(),
                                    );
                                ChartCandle {
                                    t_open_ms: r.unix_millis() as f64,
                                    open,
                                    high,
                                    low,
                                    close,
                                    volume: r.volume(),
                                }
                            }),
                    );
                }
                let have_deep = !deep_part.is_empty();
                let use_snap5 = cp.tf_ms >= 300_000 && cp.tf_ms % 300_000 == 0;
                let mut snap_part: Vec<ChartCandle> = Vec::new();
                if use_snap5 {
                    if let Some(r5) = readers.candles_5m.as_ref() {
                        let from5 =
                            moon_time_from_rel_ms(epoch_ms, from_rel_ms - cp.tf_ms.max(0) as f32);
                        r5.copy_time_range(
                            from5,
                            moonproto::MoonTime::from_unix_millis(to_ms),
                            r5.capacity(),
                            &mut cursor.server_candle_rows,
                        );
                        snap_part.extend(cursor.server_candle_rows.iter().map(|r| {
                            let (open, high, low, close) = crate::market::candles::normalize_ohlc(
                                r.open(),
                                r.high(),
                                r.low(),
                                r.close(),
                            );
                            ChartCandle {
                                t_open_ms: r.time().unix_millis() as f64,
                                open,
                                high,
                                low,
                                close,
                                volume: r.volume(),
                            }
                        }));
                        crate::market::candles::orient_range_rows(&mut snap_part);
                    }
                }
                // Use authoritative native klines from prior sessions as the visible cache portion.
                let cache_part: Vec<ChartCandle> = cursor
                    .cache_rows
                    .iter()
                    .filter(|c| {
                        let t = c.t_open_ms as i64;
                        t >= from_base_ms && t <= to_ms
                    })
                    .cloned()
                    .collect();
                // Write deep rows back to the cache without blocking when the cheap fingerprint
                // advances. Same-timestamp OHLC replacements do not advance it and remain unwritten
                // until a later bucket does. Persist the complete retained sequence rather than
                // `deep_part` clipped to the visible window; a narrow window used to save one
                // response candle and lose the remaining depth.
                if have_deep && deep_rows_sig != cursor.cache_written_sig {
                    match (kline_cache.as_ref(), exchange_key.as_ref()) {
                        (Some(cache), Some(ex)) => {
                            cursor.cache_written_sig = deep_rows_sig;
                            let full: Vec<ChartCandle> = snapshot
                                .tf_candles(market, deep_kind)
                                .map(|rows| rows.iter().map(deep_row_candle).collect())
                                .unwrap_or_default();
                            cache.merge(ex.clone(), market.to_string(), deep_kind_min, full);
                        }
                        (Some(_), None) => {
                            // The provider exchange identity is unavailable, so the cache cannot
                            // address these rows. Make this visible in the log once per panel. Keep
                            // the real signature unchanged, but use the initial marker to suppress
                            // repeated logging.
                            if cursor.cache_written_sig == 0 {
                                cursor.cache_written_sig = 1;
                                log::warn!(
                                    "kline cache: провайдер {provider} без ExchangeId — \
                                     ряды {market} не кэшируются"
                                );
                            }
                        }
                        _ => {}
                    }
                }
                // Merge every base source into the series timeframe in increasing priority:
                // 5-minute range-only snapshot < authoritative cached klines < live, freshest deep
                // history. Skip sources whose timeframe is coarser than or does not divide the
                // target, such as a 5-minute snapshot for a 1-minute series.
                {
                    let tf = cp.tf_ms;
                    let mut merged: std::collections::BTreeMap<i64, ChartCandle> =
                        std::collections::BTreeMap::new();
                    let mut scratch: Vec<ChartCandle> = Vec::new();
                    for (part, part_tf) in [
                        (&snap_part, 5 * 60_000i64),
                        (&cache_part, cursor.cache_rows_kind as i64 * 60_000),
                        (&deep_part, deep_kind_min as i64 * 60_000),
                    ] {
                        if part.is_empty() || part_tf <= 0 || tf < part_tf || tf % part_tf != 0 {
                            continue;
                        }
                        crate::market::candles::resample(part, tf, &mut scratch);
                        for c in scratch.drain(..) {
                            merged.insert(c.t_open_ms as i64, c);
                        }
                    }
                    cursor.server_candles.extend(merged.into_values());
                }
                cursor.candle_trade_rows.clear();
                if let Some(reader) = trade_reader.as_ref() {
                    // Extend the series tail through now, which is already included in to_ms. The
                    // follow-up cursor starts at now, so the copied range must reach the same point
                    // or a permanent gap remains between them.
                    reader.copy_time_range(
                        from_time,
                        moonproto::MoonTime::from_unix_millis(to_ms),
                        reader.capacity(),
                        &mut cursor.candle_trade_rows,
                    );
                    cursor.candle_trades = Some(reader.cursor_from_now());
                } else {
                    cursor.candle_trades = None;
                }
                rows_to_ticks(&cursor.candle_trade_rows, &mut cursor.candle_ticks);
                cursor.candle_series.rebuild(
                    cp.tf_ms,
                    &cursor.server_candles,
                    base_tf_ms,
                    &cursor.candle_ticks,
                );
            } else if let (Some(reader), Some(cur)) =
                (trade_reader.as_ref(), cursor.candle_trades.as_mut())
            {
                let meta =
                    reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.candle_trade_rows);
                if meta.clipped {
                    // The cursor fell behind the ring; force a full rebuild on the next pass.
                    cursor.candle_series.invalidate();
                } else if meta.copied > 0 {
                    rows_to_ticks(&cursor.candle_trade_rows, &mut cursor.candle_ticks);
                    cursor.candle_series.push_trades(&cursor.candle_ticks);
                }
            }
            read.candles_revision = cursor.candle_series.revision();
            read.candles_changed =
                cursor.candle_series.is_valid() && read.candles_revision != cp.shipped_revision;
            if read.candles_changed {
                // Extend history as far back as possible with cache-only coarser timeframes. The
                // kind-5 prefix comes from the recorder and possible deep-history writeback; the
                // retained `snap_part` already feeds the main series. Daily cache rows extend beyond
                // the kind-5 prefix. Prefix candles carry their own timeframe for shader width and
                // render muted to distinguish them from the selected timeframe.
                let series_first = cursor
                    .candle_series
                    .candles()
                    .first()
                    .map(|c| c.t_open_ms)
                    .unwrap_or(f64::INFINITY);
                let mut prefix: Vec<(ChartCandle, f32)> = Vec::new();
                let mut boundary = series_first;
                for (rows, tf) in [
                    (&cursor.cache_rows_5m, 300_000.0f64),
                    (&cursor.cache_rows_1d, 86_400_000.0f64),
                ] {
                    if (cp.tf_ms as f64) >= tf {
                        continue;
                    }
                    // Take rows that start before the boundary, allowing the boundary candle to
                    // overlap the seam by up to one full timeframe. Requiring rows to end entirely
                    // before the boundary left a gap as wide as tf_coarse; for example, a daily
                    // candle ending at 00:00 followed by a series starting at 04:06 left four hours.
                    // The overlap is invisible because the muted prefix renders below finer candles.
                    let mut taken: Vec<(ChartCandle, f32)> = rows
                        .iter()
                        .filter(|c| c.t_open_ms < boundary)
                        .map(|c| (*c, tf as f32))
                        .collect();
                    if let Some((first, _)) = taken.first() {
                        boundary = first.t_open_ms;
                    }
                    taken.extend(prefix.drain(..));
                    prefix = taken;
                }
                if prefix.is_empty() {
                    out.candles
                        .extend_from_slice(cursor.candle_series.candles());
                } else {
                    out.candles
                        .reserve(prefix.len() + cursor.candle_series.candles().len());
                    out.candle_tf_ms.reserve(out.candles.capacity());
                    for (c, tf) in &prefix {
                        out.candles.push(*c);
                        out.candle_tf_ms.push(*tf);
                    }
                    for c in cursor.candle_series.candles() {
                        out.candles.push(*c);
                        out.candle_tf_ms.push(cp.tf_ms as f32);
                    }
                }
                // Diagnose candle-to-now gaps. If the last candle is older than three timeframes,
                // log layer coverage once every 30 seconds so the exhausted layer is identifiable
                // as series, deep, cache, 5-minute, or 1-day instead of debugging screenshots.
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_millis() as f64);
                let last_ms = out.candles.last().map(|c| c.t_open_ms).unwrap_or(0.0);
                // Detect gaps inside the sequence where the next candle begins after the previous
                // one ends. This is where the scroll-to-history then return-to-live gap was hidden.
                let mut max_hole = 0.0f64;
                let mut hole_at = 0.0f64;
                for i in 1..out.candles.len() {
                    let prev = &out.candles[i - 1];
                    let prev_tf = out
                        .candle_tf_ms
                        .get(i - 1)
                        .copied()
                        .filter(|t| *t > 0.0)
                        .unwrap_or(cp.tf_ms as f32) as f64;
                    let hole = out.candles[i].t_open_ms - (prev.t_open_ms + prev_tf);
                    if hole > max_hole {
                        max_hole = hole;
                        hole_at = prev.t_open_ms + prev_tf;
                    }
                }
                if last_ms > 0.0
                    && (now_unix - last_ms > 3.0 * cp.tf_ms as f64
                        || max_hole > 3.0 * cp.tf_ms as f64)
                    && cursor
                        .last_gap_diag
                        .map_or(true, |t| t.elapsed() > Duration::from_secs(30))
                {
                    cursor.last_gap_diag = Some(Instant::now());
                    let ago_min = |ms: f64| ((now_unix - ms) / 60_000.0).round();
                    let span = |rows: &[ChartCandle]| match (rows.first(), rows.last()) {
                        (Some(f), Some(l)) => {
                            format!("{}м..{}м назад", ago_min(l.t_open_ms), ago_min(f.t_open_ms))
                        }
                        _ => "пусто".to_string(),
                    };
                    log::warn!(
                        "candle gap {market} tf={}с: последняя свеча {}м назад, макс. дыра \
                         {}м (кончается {}м назад); серия n={} \
                         [{}], префикс n={}, кэш kind{} n={} [{}], 5м n={} [{}], 1д n={} [{}]",
                        cp.tf_ms / 1000,
                        ago_min(last_ms),
                        (max_hole / 60_000.0).round(),
                        ago_min(hole_at + max_hole),
                        cursor.candle_series.candles().len(),
                        span(cursor.candle_series.candles()),
                        prefix.len(),
                        cursor.cache_rows_kind,
                        cursor.cache_rows.len(),
                        span(&cursor.cache_rows),
                        cursor.cache_rows_5m.len(),
                        span(&cursor.cache_rows_5m),
                        cursor.cache_rows_1d.len(),
                        span(&cursor.cache_rows_1d),
                    );
                }
            }
            if scan_price {
                // Include visible candle highs and lows in automatic Y scaling. Trade crosses now
                // cover only their display zone, so older visible history would otherwise not affect
                // the scale.
                if let Some((lo, hi)) = cursor
                    .candle_series
                    .price_range(epoch_ms + from_rel_ms as f64, epoch_ms + to_rel_ms as f64)
                {
                    read.tick_price_range = Some(match read.tick_price_range {
                        Some((a, b)) => (a.min(lo), b.max(hi)),
                        None => (lo, hi),
                    });
                }
                // The coarser-timeframe prefix is visible too, so include its highs and lows in
                // automatic scaling.
                let from_abs = epoch_ms + from_rel_ms as f64;
                let to_abs = epoch_ms + to_rel_ms as f64;
                let series_first = cursor
                    .candle_series
                    .candles()
                    .first()
                    .map(|c| c.t_open_ms)
                    .unwrap_or(f64::INFINITY);
                for (rows, tf) in [
                    (&cursor.cache_rows_5m, 300_000.0f64),
                    (&cursor.cache_rows_1d, 86_400_000.0f64),
                ] {
                    if (cp.tf_ms as f64) >= tf {
                        continue;
                    }
                    for c in rows.iter() {
                        if c.t_open_ms < series_first
                            && c.t_open_ms + tf > from_abs
                            && c.t_open_ms <= to_abs
                        {
                            read.tick_price_range = Some(match read.tick_price_range {
                                Some((a, b)) => (a.min(c.low), b.max(c.high)),
                                None => (c.low, c.high),
                            });
                        }
                    }
                }
            }
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
                price_rows_to_points,
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
                price_rows_to_points,
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
