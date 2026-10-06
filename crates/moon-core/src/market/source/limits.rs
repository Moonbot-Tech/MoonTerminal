//! Market trading limits, order-size rules and session valuation.

#[cfg(doc)]
use super::MarketDataSource;

/// Market price snapshot for the header ticker: last price and signed percentage deltas.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketTickerReadout {
    pub last: f64,
    pub delta_1h_pct: f64,
    pub delta_24h_pct: f64,
}

/// Where a market's maximum order size came from.
///
/// The two non-absent cases behave DIFFERENTLY in front of a user and must stay distinguishable:
/// a stated cap is a fixed exchange figure, while a derived one is recomputed from the current ask
/// and therefore drifts as the price moves. A readout that cannot tell them apart either explains
/// nothing or explains the wrong thing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaxOrderSource {
    /// The exchange stated a notional cap directly.
    Stated,
    /// No notional cap was stated, so the quantity cap was converted into quote currency.
    Derived,
    /// A quantity cap EXISTS but what it takes to convert it has not arrived — on a linear market
    /// that is the ask price, which is zero until the market's first price update lands.
    ///
    /// Distinct from [`Self::Absent`] and never merged with it: this is "not known yet", while
    /// `Absent` is "the exchange says there is no cap". Telling an operator a market has no maximum
    /// order size when the truth is that its price has not loaded is the exact misstatement the
    /// two-level unknown in [`MarketLimits`] exists to prevent.
    Pending,
    /// The exchange stated no cap of either kind.
    #[default]
    Absent,
}

/// A market's maximum order size together with how it was obtained.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MaxOrder {
    /// The cap in the quote currency; `0.0` while its source is pending or absent.
    pub value: f64,
    /// Provenance of `value`, so a readout can say WHY the figure moves — or why there is none.
    pub source: MaxOrderSource,
}

/// One market's exchange-imposed trading limits, for the leverage control.
///
/// Every field carries its own "unknown", and those unknowns are DIFFERENT facts the UI states
/// differently: an absent [`MarketDataSource::market_limits`] result means no provider, snapshot or
/// market has arrived yet, while [`MaxOrderSource::Absent`] or a zero `max_leverage` here means the
/// exchange itself stated no cap (or the market is spot, which has no leverage). Collapsing the two
/// would tell an operator "this coin has no limit" when the truth is "nothing has loaded".
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketLimits {
    /// Exchange maximum order size in the quote currency, with its provenance.
    ///
    /// FLAT: it does not vary with the selected leverage and is the same figure at x1 and at x50.
    pub max_order: MaxOrder,
    /// Maximum market leverage; `0` means spot or unknown.
    pub max_leverage: i32,
    /// The exchange's cap on the WHOLE position in this coin at the CURRENT leverage, in the quote
    /// currency (MoonBot's `<max. N>`), or `None` when the core has not stated one.
    ///
    /// Unlike `max_order` it is leverage-dependent: lowering leverage raises it. See
    /// [`position_cap`].
    pub position_cap: Option<f64>,
}

/// The leverage-dependent position cap as it may be shown, from the core's raw `bn_max_value`.
///
/// Zero and non-finite both mean the core stated nothing, so they map to `None` — the UI hides the
/// row rather than print a `0` that reads as "you may hold nothing".
///
/// Args:
///     raw: The market's `max_value()` as the core reports it.
///
/// Returns:
///     The cap in quote currency, or `None` when it is unknown.
pub(crate) fn position_cap(raw: f64) -> Option<f64> {
    (raw.is_finite() && raw > 0.0).then_some(raw)
}

/// What one unit of a market's `quantity` field IS.
///
/// A separate type rather than an `Option<f64>` because the money path needs THREE answers and an
/// option only carries two. "Linear, so quantity is coins" and "nothing about this market has
/// arrived" must never collapse into one value: the second one, read as the first, sends the USD
/// figure itself as a quantity — which on a coin-margined market is the 100x order this whole rule
/// exists to prevent. A caller that cannot get an answer must refuse, not fall back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarketQuantityUnit {
    /// Contracts, each worth this fixed amount of quote currency (coin-margined/inverse).
    Contracts(f64),
    /// The market's own coin amount, converted through the account's balance currency.
    Coins,
}

/// Whether a market is INVERSE (coin-margined), from the quote currency it reports.
///
/// A coin-margined contract is denominated in USD and reports NO quote currency; a linear one
/// names it. This is the same test `feed/live/convert/orders.rs` uses to read positions back, and the
/// same one `build_assets` uses.
///
/// Two other discriminators were tried against a live COIN-M core and neither works:
///
/// - the market NAME — `resolve_quote_on("BTCUSD_PERP", BinanceCoinM)` answers `"USD"`, so a name
///   test calls Binance's coin-margined contracts linear and only fires on Bybit-style `AAVEPERP`;
/// - `futures_type`, the protocol's settlement enum, which LOOKS authoritative and would be — the
///   2026-08-31 QQ probe found it arrives `EMPTY` for `BTCUSD_PERP`, `SOLUSD_260925` and
///   `ADAUSD_PERP` alike, while `contract_size` (100 / 10 / 10) and the empty quote both arrive
///   correctly. An unset field cannot discriminate anything.
///
/// The emptiness is only trustworthy for a market that IS in the snapshot — quote and contract
/// size travel in one wire record, so they cannot skew apart. A market that has not arrived at all
/// is a different answer, and [`MarketDataSource::market_quantity_unit`] returns `None` for it.
///
/// It is also not sufficient ON ITS OWN, which is why the caller pairs it with a contract size
/// other than 1: Hyperliquid markets report an empty catalog quote while being USDC-quoted and
/// LINEAR (`HFUN` in `label_tests.rs`), and reading one of those as contracts would send the USD
/// figure straight through as a coin quantity. `convert/orders.rs` draws the line in the same place.
pub(super) fn quote_is_absent(quote: &str) -> bool {
    quote.trim().is_empty()
}

/// What a manual order on one market must satisfy: the unit its quantity counts, and the smallest
/// order the exchange accepts.
///
/// One value rather than two lookups: both come from the same market record, and reading them
/// separately would let a snapshot land between them and pair one market's unit with another's
/// floor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrderSizeRules {
    /// What one unit of `size` is on this market.
    pub unit: MarketQuantityUnit,
    /// Smallest order the venue accepts, in USD, or `None` when that is NOT KNOWN.
    ///
    /// Stated in MONEY for both units on purpose, because money is the only currency the two
    /// share: a trader's order size is always dollars, while the wire quantity is coins on one
    /// market and contracts on another. Handing the caller the venue's raw quantity instead is
    /// what let a dollar figure be compared against a coin count — see [`min_order_floor`].
    pub min_order_usd: Option<f64>,
}

/// What a market's quantity field counts, from the two figures the core reports about it.
///
/// The two OUTBOUND readers — the maximum order cap and the size a manual order is placed with —
/// both ask here, because a copy of the test at a call site is a second place to get it wrong and
/// the two answers must never disagree: a cap stated in USD beside an order sized in contracts is
/// how a $500 order becomes $50 000.
///
/// `feed/live/convert/orders.rs` still carries its own INBOUND version of this test (contracts -> coins).
/// It is deliberately left alone here: it reads positions the core already reports and is not on
/// the money path this rule protects. Unifying the two is worth doing, and is not worth doing
/// inside a fix to order sizing.
///
/// Args:
///     quote: The market's quote currency as it reports it; empty means coin-margined.
///     contract_size: Fixed quote-currency value of one contract, as the market reports it.
///
/// Returns:
///     The unit, or `None` when it is NOT KNOWN — the market reports no quote AND no contract
///     value, so neither branch can be chosen. A caller sizing real money must refuse on that
///     rather than resolve it either way.
pub(crate) fn market_quantity_unit(quote: &str, contract_size: f64) -> Option<MarketQuantityUnit> {
    if !quote_is_absent(quote) {
        return Some(MarketQuantityUnit::Coins);
    }
    // The pair, not the empty quote alone. `contract_size == 1.0` beside an empty quote is how a
    // LINEAR Hyperliquid market presents itself, and calling that inverse would send the USD figure
    // through as a coin quantity — the same trap in the opposite direction. A genuine inverse
    // contract worth exactly one dollar is therefore read as linear too; that costs the coin's
    // price as a factor on one venue, against emptying an account on the other reading.
    // `convert/orders.rs` makes the identical trade-off inbound.
    if !contract_size.is_finite() || contract_size <= 0.0 {
        return None;
    }
    match contract_size == 1.0 {
        true => Some(MarketQuantityUnit::Coins),
        false => Some(MarketQuantityUnit::Contracts(contract_size)),
    }
}

/// The smallest order the venue accepts, in USD, whatever unit its quantity counts.
///
/// ONE derivation site for the venue's minimum, mirroring [`max_order_notional`] on the other end
/// of the same range: a floor read one way here and another way at a call site is how a coin count
/// gets compared against a dollar figure. That is not hypothetical — it refused every manual order
/// on Gate's `STONKS_USDT` on 2026-09-06, where `$200 < 1000` compared a trader's dollars against
/// the market's 1000-COIN minimum, about $17.70, while Moonbot placed $120 orders on that same
/// contract minutes earlier. Everything else that day had a minimum of 10 coins or less, which any
/// order in dollars clears by accident, so nothing but a cheap coin ever showed it.
///
/// The two units reach the same figure by different routes, and neither route works for the other:
///
/// - **Coins** — the lot value the core already derives on every price tick,
///   `max(step_size, min_qty) * mid` floored by the exchange's own `min_notional`
///   (`state/markets/prices.rs`, parity with Moonbot's Delphi `MinLotSize`), converted out of the
///   market's quote currency. Re-deriving it here from `min_qty` would restate a rule the core owns
///   AND drop the `min_notional` half of it.
/// - **Contracts** — a contract is denominated in quote currency, so its floor is the count the
///   venue insists on times one contract's value, with no exchange rate and no price involved. The
///   lot value is useless here: it multiplies a CONTRACT count by a COIN price, reading about
///   $60 000 on `BTCUSD_PERP` where one contract is $100.
///
/// Args:
///     unit: What the market's quantity counts, from [`market_quantity_unit`].
///     min_qty: Exchange minimum quantity in that unit; only a contract count is read from it, and
///         never below one whole contract, which cannot be split.
///     min_lot_value: The market's smallest lot in its own quote currency, `0` before the first
///         price tick has set it; read on a linear market only.
///     quote_usd_rate: USD value of one unit of that quote currency, from
///         [`MarketDataSource::quote_usd_rate`] — which answers `1.0` for the DEX markets that name
///         no quote at all, and is why the caller does not ask `currency_usd_rate` directly. Read
///         on a linear market only.
///
/// Returns:
///     The floor in USD, or `None` when there is none to state: a quote this core cannot price, or
///     a market whose first price tick has not landed. Both mean DO NOT BLOCK, deliberately — the
///     exchange is the authority on its own minimum and rejects an undersized order itself, whereas
///     a floor invented from figures that have not arrived refuses orders the venue would have
///     taken. Every caller must honour that.
///
/// One residual gap, and it is inherited rather than introduced: an inverse contract worth exactly
/// one dollar reads as linear, so its floor comes back coin-priced and would refuse orders.
/// [`market_quantity_unit`] already argues that trade-off and takes it deliberately — the other
/// reading empties an account — and no supported venue lists such a contract.
pub(crate) fn min_order_floor(
    unit: MarketQuantityUnit,
    min_qty: f64,
    min_lot_value: f64,
    quote_usd_rate: Option<f64>,
) -> Option<f64> {
    let usd = match unit {
        MarketQuantityUnit::Contracts(contract_size) => {
            // A count that arrives non-finite is garbage, not a minimum, and it must not take the
            // floor down with it: an infinite `min_qty` multiplies out to infinity, which the
            // finite check below would turn into "no floor at all". One contract is the smallest
            // thing the venue can accept and cannot be split, so that is what a broken count falls
            // back to. `f64::max` already maps a NaN count there; infinity does not.
            let contracts = match min_qty.is_finite() {
                true => min_qty.max(1.0),
                false => 1.0,
            };
            contracts * contract_size
        }
        MarketQuantityUnit::Coins => {
            let rate = quote_usd_rate?;
            if !(min_lot_value.is_finite() && min_lot_value > 0.0 && rate.is_finite() && rate > 0.0)
            {
                return None;
            }
            min_lot_value * rate
        }
    };
    (usd.is_finite() && usd > 0.0).then_some(usd)
}

/// The exchange maximum order size in the quote currency, from the figures a market carries.
///
/// The stated notional cap wins. Only when the exchange gave none is the QUANTITY cap converted,
/// and that conversion is not one formula but two, because a quantity does not mean the same thing
/// on every market:
///
/// - **Inverse (coin-margined) futures report quantity in CONTRACTS**, each worth a fixed amount of
///   quote currency (`contract_size` — BTCUSD is $100, other `*USD` contracts $10). The notional is
///   therefore `max_qty * contract_size` and needs no price at all. `feed/live/convert/orders.rs` derives
///   position sizes from the same fact; multiplying a contract COUNT by a coin PRICE instead would
///   be off by roughly the contract size.
/// - **Linear markets report quantity in the base coin**, so the notional is `max_qty * ask`. This
///   is why that fallback MOVES with the price while a stated cap stands still — a distinction the
///   returned [`MaxOrderSource`] preserves so the UI can say so instead of hiding it.
///
/// The market's SETTLEMENT currency separates the two — see [`market_quantity_unit`], which makes
/// that test once for both this cap and the size of an order. `contract_size != 1` does not: linear
/// QUANTO futures also carry a contract size (Gate's `ASTEROID_USDT` has 10000) while still
/// reporting quantity in coins.
///
/// A quantity cap whose conversion cannot be completed yet — a linear market before its first price
/// tick, where `ask` is still zero, or a coin-settled one whose contract value has not arrived — is
/// [`MaxOrderSource::Pending`], NOT `Absent`. Only a market stating neither cap is `Absent`. Both
/// render as a dash, and they explain themselves differently.
///
/// Takes the market's FIGURES as primitives rather than the market itself, because `moonproto`'s
/// `Market` is not re-exported at its crate root and cannot be named here. Both readers of the rule
/// — `screener_rows` and [`MarketDataSource::market_limits`] — go through this function, so the
/// Screener's `Max.Order` column and the trading toolbar can never print two different caps for one
/// coin.
///
/// Args:
///     quote: The market's quote currency; empty means coin-margined. See [`market_quantity_unit`].
///     max_notional: Exchange-stated maximum order size in quote currency, when available.
///     max_qty: Exchange-stated maximum quantity, used only when no notional cap exists.
///     ask: Current best ask used to convert a linear-market quantity cap.
///     contract_size: Fixed quote-currency value of one inverse contract.
///
/// Returns:
///     A stated, derived, pending, or absent quote-currency cap with its provenance.
pub(crate) fn max_order_notional(
    quote: &str,
    max_notional: f64,
    max_qty: f64,
    ask: f64,
    contract_size: f64,
) -> MaxOrder {
    if max_notional.is_finite() && max_notional > 0.0 {
        return MaxOrder {
            value: max_notional,
            source: MaxOrderSource::Stated,
        };
    }
    if !max_qty.is_finite() || max_qty <= 0.0 {
        return MaxOrder::default();
    }
    let value = match market_quantity_unit(quote, contract_size) {
        Some(MarketQuantityUnit::Contracts(contract_size)) => max_qty * contract_size,
        Some(MarketQuantityUnit::Coins) => max_qty * ask,
        // Coin-settled, contract value not reported yet: the cap exists and cannot be converted,
        // which is what `Pending` means. Zero falls into that branch below.
        None => 0.0,
    };
    if !value.is_finite() || value <= 0.0 {
        // The cap exists; what converts it does not yet. Never report that as "no cap".
        return MaxOrder {
            value: 0.0,
            source: MaxOrderSource::Pending,
        };
    }
    MaxOrder {
        value,
        source: MaxOrderSource::Derived,
    }
}

/// The core's per-market Session counter, valued in USDT, or `None` when it cannot be stated.
///
/// ONE rule for every surface that prints "Session": the chart caption reads it here, and the
/// Screener column reads its two halves — [`session_base_rate`] once per core and
/// [`session_to_usdt`] per market — so they can never disagree about a coin the way the Screener's
/// old "Session" column, which summed `TotalProfitB/L/S` (MoonBot's `PnL`), disagreed with the
/// caption beside it.
///
/// The three ways this is `None` are one answer, "not statable": the core publishes no
/// session-profit snapshot at all (every build predating the protocol field), the value is not a
/// number, or the core's base currency cannot be valued in USDT — which also covers the moments
/// right after connect, before `BaseCheck` names that currency. A real zero is `Some(0.0)`.
///
/// Takes the market's lock once for the counter; a caller already inside `MarketHandle::with` reads
/// `Market::session_profit` there and calls [`session_to_usdt`] itself.
///
/// Args:
///     snapshot: The CONSUMER core's snapshot — the account the counter belongs to, never the
///         market-data provider's.
///     handle: The market within that same snapshot.
///
/// Returns:
///     The counter in USDT, or `None` when the core does not state it or it cannot be valued.
pub(crate) fn session_usdt(
    snapshot: &moonproto::MoonClientSnapshot,
    handle: &moonproto::state::MarketHandle,
) -> Option<f64> {
    session_to_usdt(handle.session_profit(), session_base_rate(snapshot))
}

/// USDT per unit of the core's base currency, for valuing its Session counters; zero when unknown.
///
/// Per CORE, not per market: the rate is a property of the account, and a caller walking every
/// market of an exchange for one core resolves it once. A stablecoin base settles at 1 without
/// touching the catalogue, which is every USDT and USDC core; a coin-denominated base such as BTC
/// costs a handful of name lookups. The rule is `feed::assets::base_rate`, not moonproto's own
/// `session_profit_for`: that one resolves the rate from correlation markets alone and answers
/// `None` whenever the core sent no correlation market for its own base currency.
///
/// Args:
///     snapshot: The core's snapshot, which names its base currency and lists its markets.
///
/// Returns:
///     The rate, or `0.0` when the base currency is unnamed yet or cannot be valued.
pub(crate) fn session_base_rate(snapshot: &moonproto::MoonClientSnapshot) -> f64 {
    // Borrowed, not cloned: this runs per pane on every market revision, and the currency name is
    // read and dropped inside the same expression.
    let base_ccy = snapshot
        .server_info()
        .base_currency_name
        .as_deref()
        .unwrap_or_default();
    crate::feed::assets::base_rate(snapshot.markets(), base_ccy)
}

/// Value one Session counter in USDT at a rate from [`session_base_rate`].
///
/// Converted here rather than at the caller, because the raw value is in the CORE's base currency:
/// a coin-margined core states it in BTC, and printing that under a dollar sign is how `0.0004 BTC`
/// reads as nothing at all. A base this build cannot value is therefore withheld rather than shown
/// unconverted.
///
/// Args:
///     base_value: `Market::session_profit` — `None` when the core states no counter.
///     rate: USDT per unit of the core's base currency; zero means unknown.
///
/// Returns:
///     The counter in USDT, or `None` when it is absent, not a number, or the rate is unknown.
pub(crate) fn session_to_usdt(base_value: Option<f64>, rate: f64) -> Option<f64> {
    let base_value = base_value.filter(|v| v.is_finite())?;
    (rate > 0.0).then_some(base_value * rate)
}
