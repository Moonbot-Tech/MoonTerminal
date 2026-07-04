//! Read-плоскость источника: ревизии/цены/тикер/поиск и дренаж истории чарта.

use crate::data::OrderBookModel;
use crate::feed::SharedMoonClient;
use crate::session::CoreId;

use super::{
    drain_price_line, last_rows_to_points, mark_rows_to_points, moon_time_from_rel_ms,
    rows_to_ticks, trade_price_range, ChartHistoryBuffers, ChartHistoryCursor, ChartHistoryRead,
    LatestPriceError, MarketDataSource, MarketRevisions, MarketTickerReadout,
};

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
