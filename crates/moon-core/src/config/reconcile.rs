//! Bidirectional mapping between file formats (`schema`) and runtime `AppConfig`.
//!
//! Metadata binds to a server through a stable `uid`. Older files without uids bind once by
//! `name` and immediately receive a fresh uid; subsequent renames no longer lose their settings.

use super::groups::GroupConfig;
use super::hotkeys::HotkeysConfig;
use super::lang::Language;
use super::schema::{
    clamp_chart_memory_percent, clamp_chart_stack_height, repair_ui_font_delta, repair_ui_scale,
    ServerEntry, ServerMeta, ServersFile, SettingsFile, UiThemeMode, COREID_UID_VERSION,
    SCHEMA_VERSION,
};
use super::servers::{self, CoreSortMode};
use super::uid_counter::UidCounter;
use super::ServerConfig;
use crate::market::MarketDataMode;

/// Result of merging the two files into runtime state.
pub struct Merged {
    pub servers: Vec<ServerConfig>,
    pub groups: Vec<GroupConfig>,
    /// Interface language from settings.toml, or the system default.
    pub language: Language,
    /// Market-data source from settings.toml, or the Dedup default.
    pub market_mode: MarketDataMode,
    /// Separate chart tab per core (AddToChart).
    pub charts_split_by_core: bool,
    /// AddToChart stack: vertical scrolling (true) or divided window height (false).
    pub charts_stack_scroll: bool,
    /// Compress the scroll stack as it fills, without a scrollbar.
    pub charts_stack_compress: bool,
    /// Height of one chart in the scroll stack, in logical pixels.
    pub chart_stack_height: u16,
    /// Separate control zones, restricting orders/lines to the order-book area.
    pub separate_control_zones: bool,
    /// Main-chart auto-close delay for window inactivity, in seconds (0 = disabled).
    pub main_idle_close_secs: u32,
    /// Whether to write logs to files under logs/.
    pub log_to_file: bool,
    /// Log-file retention period in days (0 = keep everything).
    pub log_retention_days: u32,
    /// Addition to base UI font sizes in logical pixels.
    pub ui_font_delta: f32,
    /// Dark/light MoonUI theme.
    pub ui_theme_mode: UiThemeMode,
    /// Overall UI geometry scale.
    pub ui_scale: f32,
    /// Startup retained-history depth percentage applied when MoonProto feed clients are created.
    ///
    /// The legacy field name is retained for persisted-config compatibility; it no longer denotes
    /// a process-wide RAM budget, and an existing client is not resized in place.
    pub chart_memory_percent: u16,
    /// How core lists are ordered app-wide.
    pub core_sort: CoreSortMode,
    /// Durable uid counter, already advanced past any uid handed out during this merge.
    pub next_uid: UidCounter,
    /// Legacy hotkeys from settings.toml (schema < v13), used only for one-time migration
    /// to `hotkeys.toml`; ignored when hotkeys.toml already exists.
    pub hotkeys: HotkeysConfig,
    /// Whether the merged config must be persisted because the schema, assigned uids, or durable
    /// uid counter changed.
    pub dirty: bool,
    /// The config version was below `COREID_UID_VERSION`, so `charts.json` contains POSITIONAL
    /// CoreIds that must be rebound once to stable uids. The UI does this at startup because the
    /// `charts.json` format lives in the UI crate. After write-back raises the version, this flag
    /// no longer activates.
    pub chart_core_remap_needed: bool,
}

/// Merge server secrets and settings into runtime server records.
///
/// Metadata binds by uid, with a one-time name fallback for files that carry no uid. The initial
/// counter is raised above `uid_floor` before any missing uids are assigned, and a raised counter
/// marks the result dirty so the high-water mark is persisted.
pub fn merge(sf: ServersFile, meta: SettingsFile, uid_floor: Option<u64>) -> Merged {
    let mut next_uid = next_free_uid(&sf, &meta, uid_floor);
    // A counter that had to be raised is written back, so the repair survives a later boot on
    // which the stores cannot be read.
    let mut dirty = meta.version < SCHEMA_VERSION || next_uid.get() > meta.next_uid;
    // Before v11, runtime CoreId was positional, so charts.json contains positional ids.
    let chart_core_remap_needed = meta.version < COREID_UID_VERSION;
    let language = meta.language;
    let market_mode = meta.market_mode;
    let charts_split_by_core = meta.charts_split_by_core;
    let charts_stack_scroll = meta.charts_stack_scroll;
    let charts_stack_compress = meta.charts_stack_compress;
    let chart_stack_height = clamp_chart_stack_height(meta.chart_stack_height);
    let separate_control_zones = meta.separate_control_zones;
    let main_idle_close_secs = meta.main_idle_close_secs;
    let log_to_file = meta.log_to_file;
    let log_retention_days = meta.log_retention_days;
    let ui_font_delta = repair_ui_font_delta(meta.ui_font_delta);
    let ui_theme_mode = meta.ui_theme_mode;
    let ui_scale = repair_ui_scale(meta.ui_scale);
    let chart_memory_percent = clamp_chart_memory_percent(meta.chart_memory_percent);
    let core_sort = meta.core_sort;
    let hotkeys = meta.hotkeys;

    let servers = sf
        .servers
        .into_iter()
        .map(|e| {
            // Bind metadata by uid, or by name for an older file.
            let m = if e.uid != 0 {
                meta.servers.iter().find(|m| m.uid == e.uid)
            } else {
                meta.servers.iter().find(|m| m.name == e.name)
            };
            // Use the stable uid from the file or issue a fresh one and mark the config dirty.
            let uid = if e.uid != 0 {
                e.uid
            } else {
                dirty = true;
                next_uid.issue()
            };
            ServerConfig {
                // Runtime CoreId equals the stable uid, NOT a position, so it survives server
                // additions/removals/reordering without recreating windows, subscriptions, or layout.
                id: uid,
                uid,
                name: e.name,
                active: m.map(|m| m.active).unwrap_or(true),
                show_window: m.map(|m| m.show_window).unwrap_or(true),
                feed: m.map(|m| m.feed).unwrap_or_default(),
                key: e.key,
                group: m
                    .map(|m| m.group.clone())
                    .unwrap_or_else(servers::default_group),
                market: m
                    .map(|m| m.market.clone())
                    .unwrap_or_else(servers::default_market),
                color: m.map(|m| m.color).unwrap_or_else(servers::default_color),
                synthetic: false,
                chart_bundle: m.map(|m| m.chart_bundle.clone()).unwrap_or_default(),
                order_sizes: m.and_then(|m| m.order_sizes),
                order_size_sel: m.and_then(|m| m.order_size_sel),
                default_alert_strategy: m.map(|m| m.default_alert_strategy).unwrap_or(0),
            }
        })
        .collect();

    Merged {
        servers,
        groups: meta.groups,
        language,
        market_mode,
        charts_split_by_core,
        charts_stack_scroll,
        charts_stack_compress,
        chart_stack_height,
        separate_control_zones,
        main_idle_close_secs,
        log_to_file,
        log_retention_days,
        ui_font_delta,
        ui_theme_mode,
        ui_scale,
        chart_memory_percent,
        core_sort,
        next_uid,
        hotkeys,
        dirty,
        chart_core_remap_needed,
    }
}

/// Converts runtime `AppConfig` to the two file formats for writing.
#[allow(clippy::too_many_arguments)]
pub fn split(
    servers: &[ServerConfig],
    groups: &[GroupConfig],
    language: Language,
    market_mode: MarketDataMode,
    charts_split_by_core: bool,
    charts_stack_scroll: bool,
    charts_stack_compress: bool,
    chart_stack_height: u16,
    separate_control_zones: bool,
    main_idle_close_secs: u32,
    log_to_file: bool,
    log_retention_days: u32,
    ui_font_delta: f32,
    ui_theme_mode: UiThemeMode,
    ui_scale: f32,
    chart_memory_percent: u16,
    core_sort: CoreSortMode,
    next_uid: u64,
) -> (ServersFile, SettingsFile) {
    let sf = ServersFile {
        servers: servers
            .iter()
            .map(|s| ServerEntry {
                uid: s.uid,
                name: s.name.clone(),
                key: s.key.clone(),
            })
            .collect(),
    };
    let meta = SettingsFile {
        version: SCHEMA_VERSION,
        language,
        market_mode,
        charts_split_by_core,
        charts_stack_scroll,
        charts_stack_compress,
        chart_stack_height: clamp_chart_stack_height(chart_stack_height),
        separate_control_zones,
        main_idle_close_secs,
        log_to_file,
        log_retention_days,
        ui_font_delta,
        ui_theme_mode,
        ui_scale,
        chart_memory_percent: clamp_chart_memory_percent(chart_memory_percent),
        // Legacy field: since v13 it lives in hotkeys.toml and is not serialized in settings.toml.
        hotkeys: HotkeysConfig::default(),
        groups: groups.to_vec(),
        core_sort,
        next_uid,
        servers: servers
            .iter()
            .map(|s| ServerMeta {
                uid: s.uid,
                name: s.name.clone(),
                active: s.active,
                show_window: s.show_window,
                feed: s.feed,
                group: s.group.clone(),
                market: s.market.clone(),
                color: s.color,
                chart_bundle: s.chart_bundle.clone(),
                order_sizes: s.order_sizes,
                order_size_sel: s.order_size_sel,
                default_alert_strategy: s.default_alert_strategy,
            })
            .collect(),
    };
    (sf, meta)
}

/// Assign stable ids to servers with `uid == 0` and advance the durable counter.
///
/// The existing maximum supports a zero persisted counter. The raise happens even when no server
/// needs a uid so the counter catches up with configured identities before it is persisted.
pub fn ensure_uids(servers: &mut [ServerConfig], counter: &mut UidCounter) {
    let highest = servers.iter().map(|s| s.uid).max().unwrap_or(0);
    counter.raise_to(highest + 1);
    for s in servers.iter_mut() {
        if s.uid == 0 {
            let uid = counter.issue();
            s.uid = uid;
            // Keep runtime CoreId equal to uid before reconcile; otherwise layout/session
            // state and reports.sqlite would refer to different identities until restart.
            s.id = uid;
        }
    }
}

/// Seed a counter from the persisted value, both config-file maxima, and `uid_floor`.
///
/// `None` leaves the config-derived seed unchanged. See `SettingsFile::next_uid` for the
/// persistence boundary and [`UidCounter`] for the floor's guarantee.
fn next_free_uid(sf: &ServersFile, meta: &SettingsFile, uid_floor: Option<u64>) -> UidCounter {
    let from_entries = sf.servers.iter().map(|e| e.uid).max().unwrap_or(0);
    let from_meta = meta.servers.iter().map(|m| m.uid).max().unwrap_or(0);
    UidCounter::new(
        meta.next_uid.max(from_entries.max(from_meta) + 1),
        uid_floor,
    )
}

#[cfg(test)]
mod uid_tests;

#[cfg(test)]
/// Repairs `merge` must apply to values coming off disk.
mod tests;
