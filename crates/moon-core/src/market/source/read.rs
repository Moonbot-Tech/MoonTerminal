//! Read-плоскость источника: ревизии/цены/тикер/поиск и дренаж истории чарта.

use crate::data::OrderBookModel;
use crate::feed::SharedMoonClient;
use crate::session::CoreId;

use std::time::{Duration, Instant};

use moonproto::DeepHistoryKind;

/// Retained-ряд CoinCard → свеча чарта (нормализация перепутанного wire-порядка полей).
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

/// Короткий тип биржи из `exchange_type_mask` server_info: «Спот»/«Фьючи»/«DEX»/…
/// (core i18n-агностичен — строки простым текстом, UI при желании перелокализует). Маска —
/// набор возможностей подключения; для одиночного коннекта обычно ровно один торговый бит.
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

/// ТФ CoinCard-истории (мин) → wire-kind moonproto.
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

    /// Последняя цена + знаковые дельты рынка за 1ч/24ч, % (moonproto `MarketDeltaState`:
    /// `coin_1h_delta`/`coin_24h_delta` — отклонение цены от удержанного среднего, как
    /// Ядро-провайдер рыночных данных consumer-ядра — дедуп-ключ биржи: у
    /// ядер одной биржи провайдер общий. Скринер группирует ядра по нему,
    /// чтобы монеты не дублировались.
    pub fn provider_of(&self, core: CoreId) -> Option<CoreId> {
        self.inner
            .read()
            .expect("market source poisoned")
            .core_provider
            .get(&core)
            .copied()
    }

    /// Живой MoonProto-клиент КОНКРЕТНОГО ядра (не его провайдера) — для
    /// аккаунтных полей, которые персональны для ядра (скринер).
    pub(crate) fn core_client(
        &self,
        core: CoreId,
    ) -> Option<std::sync::Arc<moonproto::MoonClient>> {
        let inner = self.inner.read().expect("market source poisoned");
        inner.clients.get(&core).and_then(SharedMoonClient::get)
    }

    /// Шаг цены рынка (moonproto `chart_price_step`) — размер клавиатурного сдвига
    /// ордеров (shift_buy/sell_up/down). `None` — нет провайдера/снимка/рынка или шаг
    /// не задан (≤0): сдвиг тогда не делаем, чтобы не выдумывать шаг.
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

    /// Moonbot Coin1hDelta). Для тикера курса в шапке (и будущего скринера).
    /// `None` — нет провайдера/снимка/рынка.
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

    /// Замороженный снимок для карточки детекта: последние `bars` 5м-свечей `(high, low)`
    /// (старые→новые) + имя/тип биржи. История — из локального kline-кэша (фоновый регистратор
    /// пишет 5м-бары по ВСЕМ рынкам, 90 дней), живой хвост — из трейд-ринга провайдера; оба
    /// БЕСПЛАТНЫ (биржевой API НЕ трогаем). Данные — ДЕДУП-ПРОВАЙДЕРА биржи (общие для ядер той
    /// же биржи/рынка). Собирать ОДИН раз в момент детекта, морозить в карточке. Пусто — нет
    /// провайдера/клиента/снимка/истории.
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
            // Стабильный ключ биржи провайдера для kline-кэша (как в read_chart_history_into).
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
        // Имя/тип биржи — из идентити подключения (server_info из BaseCheck).
        let info = snapshot.server_info();
        if let Some(name) = &info.exchange_name {
            out.exchange_name = name.clone();
        }
        out.exchange_kind = exchange_kind_label(info);

        let tf_ms: i64 = 300_000; // 5м
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        // Окно 24ч: свечной режим берёт из него последние `bars` бакетов (~2ч), линейный —
        // все close-цены (до 24ч). `line_cap` = максимум 5м-бакетов в 24ч (288).
        let from_ms = now_ms - 24 * 3_600_000;
        let line_cap = (24 * 3_600_000 / tf_ms) as usize;

        // Свечи по бакету 5м (ключ = t_open, ФЛОР к tf для сшивки разных источников):
        //   база №1 = retained 5м-снимок ЯДРА (тянется при старте по скоупу → ГЛУБИНА истории);
        //   база №2 = локальный kline-кэш (реальные OHLC, перекрывает снимок);
        //   хвост   = трейд-ринг (живой край, перекрывает свежие бакеты).
        // Приоритет свежести — порядок вставки (снимок < кэш < ринг). Раньше снимок был лишь
        // фолбэком «если пусто» → ринг всегда давал пару бар → глубина снимка игнорировалась.
        let mut buckets: std::collections::BTreeMap<i64, (f32, f32, f32, f32)> =
            std::collections::BTreeMap::new();
        let bucket_key = |t_ms: i64| (t_ms.max(0) / tf_ms) * tf_ms;
        let mut snap5_n = 0usize;
        let mut cache_n = 0usize;

        if let Some(readers) = snapshot.market_history_readers(market) {
            // База №1: 5м-снимок ядра. Несёт только high/low (open==high, close==low) → тело=
            // диапазон; штампуется КОНЦОМ периода → сдвигаем на tf назад к open (совпасть с кэшем).
            // normalize_ohlc + orient_range_rows (как чарт): ориентируем «диапазонные» свечи по
            // тренду средней, иначе тела-only покрасили бы всю историю в один цвет (close<open).
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
            // База №2: kline-кэш (реальные OHLC прошлых минут/сессий, kind_min=5).
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
            // Хвост: трейд-ринг провайдера → 5м-свечи (перекрывает свежие бакеты).
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

        // Диагностика (env MOON_DETECT_DIAG=1): реальная структура бакетов — сколько свечей,
        // общий охват, МАКС дыра во времени (по-индексный рендер её скрывает, но она укажет на
        // рваность данных) и ценовой диапазон (выброс = сплющивание). Не гадать про «разрывы».
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

        // Линия (режим «линия»): все close-цены 24ч-окна, старые→новые.
        out.line = buckets.values().map(|&(_, _, _, c)| c).collect();
        // Свечи (режим «свечи»): последние `bars` бакетов (~2ч), старые→новые.
        let start = buckets.len().saturating_sub(bars);
        out.bars = buckets.values().skip(start).copied().collect();
        // Дельты 1ч/24ч = ФАКТИЧЕСКОЕ изменение цены за период (сейчас vs цена N назад) — из
        // НАШИХ бакетов, чтобы совпадали со сдвигом линии. (moonproto coin_*_delta — это
        // отклонение от СРЕДНЕЙ за период, другая метрика; она осталась тикеру шапки.)
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
        // Зона отображения трейдов (последние K свечей): кресты/сканы читаем только от неё.
        // INFINITY = трейды не отображаем вовсе (K=0). Агрегацию свечей это НЕ ограничивает.
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
            // Трейды скрыты (K=0), но ринг есть: last_price — из последнего трейда, а на
            // reset флагом combo_reset велим слою очистить кольцо крестов.
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

        // Трейды ликвидаций — отдельный ring того же типа. Синхронны с combo: на полном
        // reset combo (или первом проходе) перечитываем весь видимый диапазон, иначе тянем
        // только новый живой край. Рендер тегирует их единым цветом (side=2). Окно — как у
        // обычных трейдов (зона последних K свечей).
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

        // Серия свечей: серверный 5м-снимок (авто-снимок moonproto — история ДО подключения)
        // + локальный хвост из трейдов. Свой курсор по трейд-рингу: агрегация не зависит от
        // зоны отображения крестов. Полная пересборка — только на reset/смене ТФ; живой
        // край — дешёвый drain новых строк.
        if let Some(cp) = candle_params {
            // CoinCard deep history применима только к ТФ ≥ 1м (суб-минутные — из трейдов).
            let use_deep = cp.tf_ms >= 60_000;
            let native_kind_min =
                crate::market::candles::deep_kind_min_for_tf((cp.tf_ms / 60_000) as u32);
            // ЯДРО ДЕРЖИТ ОДИН СВЕЧНОЙ ТФ НА ЯДРО (разраб МБ, 2026-07-12): kind-флипы между
            // окнами с разными ТФ заставляли ядро перекачивать историю с биржи на каждый
            // запрос/подписку → бан API. Эффективный kind провайдера = MIN живых желаний
            // (kind'ы цепочкой делятся: 1|5|30|60|240|1440) — панели крупнее ресемплят из
            // мелкой базы ценой глубины (ринг ядра ~10к строк базового ТФ).
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
            // Локальный kline-кэш: хэндл + стабильный ключ биржи провайдера (ядра одной
            // биржи делят кэш; CoreId между сессиями нестабилен).
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
            // Подписка на живые ТФ-бары ядра: Event::LiveCandle дошивает/заменяет последний
            // ряд retained tf_candles — без неё deep-ряды заморожены с момента ответа и на
            // больших ТФ серия отставала на часы. Подписка ГЛОБАЛЬНА на клиенте (последний
            // kind выигрывает) → общий реестр per (провайдер, рынок): панели «трогают»
            // запись, протухшие (>60с без спроса) отписываются попутно.
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
                // Попутная уборка протухших подписок ЭТОГО провайдера (его клиент под рукой).
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
            // Префикс из локального kline-кэша: честные нативные klines прошлых сессий.
            // Читается из sqlite ОДИН раз на (рынок, kind, левый край) — ресеты частые
            // (пан/зум), в БД на каждый нельзя; расширение окна влево перечитывает.
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
                        // Нативного kind нет — каскадный фолбэк: 5м фонового регистратора,
                        // затем 1м deep-записей (любой ТФ кратен обоим, ресемпл в merge).
                        for fb in [5u32, 1] {
                            if !cursor.cache_rows.is_empty() || native_kind_min <= fb {
                                break;
                            }
                            cursor.cache_rows =
                                cache.read_range(ex, market, fb, need_from, i64::MAX);
                            cursor.cache_rows_kind = fb;
                        }
                        // Крупные слои для дорисовки хвоста истории старшими ТФ.
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
            // Разовый нативный бэкфилл крупного ТФ: панель хочет kind крупнее эффективного
            // («один ТФ на ядро» держит слот на мелком), а нативной глубины нет ни в
            // retained, ни в кэше → ОДИН осознанный запрос native kind за сессию на
            // (провайдер, рынок, kind). Слот ядра флипнется туда-обратно (страховка
            // свежести вернёт эффективный kind с бэкоффом), ответ уляжется в кэш — дальше
            // глубина живёт локально и флипов больше нет. Без кэша бэкфилл не делаем
            // (плоды терялись бы каждый рестарт, а флипы оставались).
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
            // Урожай нативного бэкфилла: ряды нативного kind панели (крупнее эффективного)
            // приезжают МИМО deep-сигнатуры (та следит за эффективным kind) — пишем их в
            // кэш по собственной сигнатуре и форсим перечитку префикса + пересборку серии,
            // чтобы глубина появилась сразу, а не со следующей сессии.
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
                                // Merge уехал в очередь РАНЬШЕ будущего чтения (FIFO) —
                                // перечитка префикса увидит свежие ряды.
                                cursor.cache_kind = None;
                                cursor.candle_series.invalidate();
                            }
                        }
                    }
                }
            }
            // Свежесть базы — КАЖДЫЙ проход, не только на reset: при K=0 (зона трейдов
            // выключена) ресетов между сменами cfg нет вообще, и потерянный/просроченный
            // ответ раньше замораживал серию навсегда. «Устарело» = нет ряда ТЕКУЩЕГО
            // бакета (живая подписка обязана его держать; дыра = пропущенный ответ или
            // разрыв). ВАЖНО: deep history ядро тянет с БИРЖЕВОГО API (весовые лимиты!),
            // поэтому повтор без прогресса — с ЭКСПОНЕНЦИАЛЬНЫМ бэкоффом 30с→10мин
            // (сброс по приходу новых рядов / смене kind). Ровный 30с-ретрай при молчащем
            // ядре/бирже приводил к «автостоп по превышению лимитов API» ядра.
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
                    // Глобальный дедуп поверх per-pane бэкоффа: N панелей одной монеты
                    // делят retained-ответ — шлём один запрос (монета, kind) в 30с на всё
                    // приложение. Заблокированная панель бэкофф НЕ раскручивает (запрос
                    // ушёл от соседа) — её last_deep_request уже отодвинут выше.
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
            // Сигнатура загруженных deep-строк: их приход/обновление (событие CoinCardCandles
            // будит чарт) обязан ПЕРЕСОБРАТЬ серию — иначе после смены ТФ история появлялась
            // только после переоткрытия графика.
            let deep_rows_sig = if use_deep {
                snapshot.tf_candles(market, deep_kind).map_or(0u64, |rows| {
                    let last_ms = rows.last().map_or(0, |r| r.unix_millis());
                    (rows.len() as u64).wrapping_mul(0x9e37_79b1) ^ (last_ms as u64)
                })
            } else {
                0
            };
            if deep_rows_sig != cursor.last_deep_sig {
                // Deep-ряды продвинулись (ответ/live-бар) — прогресс, бэкофф запросов заново.
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
                // Правая граница базы — ВСЕГДА не раньше «сейчас», а не правый край окна:
                // ресет во время прокрутки в прошлое обрезал базу краем ТОГО окна, возврат
                // в live ресета не делает (только расширение влево) → дыра В СЕРЕДИНЕ ряда
                // от места прокрутки до сегодняшнего live-бакета, лечившаяся любым паном.
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                let to_ms = ((epoch_ms + to_rel_ms as f64) as i64).max(now_unix);
                // База №1: CoinCard deep history — честные OHLC эффективного kind («один ТФ
                // на ядро»). База №2: бесплатный 5м-снимок (только high/low) — ПРЕФИКС
                // старше deep-части (композит: старое — диапазоны без теней, свежее —
                // честные свечи) либо вся база, пока deep не приехала. ТФ < 5м снимок
                // не использует (вниз не ресемплится).
                // База после единого merge всегда приведена к ТФ серии.
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
                // Кэш-часть: нативные честные klines прошлых сессий (видимое окно).
                let cache_part: Vec<ChartCandle> = cursor
                    .cache_rows
                    .iter()
                    .filter(|c| {
                        let t = c.t_open_ms as i64;
                        t >= from_base_ms && t <= to_ms
                    })
                    .cloned()
                    .collect();
                // Write-back свежих deep-рядов в кэш (по смене сигнатуры, неблокирующе).
                // Пишем ПОЛНЫЙ retained-ряд, НЕ обрезанный видимым окном deep_part:
                // узкое окно записывало 1 свечу из ответа — глубина терялась.
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
                            // Идентичность биржи провайдера не доехала — кэш слепнет,
                            // это надо ВИДЕТЬ в логе (разово на панель: sig не двигаем,
                            // но лог глушим по первому разу).
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
                // ЕДИНЫЙ merge базы к ТФ серии, приоритет по возрастанию:
                // 5м-снимок (только HL) < кэш (честные klines) < deep (живой, свежайший).
                // Неделимые/крупнее ТФ части пропускаются (снимок для 1м и т.п.).
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
                    // Хвост серии — до «сейчас» (to_ms уже включает now): курсор дочитки
                    // ставится «от сейчас», диапазон копии обязан дотягиваться туда же,
                    // иначе между ними дыра навсегда.
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
                    // Отстали от ринга — на следующем проходе полная пересборка.
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
                // Хвост истории дорисовывается СТАРШИМИ ТФ «до упора»: старше начала
                // серии — 5м-слой (снимок+регистратор), глубже него — дневные свечи
                // (бэкфилл/кэш). У каждой свечи хвоста свой ТФ (ширина в шейдере) —
                // рисуются приглушённо, чтобы отличались от выбранного ТФ.
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
                    // Берём ряды, НАЧИНАЮЩИЕСЯ до границы (граничная свеча может
                    // перекрыть шов до целого ТФ): правило «только целиком старше»
                    // оставляло карман гранулярности до tf_coarse (дневка кончается в
                    // 00:00, серия начинается в 04:06 → дыра 4ч). Перекрытие невидимо —
                    // хвост рисуется приглушённой подложкой ПОД мелкими свечами.
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
                // Диагностика разрыва «свечи ↔ сейчас»: если последняя свеча старше 3 ТФ,
                // раз в 30с печатаем покрытие всех слоёв — по логу видно, КАКОЙ слой
                // кончился (серия/deep/кэш/5м/1д), вместо слепой чинки по скриншотам.
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_millis() as f64);
                let last_ms = out.candles.last().map(|c| c.t_open_ms).unwrap_or(0.0);
                // Дыры и В СЕРЕДИНЕ ряда (следующая свеча позже конца предыдущей) — разрыв
                // «прокрутка в прошлое → возврат в live» прятался именно там.
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
                // Авто-Y учитывает high/low видимых свечей (кресты теперь только в зоне —
                // без этого прошлое за зоной не влияло бы на масштаб).
                if let Some((lo, hi)) = cursor
                    .candle_series
                    .price_range(epoch_ms + from_rel_ms as f64, epoch_ms + to_rel_ms as f64)
                {
                    read.tick_price_range = Some(match read.tick_price_range {
                        Some((a, b)) => (a.min(lo), b.max(hi)),
                        None => (lo, hi),
                    });
                }
                // Хвост старших ТФ тоже виден — его high/low входят в авто-масштаб.
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
