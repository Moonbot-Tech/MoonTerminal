//! Данные скринера («Таблица монет»): срез по ВСЕМ рынкам ядра-провайдера.
//!
//! Рыночная часть (объёмы/дельты/фандинг/шаг цены) читается один раз с
//! ядра-провайдера — тот же дедуп по бирже, что и весь market-слой: рынок
//! `BTCUSDT@Bybit` идентичен у всех ядер этой биржи. Аккаунтная часть
//! (сессионный профит/позиция/плечо) персональна для каждого ядра, поэтому
//! суммируется по всем ядрам группы (`members`).
//!
//! Объёмы 1м/3м/5м и короткие дельты считает retained-история moonproto,
//! которая на провайдере покрывает ВСЕ рынки биржи (`subscribe_all_trades` →
//! `TradeStorageScope::All` + авто-свечи), поэтому колонки заполняются без
//! точечных подписок.

use moonproto::MoonTime;

use crate::session::CoreId;

use super::source::MarketDataSource;

/// Строка скринера: рыночные данные провайдера + аккаунтный оверлей группы.
#[derive(Clone, Debug, Default)]
pub struct ScreenerRow {
    /// Ядро-провайдер рыночных данных (для открытия чарта по строке).
    pub provider: CoreId,
    /// Каноничное имя рынка = `MarketHandle::name()` (биржевое `bn_market_name`).
    /// Именно по нему ключуются `handles_by_name`/подписки/поиск — display-имя
    /// MoonBot (`market_name`) для открытия чарта НЕ годится (пустой чарт).
    pub market: String,
    /// Монета («BTC»).
    pub coin: String,
    /// 24ч объём из листа рынков сервера, в котируемой валюте.
    pub vol_24h: f64,
    /// Часовой объём (сумма 5м-свечей за час), в котируемой валюте.
    pub vol_1h: f64,
    /// Скользящие объёмы сделок (окна 1/3/5 минут), в котируемой валюте.
    pub vol_1m: f64,
    pub vol_3m: f64,
    pub vol_5m: f64,
    /// Лучшая цена продажи.
    pub ask: f64,
    /// Максимум цены за последний час (по 5м-свечам + текущей).
    pub high_1h: f64,
    /// Макс. размер ордера биржи в котируемой валюте (MoonBot Max.Order):
    /// `max_notional`, фолбэк `max_qty × ask`. 0 — биржа лимит не отдала.
    pub max_order: f64,
    /// Дельты, %. 24ч/1ч — знаковые серверные (`MarketDeltaState`, парность
    /// MoonBot Coin1hDelta/Coin24hDelta); 3ч/15м/1м/72ч — derived-максимумы
    /// retained-истории (беззнаковая величина движения, как в таблице MoonBot).
    pub d_24h: f64,
    pub d_3h: f64,
    pub d_1h: f64,
    pub d_15m: f64,
    pub d_1m: f64,
    pub d_72h: f64,
    /// Фандинг, % (`funding_rate` × 100).
    pub funding_pct: f64,
    /// Отклонение mark price от last, %; None — mark не приходил.
    pub mark_delta_pct: Option<f64>,
    /// Шаг цены графика (абсолютный).
    pub price_step: f64,
    /// Макс. плечо рынка; 0 — спот/неизвестно.
    pub max_leverage: i32,
    /// Активное плечо аккаунта (макс. по ядрам группы); 0 — не выставлено.
    pub leverage_x: i32,
    /// Isolated-маржа (от ядра с активным плечом); None — плечо не выставлено.
    pub isolated: Option<bool>,
    /// Сессионный профит по монете (b+l+s), сумма по ядрам группы.
    pub session_pnl: f64,
    /// Суммарная позиция по ядрам группы, в монете.
    pub pos_size: f64,
    /// Открытые ордера по монете (сумма по ядрам группы). Заполняет UI-оверлей
    /// из `CoreData.orders` — market-слой ордеров не видит.
    pub orders: u32,
}

impl MarketDataSource {
    /// Построить строки скринера для группы ядер одной биржи.
    ///
    /// `provider` — ядро-провайдер рыночных данных группы (см.
    /// [`MarketDataSource::provider_of`]), `members` — все ядра группы, чьи
    /// аккаунтные поля складываются в оверлей. Пустой вектор — у провайдера
    /// ещё нет клиента/снимка.
    pub fn screener_rows(&self, provider: CoreId, members: &[CoreId]) -> Vec<ScreenerRow> {
        let Some(client) = self.core_client(provider) else {
            return Vec::new();
        };
        let Some(snap) = client.snapshot_versioned() else {
            return Vec::new();
        };
        // Аккаунтные снимки ядер группы (провайдер — тоже член группы).
        let member_snaps: Vec<_> = members
            .iter()
            .filter_map(|&core| self.core_client(core)?.snapshot_versioned())
            .collect();
        // Часовой хай: закрытые 5м-свечи штампуются КОНЦОМ периода, так что
        // отсечка по времени честно ограничивает окно последним часом даже на
        // рынках без свежих сделок.
        let hour_cutoff_ms = MoonTime::now().unix_millis() - 3_600_000;

        let markets = snap.markets();
        let mut rows = Vec::with_capacity(markets.market_count());
        for handle in markets.iter() {
            let name = handle.name();
            let mut row = handle.with(|m| ScreenerRow {
                provider,
                market: name.to_string(),
                coin: m.market_currency.clone(),
                vol_24h: m.volume,
                ask: m.price.ask,
                max_order: if m.max_notional() > 0.0 {
                    m.max_notional()
                } else {
                    m.max_qty() * m.price.ask
                },
                d_24h: m.delta_state.coin_24h_delta,
                d_1h: m.delta_state.coin_1h_delta,
                funding_pct: m.funding_rate * 100.0,
                mark_delta_pct: (m.price.mark_price_found
                    && m.price.p_last > 0.0
                    && m.price.mark_price > 0.0)
                    .then(|| (m.price.mark_price / m.price.p_last - 1.0) * 100.0),
                price_step: m.price.chart_price_step,
                max_leverage: m.max_leverage,
                ..ScreenerRow::default()
            });
            if let Some(d) = snap.market_history_derived_snapshot_now(name) {
                row.vol_1h = d.candle_volumes.one_hour;
                row.vol_1m = d.trade_volumes.one_minute.total_value();
                row.vol_3m = d.trade_volumes.three_minutes.total_value();
                row.vol_5m = d.trade_volumes.five_minutes.total_value();
                row.d_3h = d.deltas.three_hours;
                row.d_15m = d.deltas.fifteen_minutes;
                row.d_1m = d.deltas.one_minute;
                row.d_72h = d.deltas.seventy_two_hours;
                if let Some(c) = d.current_candle {
                    row.high_1h = f64::from(c.high());
                }
            }
            if let Some(candles) = snap
                .market_history_readers(name)
                .and_then(|r| r.candles_5m)
            {
                candles.with_last(12, |view| {
                    view.for_each(|c| {
                        if c.time().unix_millis() >= hour_cutoff_ms {
                            row.high_1h = row.high_1h.max(f64::from(c.high()));
                        }
                    })
                });
            }
            for msnap in &member_snaps {
                let Some(mh) = msnap.markets().get(name) else {
                    continue;
                };
                mh.with(|m| {
                    row.session_pnl += m.total_profit_b + m.total_profit_l + m.total_profit_s;
                    row.pos_size += m.pos_size;
                    if m.leverage_x != 0 && m.leverage_x > row.leverage_x {
                        row.leverage_x = m.leverage_x;
                        row.isolated = Some(m.position_type.is_isolated());
                    }
                });
            }
            rows.push(row);
        }
        rows
    }
}
