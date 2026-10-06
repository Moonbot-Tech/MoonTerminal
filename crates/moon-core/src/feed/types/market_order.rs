//! Market identity, price data and order-row types.

/// Side of a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// Core exchange identifier composed of moonproto's `ExchangeCode` byte and a HIP-3 DEX
/// discriminator.
///
/// Spot and futures exchanges have distinct codes, such as Binance=3, FBinance=4, ByBit=7, and
/// FBybit=2. Cores with the same `ExchangeId` see identical market data and can share one provider.
/// Hyperliquid futures on different HIP-3 DEXes such as `xyz` and `crypto` share the same `code`
/// but have different market universes, so the key also contains a hash of `dex_name`. Without it,
/// deduplication would merge them under one provider with an incomplete market list, causing
/// missing prices, order books, trades, and search results such as `xyz:HOOD`. Primitive fields
/// keep the domain types independent of moonproto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExchangeId {
    /// `ExchangeCode` byte from moonproto.
    pub code: u8,
    /// HIP-3 DEX discriminator derived from a `dex_name` hash; `0` means a regular non-DEX exchange.
    pub dex: u32,
}

impl ExchangeId {
    /// Construct a regular spot or futures exchange without a HIP-3 DEX, using `dex = 0`.
    pub const fn new(code: u8) -> Self {
        Self { code, dex: 0 }
    }

    /// Construct an exchange with a HIP-3 DEX discriminator.
    ///
    /// An empty DEX name maps to `dex = 0` like a regular exchange. Otherwise the discriminator is
    /// a deterministic FNV-1a hash of the name. Letter case is deliberately not normalized because
    /// `dex_name` arrives from BaseCheck unchanged and remains stable for the session.
    pub fn with_dex(code: u8, dex_name: &str) -> Self {
        Self {
            code,
            dex: fnv1a32(dex_name.as_bytes()),
        }
    }
}

/// Compute a 32-bit FNV-1a hash, mapping empty input to `0` so no DEX and an empty DEX coincide.
fn fnv1a32(bytes: &[u8]) -> u32 {
    if bytes.is_empty() {
        return 0;
    }
    let mut hash: u32 = 0x811c_9dc5;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// One trade tick represented as a semantic chart point.
#[derive(Debug, Clone, Copy)]
pub struct Tick {
    /// Unix time in milliseconds from the core's `row.unix_millis()`.
    pub time_ms: f64,
    pub price: f32,
    /// Absolute trade quantity in the base currency.
    pub qty: f32,
    pub side: Side,
}

/// Retained LastPrice or MarkPrice line point with time already converted to Unix milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct PricePoint {
    pub time_ms: f64,
    pub price: f32,
}

/// Order-book level.
#[derive(Debug, Clone, Copy)]
pub struct Level {
    pub price: f32,
    pub qty: f32,
}

/// Snapshot of the top order-book bids and asks.
#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    /// Bids in descending price order.
    pub bids: Vec<Level>,
    /// Asks in ascending price order.
    pub asks: Vec<Level>,
}

/// Point in a server-provided order trace for the chart, already in Unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrderTracePoint {
    pub time_ms: f64,
    pub price: f32,
}

/// Server-provided polyline trace for an order's buy or sell line.
///
/// Moonproto remains within the feed layer, while the UI receives only this domain structure.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OrderTrace {
    pub points: Vec<OrderTracePoint>,
    pub tmp_point: Option<OrderTracePoint>,
    pub stop_price: Option<f32>,
    pub stop_time_ms: Option<f64>,
}

/// Open order displayed in the bottom dock.
#[derive(Debug, Clone)]
pub struct OrderRow {
    /// Market name used as the data key from moonproto `market_name` for subscriptions, prices,
    /// chart opening, and matching. For Hyperliquid spot this is an index such as `@206`, not a
    /// human-readable name.
    pub market: String,
    /// Display market name from `market_name_mb_classic`, such as `@206` becoming `UENAUSDT`;
    /// regular markets equal `market`. Data lookups use `market`. Keeping them separate preserves
    /// key-based matching and lookups.
    pub market_display: String,
    /// Coin token for this market under its own exchange's naming rules: `ADA` for `ADAUSDT`,
    /// `BEAT` for OKX's `BEAT-USDT-SWAP`, `UENA` for Hyperliquid spot's `@206`.
    ///
    /// Resolved ONCE, here, so every consumer shows and writes the same token: the Orders table,
    /// the order-edit title and the coin menu that writes this value into the core's and the
    /// strategy's coin blacklists, which the core matches against its own `market_currency`.
    /// Re-deriving it per panel is what let those disagree.
    ///
    /// Read from the core's own catalog field `market_currency`, which is what the core matches
    /// its coin lists against by exact text and what its report writes. It carries foldings no
    /// rule could derive from the market name — Bybit's `1000BONKPERP` is `1kBONKPERP`, COIN-M's
    /// `AAVEUSD_PERP` is `AAVE_RP` — which is why a token derived from the name would be written
    /// into a blacklist the core then fails to match.
    ///
    /// Not `market_currency_canonic`: that is the contract-free WALLET identity (`BONKPERP`,
    /// `AAVE`), correct for deduplicating holdings in `feed::assets` and wrong here.
    ///
    /// A market the catalog no longer holds falls back to the per-exchange name rules in
    /// `moon_core::symbol::parse`.
    pub coin: String,
    /// Quote currency for this market from the catalog's `base_currency`, uppercase, resolved
    /// beside [`Self::coin`] so a panel can label the pair without asking the market source.
    /// Empty when neither the catalog nor the name carries one — a COIN-M contract reports none.
    pub quote: String,
    /// true = Short, false = Long.
    pub is_short: bool,
    /// Entry-leg size in the base currency: buy for long or sell for short.
    pub size: f64,
    /// Remaining exit-leg size in the base currency. The chart's sell-line label follows Moonbot
    /// by showing `QuantityRemaining`, not the original entry size.
    pub remaining_size: f64,
    /// Whether SL/TS will act: the order's own per-order `StopSettings` flag, or — only while the
    /// order holds no position — the stop its strategy is about to give it at the fill. Once the
    /// position exists the core owns these stops and the order's own flag is the whole answer, so
    /// one switched off by hand stays off instead of being re-supplied by the strategy. These flags
    /// are toggled by a click.
    pub sl_on: bool,
    pub ts_on: bool,
    pub vstop_on: bool,
    // --- Raw per-order stop parameters from the wire for the order editor. ---
    // Absolute line prices after resolving percentages are below in category C.
    /// Whether SL uses a fixed price from wire field `sl_fixed`; `false` selects global or
    /// percentage mode.
    pub sl_fixed: bool,
    /// Whether TS uses a fixed price from wire field `trailing_fixed`.
    pub ts_fixed: bool,
    /// Whether VStop uses a fixed level from wire field `vstop_fixed`.
    pub vstop_fixed: bool,
    /// Raw VStop level from the wire: a price when `vstop_fixed`, otherwise a percentage.
    pub vstop_level: f64,
    /// VStop trigger volume threshold (`Vol <`).
    pub vstop_vol: f64,
    /// Entry price from `buy_price`.
    pub buy_price: f64,
    /// Sell price from `sell_price`; `0` means unset.
    pub sell_price: f64,
    /// Order creation time in Unix milliseconds, used as the line start; `0` means unknown.
    pub create_time_ms: f64,
    /// Exit-leg creation time in Unix milliseconds, used as the SELL line's start; `0` means the
    /// wire carries none.
    ///
    /// Without it the sell line can only start where this process first saw the order, so a
    /// position opened before the terminal launched drew its exit from the launch moment. The core
    /// anchors the same line to this field: an initial trace point opens the line at the leg's own
    /// `create_time` (moonproto `state/orders/apply_helpers.rs`), and it arrives in the canonical
    /// SELL_PLACEMENT section, so it survives a restart while the server trace does not.
    pub sell_create_time_ms: f64,
    /// Entry-leg close time in Unix milliseconds — the fill — used as the start of the protective
    /// stop lines that exist only after it; `0` means the wire carries none.
    pub entry_fill_time_ms: f64,
    /// Current market price from `p_last`.
    pub price: f32,
    /// Entry-leg fill percentage.
    pub fill_pct: f32,
    /// Order strategy kind name (e.g. `Delta`, `Combo`), or the numeric `strat_id` when the
    /// strategy snapshot is unknown. This is the strategy TYPE, not its user-assigned name; see
    /// [`Self::strat_name`].
    pub strat: String,
    /// Order strategy user-assigned name (`StrategyName`). Empty for a manual order
    /// (`strat_id == 0`) or a strategy that has no name set.
    pub strat_name: String,
    /// Numeric order strategy ID equal to `StrategyRow::id`; `0` means no strategy. Used to count
    /// open orders for a particular strategy in the tree.
    pub strat_id: u64,
    /// Worker status name from `OrderWorkerStatus.name()`, such as None, BuySet, BuyDone, or
    /// SellSet. This is the authoritative entry/exit lifecycle phase used to classify BUY, SELL,
    /// Short-S, and Short-B because a short leg's `fill_pct` does not represent entry execution.
    pub status: String,
    /// Order `uid`, or task ID, which increases with creation so larger values are newer. Used for
    /// creation-order sorting in either newest-first or oldest-first order.
    pub uid: u64,
    /// Whether this is an emulated rather than live order, used for filtering and the `(E)` marker.
    pub emulator: bool,
    /// Whether the core considers the order terminal through `job_is_done`: filled or cancelled
    /// and awaiting deferred removal. This is the authoritative closure flag, equivalent to
    /// Moonbot's `o.IsClosed`; the store marks the line closed immediately while the order is still
    /// present instead of waiting for disappearance plus a grace period.
    pub job_is_done: bool,

    // --- Chart line prices, category C: horizontal price levels. ---
    // `feed/live/convert/orders.rs::build_order_row` derives these from StopSettings, buy_price, and market
    // liquidation data, resolving percentages into absolute prices there. Rendering receives final
    // prices and only maps them to pixels through a shader uniform. `None` means the line is inactive.
    /// Whether the order is still pending, which renders the entry line as dashed.
    pub pending: bool,
    /// Whether the entry leg is filled and the position open, gating stop, trailing, and
    /// liquidation lines.
    pub filled: bool,
    /// Stop-loss absolute price.
    pub stop_loss: Option<f64>,
    /// Trailing-stop absolute price, estimated from the entry price in percentage mode.
    pub trailing: Option<f64>,
    /// Take-profit absolute price.
    pub take_profit: Option<f64>,
    /// VStop level as an absolute price.
    pub vstop: Option<f64>,
    /// Pending-order condition price from `BuyCondPrice`.
    pub pending_cond: Option<f64>,
    /// Position liquidation price from the market for the relevant side.
    pub liq: Option<f64>,
    /// Local or server-provided PanicSell flag.
    pub panic_sell: bool,
    /// Moon-shot corridor active marker.
    pub is_moon_shot: bool,
    /// Corridor price band from server, 0/NaN means absent.
    pub corridor_price_down: f32,
    pub corridor_price_up: f32,
    /// Server-provided buy-line trace when the core has built one.
    pub buy_trace: Option<OrderTrace>,
    /// Server-provided sell-line trace when the core has built one.
    pub sell_trace: Option<OrderTrace>,
}
