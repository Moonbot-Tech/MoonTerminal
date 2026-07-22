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
}

impl Default for StorageCfg {
    fn default() -> Self {
        Self {
            strategies: StrategiesStoreCfg::default(),
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
    toml_io::load_or_default(&path, "storage.toml", |_| {})
}

/// Saves storage settings from the Storage tab.
pub fn save(cfg: &StorageCfg) {
    if let Err(e) = toml_io::save(&paths::storage_path(), cfg, "storage.toml") {
        log::warn!("storage.toml: сохранение не удалось: {e:#}");
    }
}
