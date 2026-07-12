//! Read-плоскость источника: ревизии/цены/тикер/поиск и дренаж истории чарта.

use crate::data::OrderBookModel;
use crate::feed::SharedMoonClient;
use crate::session::CoreId;

use std::time::{Duration, Instant};

use moonproto::DeepHistoryKind;

use super::{
    drain_price_line, moon_time_from_rel_ms, price_rows_to_points, rows_to_ticks,
    trade_price_range, CandleReadParams, ChartHistoryBuffers, ChartHistoryCursor,
    ChartHistoryRead, LatestPriceError, MarketDataSource, MarketRevisions, MarketTickerReadout,
};
use crate::market::candles::ChartCandle;

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
    pub(crate) fn core_client(&self, core: CoreId) -> Option<std::sync::Arc<moonproto::MoonClient>> {
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

    /// MoonBot Coin1hDelta). Для тикера курса в шапке (и будущего скринера).
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
        let display_trades =
            candle_params.map_or(true, |cp| cp.trades_from_rel_ms.is_finite());
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
            let deep_kind_min =
                crate::market::candles::deep_kind_min_for_tf((cp.tf_ms / 60_000) as u32);
            let deep_kind = deep_history_kind(deep_kind_min);
            // Подписка на живые ТФ-бары ядра: Event::LiveCandle дошивает/заменяет последний
            // ряд retained tf_candles (правило candle-window ядра). БЕЗ подписки deep-ряды
            // заморожены с момента ответа — на больших ТФ (1д/4ч) серия отставала на часы.
            // Курсор per-pane: переподписка на смене kind/рынка, снятие при use_deep=false.
            if use_deep {
                let sub_ok = cursor
                    .candle_sub
                    .as_ref()
                    .is_some_and(|(m, k)| m == market && *k == deep_kind);
                if !sub_ok {
                    if let Some((old_market, _)) = cursor.candle_sub.take() {
                        let _ = client.streams().unsubscribe_candles([old_market]);
                    }
                    if client.streams().subscribe_candles([market], deep_kind).is_ok() {
                        cursor.candle_sub = Some((market.to_string(), deep_kind));
                    }
                }
            } else if let Some((old_market, _)) = cursor.candle_sub.take() {
                let _ = client.streams().unsubscribe_candles([old_market]);
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
                        let mut gate =
                            inner.deep_req_gate.lock().expect("deep req gate poisoned");
                        let key = (provider, market.to_string(), deep_kind_min);
                        let now_i = Instant::now();
                        match gate.get(&key) {
                            Some(t) if now_i.duration_since(*t) < Duration::from_secs(30) => {
                                false
                            }
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
                let to_ms = (epoch_ms + to_rel_ms as f64) as i64;
                // База №1: CoinCard deep history — честные OHLC родного ТФ (замер
                // показал, что bulk-снимок 5м несёт только high/low). Запрашивается
                // ниже неблокирующе; пока не приехала — фолбэк на 5м-снимок.
                let mut base_tf_ms = deep_kind_min as i64 * 60_000;
                if let Some(rows) = snapshot.tf_candles(market, deep_kind).filter(|_| use_deep) {
                    cursor.server_candles.extend(
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
                let have_deep = !cursor.server_candles.is_empty();
                if !have_deep {
                    // База №2 (фолбэк): retained 5м-снимок — только high/low; тела
                    // ориентируем по направлению, честные OHLC заменят их по приходу
                    // deep history (событие CoinCardCandles будит чарт).
                    base_tf_ms = 5 * 60_000;
                    if let Some(r5) = readers.candles_5m.as_ref() {
                        let from5 =
                            moon_time_from_rel_ms(epoch_ms, from_rel_ms - cp.tf_ms.max(0) as f32);
                        r5.copy_time_range(
                            from5,
                            to_time,
                            r5.capacity(),
                            &mut cursor.server_candle_rows,
                        );
                        cursor
                            .server_candles
                            .extend(cursor.server_candle_rows.iter().map(|r| {
                                let (open, high, low, close) =
                                    crate::market::candles::normalize_ohlc(
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
                        crate::market::candles::orient_range_rows(&mut cursor.server_candles);
                    }
                }
                cursor.candle_trade_rows.clear();
                if let Some(reader) = trade_reader.as_ref() {
                    reader.copy_time_range(
                        from_time,
                        to_time,
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
                out.candles.extend_from_slice(cursor.candle_series.candles());
            }
            if scan_price {
                // Авто-Y учитывает high/low видимых свечей (кресты теперь только в зоне —
                // без этого прошлое за зоной не влияло бы на масштаб).
                if let Some((lo, hi)) = cursor.candle_series.price_range(
                    epoch_ms + from_rel_ms as f64,
                    epoch_ms + to_rel_ms as f64,
                ) {
                    read.tick_price_range = Some(match read.tick_price_range {
                        Some((a, b)) => (a.min(lo), b.max(hi)),
                        None => (lo, hi),
                    });
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
