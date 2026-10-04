//! JSON shapes for Mini App read routes and owner commands.
//!
//! This crate does not localize. The Backend fills these values and the page renders them.

/// Named report window. Serialized as snake_case (`last_month`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportPeriodDto {
    Today,
    Yesterday,
    Month,
    LastMonth,
}

/// Money total. `usdt: None` means unvalued, never serialize 0 for it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct MoneyDto {
    pub usdt: Option<f64>,
    pub text: Option<String>,
    pub orders: u64,
    pub unknown_orders: u64,
}

/// One named slice of a report total.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct RowDto {
    pub key: String,
    pub name: String,
    /// Exchange section caption of a per-core row; absent on a per-exchange row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub money: MoneyDto,
}

/// One day in a report series. `start` is an ISO date `YYYY-MM-DD`.
///
/// `trades` is the day's closed-trade count, the same figure the chat report's day table shows.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct DayDto {
    pub start: String,
    pub usdt: Option<f64>,
    pub text: Option<String>,
    pub trades: u64,
}

/// Authenticated report for one [`ReportPeriodDto`].
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ReportDto {
    pub from: String,
    pub to: String,
    /// Window start as the chat report stamps it, `DD.MM.YYYY HH:MM` in the report zone.
    pub from_text: String,
    /// Window end, same format as `from_text`.
    pub to_text: String,
    pub total: MoneyDto,
    pub by_exchange: Vec<RowDto>,
    pub by_core: Vec<RowDto>,
    pub days: Vec<DayDto>,
}

/// Connection phase of one core. Serialized as snake_case.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnDto {
    Ready,
    Connecting,
    Stage,
    Failed,
    Disconnected,
}

/// Live status of one core.
///
/// `fault`, when present, is one of the closed kind keys [`crate::feed::fault_keys::fault_kind`]
/// derives from [`crate::feed::ConnFaultKind`] (`key_empty`, `connect_timed_out`, ...). It is not
/// localized text.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CoreStatusDto {
    pub id: u64,
    pub name: String,
    pub exchange: String,
    pub conn: ConnDto,
    pub ping_ms: Option<u32>,
    pub exch_ping_ms: Option<u32>,
    pub cpu_proc: Option<f32>,
    pub cpu_sys: Option<f32>,
    pub fault: Option<String>,
    /// Trading switch; `None` when the core has not reported it.
    pub trading: Option<bool>,
    /// Auto-detect switch; `None` when the core has not reported it.
    pub auto_detect: Option<bool>,
    /// Core build text; `None` when the core has not reported its version.
    pub version: Option<String>,
    /// Memory the core process uses, in MB; `None` when not reported.
    pub mem_mb: Option<u32>,
    /// Free physical memory on the core host, in MB; `None` when not reported.
    pub free_mem_mb: Option<u32>,
    /// This core's free and total. Always present. A core the store has not seen is awaiting,
    /// with empty figure text.
    pub balance: CoreBalanceFigureDto,
    /// Coin holdings after priced-dust filtering; unpriced holdings are retained.
    pub coins: Vec<CoinBalanceDto>,
}

/// Core status list for one chat, including the balance total those cores share.
///
/// `can_control` is true only for the owner. `total` is `None` and `total_text` is empty when
/// `counted == 0` (unavailable, never a fake zero). `excluded` counts awaiting and unpriced
/// cores. `stale` counts cores inside `counted` whose figure is stale.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CoresDto {
    pub cores: Vec<CoreStatusDto>,
    pub can_control: bool,
    pub total: Option<f64>,
    pub total_text: String,
    pub counted: u32,
    pub stale: u32,
    pub excluded: u32,
    pub per_exchange: Vec<ExchangeBalanceDto>,
}

/// Core switch a Mini App command flips. Serialized as snake_case.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreSwitchDto {
    Trading,
    AutoDetect,
}

/// Freshness of one core's balance. Serialized as snake_case.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BalanceStateDto {
    Live,
    Stale,
    Awaiting,
    Unpriced,
}

/// One core's free and total on the Cores response.
///
/// `free` and `total` are `Some` only when the reading is usable (live or stale, and finite).
/// The texts are empty when it is not, so a missing figure is not serialized as zero.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CoreBalanceFigureDto {
    pub state: BalanceStateDto,
    pub free: Option<f64>,
    pub total: Option<f64>,
    pub free_text: String,
    pub total_text: String,
}

/// One held coin on a core.
///
/// `value` is `Some` only when that core's balance reading is usable and the coin's USDT value
/// is positive. Otherwise `value_text` is the localized unpriced word. The reader omits dust.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CoinBalanceDto {
    pub coin: String,
    pub qty_text: String,
    pub value: Option<f64>,
    pub value_text: String,
}

/// Sum across the cores of one exchange.
///
/// `total` and `total_text` are `None` when `counted == 0`. `excluded` counts cores left out
/// of the sum (awaiting or unpriced). `stale` counts cores that did contribute a stale figure.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ExchangeBalanceDto {
    pub exchange: String,
    pub total: Option<f64>,
    pub total_text: Option<String>,
    pub counted: u32,
    pub stale: u32,
    pub excluded: u32,
}

/// One open order. Quote-money volume and prices are preformatted text.
///
/// `side` is the closed set `"buy"` (long) or `"sell"` (short).
/// `change_pct` and `change_text` are the directional move from entry to the
/// current mark, the same percent the desktop orders table shows. Both are
/// absent when that percent cannot be computed, which includes every order
/// whose entry has not filled: it holds no position, so it has no PnL.
/// `to_entry_pct` and `to_entry_text` cover exactly that case instead: how far
/// the current mark still has to travel to reach the entry. The percent is
/// signed so that a negative value means the mark has already passed it; the
/// text is unsigned, since it is a distance, not a result. They are absent once
/// the order holds a position. Adaptive `entry_text` and `mark_text` drop
/// cents once a price reaches 1000, so the page cannot recover either percent
/// by parsing them.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct OrderDto {
    pub core: u64,
    pub core_name: String,
    /// Exchange section caption of the order's core.
    pub exchange: String,
    /// Decimal string on the wire ([`id_text`]); moonproto types it as a full `u64`.
    #[serde(serialize_with = "id_text::serialize")]
    pub uid: u64,
    pub coin: String,
    pub market: String,
    pub side: String,
    /// Entry notional in the market quote; absent for unusable inputs or an unknown quote.
    pub volume_text: Option<String>,
    pub entry_text: Option<String>,
    pub mark_text: Option<String>,
    /// Open PnL in the market's own quote currency.
    pub pnl: Option<f64>,
    /// [`Self::pnl`] with its unit: `$` for a USD-stable quote, otherwise the quote ticker; a bare
    /// number when the quote is unknown.
    pub pnl_text: Option<String>,
    /// Sign of [`Self::pnl_text`] as printed (-1, 0, 1): the row's tone follows the rounded text,
    /// so a loss that rounds to zero is not painted red.
    pub pnl_sign: i8,
    /// [`Self::pnl`] in dollars through the USD rate of the same quote [`Self::pnl_text`] names;
    /// absent while that quote or its rate is unknown.
    /// Order sums add this, never `pnl`, which is in a different currency per market.
    pub pnl_usd: Option<f64>,
    /// An emulated order: marked, and kept out of every order sum, as the desktop keeps it apart.
    pub emulator: bool,
    pub change_pct: Option<f64>,
    pub change_text: Option<String>,
    pub to_entry_pct: Option<f64>,
    pub to_entry_text: Option<String>,
    pub panic_armed: bool,
}

/// Open orders, plus whether this chat may send money commands.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct OrdersDto {
    pub orders: Vec<OrderDto>,
    pub can_control: bool,
}

/// One closed trade. Money, prices and dates are preformatted text.
///
/// `side` is the closed set `"buy"` (long) or `"sell"` (short). `closed_at` is UTC seconds.
/// `profit` and `profit_pct` are `None` when the trade could not be valued.
/// `volume_text` is the safe entry notional in USDT, absent when it cannot be reconstructed.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TradeDto {
    pub core: u64,
    pub core_name: String,
    /// Exchange section caption of the trade's core.
    pub exchange: String,
    pub rec_id: i64,
    pub coin: String,
    pub side: String,
    pub profit: Option<f64>,
    pub profit_text: Option<String>,
    pub profit_pct: Option<f64>,
    pub profit_pct_text: Option<String>,
    pub closed_at: i64,
    pub closed_text: String,
    /// Compact close time for the list row: `HH:MM` today, `DD.MM HH:MM` otherwise.
    pub closed_short_text: String,
    pub entry_text: Option<String>,
    pub exit_text: Option<String>,
    pub qty_text: String,
    pub volume_text: Option<String>,
    /// Seconds from entry to close; `None` when the entry time is unknown.
    pub duration_secs: Option<i64>,
    /// Strategy name; `None` for a manual trade or one whose strategy is unknown.
    pub strategy: Option<String>,
    /// The row marks the trade as manual (`strategyid = 0`); `false` when a strategy is named or
    /// the row carries no strategy id at all.
    pub manual: bool,
}

/// Latest closed trades, newest first, at most `limit` of them.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TradesDto {
    pub trades: Vec<TradeDto>,
    pub limit: u32,
}

/// Unconfirmed state of a strategy toggle. Serialized as snake_case.
///
/// `TimedOut` means the core did not confirm in time; the real state is unknown, not rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyPendingDto {
    Pending,
    TimedOut,
}

/// One strategy row. `wanted` is the state last asked for while it is unconfirmed.
///
/// `id` goes out as a decimal string ([`id_text`]): strategy ids span the whole 64-bit range.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct StrategyDto {
    #[serde(serialize_with = "id_text::serialize")]
    pub id: u64,
    pub name: String,
    pub checked: bool,
    pub wanted: Option<bool>,
    pub pending: Option<StrategyPendingDto>,
}

/// Strategies of one folder. `path` is `"/"`-joined; `""` is the root.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct StrategyFolderDto {
    pub path: String,
    pub strategies: Vec<StrategyDto>,
}

/// Strategies of one core, grouped by folder.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CoreStrategiesDto {
    pub core: u64,
    pub core_name: String,
    pub exchange: String,
    pub folders: Vec<StrategyFolderDto>,
}

/// Strategy list. `can_control` is true only for the owner.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct StrategiesDto {
    pub cores: Vec<CoreStrategiesDto>,
    pub can_control: bool,
}

/// Result of a Mini App money command. `armed` is the state after a panic command.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CommandResultDto {
    pub ok: bool,
    pub armed: Option<bool>,
    pub error: Option<CommandErrorDto>,
}

/// Result of a Mini App command over several cores.
///
/// Of `requested` known cores, `sent` accepted the command now, `already` were in the asked state
/// and needed nothing, and `offline` were skipped because they are not connected (a command queued
/// for them would fire whenever they come back). `ok` only when every core ended up sent or
/// already there.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ScopeResultDto {
    pub ok: bool,
    pub sent: u32,
    pub requested: u32,
    pub already: u32,
    pub offline: u32,
    pub error: Option<CommandErrorDto>,
}

/// Why a Mini App money command did not succeed. Serialized as snake_case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorDto {
    NotFound,
    Unavailable,
    Rejected,
    Forbidden,
    /// The core is not connected, so nothing was sent.
    Offline,
}

/// One core the chat may include in a notification rule.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct NotifyCoreDto {
    /// Saved core id, serialized as a JSON number like the Cores list's ids.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Exchange section caption.
    pub exchange: String,
}

/// Notification settings for the signed chat, plus the cores that chat may name.
///
/// `error` and `fault` are always present. `null` means the read or the save succeeded.
/// `revision` is the stored counter the next save must send back.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct NotifyDto {
    /// Stored rules. A missing chat is the all-off default.
    pub settings: crate::telegram::notify::NotifySettings,
    /// Cores this chat is allowed to see, in Mini App order.
    pub cores: Vec<NotifyCoreDto>,
    /// Stored chat revision. `0` when this chat has no row yet.
    pub revision: u64,
    /// Localized save problem, or `null` when nothing went wrong.
    pub error: Option<String>,
    /// Machine kind of a save problem, or `null` when the read or the save succeeded.
    ///
    /// `stale`, `cores`, `invalid`, and `save` are the only values. The page branches on
    /// `stale` and still shows `error`.
    pub fault: Option<String>,
}

/// 64-bit identifiers as decimal strings on the Mini App wire.
///
/// JavaScript parses every JSON number into an IEEE double, which holds integers exactly only up
/// to 2^53. Strategy ids and order uids use the whole `u64` range, so as numbers the page would
/// round them and send a different id back. As strings they survive both ways unchanged. Small
/// counters (core ids from `UidCounter`, trade row ids) stay numbers.
pub mod id_text {
    use serde::{Deserialize, Deserializer, Serializer};

    /// Write `id` as its decimal string.
    ///
    /// Args:
    ///     id: Identifier to write.
    ///     serializer: Target serializer.
    ///
    /// Returns:
    ///     The serializer's result.
    pub fn serialize<S: Serializer>(id: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(id)
    }

    /// Read an identifier sent as a decimal string.
    ///
    /// Args:
    ///     deserializer: Source deserializer.
    ///
    /// Returns:
    ///     The id; a JSON number or a string that is not a `u64` is an error, so a rounded id
    ///     can never be accepted silently.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}
