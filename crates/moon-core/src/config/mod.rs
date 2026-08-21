//! Primary server configuration split across two files below [`paths::data_dir`]:
//! - `servers.enc` in the data root (encrypted): uid/name/key as portable secrets; host, port, and
//!   transport are encoded in the Moonbot key itself;
//! - `cfg/settings.toml` (plaintext): schema version, groups, and per-server metadata such as
//!   active/show_window/feed flags, group, market, and color, joined to servers by uid.
//!
//! An older `settings.toml` without newer fields remains readable through serde defaults. A
//! `version` below `SCHEMA_VERSION` triggers one write that adds defaulted fields while retaining
//! existing values.
//!
//! Module responsibilities:
//! - `schema` defines the serde file structures and schema version;
//! - `store` reads and writes files, including encryption and damaged-settings backups;
//! - `reconcile` merges file data into runtime state and assigns stable uids;
//! - `migrate` performs one-time migrations from legacy formats;
//! - `backup` creates daily snapshots in `backups/settings/` and protects schema migrations;
//! - `uid_counter` requires every counter construction path to name the optional store floor.

pub mod arb_view;
pub mod badges;
pub mod chart_labels;
pub mod core_groups;
pub mod crypto;
pub mod detect_view;
pub mod groups;
pub mod hotkeys;
pub mod lang;
pub mod layout;
pub mod moonbot_import;
pub mod news_tags;
pub mod orders;
pub mod paths;
pub mod quiet;
pub mod secrets;
pub mod servers;
pub mod storage;
pub mod tab_badges;
pub mod theme;
pub mod theme_legacy;

mod backup;
mod migrate;
mod reconcile;
mod schema;
mod store;
pub(crate) mod toml_io;
mod uid_counter;

#[cfg(test)]
mod tests;

pub use arb_view::{ArbRow, ArbShow, ArbViewCfg, ArbVenueCfg, ARB_MAX_ROWS};
pub use badges::{BadgeEntry, BadgesConfig};
pub use chart_labels::{
    ChartLabelField, ChartLabelGroup, ChartLabelPart, ChartLabelRow, ChartLabelsCfg, LabelAlign,
    LabelColor, LabelFlow, LabelPreset, LabelStyle, LabelWindow, LabelZone, PnlBasis,
    ResolvedLabelStyle, ARB_PART_BASE, CHART_LABEL_PARTS, CHART_LABEL_ROWS, LABEL_GAP_MAX,
    LABEL_SIZE_MULT_MAX, LABEL_SIZE_MULT_MIN, LABEL_WINDOW_COUNT, PREFIX_PART_BASE, ROW_NAME_PART,
    ROW_RUN_STRIDE,
};
pub use core_groups::{
    move_group, sanitize_core_groups, unique_group_name, CoreGroup, CORE_GROUPS_MAX,
    CORE_GROUP_MEMBERS_MAX, CORE_GROUP_NAME_MAX,
};
pub use detect_view::{
    detect_slot_count, DetectChart, DetectField, DetectSizeCfg, DetectSlot, DetectViewCfg,
    DetectViewFile, DETECT_RAIL_MAX, DETECT_SIZE_LARGE, DETECT_SIZE_MEDIUM, DETECT_SIZE_MINI,
};
pub use groups::{
    GroupConfig, GroupExitSettings, GroupTradeSettings, TakeProfitMode, DEFAULT_ORDER_SIZES_USD,
};
pub use hotkeys::{
    HotkeysConfig, MouseGestureBinding, MoveGestureCommand, MoveKind, MoveSide,
    MANUAL_STRATEGY_KEYS, ORDER_SIZE_KEYS, SELL_PRESET_KEYS, SHIFT_PERCENT, SPLIT_ORDER_PARTS,
    SPLIT_PARTS_MAX, SPLIT_PARTS_MIN,
};
pub use lang::Language;
pub use layout::{
    clamp_auto_workspace_rail_width, ChartGraphicsCfg, DetachedLayout, GeomRect, GroupLayout,
    ReportFilterPrefs, TableSortPreference, WindowLayout, WorkspaceMode,
    AUTO_WORKSPACE_RAIL_WIDTH_DEFAULT, AUTO_WORKSPACE_RAIL_WIDTH_MAX,
    AUTO_WORKSPACE_RAIL_WIDTH_MIN,
};
pub use news_tags::NewsTagSettings;
pub use orders::{LineStyle, OrdersStyle, OrdersStyleSet};
pub use quiet::{QuietCfg, QuietWarnBypass};
pub use schema::UiThemeMode;
pub use secrets::Secret;
pub use servers::{ChartBucket, CoreSortMode, FeedFlags, ServerConfig};
pub use tab_badges::TabBadgeSettings;
pub use theme::{ChartTheme, ChartThemeSet};
// Keep the counter private to `config` so external code cannot construct or replace it.
use uid_counter::UidCounter;

use std::collections::HashSet;
use std::path::Path;

use crate::db::valuation::ValuationMode;
use crate::market::MarketDataMode;

/// Interface preferences readable WITHOUT any key, for a window shown before the config is open.
///
/// The login window has to be painted before `servers.enc` can be decrypted, and everything it
/// needs is in plaintext `settings.toml` anyway. Exposing these four values keeps that window from
/// either flashing default colours or reaching into the config internals.
#[derive(Clone, Copy, Debug)]
pub struct PresentationPrefs {
    pub ui_theme_mode: UiThemeMode,
    pub ui_font_delta: f32,
    pub ui_scale: f32,
    pub language: Language,
}

/// Read interface preferences from `settings.toml`, falling back to defaults.
///
/// Uses the same repair helpers as a full load, so a hand-edited `nan` cannot reach layout through
/// this shorter path.
pub fn presentation_prefs() -> PresentationPrefs {
    let (settings, _) = store::read_settings();
    PresentationPrefs {
        ui_theme_mode: settings.ui_theme_mode,
        ui_font_delta: schema::repair_ui_font_delta(settings.ui_font_delta),
        ui_scale: schema::repair_ui_scale(settings.ui_scale),
        language: settings.language,
    }
}

/// Atomically writes a user config/layout file through a temporary sibling plus rename.
/// Used for both TOML and persisted UI JSON.
pub fn write_file_atomic(path: &Path, bytes: &[u8], label: &str) -> anyhow::Result<()> {
    toml_io::write_atomic(path, bytes, label)
}

/// Bring the latest due settings slot current for the unified backup scheduler.
pub(crate) fn backup_due_at(now_ms: i64) -> anyhow::Result<crate::backups::DueOutcome> {
    backup::backup_due_at(now_ms)
}

/// Add a default [`GroupConfig`] for every server group that has no persisted row yet.
///
/// Existing rows are never replaced or removed, so callers can use this while editing an
/// incomplete group name without losing its local manual-trading state. Returns whether at least
/// one row was inserted.
pub fn ensure_server_group_configs(
    servers: &[ServerConfig],
    groups: &mut Vec<GroupConfig>,
) -> bool {
    let mut names: Vec<String> = servers.iter().map(|server| server.group.clone()).collect();
    names.sort();
    names.dedup();

    let mut changed = false;
    for name in names {
        if groups.iter().all(|group| group.name != name) {
            groups.push(GroupConfig::new(name));
            changed = true;
        }
    }
    changed
}

/// Runtime configuration merged from the persisted config files.
///
/// This type is deliberately not `Default`; config-internal construction goes through
/// [`AppConfig::blank`] so every path names an optional durable-store uid floor. See
/// [`UidCounter`] for the invariant.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub servers: Vec<ServerConfig>,
    pub groups: Vec<GroupConfig>,
    /// User-saved named sets of cores (settings.toml), applied from the shared core picker.
    ///
    /// Unrelated to [`Self::groups`], which is per-server-group trading state. These are a
    /// selection bookmark and nothing else, so they are deliberately absent from
    /// [`Self::structural_sig`]: renaming one must never reconnect a core.
    pub core_groups: Vec<CoreGroup>,
    /// Interface language (settings.toml). Defaults to the system locale.
    pub language: Language,
    /// Market-data source (settings.toml). Defaults to Dedup, one provider per exchange.
    pub market_mode: MarketDataMode,
    /// Separate AddToChart tab per core (settings.toml).
    pub charts_split_by_core: bool,
    /// AddToChart stack: vertical scrolling (true) or divided window height (false, as before).
    pub charts_stack_scroll: bool,
    /// Compress the scroll stack as it fills so no scrollbar appears. Defaults to false.
    pub charts_stack_compress: bool,
    /// Height of one chart in the scroll stack, in logical pixels. Defaults to 360.
    pub chart_stack_height: u16,
    /// Separate control zones: orders/lines only in the order-book area (settings.toml). Defaults to true.
    pub separate_control_zones: bool,
    /// Main-chart auto-close delay for window inactivity, in seconds (settings.toml). 0 disables it.
    /// Inactive means either unfocused or focused with no mouse movement. Each chart closes after
    /// N seconds of its own inactivity, newest last; fullscreen charts are included.
    pub main_idle_close_secs: u32,
    /// Whether to write application/core logs to files under logs/ (settings.toml). Defaults to on.
    pub log_to_file: bool,
    /// Log-file retention in days; 0 keeps everything (settings.toml). Defaults to 14.
    pub log_retention_days: u32,
    /// Addition to base UI font sizes in logical pixels. Defaults to +2.
    pub ui_font_delta: f32,
    /// Dark/light MoonUI theme (plaintext settings.toml).
    pub ui_theme_mode: UiThemeMode,
    /// Overall UI geometry scale. Defaults to 1.0.
    pub ui_scale: f32,
    /// Startup retained-history depth percentage passed to MoonProto.
    ///
    /// Dense market/category histories allocate lazily. The legacy field name remains for config
    /// compatibility; the value is applied when a feed client is created or explicitly respawned.
    pub chart_memory_percent: u16,
    /// Order of every core list in the application (`settings.toml`). Defaults to `Name`.
    pub core_sort: CoreSortMode,
    /// Which conversion every quote-money surface applies (`settings.toml`). Defaults to
    /// `Historical`, the at-trade-time rate; the current-rate mode is deliberately opt-in.
    pub report_valuation_mode: ValuationMode,
    /// Next uid candidate after config entries and the supplied durable-store floor are reconciled.
    ///
    /// Private to `config`: a `pub` field could be overwritten with a stale counter from
    /// outside, undoing the floor without ever calling a constructor.
    pub(in crate::config) next_uid: UidCounter,
    /// Terminal hotkeys (plaintext hotkeys.toml).
    pub hotkeys: HotkeysConfig,
    /// Chart theme per UI mode (dark/light) in separate portable theme.toml.
    pub theme: ChartThemeSet,
    /// Order-line styles per theme (dark/light) in separate portable orders.toml.
    pub orders: OrdersStyleSet,
    /// Detect-type badges (code + per-type colors, per theme) in separate portable badges.json.
    pub badges: BadgesConfig,
    /// Runtime flag (NOT serialized): `settings.toml` EXISTS but could not be read because of
    /// permissions, a share, or an unhydrated cloud placeholder, so memory holds DEFAULTS rather
    /// than the user's settings.
    ///
    /// While set, EVERY config write is forbidden; see `save_impl`. Suppressing only the automatic
    /// startup save is insufficient: the routine `config_dirty` drain from the 100-ms loop, the
    /// application-exit write, and the Save button in Settings are three independent paths, each of
    /// which could turn a temporary read failure into irreversible replacement of the live config
    /// with defaults.
    pub settings_unreadable: bool,
    /// Runtime flag (NOT serialized): the config came from before `COREID_UID_VERSION`, when
    /// `charts.json` stored positional CoreIds. At startup, the UI rebinds them once to stable
    /// uids (see `chart_persist::remap_core_ids`). Defaults to false.
    pub chart_core_remap_needed: bool,
}

impl AppConfig {
    /// Build a default-valued config whose uid counter incorporates `uid_floor`.
    ///
    /// Confined to `config` so code elsewhere cannot create and persist a config without naming
    /// the optional floor. See [`UidCounter`] for the guarantee and its best-effort boundary.
    ///
    /// Every other field uses its `Default` implementation, and spelling out every field makes a
    /// newly added field a compile error until its initialization is considered here.
    pub(in crate::config) fn blank(uid_floor: Option<u64>) -> Self {
        Self {
            next_uid: UidCounter::new(0, uid_floor),
            servers: Default::default(),
            groups: Default::default(),
            core_groups: Default::default(),
            language: Default::default(),
            market_mode: Default::default(),
            charts_split_by_core: Default::default(),
            charts_stack_scroll: Default::default(),
            charts_stack_compress: Default::default(),
            chart_stack_height: Default::default(),
            separate_control_zones: Default::default(),
            main_idle_close_secs: Default::default(),
            log_to_file: Default::default(),
            log_retention_days: Default::default(),
            ui_font_delta: Default::default(),
            ui_theme_mode: Default::default(),
            ui_scale: Default::default(),
            chart_memory_percent: Default::default(),
            core_sort: Default::default(),
            report_valuation_mode: Default::default(),
            hotkeys: Default::default(),
            theme: Default::default(),
            orders: Default::default(),
            badges: Default::default(),
            settings_unreadable: Default::default(),
            chart_core_remap_needed: Default::default(),
        }
    }

    /// Load and merge server secrets, settings, and separate UI config files.
    ///
    /// The `settings.toml` read status is computed before choosing a load branch so every
    /// construction path sets `settings_unreadable` and blocks an unsafe write-back.
    ///
    /// Runs the two storage migrations itself so it stays self-contained, even though the caller
    /// must already have run them to resolve `uid_floor` against the post-move paths. Both are
    /// idempotent, so the second pass costs a handful of `exists` checks.
    ///
    /// `uid_floor` is the highest core uid observed in the durable stores that outlive this
    /// config — reports, strategy history, and the persisted UI state keyed by core. The caller
    /// reads them, so config keeps no dependency on the database. It must be supplied HERE
    /// rather than raised afterwards: this function already assigns uids to entries that carry
    /// none and persists them, so a floor applied later would arrive after the collision it
    /// exists to prevent. `None` means no durable store contributed a uid, whether because the
    /// stores were absent, empty, or unreadable.
    /// `backups_allowed` is false for FireTest, whose run must not create durable backups.
    pub fn load(uid_floor: Option<u64>, backups_allowed: bool) -> anyhow::Result<Self> {
        // On macOS/Linux, the first launch after the storage move migrates configs from the
        // bundle beside the executable to the user-data directory. On Windows this is a no-op
        // because data_dir == exe_dir.
        paths::migrate_bundle_data();
        // Settings/layout moved from the data root to `cfg/`; move legacy flat files once on
        // every platform after bundle migration.
        paths::migrate_flat_to_cfg();
        // Theme and order-line style are separate portable files loaded independently of
        // servers/groups.
        let theme = ChartThemeSet::load();
        let orders = OrdersStyleSet::load();
        let badges = BadgesConfig::load();
        // Hotkeys use a separate portable file since v13; None means not yet migrated.
        let hotkeys_file = HotkeysConfig::load();
        if let Some(cfg) =
            Self::load_plaintext_env(uid_floor, theme.clone(), orders.clone(), badges.clone())?
        {
            return Ok(cfg);
        }
        // Read ONCE and BEFORE branch selection: the settings.toml status decides whether ANY
        // branch below may write to disk, not only the primary branch. Reading it only inside the
        // `servers.enc` branch misses an absent servers.enc paired with an unreadable settings.toml
        // after partial sync, manual deletion, or restoration. Migration and fresh-config branches
        // would then leave the flag clear, allowing the next Settings save to replace unread
        // settings with defaults.
        let (meta, meta_load) = store::read_settings();
        let settings_unreadable = !meta_load.permits_overwrite();

        if paths::servers_path().exists() {
            let sf = store::read_servers()?;
            // Capture this BEFORE `merge` consumes `meta`, avoiding an extra `Merged` field for a
            // value already known here.
            let schema_upgrade = meta.version < schema::SCHEMA_VERSION;
            // Snapshot BEFORE any write on this path: the save below replaces both files by atomic
            // rename, after which the pre-migration bytes cannot be recovered.
            if schema_upgrade && backups_allowed {
                backup::backup_before_schema_migration();
            }
            let merged = reconcile::merge(sf, meta, uid_floor);
            // hotkeys.toml takes precedence. If absent, migrate the legacy settings.toml
            // section (or its default) once into the new file.
            let hotkeys = hotkeys_file.unwrap_or_else(|| {
                let mut h = merged.hotkeys.clone();
                // The legacy section predates the shipped keys, so it goes through the same
                // one-time fill the new file gets on load — otherwise an upgrader would see the
                // unbound slots on this launch and the keys only on the next one.
                h.fill_unbound_slots();
                if let Err(e) = h.save() {
                    log::warn!("не записал hotkeys.toml при миграции: {e:#}");
                }
                h
            });
            let mut cfg = Self {
                servers: merged.servers,
                groups: merged.groups,
                core_groups: merged.core_groups,
                language: merged.language,
                market_mode: merged.market_mode,
                charts_split_by_core: merged.charts_split_by_core,
                charts_stack_scroll: merged.charts_stack_scroll,
                charts_stack_compress: merged.charts_stack_compress,
                chart_stack_height: merged.chart_stack_height,
                separate_control_zones: merged.separate_control_zones,
                main_idle_close_secs: merged.main_idle_close_secs,
                log_to_file: merged.log_to_file,
                log_retention_days: merged.log_retention_days,
                ui_font_delta: merged.ui_font_delta,
                ui_theme_mode: merged.ui_theme_mode,
                ui_scale: merged.ui_scale,
                chart_memory_percent: merged.chart_memory_percent,
                core_sort: merged.core_sort,
                report_valuation_mode: merged.report_valuation_mode,
                next_uid: merged.next_uid,
                hotkeys,
                theme,
                orders,
                badges,
                settings_unreadable,
                chart_core_remap_needed: merged.chart_core_remap_needed,
            };
            log::info!(
                "конфиг: {} серверов, {} групп",
                cfg.servers.len(),
                cfg.groups.len()
            );
            // Write new defaults and freshly assigned uids back to disk. Failure is non-fatal;
            // continue with the state already in memory.
            //
            // FAIL CLOSED: `save` itself rejects `settings_unreadable` (see `save_impl`). This
            // explicit log makes the reason visible instead of presenting a mysterious write
            // failure.
            // `needs_rewrite` is the third reason to save: reading the file changed its key
            // slots without changing any setting — an upgrade from the pre-slot format, or this
            // machine recording its own slot. Nothing else would ask for that write, and until it
            // happens the work is repeated on every launch.
            if merged.dirty || backup::pair_write_pending() || crypto::access_state().needs_rewrite
            {
                if cfg.settings_unreadable {
                    log::error!(
                        "settings.toml не прочитался — запись конфига отключена на весь сеанс, \
                         чтобы не затереть настройки дефолтами"
                    );
                } else if let Err(e) = cfg.save() {
                    log::warn!("не удалось дослоить конфиг на диск: {e}");
                }
            }
            return Ok(cfg);
        }

        // One-time migration from legacy formats; save() writes the new files.
        if paths::legacy_enc_path().exists() {
            let mut cfg = migrate::from_legacy_enc(uid_floor)?;
            cfg.theme = theme;
            cfg.orders = orders;
            cfg.badges = badges.clone();
            cfg.charts_split_by_core = true;
            cfg.chart_stack_height = schema::default_chart_stack_height();
            cfg.log_to_file = true;
            cfg.log_retention_days = 14;
            cfg.ui_font_delta = schema::default_ui_font_delta();
            cfg.ui_theme_mode = UiThemeMode::default();
            cfg.ui_scale = schema::default_ui_scale();
            // The serde default is `true` while the field's `Default` is `false`; `save()` below
            // persists the selected value.
            cfg.separate_control_zones = servers::default_true();
            cfg.chart_memory_percent = schema::default_chart_memory_percent();
            cfg.hotkeys = hotkeys_file.unwrap_or_default();
            cfg.settings_unreadable = settings_unreadable;
            if settings_unreadable {
                log::error!(
                    "settings.toml есть, но не прочитался — миграция из config.enc НЕ записана"
                );
            } else {
                cfg.save()?;
                log::info!("мигрировано из config.enc → servers.enc + settings.toml");
            }
            return Ok(cfg);
        }
        if paths::legacy_toml_path().exists() {
            let mut cfg = migrate::from_legacy_toml(uid_floor)?;
            cfg.theme = theme;
            cfg.orders = orders;
            cfg.badges = badges.clone();
            cfg.charts_split_by_core = true;
            cfg.chart_stack_height = schema::default_chart_stack_height();
            cfg.log_to_file = true;
            cfg.log_retention_days = 14;
            cfg.ui_font_delta = schema::default_ui_font_delta();
            cfg.ui_theme_mode = UiThemeMode::default();
            cfg.ui_scale = schema::default_ui_scale();
            // The serde default is `true` while the field's `Default` is `false`; `save()` below
            // persists the selected value.
            cfg.separate_control_zones = servers::default_true();
            cfg.chart_memory_percent = schema::default_chart_memory_percent();
            cfg.hotkeys = hotkeys_file.unwrap_or_default();
            cfg.settings_unreadable = settings_unreadable;
            if settings_unreadable {
                log::error!(
                    "settings.toml есть, но не прочитался — миграция из config.toml НЕ записана"
                );
            } else {
                cfg.save()?;
                log::info!("мигрировано из config.toml → servers.enc + settings.toml");
            }
            return Ok(cfg);
        }

        log::warn!("конфиг не найден — добавь сервера в Настройках");
        // Even an empty config must name the optional store floor; see `UidCounter` for why.
        let fresh = Self {
            theme,
            orders,
            badges,
            charts_split_by_core: true, // Default to a separate tab per core.
            chart_stack_height: schema::default_chart_stack_height(),
            log_to_file: true,
            log_retention_days: 14,
            ui_font_delta: schema::default_ui_font_delta(),
            ui_theme_mode: UiThemeMode::default(),
            ui_scale: schema::default_ui_scale(),
            chart_memory_percent: schema::default_chart_memory_percent(),
            hotkeys: hotkeys_file.unwrap_or_default(),
            // Set explicitly instead of inheriting it from `blank` because the serde default is
            // `true` while the field's own `Default` is `false`, which would make the first
            // Settings save invert the control zones silently. The other fields introduced with
            // this struct update all have zero defaults.
            separate_control_zones: servers::default_true(),
            // Core files are absent, but settings.toml may still exist and be unreadable; this
            // branch must not overwrite it either.
            settings_unreadable,
            ..Self::blank(uid_floor)
        };
        Ok(fresh)
    }

    /// Build the environment-backed plaintext config when that mode is enabled.
    ///
    /// The fixed core uses uid 1, while `uid_floor` supplies the optional lower bound for its next
    /// uid. Returns `Ok(None)` when plaintext mode is disabled.
    fn load_plaintext_env(
        uid_floor: Option<u64>,
        theme: ChartThemeSet,
        orders: OrdersStyleSet,
        badges: BadgesConfig,
    ) -> anyhow::Result<Option<Self>> {
        if std::env::var_os("MOON_CONFIG_PLAINTEXT").is_none() {
            return Ok(None);
        }

        // `MOON_CONFIG_PLAINTEXT_SYNTHETIC=1` makes that core the SYNTHETIC feed (`feed::synth`):
        // no network, no API key, a deterministic tick/book/news stream. It is how chart behaviour
        // that needs specific market or news data is driven locally without touching a real config.
        let synthetic = std::env::var_os("MOON_CONFIG_PLAINTEXT_SYNTHETIC").is_some();
        let key = match std::env::var("MOON_CONFIG_PLAINTEXT_KEY") {
            Ok(key) if !key.trim().is_empty() => key,
            // A synthetic core never authenticates, so it needs no key.
            _ if synthetic => "synthetic".to_string(),
            _ => {
                let path = std::env::var("MOON_CONFIG_PLAINTEXT_KEY_FILE").map_err(|_| {
                    anyhow::anyhow!(
                        "MOON_CONFIG_PLAINTEXT=1 задан, но нет MOON_CONFIG_PLAINTEXT_KEY \
                         или MOON_CONFIG_PLAINTEXT_KEY_FILE"
                    )
                })?;
                std::fs::read_to_string(&path).map_err(|e| {
                    anyhow::anyhow!("не прочитал MOON_CONFIG_PLAINTEXT_KEY_FILE {path}: {e}")
                })?
            }
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            anyhow::bail!("MOON_CONFIG_PLAINTEXT key пустой");
        }

        let name = std::env::var("MOON_CONFIG_PLAINTEXT_NAME").unwrap_or_else(|_| "default".into());
        let group = std::env::var("MOON_CONFIG_PLAINTEXT_GROUP")
            .unwrap_or_else(|_| servers::default_group());
        let market = std::env::var("MOON_CONFIG_PLAINTEXT_MARKET")
            .unwrap_or_else(|_| servers::default_market());

        log::warn!(
            "MOON_CONFIG_PLAINTEXT=1: тестовый plaintext-конфиг, servers.enc/keyring пропущены"
        );
        Ok(Some(Self::build_plaintext_config(
            uid_floor, theme, orders, badges, name, group, market, key, synthetic,
        )))
    }

    /// Build the runtime config after environment parsing has supplied one plaintext server.
    ///
    /// The constructor is separate from environment access so its complete runtime invariants can
    /// be tested without reading or mutating process credentials.
    #[allow(clippy::too_many_arguments)]
    fn build_plaintext_config(
        uid_floor: Option<u64>,
        theme: ChartThemeSet,
        orders: OrdersStyleSet,
        badges: BadgesConfig,
        name: String,
        group: String,
        market: String,
        key: String,
        synthetic: bool,
    ) -> Self {
        let mut config = Self {
            servers: vec![ServerConfig {
                id: 1,
                uid: 1,
                name,
                active: true,
                show_window: true,
                feed: FeedFlags::default(),
                key: Secret::new(key),
                group,
                market,
                color: servers::default_color(),
                synthetic,
                chart_bundle: String::new(),
                default_alert_strategy: 0,
            }],
            groups: Vec::new(),
            core_groups: Vec::new(),
            language: Language::default(),
            market_mode: MarketDataMode::default(),
            charts_split_by_core: true,
            charts_stack_scroll: false,
            charts_stack_compress: false,
            chart_stack_height: schema::default_chart_stack_height(),
            separate_control_zones: true,
            main_idle_close_secs: 0,
            log_to_file: true,
            log_retention_days: servers::default_log_retention_days(),
            ui_font_delta: schema::default_ui_font_delta(),
            ui_theme_mode: UiThemeMode::default(),
            ui_scale: schema::default_ui_scale(),
            chart_memory_percent: schema::default_chart_memory_percent(),
            core_sort: CoreSortMode::default(),
            report_valuation_mode: ValuationMode::default(),
            // The plaintext config issues uid 1 above, so the counter starts at 2 before applying
            // the optional store floor. This mode can share `data/` with encrypted-config runs.
            next_uid: UidCounter::new(2, uid_floor),
            hotkeys: HotkeysConfig::default(),
            theme,
            orders,
            badges,
            // Plaintext mode never reads settings.toml, so there is nothing to overwrite or damage.
            settings_unreadable: false,
            chart_core_remap_needed: false,
        };
        ensure_server_group_configs(&config.servers, &mut config.groups);
        config
    }

    /// Saves split server/settings configuration plus portable theme, orders, badges, and hotkeys
    /// files. Assigns stable uids and validates unique names and API keys.
    /// Takes `&mut self` because it may assign uids to new cores.
    ///
    /// Daily recovery snapshots are owned by the unified scheduler, not by individual saves.
    pub fn save(&mut self) -> anyhow::Result<()> {
        self.save_impl()
    }

    /// Shared save implementation for every config persistence path.
    ///
    /// This is the ONLY config write point, so the write block belongs here. It covers Settings,
    /// the timer drain, the exit write, and migration rather than only one remembered path.
    fn save_impl(&mut self) -> anyhow::Result<()> {
        if self.settings_unreadable {
            anyhow::bail!(
                "settings.toml не был прочитан при старте — запись запрещена, чтобы не \
                 заменить настройки дефолтами; перезапустите приложение"
            );
        }
        reconcile::ensure_uids(&mut self.servers, &mut self.next_uid);
        ensure_server_group_configs(&self.servers, &mut self.groups);
        self.prune_orphan_groups();
        // Sanitize on the way OUT as well as on the way in: every settings persistence path
        // funnels through this write, so the file cannot retain a shape the loader would repair.
        // Deliberately NOT pruned like `prune_orphan_groups` above — a saved core group keeps a
        // member whose core is currently gone.
        core_groups::sanitize_core_groups(&mut self.core_groups);
        self.validate()?;
        let (sf, meta) = reconcile::split(
            &self.servers,
            &self.groups,
            &self.core_groups,
            self.language,
            self.market_mode,
            self.charts_split_by_core,
            self.charts_stack_scroll,
            self.charts_stack_compress,
            self.chart_stack_height,
            self.separate_control_zones,
            self.main_idle_close_secs,
            self.log_to_file,
            self.log_retention_days,
            self.ui_font_delta,
            self.ui_theme_mode,
            self.ui_scale,
            self.chart_memory_percent,
            self.core_sort,
            self.report_valuation_mode,
            self.next_uid.get(),
        );
        // Refuse an unwritable servers.enc BEFORE the pair write starts. Failing inside the block
        // below would leave the "replacement in progress" marker behind with settings.toml never
        // written, turning a refusal into a permanently half-replaced pair.
        store::ensure_servers_writable()?;
        // Hold one pair lock across both atomic replacements so a background snapshot cannot mix
        // servers from one config generation with settings metadata from another.
        backup::with_config_pair(|| -> anyhow::Result<()> {
            backup::begin_pair_write()?;
            store::write_servers(&sf)?;
            store::write_settings(&meta)?;
            backup::finish_pair_write()?;
            Ok(())
        })?;
        // Theme, line styles, detect badges, and hotkeys each use their own portable file,
        // independently of settings.toml.
        self.theme.save()?;
        self.orders.save()?;
        self.badges.save();
        self.hotkeys.save()?;
        Ok(())
    }

    /// Chart theme for the active UI mode (dark/light according to `ui_theme_mode`).
    pub fn chart_theme(&self) -> &ChartTheme {
        self.theme.get(self.ui_theme_mode == UiThemeMode::Light)
    }

    /// A group is meaningful only while at least one core references it. Do not save orphans,
    /// such as intermediate values created while typing a name.
    fn prune_orphan_groups(&mut self) {
        let used: HashSet<&str> = self.servers.iter().map(|s| s.group.as_str()).collect();
        self.groups.retain(|g| used.contains(g.name.as_str()));
    }

    /// Validates unique server names and keys. The endpoint is encoded in the key, so an
    /// identical key means the same core was added twice. Empty keys are ignored because they
    /// represent incomplete rows during editing.
    fn validate(&self) -> anyhow::Result<()> {
        let mut names = HashSet::new();
        let mut keys = HashSet::new();
        for s in &self.servers {
            // The core is i18n-agnostic, so validation uses plain text. This previously called
            // t!("err.dup_name"/"err.dup_key"); the UI may localize it if needed.
            if !names.insert(s.name.to_lowercase()) {
                anyhow::bail!("duplicate server name: {}", s.name);
            }
            if !s.key.is_empty() && !keys.insert(s.key.expose().to_owned()) {
                anyhow::bail!("duplicate API key (server: {})", s.name);
            }
        }
        Ok(())
    }

    /// Signature of the structural config: servers + server-group settings, WITHOUT saved core
    /// groups, theme, language, market mode, or hotkeys. The application uses it to decide whether
    /// saving settings requires reconnecting cores and recreating windows. Saved core groups and
    /// the other presentation or interaction settings are neutralized because they need no
    /// reconnect.
    pub fn structural_sig(&self) -> String {
        // Chart bundles and manual-trading values are local presentation/behavior settings.
        // Changing them does not reconnect cores or rebuild sessions.
        let servers: Vec<ServerConfig> = self
            .servers
            .iter()
            .map(|s| ServerConfig {
                chart_bundle: String::new(),
                default_alert_strategy: 0,
                ..s.clone()
            })
            .collect();
        let groups: Vec<GroupConfig> = self
            .groups
            .iter()
            .map(|g| GroupConfig {
                trade: GroupTradeSettings::default(),
                ..g.clone()
            })
            .collect();
        let (sf, meta) = reconcile::split(
            &servers,
            &groups,
            // Saved core groups are a selection bookmark: renaming, adding or deleting one must
            // never reconnect a core or rebuild a window, so they are neutralized to empty.
            &[],
            Language::default(),
            MarketDataMode::default(),
            true,  // The chart toggle is non-structural; no rebuild.
            false, // charts_stack_scroll is purely visual, not structural.
            false, // charts_stack_compress is purely visual.
            schema::default_chart_stack_height(), // Stack height is not structural.
            false, // separate_control_zones is behavioral, not structural.
            0,     // main_idle_close_secs is behavioral.
            true,  // Log settings are non-structural.
            14,
            schema::default_ui_font_delta(),
            UiThemeMode::default(),
            schema::default_ui_scale(),
            schema::default_chart_memory_percent(),
            // Sort mode affects presentation only. The server Vec order remains in the signature:
            // it builds `SessionManager::config_order`, which determines a reactivated session's
            // position and therefore affects the session layer.
            CoreSortMode::default(),
            // The conversion applied to quote money is a read-side presentation choice: it changes
            // no session, window, or connection.
            ValuationMode::default(),
            // The uid counter advances on save and does not describe structure by itself.
            0,
        );
        let a = toml::to_string(&sf).unwrap_or_default();
        let b = toml::to_string(&meta).unwrap_or_default();
        format!("{a}\n{b}")
    }

    /// Return persisted group properties by name without allocating a default copy.
    pub fn group_ref(&self, name: &str) -> Option<&GroupConfig> {
        self.groups.iter().find(|g| g.name == name)
    }

    /// Group properties by name, returning existing values or defaults.
    pub fn group(&self, name: &str) -> GroupConfig {
        self.group_ref(name)
            .cloned()
            .unwrap_or_else(|| GroupConfig::new(name))
    }

    /// Return mutable group properties, inserting safe defaults when the group is not persisted yet.
    pub fn group_mut(&mut self, name: &str) -> &mut GroupConfig {
        if let Some(index) = self.groups.iter().position(|group| group.name == name) {
            return &mut self.groups[index];
        }
        self.groups.push(GroupConfig::new(name));
        self.groups
            .last_mut()
            .expect("a group was inserted immediately before lookup")
    }

    /// Whether config has been set up, defined as at least one server with a key. False means
    /// first launch with no input yet, when only the Settings window is shown.
    pub fn has_keyed_server(&self) -> bool {
        self.servers.iter().any(|s| !s.key.is_empty())
    }
}

#[cfg(test)]
mod structural_sig_tests;
