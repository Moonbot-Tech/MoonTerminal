//! Wallet, balance and license snapshot types.

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
