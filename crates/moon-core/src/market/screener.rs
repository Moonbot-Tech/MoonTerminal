//! Screener data (the "Coin Table"): a snapshot of ALL markets from a provider core.
//!
//! Market fields (volumes, deltas, funding, and price step) are read once from the provider
//! core, using the same per-exchange deduplication as the rest of the market layer: the
//! `BTCUSDT@Bybit` market is identical across all cores on that exchange. Account fields are
//! specific to each core, so the account overlay uses exactly the supplied `members`: session
//! profit and position are summed, while leverage is selected as their maximum. A core-filtered
//! caller may supply only the selected core and omit the market-data provider.
//!
//! Moonproto retained history computes the 1m/3m/5m volumes and short-term deltas. On the
//! provider it covers ALL exchange markets (`subscribe_all_trades` -> `TradeStorageScope::All`
//! plus automatic candles), so these columns need no per-market subscriptions.

use moonproto::{MoonTime, state::DerivedDeltaSnapshot};

use crate::session::CoreId;

use super::source::{MarketDataSource, max_order_notional};

/// Screener row combining provider market data with an overlay from the supplied members.
#[derive(Clone, Debug, Default)]
pub struct ScreenerRow {
    /// Market-data provider core associated with this row.
    pub provider: CoreId,
    /// Canonical market name from `MarketHandle::name()` (exchange `bn_market_name`).
    /// `handles_by_name`, subscriptions, and searches use this key. The Moonbot display name
    /// (`market_name`) cannot open a chart and would produce an empty chart.
    pub market: String,
    /// Coin symbol, such as `BTC`.
    pub coin: String,
    /// 24-hour volume from the server's market list, in the quote currency.
    pub vol_24h: f64,
    /// One-hour volume from 5-minute candles, in the quote currency.
    pub vol_1h: f64,
    /// Rolling trade volumes over 1-, 3-, and 5-minute windows, in the quote currency.
    pub vol_1m: f64,
    pub vol_3m: f64,
    pub vol_5m: f64,
    /// Best ask price.
    pub ask: f64,
    /// Highest price in the last hour across 5-minute candles and the current candle.
    pub high_1h: f64,
    /// Exchange maximum order size in the quote currency (Moonbot Max.Order).
    ///
    /// Derived by [`max_order_notional`], which is also what the trading toolbar's readout uses, so
    /// this column and that readout cannot disagree: a stated `max_notional` wins, and otherwise the
    /// quantity cap is converted — through `contract_size` on a coin-margined market, where quantity
    /// counts CONTRACTS, and through the ask elsewhere. Zero means no cap was provided, or that the
    /// price needed to convert one has not arrived yet.
    pub max_order: f64,
    /// Unsigned retained-history movement magnitudes matching the Moonbot Screener table.
    ///
    /// Every period uses MoonProto's combined max/min range contract. Its production-compatible
    /// 3h bucket covers roughly four hours, while 72h covers the available retained candle tail.
    pub d_24h: f64,
    pub d_3h: f64,
    pub d_1h: f64,
    pub d_15m: f64,
    pub d_1m: f64,
    pub d_72h: f64,
    /// Funding percentage, as the wire already states it.
    ///
    /// NOT `funding_rate * 100`, which this column printed until 2026-08-24: the value arrives in
    /// percent, so the multiplication showed a −23 % funding where the reference terminal showed
    /// −0.23 %. See `super::source::funding_from_wire`.
    pub funding_pct: f64,
    /// Mark-price deviation from the last price, in percent; `None` if no mark price arrived.
    pub mark_delta_pct: Option<f64>,
    /// Absolute chart price step.
    pub price_step: f64,
    /// Maximum market leverage; zero means spot or unknown.
    pub max_leverage: i32,
    /// Active account leverage, maximized across supplied members; zero means unset.
    pub leverage_x: i32,
    /// Whether margin is isolated on the core supplying active leverage; `None` if leverage is unset.
    pub isolated: Option<bool>,
    /// Per-coin session profit (b+l+s), summed across supplied members.
    pub session_pnl: f64,
    /// Aggregate position across supplied members, denominated in the coin.
    pub pos_size: f64,
    /// Open per-coin orders summed across supplied members. The UI overlay fills this from
    /// `CoreData.orders` because the market layer does not see orders.
    pub orders: u32,
}

impl ScreenerRow {
    /// Apply one coherent retained-history delta snapshot to every Screener period.
    ///
    /// Args:
    ///     deltas: Combined range deltas, or `None` before retained history is available.
    ///
    /// Returns:
    ///     Nothing; all six delta fields are replaced, with missing history becoming neutral zero.
    fn apply_range_deltas(&mut self, deltas: Option<DerivedDeltaSnapshot>) {
        let deltas = deltas.unwrap_or_default();
        self.d_24h = deltas.twenty_four_hours;
        self.d_3h = deltas.three_hours;
        self.d_1h = deltas.one_hour;
        self.d_15m = deltas.fifteen_minutes;
        self.d_1m = deltas.one_minute;
        self.d_72h = deltas.seventy_two_hours;
    }
}

impl MarketDataSource {
    /// Builds screener rows for a group of cores on one exchange.
    ///
    /// `provider` is the group's market-data provider core (see
    /// [`MarketDataSource::provider_of`]); `members` is the exact set of cores whose account fields
    /// contribute to the overlay. In core-filtered mode it may exclude `provider`. The function
    /// returns an empty result if the provider has no client or snapshot yet.
    pub fn screener_rows(&self, provider: CoreId, members: &[CoreId]) -> Vec<ScreenerRow> {
        let Some(client) = self.core_client(provider) else {
            return Vec::new();
        };
        let Some(snap) = client.snapshot_versioned() else {
            return Vec::new();
        };
        // Account snapshots come from exactly the supplied members; the provider is not implicit.
        let member_snaps: Vec<_> = members
            .iter()
            .filter_map(|&core| self.core_client(core)?.snapshot_versioned())
            .collect();
        // Closed 5-minute candles are timestamped at the END of their period, so the cutoff
        // accurately limits the high window to the last hour even for markets with no recent trades.
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
                max_order: max_order_notional(
                    &m.base_currency,
                    m.max_notional(),
                    m.max_qty(),
                    m.price.ask,
                    m.contract_size(),
                )
                .value,
                funding_pct: m.funding_rate,
                mark_delta_pct: (m.price.mark_price_found
                    && m.price.p_last > 0.0
                    && m.price.mark_price > 0.0)
                    .then(|| (m.price.mark_price / m.price.p_last - 1.0) * 100.0),
                price_step: m.price.chart_price_step,
                max_leverage: m.max_leverage,
                ..ScreenerRow::default()
            });
            let derived = snap.market_history_derived_snapshot_now(name);
            row.apply_range_deltas(derived.map(|snapshot| snapshot.deltas));
            if let Some(d) = derived {
                row.vol_1h = d.candle_volumes.one_hour;
                row.vol_1m = d.trade_volumes.one_minute.total_value();
                row.vol_3m = d.trade_volumes.three_minutes.total_value();
                row.vol_5m = d.trade_volumes.five_minutes.total_value();
                if let Some(c) = d.current_candle {
                    row.high_1h = f64::from(c.high());
                }
            }
            if let Some(candles) = snap.market_history_readers(name).and_then(|r| r.candles_5m) {
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

#[cfg(test)]
mod tests;
