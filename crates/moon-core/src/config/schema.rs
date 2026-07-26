//! On-disk config formats (serde). This module contains ONLY data structures, with no
//! reading/writing (see `store`) or runtime merging (see `reconcile`).
//!
//! Forward compatibility: mark every new field with `#[serde(default = …)]` so an older
//! file without it remains readable and receives the default. The `version` below allows
//! these defaults to be written back once (see `AppConfig::load`).

use serde::{Deserialize, Serialize};

use super::groups::GroupConfig;
use super::hotkeys::HotkeysConfig;
use super::lang::Language;
use super::secrets::Secret;
use super::servers::{self, CoreSortMode, FeedFlags};
use crate::market::MarketDataMode;

/// Current `settings.toml` schema version.
///
/// Incremented when persisted fields need serde defaults written back.
///
/// Version 16 moves manual size/exit controls from per-core base-coin settings to group-local
/// USD-equivalent settings and deliberately discards old sizes whose units are ambiguous.
/// Version 17 makes every group exit generation complete and neutral from first load, removing
/// the startup dependency on whichever core settings snapshot happens to arrive first.
pub const SCHEMA_VERSION: u32 = 17;

/// Schema version from which runtime `CoreId == uid` and is stable. Older configs stored
/// POSITIONAL CoreIds in `charts.json`, which must be rebound to uids once. This is fixed,
/// NOT `SCHEMA_VERSION`, so future schema bumps do not repeat the remap. See
/// `reconcile::merge` and `chart_persist::remap_core_ids`.
pub const COREID_UID_VERSION: u32 = 11;

/// Older files without `version` load as 0, below SCHEMA_VERSION, triggering a save
/// that writes newly defaulted fields.
pub fn default_version() -> u32 {
    0
}

pub fn default_ui_font_delta() -> f32 {
    2.0
}

pub fn default_ui_scale() -> f32 {
    1.0
}

/// Repair a stored UI scale on the way in, touching ONLY values that cannot mean anything.
///
/// `MoonScale::ui` multiplies control heights, gaps, paddings and hit areas, and MoonUI's
/// `MoonThemeTokens::ui` floors the factor at `0.25`. So a stored `0.0` does not blank the
/// interface — it renders everything at a quarter size, which still paints text at its own font
/// metric while shrinking every hit rectangle to the point where clicks stop landing. A
/// `settings.toml` written before the loader applied schema defaults holds exactly that.
///
/// Only non-finite and non-positive values are repaired. There is deliberately NO upper or lower
/// bound beyond that: `ui_scale` has no settings-UI control, so hand-editing the file is the only
/// way to set it, and the repaired value is persisted by the next `save()` — clamping a merely
/// unusual number would silently destroy a deliberate choice with no way to get it back.
pub fn repair_ui_scale(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        default_ui_scale()
    }
}

/// Repair a stored UI font delta, preserving every value that can mean something.
///
/// `0.0` is a legitimate choice here — it is "no adjustment", not a missing value — so unlike a
/// scale it is passed through untouched. Only non-finite values are repaired: TOML parses `nan`
/// and `inf` happily, so they survive the loader, and MoonUI adds this delta straight into text
/// metrics (`MoonThemeTokens::font`), where an infinity propagates into layout dimensions.
pub fn repair_ui_font_delta(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        default_ui_font_delta()
    }
}

/// Return MoonProto's production-baseline retained-history depth percentage.
pub fn default_chart_memory_percent() -> u16 {
    moonproto::state::MarketHistorySizing::DEFAULT_BUDGET_PERCENT
}

/// Clamp a persisted retained-history depth to MoonProto's supported startup range.
pub fn clamp_chart_memory_percent(value: u16) -> u16 {
    moonproto::state::MarketHistorySizing::clamp_budget_percent(value)
}

pub fn default_chart_stack_height() -> u16 {
    360
}

pub fn clamp_chart_stack_height(value: u16) -> u16 {
    value.clamp(120, 2000)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiThemeMode {
    Light,
    #[default]
    Dark,
}

/// Server entry in servers.enc (secret + stable uid).
///
/// host/port are NOT stored because they are encoded in the Moonbot key itself (see
/// `parse_key_info` in feed/live/mod.rs). Older servers.enc files with host/port fields still
/// load: serde ignores unknown fields and connection details come from the key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerEntry {
    /// Stable core identifier (see `ServerConfig::uid`). A 0 in older files is assigned
    /// on first load (see `reconcile::merge`).
    #[serde(default)]
    pub uid: u64,
    pub name: String,
    #[serde(default)]
    pub key: Secret,
}

#[derive(Default, Serialize, Deserialize)]
pub struct ServersFile {
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
}

/// Per-server metadata in plaintext settings.toml, without secrets.
/// Binds to a server by stable `uid`; older files without a uid bind once by `name`
/// (see `reconcile::merge`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerMeta {
    #[serde(default)]
    pub uid: u64,
    /// Duplicated from servers.enc for plaintext-file readability and legacy binding.
    pub name: String,
    #[serde(default = "servers::default_true")]
    pub active: bool,
    #[serde(default = "servers::default_true")]
    pub show_window: bool,
    #[serde(default)]
    pub feed: FeedFlags,
    #[serde(default = "servers::default_group")]
    pub group: String,
    #[serde(default = "servers::default_market")]
    pub market: String,
    #[serde(default = "servers::default_color")]
    pub color: [u8; 3],
    /// AddToChart chart-bundle name (see `ServerConfig::chart_bundle`). Empty uses the
    /// global setting. Older files default to an empty string.
    #[serde(default)]
    pub chart_bundle: String,
    /// Default alert strategy (id of type "Alerts"); see `ServerConfig::default_alert_strategy`.
    #[serde(default)]
    pub default_alert_strategy: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct SettingsFile {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Interface language. Missing in older files means the serde default, the system locale.
    #[serde(default)]
    pub language: Language,
    /// Market-data source (deduplicated by provider or per core). Older files use the default.
    #[serde(default)]
    pub market_mode: MarketDataMode,
    /// Separate chart tab per core (AddToChart): true = 1-HL-core, false = all cores in
    /// one 1-HL tab. Older files default to true.
    #[serde(default = "servers::default_true")]
    pub charts_split_by_core: bool,
    /// An AddToChart tab with multiple charts: true enables vertical scrolling with fixed chart
    /// heights; false divides the window height as before. Older files default to false.
    #[serde(default)]
    pub charts_stack_scroll: bool,
    /// Scroll mode compression: charts retain their configured height until they fill the window,
    /// then shrink as in non-scroll mode instead of showing a scrollbar. Defaults to false.
    #[serde(default)]
    pub charts_stack_compress: bool,
    /// Height of one chart in logical pixels in scroll mode. Defaults to 360.
    #[serde(default = "default_chart_stack_height")]
    pub chart_stack_height: u16,
    /// Separate control zones: true allows placing orders and moving lines ONLY in the order-book
    /// area; false allows it across the whole chart. Defaults to true.
    #[serde(default = "servers::default_true")]
    pub separate_control_zones: bool,
    /// Auto-close delay for Main charts when the window is inactive, in seconds. 0 disables it.
    #[serde(default)]
    pub main_idle_close_secs: u32,
    /// Whether to write application and core logs to `logs/<date>_<source>.log`. Defaults to on.
    #[serde(default = "servers::default_true")]
    pub log_to_file: bool,
    /// Number of days to retain log files; older files are deleted. 0 keeps all. Defaults to 14.
    #[serde(default = "servers::default_log_retention_days")]
    pub log_retention_days: u32,
    /// Addition to base UI font sizes in logical pixels. Default +2 turns designed 10 px text
    /// into 12 px at 1x without zooming the whole interface.
    #[serde(default = "default_ui_font_delta")]
    pub ui_font_delta: f32,
    /// Dark/light MoonUI theme. This plaintext setting is neither a secret nor the chart theme.
    #[serde(default)]
    pub ui_theme_mode: UiThemeMode,
    /// Overall UI geometry scale. It currently has no public control but is stored beside
    /// font_delta so the component theme has one source of truth.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// Startup retained-history depth percentage passed to MoonProto.
    ///
    /// The legacy field name is retained for on-disk compatibility. Dense market/category
    /// histories allocate lazily; 75 shortens heavy histories, 100 is the production baseline,
    /// and 800 is the maximum. A saved value applies when a feed client is created or respawned,
    /// not by resizing an existing client in place.
    #[serde(default = "default_chart_memory_percent")]
    pub chart_memory_percent: u16,
    /// Legacy (schema < v13): hotkeys lived in this section and now use a separate portable
    /// `hotkeys.toml`. Read for one-time migration but never written.
    #[serde(default, skip_serializing)]
    pub hotkeys: HotkeysConfig,
    #[serde(default)]
    pub groups: Vec<GroupConfig>,
    /// How core lists are ordered app-wide; missing values default to `Name`.
    #[serde(default)]
    pub core_sort: CoreSortMode,
    /// Next uid to issue, persisted so deleted identities are not reused.
    ///
    /// Zero falls back to one past the highest surviving uid. This field is the only durable
    /// record of deleted high-water marks, so losing it also loses that history.
    #[serde(default)]
    pub next_uid: u64,
    #[serde(default)]
    pub servers: Vec<ServerMeta>,
}

#[cfg(test)]
mod tests;
