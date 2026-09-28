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
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct DayDto {
    pub start: String,
    pub usdt: Option<f64>,
    pub text: Option<String>,
}

/// Authenticated report for one [`ReportPeriodDto`].
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ReportDto {
    pub from: String,
    pub to: String,
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
/// `fault`, when present, is one of the closed kind keys derived from
/// [`crate::feed::ConnFaultKind`]: `key_empty`, `key_unparsable`, `local_bind_failed`,
/// `aborted`, `connect_timed_out`, `not_authenticated`, `init_step_timed_out`,
/// `startup_stalled`, `init_step_failed`. It is not localized text.
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
}

/// Core status list. `can_control` is true only for the owner.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CoresDto {
    pub cores: Vec<CoreStatusDto>,
    pub can_control: bool,
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

/// One core's balance, including optional preformatted text.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CoreBalanceDto {
    pub id: u64,
    pub name: String,
    pub exchange: String,
    pub state: BalanceStateDto,
    pub free: Option<f64>,
    pub total: Option<f64>,
    pub free_text: Option<String>,
    pub total_text: Option<String>,
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

/// Balance page: grand total, per core, per exchange, and how many cores sit behind that total.
///
/// `total` and `total_text` are `None` when `counted == 0` (unavailable, never a fake zero).
/// `excluded` counts awaiting and unpriced cores. `stale` counts cores inside `counted` whose
/// figure is stale.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct BalancesDto {
    pub total: Option<f64>,
    pub total_text: Option<String>,
    pub per_core: Vec<CoreBalanceDto>,
    pub per_exchange: Vec<ExchangeBalanceDto>,
    pub excluded: u32,
    pub counted: u32,
    pub stale: u32,
}

/// One open order. Quantity and prices are preformatted text.
///
/// `side` is the closed set `"buy"` (long) or `"sell"` (short).
/// `change_pct` and `change_text` are the directional move from entry to the
/// current mark, the same percent the desktop orders table shows. Both are
/// absent when that percent cannot be computed. Adaptive `entry_text` and
/// `mark_text` drop cents once a price reaches 1000, so the page cannot
/// recover the percent by parsing them.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct OrderDto {
    pub core: u64,
    pub core_name: String,
    /// Exchange section caption of the order's core.
    pub exchange: String,
    pub uid: u64,
    pub coin: String,
    pub market: String,
    pub side: String,
    pub qty_text: String,
    pub entry_text: Option<String>,
    pub mark_text: Option<String>,
    pub pnl: Option<f64>,
    pub pnl_text: Option<String>,
    pub change_pct: Option<f64>,
    pub change_text: Option<String>,
    pub panic_armed: bool,
}

/// Open orders, plus whether this chat may send money commands.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct OrdersDto {
    pub orders: Vec<OrderDto>,
    pub can_control: bool,
}

/// Result of a Mini App money command. `armed` is the state after a panic command.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CommandResultDto {
    pub ok: bool,
    pub armed: Option<bool>,
    pub error: Option<CommandErrorDto>,
}

/// Result of a Mini App command over several cores: `sent` of `requested` were accepted.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ScopeResultDto {
    pub ok: bool,
    pub sent: u32,
    pub requested: u32,
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
}
