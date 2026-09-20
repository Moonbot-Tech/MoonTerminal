//! Local storage settings in `cfg/storage.toml`.
//!
//! One settings file covers all local SQLite databases. The Settings UI's Storage tab writes
//! its keys here, while system settings such as the version ignore list are edited manually.
//! If the file is absent, the first read writes defaults so system keys are visible and editable
//! without consulting the source.

use serde::{Deserialize, Serialize};

use super::{paths, toml_io};

/// Strategy fields whose changes do NOT create a new version (cosmetics/status/presentation).
/// Ported from mb_ai after a year of production use, plus:
/// - `PreventWorkingUntil`: sgStop/sgStart is state, not a parameter edit (tracked in
///   head.checked); otherwise every stop would create a version;
/// - `OrderSize`: order size changes routinely, including through hotkeys, and does not need
///   a parameter version (decision 2026-07-16);
/// - `StrategyName`: renaming is not a parameter edit; the current name lives in the head row
///   (decision 2026-07-16).
pub const DEFAULT_IGNORE_FIELDS: &[&str] = &[
    "Active",
    "LastEditDate",
    "ReportToTelegram",
    "ReportTradesToTelegram",
    "SoundAlert",
    "SoundKind",
    "KeepAlert",
    "SilentNoCharts",
    "AddToChart",
    "KeepInChart",
    "DontKeepOrdersOnChart",
    "UseCustomColors",
    "OrderLineKind",
    "SellOrderColor",
    "BuyOrderColor",
    "DontWriteLog",
    "DebugLog",
    "Comment",
    "PreventWorkingUntil",
    "OrderSize",
    "StrategyName",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageCfg {
    pub strategies: StrategiesStoreCfg,
    pub trade_replay: TradeReplayStoreCfg,
}

impl Default for StorageCfg {
    fn default() -> Self {
        Self {
            strategies: StrategiesStoreCfg::default(),
            trade_replay: TradeReplayStoreCfg::default(),
        }
    }
}

/// The `[trade_replay]` section for the persisted trade prints (`trades.sqlite`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TradeReplayStoreCfg {
    /// Whether the prints a trade window fetched are kept on disk for the next window and the
    /// next launch. Off, they live in memory for the session only and the file is not touched.
    pub persist_trades: bool,
    /// Ceiling on the packed prints the file may hold, in megabytes; past it the oldest spans
    /// go first. `0` keeps everything the retention window admits.
    pub max_mb: u32,
    /// Minutes of prints kept around a trade, per end: a short position gets this many minutes
    /// before its entry and after its exit; a long one (over an hour) gets this many minutes
    /// centred on each end, half before and half after, with bars between. It sizes what a trade
    /// window fetches, what a close copies out of the core's ring, and what the file keeps.
    /// `0` is the position alone; clamped to [`MAX_TRADE_MARGIN_MIN`] on load.
    pub margin_min: u32,
}

/// Default ceiling on `trades.sqlite`, megabytes: a day of busy replays is tens of megabytes,
/// so this is months of them for the reader who never touches the setting.
pub const DEFAULT_TRADES_MAX_MB: u32 = 256;

/// Default minutes of prints around a trade, per end (the developer's call, 2026-09-20).
pub const DEFAULT_TRADE_MARGIN_MIN: u32 = 15;

/// Ceiling on [`TradeReplayStoreCfg::margin_min`]: the bar context after an exit is two hours at
/// least, and prints past the bars would have nowhere to draw.
pub const MAX_TRADE_MARGIN_MIN: u32 = 120;

impl Default for TradeReplayStoreCfg {
    fn default() -> Self {
        Self {
            persist_trades: true,
            max_mb: DEFAULT_TRADES_MAX_MB,
            margin_min: DEFAULT_TRADE_MARGIN_MIN,
        }
    }
}

/// The `[strategies]` section for the local strategy and version database.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategiesStoreCfg {
    /// Whether to maintain the local strategy database (head + versions). Disabling stops
    /// writes while preserving existing history on disk.
    pub enabled: bool,
    /// Maximum versions per strategy (0 = unlimited). Older versions are pruned.
    pub version_limit: u32,
    /// Fields whose changes do not create a version. This system setting is not exposed in
    /// the UI and is edited manually; accidentally excluding a field silently loses edit history.
    pub ignore_fields: Vec<String>,
}

impl Default for StrategiesStoreCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            version_limit: 0,
            ignore_fields: DEFAULT_IGNORE_FIELDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// Reads `storage.toml`; if absent, writes defaults so system keys are visible. A corrupt file
/// is logged and defaults are used without overwriting it.
pub fn load() -> StorageCfg {
    let path = paths::storage_path();
    if !path.exists() {
        let cfg = StorageCfg::default();
        if let Err(e) = toml_io::save(&path, &cfg, "storage.toml") {
            log::warn!("storage.toml: не удалось записать дефолт: {e:#}");
        }
        return cfg;
    }
    sanitize(toml_io::load_or_default(&path, "storage.toml", |_| {}))
}

/// Bound what a hand-edited file may carry: the margin never exceeds [`MAX_TRADE_MARGIN_MIN`].
fn sanitize(mut cfg: StorageCfg) -> StorageCfg {
    cfg.trade_replay.margin_min = cfg.trade_replay.margin_min.min(MAX_TRADE_MARGIN_MIN);
    cfg
}

/// Saves storage settings from the Storage tab.
pub fn save(cfg: &StorageCfg) {
    if let Err(e) = toml_io::save(&paths::storage_path(), cfg, "storage.toml") {
        log::warn!("storage.toml: сохранение не удалось: {e:#}");
    }
}

#[cfg(test)]
mod tests;
