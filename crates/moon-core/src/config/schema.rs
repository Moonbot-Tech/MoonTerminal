//! On-disk config formats (serde). This module contains ONLY data structures, with no
//! reading/writing (see `store`) or runtime merging (see `reconcile`).
//!
//! Forward compatibility: mark every new field with `#[serde(default = …)]` so an older
//! file without it remains readable and receives the default. The `version` below allows
//! these defaults to be written back once (see `AppConfig::load`).

use serde::{Deserialize, Serialize};

use super::ProfileAge;
use super::core_groups::CoreGroup;
use super::groups::GroupConfig;
use super::hotkeys::HotkeysConfig;
use super::lang::Language;
use super::secrets::Secret;
use super::servers::{self, CoreSortMode, FeedFlags, TransportVersion};
use super::toml_io::ConfigLoad;
use crate::db::valuation::ValuationMode;
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

/// Lower bound the settings-UI Font slider and tick loop hold `ui_font_delta` within.
///
/// [`default_ui_font_delta`] must lie inside `UI_FONT_DELTA_MIN..=UI_FONT_DELTA_MAX`, so a future
/// range narrowing cannot ship a default the slider cannot represent. `repair_ui_font_delta` does
/// NOT clamp to this range: a hand-edited `settings.toml` is allowed a deliberate out-of-range
/// choice, and these constants exist for the slider and for that assertion, not for repair.
pub const UI_FONT_DELTA_MIN: i32 = -2;
/// Upper bound the settings-UI Font slider and tick loop hold `ui_font_delta` within. See
/// [`UI_FONT_DELTA_MIN`].
pub const UI_FONT_DELTA_MAX: i32 = 6;

/// Return the `ui_font_delta` used when a `settings.toml` field is ABSENT, and as the repair
/// fallback [`repair_ui_font_delta`] applies to a PRESENT but non-finite stored value.
///
/// A present FINITE value — including `0.0` — is never replaced by either path, which is what
/// keeps an existing user's chosen delta untouched. `3.0` lies within
/// `UI_FONT_DELTA_MIN..=UI_FONT_DELTA_MAX` (`settings/general.rs`).
pub fn default_ui_font_delta() -> f32 {
    3.0
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

/// Interface theme a profile receives when `settings.toml` has never been written.
///
/// Deliberately NOT the `Default` of [`UiThemeMode`]. `Default` is what serde substitutes for an
/// absent FIELD in a settings file that DOES exist, so moving it would re-theme every long-time
/// user whose file predates `ui_theme_mode`. First run is a property of the FILE, not the field,
/// and [`resolve_ui_theme_mode`] is the only place the two are told apart.
///
/// Returns:
///     The theme a brand-new profile opens in.
pub const fn first_run_ui_theme_mode() -> UiThemeMode {
    UiThemeMode::Light
}

/// Choose the interface theme from a settings read, keeping first run distinct from an old file.
///
/// Only [`ConfigLoad::Absent`] paired with an otherwise empty profile is a first run. The other
/// three outcomes all mean the file EXISTED, and existence is proof of a profile someone already
/// configured:
///
/// * `Present` — the user's own value, whether they chose it or inherited it from the field's
///   `Default` because their file predates it.
/// * `Unreadable` — a permissions error, a sharing violation or an unhydrated cloud placeholder.
///   This repeats per launch, so treating it as first run would re-theme the user on every failed
///   read and un-theme them on the next success — a flicker with no user act behind it.
/// * `Corrupt` — the file was just quarantined to `.bak`. Re-theming on top of that reads as data
///   loss rather than a fresh start.
///
/// The match is EXHAUSTIVE for the same reason [`ConfigLoad::permits_overwrite`] is: a fifth
/// variant added later for some other partial read must not compile here until its author decides
/// which side it belongs on.
///
/// Args:
///     stored: Value the settings read produced, already defaulted by serde where the field was
///         absent.
///     load: How that read went. Kept SEPARATE from `age` because it is the only thing that can
///         tell `Corrupt` and `Unreadable` apart from `Absent`, a distinction the age of the
///         profile cannot express.
///     age: The one shared provenance fact, taken from the disk as it was at launch. Required
///         because `settings.toml` can be missing beside a live `servers.enc` after a partial
///         restore, and the pre-login window resolves its theme from `settings.toml` ALONE while
///         the shell resolves it from the merged config; without a shared fact the two disagree
///         and an existing user gets a light login screen that flashes to dark.
///
/// Returns:
///     The theme to install.
pub(crate) fn resolve_ui_theme_mode(
    stored: UiThemeMode,
    load: ConfigLoad,
    age: ProfileAge,
) -> UiThemeMode {
    match (age, load) {
        (ProfileAge::FirstRun, ConfigLoad::Absent) => first_run_ui_theme_mode(),
        (
            _,
            ConfigLoad::Absent | ConfigLoad::Present | ConfigLoad::Corrupt | ConfigLoad::Unreadable,
        ) => stored,
    }
}

/// Server entry in servers.enc (secret + stable uid).
///
/// host/port are NOT stored because they are encoded in the Moonbot key itself (see
/// `parse_key_info` in feed/live/mod.rs). Older servers.enc files with host/port fields still
/// load: serde ignores unknown fields and connection details come from the key. The transport
/// mode is the one connection detail the key only SEEDS -- it is kept in `ServerMeta::transport`,
/// because MoonBot lets a core's own V0/V1/V2 switch move without issuing a new key.
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
    /// Per-core manual-config opt-in; see `ServerConfig::use_core_manual_config`.
    #[serde(default)]
    pub use_core_manual_config: bool,
    /// MoonProto transport mode (`V0`/`V1`/`V2`); see `ServerConfig::transport`. Absent in older
    /// files and while no key has been read, in which case the key decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportVersion>,
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
    /// Addition to base UI font sizes in logical pixels. Default +3 turns designed 10 px text
    /// into 13 px at 1x without zooming the whole interface.
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
    /// User-saved named sets of cores, applied from the shared core picker.
    ///
    /// Named `core_groups` rather than `groups`, which is already taken by the unrelated
    /// server-group settings above; the two are different concepts and must not be conflated.
    ///
    /// No schema bump accompanies this field. The empty list is both the serde default and the
    /// intended default, so an older file that omits it already reads correctly, and a bump would
    /// force a `backups/` snapshot on every user's next launch for nothing.
    #[serde(default)]
    pub core_groups: Vec<CoreGroup>,
    /// How core lists are ordered app-wide; missing values default to `Name`.
    #[serde(default)]
    pub core_sort: CoreSortMode,
    /// Which conversion every quote-money surface applies; missing values default to `Historical`.
    ///
    /// No schema bump accompanies this field: `Historical` is both the serde default and the
    /// intended default, so an older file that omits it already reads correctly, and the next save
    /// materializes the key.
    #[serde(default)]
    pub report_valuation_mode: ValuationMode,
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
