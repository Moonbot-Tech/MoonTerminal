//! Screener data (the "Coin Table"): a snapshot of ALL markets from a provider core.
//!
//! Market fields (volumes, deltas, funding, and price step) are read once from the provider
//! core, using the same per-exchange deduplication as the rest of the market layer: the
//! `BTCUSDT@Bybit` market is identical across all cores on that exchange. Account fields are
//! specific to each core, so a row is a market ON ONE CORE: [`MarketDataSource::screener_rows`]
//! states the market half once and repeats it per supplied member with that member's own PnL,
//! Session, position and leverage. Until 2026-09-12 those were SUMMED across the members into one
//! row that carried the provider's name — a row reading "BinF1" while its Session was the whole
//! exchange's, which is what a chart caption on BinF1 then disagreed with. A caller that needs the
//! market half alone, such as the coin search, reads [`MarketDataSource::screener_market_rows`].
//!
//! Moonproto retained history computes the 1m/3m/5m volumes and short-term deltas. On the
//! provider it covers ALL exchange markets (`subscribe_all_trades` -> `TradeStorageScope::All`
//! plus automatic candles), so these columns need no per-market subscriptions.

use moonproto::{MoonTime, state::DerivedDeltaSnapshot};

use crate::session::CoreId;

use super::source::{MarketDataSource, max_order_notional, session_base_rate, session_to_usdt};

/// One market on one core: the provider's market data with that core's account fields.
#[derive(Clone, Debug, Default)]
pub struct ScreenerRow {
    /// Market-data provider core the market half was read from — the row's EXCHANGE identity:
    /// `(provider, market)` is unique across a whole screener, while `market` alone repeats across
    /// exchanges and `(core, market)` repeats across the cores sharing one.
    pub provider: CoreId,
    /// The core whose account fields this row states — the one a chart opened from it belongs to.
    /// Equal to `provider` on a market-only row from [`MarketDataSource::screener_market_rows`].
    pub core: CoreId,
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
    /// Active account leverage on [`Self::core`]; zero means unset.
    pub leverage_x: i32,
    /// Whether margin is isolated on [`Self::core`]; `None` if leverage is unset.
    pub isolated: Option<bool>,
    /// The core's own per-coin profit counter (`b + l + s`), which MoonBot prints as `PnL`.
    ///
    /// NOT the Session counter — that one is [`Self::session`]. Zero on part of the venues even
    /// where MoonBot shows an amount, so a zero here is not "traded to break even". Stated RAW, as
    /// the chart's `PnL` caption prints it: on a coin-margined venue the exchange states it in the
    /// settlement coin, and neither surface converts it. Until 2026-09-12 this column was titled
    /// "Session", which is how it came to disagree with the chart caption of that name.
    pub core_pnl: f64,
    /// The Session counter MoonBot's markets table resets, in USDT through [`session_to_usdt`] —
    /// the rule the chart caption reads, so the two print one number for one core.
    ///
    /// `None` when the core cannot state it: a build predating the protocol field, a base currency
    /// the terminal cannot value in USDT, or no balance row for this market on this core. A real
    /// zero is `Some(0.0)`.
    pub session: Option<f64>,
    /// Open position on [`Self::core`], denominated in the coin.
    pub pos_size: f64,
    /// Open orders for this market on [`Self::core`]. The UI overlay fills this from
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
    /// Builds one screener row per market PER MEMBER core of a group on one exchange.
    ///
    /// `provider` is the group's market-data provider core (see
    /// [`MarketDataSource::provider_of`]); `members` are the cores whose account fields the rows
    /// state, one row each per market. In core-filtered mode it may exclude `provider`. A member
    /// whose snapshot lacks a market — or has no snapshot at all yet — still gets every market's
    /// row, with account fields at their defaults: the market exists on the exchange, this core
    /// just states nothing for it, and a Screener filtered to a core still connecting shows the
    /// exchange rather than an unexplained blank. Empty only when the provider has no snapshot.
    ///
    /// Args:
    ///     provider: The group's market-data provider core.
    ///     members: The cores to state a row for, in the order the rows come out.
    ///
    /// Returns:
    ///     Rows grouped by member, each member's in the provider's market order.
    pub fn screener_rows(&self, provider: CoreId, members: &[CoreId]) -> Vec<ScreenerRow> {
        let market_rows = self.screener_market_rows(provider);
        if market_rows.is_empty() {
            return market_rows;
        }
        // Account snapshots come from exactly the supplied members; the provider is not implicit.
        // Each carries its Session base rate, resolved ONCE per member: the rate is a property of
        // the account, and the loop below asks for it on every market of the exchange.
        let member_snaps: Vec<_> = members
            .iter()
            .map(|&core| {
                let snap = self
                    .core_client(core)
                    .and_then(|client| client.snapshot_versioned())
                    .map(|snap| {
                        let rate = session_base_rate(&snap);
                        (snap, rate)
                    });
                (core, snap)
            })
            .collect();
        let mut rows = Vec::with_capacity(market_rows.len() * member_snaps.len());
        for (core, msnap) in &member_snaps {
            let markets = msnap.as_ref().map(|(snap, _)| snap.markets());
            for market_row in &market_rows {
                let mut row = market_row.clone();
                row.core = *core;
                if let Some((mh, rate)) = markets
                    .and_then(|markets| markets.get(&row.market))
                    .zip(msnap.as_ref().map(|(_, rate)| *rate))
                {
                    // The Session counter is read inside the same lock as the balance fields
                    // rather than through `MarketHandle::session_profit`, which would take it a
                    // second time per market per member. A core without a balance row for this
                    // market is not a core without a counter: the sparse snapshot states zero for
                    // every market it omits, so `Some(0.0)` lands here.
                    let session = mh.with(|m| {
                        row.core_pnl = m.total_profit();
                        row.pos_size = m.pos_size;
                        if m.leverage_x != 0 {
                            row.leverage_x = m.leverage_x;
                            row.isolated = Some(m.position_type.is_isolated());
                        }
                        m.session_profit
                    });
                    row.session = session_to_usdt(session, rate);
                }
                rows.push(row);
            }
        }
        rows
    }

    /// Builds the market half of the screener: one row per market of `provider`, no account fields.
    ///
    /// What every core on the exchange shares — volumes, deltas, funding, caps — read once from the
    /// provider. `core` is the provider and the account fields stay at their defaults; a
    /// caller that wants them per core goes through [`Self::screener_rows`], which builds on this.
    ///
    /// Args:
    ///     provider: A market-data provider core (see [`MarketDataSource::provider_of`]).
    ///
    /// Returns:
    ///     Rows in the provider's market order; empty when it has no client or snapshot yet.
    pub fn screener_market_rows(&self, provider: CoreId) -> Vec<ScreenerRow> {
        let Some(client) = self.core_client(provider) else {
            return Vec::new();
        };
        let Some(snap) = client.snapshot_versioned() else {
            return Vec::new();
        };
        // Closed 5-minute candles are timestamped at the END of their period, so the cutoff
        // accurately limits the high window to the last hour even for markets with no recent trades.
        let hour_cutoff_ms = MoonTime::now().unix_millis() - 3_600_000;

        let markets = snap.markets();
        let mut rows = Vec::with_capacity(markets.market_count());
        for handle in markets.iter() {
            let name = handle.name();
            let mut row = handle.with(|m| ScreenerRow {
                provider,
                core: provider,
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
            rows.push(row);
        }
        rows
    }
}

#[cfg(test)]
mod tests;
