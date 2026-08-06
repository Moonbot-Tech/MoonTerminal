//! Domain types sent from the backend to the UI. They are independent of moonproto so the UI and
//! rendering layer do not need to know about the transport.

use std::net::IpAddr;

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

/// Retained price-line source kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceLineKind {
    Last,
    Mark,
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
    /// Whether SL/TS are enabled on the order's own per-order `StopSettings`. These flags are
    /// toggled by a click.
    pub sl_on: bool,
    pub ts_on: bool,
    /// Whether SL, TS, and VStop are enabled in the order's strategy, resolved by `strat_id` from
    /// the Delphi fields `UseStopLoss`, `UseTrailing`, and `UseBV_SV_Stop`. A field absent from the
    /// snapshot uses its schema default because the serializer omits default values. These are
    /// fallback column indicators: when a per-order flag is off but the strategy enables it, the
    /// UI shows the stop as inherited because the strategy still protects the order.
    pub sl_strat: bool,
    pub ts_strat: bool,
    pub vstop_strat: bool,
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
    // `feed/live/convert.rs::build_order_row` derives these from StopSettings, buy_price, and market
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

/// One core detect for the toolbar and history, decoupled from moonproto.
#[derive(Debug, Clone)]
pub struct DetectRow {
    /// Monotonic per-core sequence number used as the ingestion cursor for the detects feed.
    pub seq: u64,
    /// Market or coin.
    pub market: String,
    /// Receipt time in Unix milliseconds.
    pub time_ms: f64,
    /// Whether the source strategy enables a sound alert through `SoundAlert=Yes`. The UI drops a
    /// detect when both this and `is_alert` are false; drawn-object alerts may therefore render
    /// without `sound_alert`. Detects auto-added to charts are excluded from the regular buttons.
    pub sound_alert: bool,
    /// Number of seconds to keep the button, from strategy `KeepAlert`, defaulting to 60.
    pub keep_alert_secs: u32,
    /// Strategy `AddToChart` tab number, such as 1, 2, or 3, to which the coin chart is added
    /// automatically. `0` only disables automatic addition; a regular button still requires
    /// `sound_alert` or `is_alert`.
    pub add_to_chart: u32,
    /// Strategy `KeepInChart` duration in seconds before closing the automatically added coin
    /// chart while retaining the tab, defaulting to 60.
    pub keep_in_chart_secs: u32,
    /// Strategy sound name as a WAV stem to play when the detect arrives; `None` is silent.
    pub sound_name: Option<String>,
    /// Whether this detect is a drawn-object alert trigger, `DETECT_KIND_ALERT`. These are shown and
    /// played even without a strategy, using the default sound when the strategy has none.
    pub is_alert: bool,
    /// Source strategy-kind ordinal from `StrategyKind`; see `strat_kind_name`. `0` means Unknown
    /// or a missing strategy snapshot, while an alert trigger without a strategy maps to 22
    /// (Alerts). Used for the detect-kind badge in the feed.
    pub kind: u8,
    /// Source strategy direction from `is_short`, where `true` means short. Used to outline the
    /// feed badge by direction. A missing strategy snapshot defaults to `false`, or long.
    pub is_short: bool,
}

/// One core server-log line from `Event::ServerLog`, decoupled from moonproto.
#[derive(Debug, Clone)]
pub struct CoreLogLine {
    /// Line time in Unix milliseconds from `ServerLogEvent::unix_millis`.
    pub time_ms: i64,
    /// Terminal-local receipt time recorded by the feed thread in Unix milliseconds.
    pub recv_ms: i64,
    pub msg: String,
}

/// One chart alert accepted by the core through `Event::ChartAlert::Upserted`.
///
/// In Moonbot an alert is a drawn chart object, such as a line, channel, or Fibonacci tool, with
/// its Alert option enabled. `blob` is its opaque `TChartObject.Save()` binary. The terminal keeps
/// it unchanged for subsequent `upsert` calls that enable or disable the alert and for format
/// reverse engineering in phase zero. This type is decoupled from moonproto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartAlertRow {
    pub market: String,
    pub obj_uid: u64,
    pub blob: Vec<u8>,
}

/// Change to the core's authoritative chart-alert set.
///
/// The server owns the set, and the terminal requests a full snapshot after reconnecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChartAlertUpdate {
    Upserted(ChartAlertRow),
    Deleted { market: String, obj_uid: u64 },
}

/// Engine action accepted or rejected by the core through `Event::EngineAction`.
///
/// This is decoupled from moonproto so the UI formats the toast text itself.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineActionKind {
    CancelAllOrders,
    SetLeverage {
        market: String,
        leverage: i32,
    },
    SetHedgeMode {
        on: bool,
    },
    ChangePositionType {
        market: String,
    },
    ConvertDust,
    ConfirmRiskLimit {
        market: String,
    },
    SetMaMode {
        on: bool,
    },
    TransferAsset {
        asset: String,
        qty: f64,
        from: WalletKind,
        to: WalletKind,
    },
    ReloadOrderBook,
}

/// Result of an asynchronous Engine action on the core.
///
/// A result also arrives on disconnect with `success=false` and a disconnected error, preserving
/// the toast that reports the action was not delivered.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineActionResult {
    pub kind: EngineActionKind,
    pub success: bool,
    /// Exchange or core error code; `0` means no error.
    pub error_code: i32,
    /// Error text, empty on success.
    pub error_msg: String,
}

/// One core strategy for the Strategies window, decoupled from moonproto.
#[derive(Debug, Clone)]
pub struct StrategyRow {
    pub id: u64,
    /// Strategy name from `StrategyName`, or a fallback.
    pub name: String,
    /// Human-readable strategy type or kind.
    pub kind: String,
    /// Kind ordinal used to associate the strategy with its schema sections and fields.
    pub kind_ordinal: u8,
    /// Folder placement preserved verbatim from `StrategySnapshot::path` for the UI to parse.
    pub folder_path: String,
    /// Whether the strategy checkbox is checked; this selection does not prove it is running.
    pub checked: bool,
    pub is_short: bool,
    /// Strategy field values as name-to-formatted-string pairs that populate editable controls.
    pub fields: Vec<(String, String)>,
}

/// Schema-field widget kind from moonproto `StrategyFieldUiKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaFieldUi {
    Edit,
    Checkbox,
    Combo,
    Color,
}

/// Description of one strategy-schema field, decoupled from moonproto.
#[derive(Debug, Clone)]
pub struct SchemaField {
    pub name: String,
    /// Type name from the core schema, such as `Bool`, `Int32`, `Double`, or `String`. The UI uses
    /// it to avoid rendering numeric fields as multiline memos; see `is_memo_field`.
    pub type_name: String,
    pub ui: SchemaFieldUi,
    /// Static value list used to populate the field's Combo editor.
    #[allow(dead_code)]
    pub picklist: Vec<String>,
    /// Formatted default value when the schema provides one.
    pub default: Option<String>,
}

/// Field section for one strategy kind, such as main or filters.
#[derive(Debug, Clone)]
pub struct SchemaSection {
    pub title: String,
    pub fields: Vec<SchemaField>,
}

/// Schema for one strategy kind and its sections.
#[derive(Debug, Clone)]
pub struct SchemaKind {
    pub ordinal: u8,
    /// Kind name from the core schema, authoritative over hard-coded `strat_kind_name` and consumed
    /// by strategy creation and kind/filter UI.
    #[allow(dead_code)]
    pub name: String,
    pub sections: Vec<SchemaSection>,
}

/// Complete schema for all core strategy kinds, sent when the schema revision changes.
#[derive(Debug, Clone, Default)]
pub struct StrategySchemaModel {
    pub kinds: Vec<SchemaKind>,
}

/// Exchange wallet used by the asset-transfer tree.
///
/// This mirrors moonproto `ExchangeKind` with Spot=0, Futures=1, and Quarterly=2, but is decoupled
/// so the UI and store do not depend on moonproto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WalletKind {
    Spot,
    Futures,
    Quarterly,
}

impl WalletKind {
    /// All wallets in display order as tree branches.
    pub const ALL: [WalletKind; 3] = [WalletKind::Spot, WalletKind::Futures, WalletKind::Quarterly];

    /// Return the human-readable branch label.
    pub fn label(self) -> &'static str {
        match self {
            WalletKind::Spot => "Спот",
            WalletKind::Futures => "Фьючерсы",
            WalletKind::Quarterly => "Квартальные",
        }
    }

    /// Return the stable persistence code used for expanded branches and selection.
    pub fn to_u8(self) -> u8 {
        match self {
            WalletKind::Spot => 0,
            WalletKind::Futures => 1,
            WalletKind::Quarterly => 2,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => WalletKind::Futures,
            2 => WalletKind::Quarterly,
            _ => WalletKind::Spot,
        }
    }
}

/// One core asset or position for a market in the Assets window, decoupled from moonproto.
///
/// The feed normalizes values to USDT, while the UI filters dust and the store retains all rows.
#[derive(Debug, Clone)]
pub struct AssetRow {
    /// Core market name, such as `ADAUSDT`.
    pub market: String,
    /// Base coin or asset, such as `ADA`.
    pub coin: String,
    /// Market quote currency, such as `USDT` or `BTC`.
    pub quote: String,
    /// Market `ListedType`: 0 unknown, 1 spot, 2 futures, or 3 both.
    pub listed: u8,
    /// Asset balance from `asset_balance`, in the base coin.
    pub qty: f64,
    /// Full asset balance from `asset_balance_full`, in the base coin.
    pub qty_full: f64,
    /// Current market price from `p_last`, denominated in the quote currency.
    pub price: f64,
    /// Current held coin-balance value in USDT as
    /// `max(abs(qty_full), abs(qty)) * price * quote/USDT rate`, calculated by the feed so locked
    /// holdings remain valued. `0` means the rate is unknown.
    pub value_usdt: f64,
    /// Minimum market-lot value in USDT from `MarketPrice::min_lot_size * quote/USDT rate`.
    /// Smaller balances are unsellable dust hidden by the UI; `0` means unknown.
    pub min_lot_usd: f64,
    /// Whether this row's coin is the account quote currency, such as USDT for a USDT bot. Its
    /// balance is cash rather than a purchased coin, so the UI hides the row from the assets table.
    pub is_quote_asset: bool,
    /// Futures mark price; `0` means unavailable.
    pub mark_price: f64,
    /// Position size from `pos_size`.
    pub pos_size: f64,
    /// Position price from `pos_price`.
    pub pos_price: f64,
    /// Position liquidation price from `liq_price`; `0` means unavailable.
    pub liq_price: f64,
    /// Market leverage for this core from `Market.leverage_x`. This per-core account field appears
    /// in the toolbar because Lev depends on both the core and coin; `0` means unknown.
    pub leverage: i32,
    /// Live unrealized position PnL in USDT as `(current price - entry price) * size`, calculated
    /// per long and short hedge leg when present, otherwise net by `pos_dir`. The feed rebuilds it
    /// only after domain events, rate-capped at once per second while an Assets view rendered
    /// recently and once per five seconds otherwise. This is not the server's period-accumulated
    /// `total_profit_*`, which remains frozen between balance pushes. Without a position or entry
    /// price, as for a spot balance, it falls back to server total profit times the conversion rate.
    pub pnl_usdt: f64,
    /// Whether [`Self::pnl_usdt`] really is the LIVE unrealized figure: derived from a mark/last
    /// price and an entry price, and converted with a known quote rate.
    ///
    /// `false` covers the two cases a consumer cannot tell apart by looking at the number: the
    /// accumulated-server-profit fallback above (a spot balance, or a position whose mark or entry
    /// price is missing), and an unknown quote rate, which silently turns any PnL into a confident
    /// `0.00`. A display that prints unrealized PnL must show nothing rather than either of those.
    pub pnl_live: bool,
}

/// Core account totals from `GlobalBalance`, decoupled from moonproto.
#[derive(Debug, Clone, Default)]
pub struct GlobalBalanceRow {
    /// BTC-equivalent available, locked, and full balances, including unrealized PnL in the latter.
    pub btc_total: f64,
    pub btc_locked: f64,
    pub btc_full: f64,
    /// `special_coin_balance`, such as USDT for futures or BUSD/USDC in MA mode.
    pub special_coin: f64,
    /// Total core PnL in the base currency. The server's `total_pnl` is Moonbot
    /// `RecalcTotalPnl`: the sum of `total_profit` only for base-currency markets marked
    /// `is_btc_market`. This authoritative core PnL differs from summing `profit_*` across every
    /// table row, where quote currencies are mixed.
    pub total_pnl: f64,
    /// Free account balance in USDT as `btc_balance_total * base-currency/USDT rate`. The core
    /// accounts for the base currency: a USDT bot's `btc_balance_*` is already in USDT and uses a
    /// rate of 1, while a BTC bot multiplies by BTCUSDT. `0` means the rate is unknown.
    pub free_usdt: f64,
    /// Total account balance in USDT as `btc_balance_full * rate`, including unrealized PnL.
    pub total_usdt: f64,
    /// Server-provided core PnL from `total_pnl`, converted to USDT with the same base rate as
    /// `free_usdt` and `total_usdt`. The header PnL uses this value instead of a local sum.
    pub pnl_usdt: f64,
    /// Whether `free_usdt`/`total_usdt` carry a complete, finite USD valuation. Global equity
    /// requires a known base-currency rate; coin-wallet equity requires a valid price for every
    /// held coin. Missing pricing can otherwise yield a finite zero or a misleading partial sum.
    ///
    /// Scope is those two fields ONLY. `pnl_usdt` is always `total_pnl × rate`, so on a
    /// coin-margined account where equity comes from priced coin wallets while `rate` is zero,
    /// this can be `true` even though `pnl_usdt` is not valued. Consumers of PnL must establish
    /// their own pricing validity rather than use this flag.
    pub usd_rate_known: bool,
}

/// Core assets snapshot for the Assets window, decoupled from moonproto.
#[derive(Debug, Clone, Default)]
pub struct AssetsSnapshot {
    pub rows: Vec<AssetRow>,
    pub global: GlobalBalanceRow,
    /// Whether the core trades futures, including CoinM, according to the FUTURES bit in
    /// BaseCheck `exchange_type_mask`. For futures cores, the assets table shows only open
    /// positions because balances there are quote or margin currencies rather than purchased
    /// assets.
    pub futures_account: bool,
    /// Account base or quote currency from BaseCheck `base_currency_name`, such as USDT, USDC, or
    /// BTC. The UI uses it to hide the quote currency from spot assets, such as USDC on a core
    /// trading BTCUSDC, because that balance is cash rather than a purchased coin.
    pub base_currency: String,
    /// Names of every market in the core's catalog. The UI gates the Market Sell button on this
    /// set because a coin can be sold only when `<coin><quote>` exists. For example, if a USDC
    /// account has no `USDTUSDC` market, the button for USDT is hidden.
    pub markets: std::collections::HashSet<String>,
    /// Per-core leverage from `leverage_x` for every tracked market, not only markets with a
    /// position. The toolbar reads it for the main chart's coin. Markets without account data are
    /// omitted because the core resets their `leverage_x` to 1, leaving their actual leverage
    /// unknown and displayed as a dash.
    pub leverage: std::collections::HashMap<String, i32>,
}

/// One transferable wallet asset for the transfer tree, decoupled from moonproto.
#[derive(Debug, Clone)]
pub struct TransferAssetRow {
    /// Currency or coin, such as USDT or BTC.
    pub currency: String,
    /// Amount the exchange makes available for transfer.
    pub amount: f64,
    /// Total amount in the wallet.
    pub total: f64,
    /// Value of `total` in USDT through the feed's full `coin_to_usdt` pricing cascade; `0` means
    /// the rate is unknown.
    pub value_usdt: f64,
}

/// Snapshot of a core's transferable assets across Spot, Futures, and Quarterly wallets.
///
/// This supplies the transfer tree and refreshes on request through `refresh_transfer_assets`.
#[derive(Debug, Clone, Default)]
pub struct TransferAssetsSnapshot {
    pub spot: Vec<TransferAssetRow>,
    pub futures: Vec<TransferAssetRow>,
    pub quarterly: Vec<TransferAssetRow>,
}

impl TransferAssetsSnapshot {
    /// Return the assets for the selected wallet tree branch.
    pub fn wallet(&self, kind: WalletKind) -> &[TransferAssetRow] {
        match kind {
            WalletKind::Spot => &self.spot,
            WalletKind::Futures => &self.futures,
            WalletKind::Quarterly => &self.quarterly,
        }
    }
}

/// License/module/MoonCredits state of one Moonbot core.
/// Decoupled from moonproto so the UI sees only a ready account snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LicenseState {
    pub paid_version: bool,
    pub reg_id: i32,
    pub moon_credits: i32,
    pub moon_credits_hold: i32,
    pub moon_credits_auction: i32,
    pub can_use_watcher: bool,
    /// News-module subscription validity, Unix ms, or `None` when the core reports no subscription.
    /// The News panel shows it as the feed's "subscription until" status.
    pub news_valid_until: Option<i64>,
    /// Whether the news-module trial has been consumed.
    pub news_trial_used: bool,
}

/// Core client-settings snapshot from moonproto `ClientSettings`, flattened for toolbar TP, SL,
/// and sell presets. This is decoupled from moonproto: raw fields such as `s_price` and `sb_num`
/// are `pub(crate)` in production and are read only through the command's public helpers.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientSettings {
    /// Effective take-profit percentage from `effective_take_profit_percent`. Under
    /// `fixed_sell_mode` it equals the selected S-slot percentage and must not be shown on the TP
    /// button; see `take_profit_main_pct`.
    pub take_profit_pct: f64,
    /// TP button's own take-profit value from `x_sell` or scalp, independent of `fixed_sell_mode`.
    /// The button always shows this value so selecting an S slot does not replace its displayed TP.
    pub take_profit_main_pct: f64,
    /// Extended TP range from the `x_tmode` or `s9` flag: off means 0..100%, on means 100..900%,
    /// stored on the wire as `x_sell * 10`. This determines the slider range and popup checkbox.
    pub take_profit_extended: bool,
    /// Exact main-TP encoding, including scalp mode where `x_sell == 0`.
    pub take_profit_mode: crate::config::TakeProfitMode,
    /// Whether fixed-sell mode is enabled.
    pub fixed_sell_mode: bool,
    /// Stop-loss / price-drop level, % (`price_drop_level`).
    pub stop_loss_pct: f32,
    /// Trailing-stop percentage from `trailing_drop`.
    pub trailing_drop_pct: f32,
    /// Whether global take profit is enabled through `use_g_take_profit`, with its percentage from
    /// `g_take_profit`.
    pub use_global_take_profit: bool,
    pub global_take_profit_pct: f64,
    /// Panic-on-price-drop state from `panic_if_price_drop`.
    pub panic_if_price_drop: bool,
    /// Emulator mode from `emu_mode`.
    pub emu_mode: bool,
    pub buy_iceberg: bool,
    pub sell_iceberg: bool,
    pub sign_orders: bool,
    pub use_stop_market: bool,
    /// Default VStop BID-volume drop level as an integer percentage from `vol_drop_level`.
    pub vol_drop_level: i32,
    /// Coin blacklist enabled state from `use_coins_black_list` and its text from
    /// `coins_black_list_text`.
    pub use_blacklist: bool,
    pub blacklist_text: String,
    /// Six fixed-sell presets as visible percentages for buttons S1-S6.
    pub fixed_sell_pcts: [f64; 6],
    /// Selected fixed-sell slot in 1..=6 from `selected_fixed_sell_slot`.
    pub fixed_sell_slot: usize,
    /// Whether the manual strategy is enabled through `use_manual_strategy`. Manual orders then
    /// follow that strategy: the core places sells and stops from its fields, while toolbar TP, S,
    /// and SL settings do not apply to new orders.
    pub use_manual_strategy: bool,
    /// Selected manual-strategy ID from `manual_strategy_id`; `0` means none is selected.
    pub manual_strategy_id: u64,
}

impl ClientSettings {
    /// Project visible core values into the group-local manual-exit contract.
    pub fn group_exit_settings(&self) -> crate::config::GroupExitSettings {
        crate::config::GroupExitSettings {
            take_profit_pct: self.take_profit_main_pct,
            take_profit_mode: self.take_profit_mode,
            fixed_sell_pcts: self.fixed_sell_pcts,
            fixed_sell_slot: self.fixed_sell_mode.then_some(self.fixed_sell_slot),
            stop_loss_pct: self.stop_loss_pct,
            stop_loss_enabled: self.panic_if_price_drop,
            use_stop_market: self.use_stop_market,
        }
    }
}

/// Core leverage-management settings from moonproto `LevManage`, kept as a separate snapshot as in
/// Moonbot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LevManageState {
    pub auto_max_order: bool,
    pub auto_lev_up: bool,
    pub auto_isolated: bool,
    pub auto_cross: bool,
    pub auto_fix_lev: bool,
    pub fix_lev: i32,
    pub tlg_report: bool,
    pub lev_control: String,
}

/// Core runtime state from moonproto `RuntimeState`: whether the market runtime is running and
/// automatic detection is active. Passive mode is specifically `is_started=true` with
/// `auto_detect_active=false`; a false value alone does not identify passive mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeState {
    pub is_started: bool,
    pub auto_detect_active: bool,
}

/// Targeted `ClientSettings` edit from the toolbar.
///
/// The feed applies it through public helpers to the retained moonproto snapshot at
/// `client.snapshot().settings().client_settings`, preserving append-only tails and AutoStart
/// blobs invisible to the UI. It then sends the full snapshot back to the core through
/// `settings().send`.
#[derive(Debug, Clone, Copy)]
pub enum ClientSettingsEdit {
    /// Main take-profit percentage and extended-range mode from `x_tmode` or `s9`. With
    /// `extended`, writes `x_tmode=true` and `x_sell=round(pct/10)` for 100..900%; otherwise writes
    /// `x_tmode=false` and `x_sell=round(pct)` for 1..100%. Clears fixed-sell and scalp modes.
    TakeProfit { pct: f64, extended: bool },
    /// Stop-loss or price-drop level as a signed core percentage in -20..+1.
    StopLossPct(f32),
    /// Scalp take profit for the fine TP slider, stored as a sub-percent value through
    /// `x_sell_scalp` with `x_sell=0`. The core's actual step is 1/50, or 0.02%. Clears fixed-sell.
    ScalpTakeProfit(f64),
    /// Select a fixed-sell slot in 1..=6 from buttons S1-S6, enabling `fixed_sell_mode`.
    SelectFixedSellSlot(usize),
    /// Return control to the main TP by setting `fixed_sell_mode=false` without changing the TP
    /// value in `x_sell` or scalp. Triggered by the TP button or a second click on the active S slot.
    EngageMainTakeProfit,
    /// Fixed-sell preset value as a slot in 1..=6 and visible percentage, edited by the wheel or
    /// inline editing on an S button.
    SetFixedSellPct { slot: usize, pct: f64 },
    /// Use a stop-market rather than stop-limit order through `use_stop_market`.
    UseStopMarket(bool),
    /// Panic on price drop through `panic_if_price_drop`.
    PanicIfPriceDrop(bool),
    /// Global take-profit enabled state and percentage through
    /// `use_g_take_profit`/`g_take_profit`.
    GlobalTakeProfit { on: bool, pct: f64 },
    /// Trailing-stop percentage through `trailing_drop`.
    TrailingDrop(f32),
    /// Buy-order iceberg mode through `buy_iceberg`.
    BuyIceberg(bool),
    /// Sell-order iceberg mode through `sell_iceberg`.
    SellIceberg(bool),
    /// Order signing through `sign_orders`.
    SignOrders(bool),
    /// Core emulator mode through `emu_mode`.
    EmuMode(bool),
    /// Default VStop BID-volume drop level as an integer percentage through `vol_drop_level`.
    VolDropLevel(i32),
    /// Manual-strategy enabled state and ID through
    /// `use_manual_strategy`/`manual_strategy_id`. Disabling preserves the ID so toggling it again
    /// restores the same strategy.
    ManualStrategy { on: bool, id: u64 },
}

/// Profit counter to reset through moonproto `ResetProfitKind`, selected by the Session or
/// All-Time buttons in the core-settings popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetProfitKind {
    /// Current trading session.
    Session,
    /// All accumulated time.
    All,
}

/// Targeted leverage-management edit for moonproto `LevManage`, applied to the retained snapshot
/// and sent through `settings().manage_leverage`.
#[derive(Debug, Clone, Copy)]
pub enum LevManageEdit {
    /// Fix the target leverage by setting `auto_fix_lev=true` and `fix_lev=n`.
    FixLev(i32),
    /// Automatic maximum order size through `auto_max_order`.
    AutoMaxOrder(bool),
    /// Automatic leverage increase through `auto_lev_up`.
    AutoLevUp(bool),
    /// Isolated margin through `auto_isolated`, mutually exclusive with cross margin.
    AutoIsolated(bool),
    /// Cross margin through `auto_cross`, mutually exclusive with isolated margin.
    AutoCross(bool),
    /// Telegram reporting through `tlg_report`.
    TlgReport(bool),
}

/// Connection status for a core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnStatus {
    Connecting,
    /// Intermediate connection or initialization stage, carrying badge text.
    Stage(String),
    Ready,
    Failed(String),
    Disconnected,
}

/// Market-data domains that can wake a visible chart.
///
/// The payload is intentionally small: data rows stay in MoonProto/MarketStore,
/// while the terminal keeps causal per-market revisions and pulls only visible
/// chart targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketDirtyFlags(u8);

impl MarketDirtyFlags {
    pub const HISTORY: Self = Self(1 << 0);
    pub const ORDERBOOK: Self = Self(1 << 1);
    pub const MARKET_META: Self = Self(1 << 2);
    pub const ALL: Self = Self(Self::HISTORY.0 | Self::ORDERBOOK.0 | Self::MARKET_META.0);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOr for MarketDirtyFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketDirty {
    pub market: String,
    pub flags: MarketDirtyFlags,
}

impl MarketDirty {
    pub fn new(market: impl Into<String>, flags: MarketDirtyFlags) -> Self {
        Self {
            market: market.into(),
            flags,
        }
    }
}

/// Message from a backend to the UI.
///
/// Account messages such as Status, Orders, Detects, and Strategies carry ready UI state for one
/// core. Market ticks, order books, and price lines do not travel through this channel: the feed
/// thread publishes them to MoonProto/MarketStore and sends only a lightweight
/// [`MarketDataChanged`] wake-up for consumer-side pulling.
#[derive(Debug, Clone)]
pub enum FeedMsg {
    Status(ConnStatus),
    /// Network endpoint selected from the exported MoonBot key before the connection attempt.
    ///
    /// The live feed publishes this domain value after parsing the key so UI consumers never need
    /// access to plaintext credentials. Several cores may use different ports on one host.
    Endpoint(CoreEndpoint),
    /// Core exchange from `server_info` after BaseCheck, sent once.
    Identity(ExchangeId),
    /// Core account base currency such as USDT or BTC from `server_info`, sent once alongside
    /// `Identity`. The UI uses it to convert a group-local USD-equivalent size before placement.
    CoreBase {
        base: String,
    },
    /// Notify that the market read model changed. This lightweight wake-up makes
    /// `SessionManager` mark particular markets dirty while visible charts pull the snapshots they
    /// need. The ticks and order book themselves do not travel through the UI channel.
    MarketDataChanged(Vec<MarketDirty>),
    /// Open core orders across all markets.
    Orders(Vec<OrderRow>),
    /// Fast order snapshot only for the chart/order-line store. The Orders table remains gated by
    /// `Orders`, while the chart retains a brief terminal status between `OrderEvent::Updated` and
    /// deferred removal.
    OrderLines(Vec<OrderRow>),
    /// Batch of new detects accumulated during one event-drain tick.
    Detects(Vec<DetectRow>),
    /// Batch of new core server-log lines accumulated during one event-drain tick.
    ServerLog(Vec<CoreLogLine>),
    /// Core strategy snapshot sent when its signature changes.
    Strategies(Vec<StrategyRow>),
    /// The core acknowledged a checkbox delta this terminal sent (`TStratCheckedEcho` /
    /// `TStratCheckedSync`).
    ///
    /// This is the ONLY evidence that a checkbox change was committed by the core.
    /// `Strategies` cannot serve: the protocol library flips its own snapshot the moment
    /// `set_checked` is called — before a single byte is sent — so a `checked` flag read back from
    /// `Strategies` only proves the terminal asked, never that the core agreed.
    StrategiesAck,
    /// Core strategy schema with sections and fields by kind, sent when its revision changes.
    StrategySchema(StrategySchemaModel),
    /// Core asset and position snapshot for the Assets window, sent after domain events at most
    /// once per second while the window is active and once every five seconds otherwise.
    Assets(AssetsSnapshot),
    /// Snapshot of transferable core assets by wallet for the transfer tree, sent when its revision
    /// changes after a `RefreshTransferAssets` request.
    TransferAssets(TransferAssetsSnapshot),
    /// Core License, Free-PRO, and MoonCredits state.
    License(LicenseState),
    /// Core client-settings snapshot for TP, SL, sell, iceberg, and related settings, sent on
    /// `ClientSettingsUpdated`.
    ClientSettings(ClientSettings),
    /// Core leverage-management snapshot sent on `LevManageUpdated`.
    LevManage(LevManageState),
    /// Core runtime and passive-mode state sent on `RuntimeStateUpdated`.
    RuntimeState(RuntimeState),
    /// Core account hedge mode for dual-side positions, sent on `HedgeModeUpdated`.
    HedgeMode(bool),
    /// Exchange API-key expiration for this core, sent on a successful
    /// `ApiExpirationUpdated`. A failed check publishes nothing, so the store keeps the last
    /// known answer instead of falling back to "unknown" on one dropped request.
    ApiExpiry(ApiKeyExpiry),
    /// Batch of Engine-action results such as leverage, hedge, cancel-all, or transfer accumulated
    /// during one event-drain tick. The UI displays them as toasts in the active window.
    EngineActions(Vec<EngineActionResult>),
    /// Batch of core chart-alert changes from one drain tick, gated by `feed.alerts`.
    ChartAlerts(Vec<ChartAlertUpdate>),
    /// Core resource telemetry from protocol-v4 `Event::KernelHealth`.
    /// Emitted for every health event; the store gates the Core Status panel with
    /// `sys_rev` only when metric values change.
    SysStatus(CoreSysStatus),
    /// Core news snapshot: logical news items plus the tags catalog, rebuilt from the retained
    /// moonproto `NewsState` when any `Event::News` arrives. Moonproto-free — the reduction lives in
    /// `feed::news` and the projection in `feed::live::convert`. The store gates the News panel with
    /// `news_rev` only when the reduced snapshot changes.
    News(super::news::NewsSnapshot),
}

/// Exchange API-key expiration reported by one core.
///
/// A CURRENT core sends a timezone-safe remaining duration, which MoonProto turns into an absolute
/// date against the CLIENT's clock — so the core's own time zone cannot shift it. A LEGACY core
/// (the 8-byte payload) sends only a server-local timestamp, which nothing normalizes; that answer
/// carries no [`Self::at_unix`] and its day count can be off by the terminal↔core zone difference.
///
/// `known == false` means the check SUCCEEDED and the exchange reports no expiration for this key.
/// That is not "zero days left": conflating the two would light a warning on every perpetual key.
/// A failed or never-answered check is the absence of this value altogether (`None` in the store).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiKeyExpiry {
    /// Whether the response carried a real expiration at all.
    pub known: bool,
    /// Whole days left AT THE MOMENT OF THE CHECK, as counted by the core. `None` while `known` is
    /// false. Read it through [`Self::days_left_at`], which ages it — this raw field goes stale
    /// between polls and while the core is down.
    pub days_left: Option<i32>,
    /// Absolute expiration as whole Unix seconds, on the terminal's clock. `None` for a legacy
    /// answer that carries no normalized duration, and while `known` is false.
    pub at_unix: Option<i64>,
    /// Unix ms the terminal received this answer, so a stored day count can be aged.
    pub checked_ms: i64,
}

impl ApiKeyExpiry {
    /// Days left as of `now_ms`, negative once the key has expired.
    ///
    /// A stored answer must never be read as-is: it is a snapshot, and the terminal keeps showing
    /// it while a core is down — for weeks, if the core stays down. Aged here so a key that runs
    /// out during an outage still crosses the warning threshold instead of standing frozen at the
    /// count it had when the core was last reachable.
    ///
    /// The absolute date is preferred because it does not accumulate error; the legacy path ages
    /// the core's own count by whole elapsed days instead.
    ///
    /// Args:
    ///     now_ms: Current Unix milliseconds.
    ///
    /// Returns:
    ///     Whole days remaining, or `None` for a key with no expiration or no usable count.
    pub fn days_left_at(&self, now_ms: i64) -> Option<i32> {
        if !self.known {
            return None;
        }
        if let Some(at_unix) = self.at_unix {
            let seconds_left = at_unix.saturating_sub(now_ms.div_euclid(1_000));
            // Floor division, so a key with 23 hours left reads 0 days and only a key already past
            // its date goes negative.
            return Some(clamp_to_i32(seconds_left.div_euclid(86_400)));
        }
        let days = self.days_left?;
        let aged = now_ms.saturating_sub(self.checked_ms).max(0) / 86_400_000;
        Some(days.saturating_sub(clamp_to_i32(aged)))
    }

    /// Whether two answers say the same thing.
    ///
    /// The absolute date is compared at DAY granularity on purpose: MoonProto rebuilds it as
    /// `client_now + remaining`, so an unchanged key answers with a date a few seconds apart every
    /// poll, and an exact comparison would report a change every six hours.
    pub fn answer_eq(&self, other: &Self) -> bool {
        self.known == other.known
            && self.days_left == other.days_left
            && self.at_unix.map(|at| at.div_euclid(86_400))
                == other.at_unix.map(|at| at.div_euclid(86_400))
    }
}

/// Saturate an `i64` day count into `i32`, so a nonsense date cannot wrap into a plausible one.
fn clamp_to_i32(days: i64) -> i32 {
    days.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Network endpoint of one MoonBot core, decoded from its exported key by the live feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoreEndpoint {
    /// Host IP selected for the MoonProto connection.
    pub address: IpAddr,
    /// UDP port selected for this core on the host.
    pub port: u16,
}

/// Latest resource telemetry for one core, from protocol v4 `Event::KernelHealth`.
///
/// Every `Option` is `None` until that field first arrives. CPU refreshes on
/// every Ping; memory and the logical-CPU count are a lower-rate tail (`None`
/// until the first memory-bearing Ping, then the retained snapshot keeps the last
/// value). Fields carry a SCOPE distinction (process vs whole machine), NOT a time
/// one: `system_cpu_percent` is machine-wide CPU, never an average of the process
/// CPU. Moonproto-free — the `KernelHealth` projection lives in `feed::live::convert`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CoreSysStatus {
    /// MoonBot process CPU, % of the whole machine.
    pub process_cpu_percent: Option<u8>,
    /// Whole-machine CPU, %. The process-vs-machine SCOPE counterpart of
    /// `process_cpu_percent`, not a time average.
    pub system_cpu_percent: Option<u8>,
    /// MoonBot process memory, MB (decimal).
    pub used_memory_mb: Option<u16>,
    /// Free physical memory on the machine, MB (decimal).
    pub free_physical_memory_mb: Option<u16>,
    /// Logical CPU count of the machine.
    pub logical_cpu_count: Option<u8>,
    /// Smoothed client↔core UDP round-trip time, ms — the transport ping between this terminal and
    /// the core (NOT the core→exchange path). `None` until the core has measured one Ping response.
    /// This is moonproto's `core_round_trip_ms`; MoonBot's displayed one-way ping is half of it.
    pub round_trip_ms: Option<u32>,
    /// Smoothed core→exchange order-API latency, ms — how long the core's real order requests take
    /// to the exchange (NOT a standalone REST ping, and NOT the transport ping above). `None` until
    /// the core has an order sample. This is moonproto's `order_api_latency_ms`.
    pub order_api_latency_ms: Option<u16>,
    /// Receipt time of the last `KernelHealth`, unix ms (`0` — none yet).
    pub updated_ms: i64,
}

impl CoreSysStatus {
    /// Whether the telemetry metrics are equal, ignoring the `updated_ms` receipt stamp.
    /// The store bumps `sys_rev` only on a metric change; `updated_ms` advancing on
    /// every Ping must not churn the panel's repaint signature (the panel keeps
    /// "Updated" live via its own 1 Hz tick).
    pub fn metrics_eq(&self, other: &Self) -> bool {
        self.process_cpu_percent == other.process_cpu_percent
            && self.system_cpu_percent == other.system_cpu_percent
            && self.used_memory_mb == other.used_memory_mb
            && self.free_physical_memory_mb == other.free_physical_memory_mb
            && self.logical_cpu_count == other.logical_cpu_count
            && self.round_trip_ms == other.round_trip_ms
            && self.order_api_latency_ms == other.order_api_latency_ms
    }
}

#[cfg(test)]
mod tests;
