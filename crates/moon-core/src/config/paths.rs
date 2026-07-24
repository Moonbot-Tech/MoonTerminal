//! Config and user-data paths.
//!
//! - **Windows**: files live beside the executable for a portable installation.
//! - **macOS / Linux**: files live in a user-writable directory *outside* the application
//!   bundle so an update (replacing the `.app` through a DMG/package) does not delete cores
//!   and settings:
//!     - macOS: `~/Library/Application Support/com.moonbot.moonterminal/`
//!     - Linux: `~/.config/com.moonbot.moonterminal/`
//!   The `servers.enc` encryption key is stored in the OS keyring (see `crypto.rs`).
//!
//! All paths are based on `data_dir()`; on Windows, `data_dir() == exe_dir()`.
//!
//! UI settings and layout (plaintext `settings.toml`/`theme.toml`/`orders.toml`/
//! `layout.toml`/`docks.json`/`detached.json`/`charts.json`) live in `cfg/` under
//! `data_dir`. The data root retains the `servers.enc` secret, logs in `logs/`, and config
//! snapshots in `backups/`; databases use `db_dir()` (`data/` on Windows, the data root
//! elsewhere). See `config::backup`. Flat config files are moved to `cfg/` once at startup;
//! see `migrate_flat_to_cfg`.

use std::path::PathBuf;

/// Application identifier for user directories outside the bundle. Matches
/// `CFBundleIdentifier` in `.github/scripts/make-dmg.sh`. Unused on Windows, where data
/// lives beside the executable.
#[cfg_attr(windows, allow(dead_code))]
const APP_ID: &str = "com.moonbot.moonterminal";

/// Directory beside the executable. This is the data directory on Windows; on macOS/Linux,
/// it is only the source of a one-time migration from inside the bundle.
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// User-data directory without creating it as a side effect, used for migration path comparison.
/// Location logic is centralized here.
fn data_dir_raw() -> PathBuf {
    #[cfg(windows)]
    {
        exe_dir()
    }
    #[cfg(target_os = "macos")]
    {
        // ~/Library/Application Support
        dirs::data_dir()
            .map(|d| d.join(APP_ID))
            .unwrap_or_else(exe_dir)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        // Linux and other Unix systems: ~/.config
        dirs::config_dir()
            .map(|d| d.join(APP_ID))
            .unwrap_or_else(exe_dir)
    }
}

/// User data/config directory. Created on first access; on Windows this is exe_dir and already
/// exists, so `create_dir_all` is a no-op.
pub fn data_dir() -> PathBuf {
    let dir = data_dir_raw();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!(
            "не удалось создать директорию данных {}: {e}",
            dir.display()
        );
    }
    dir
}

/// The `cfg/` data subdirectory contains UI settings and layout as plaintext TOML/JSON.
/// Flat configuration files move here; `servers.enc`, databases under `db_dir()`, `logs/`, and
/// `backups/` remain outside. `db_dir()` is `data/` on Windows and the `data_dir` root elsewhere.
/// Created on first access.
pub fn cfg_dir() -> PathBuf {
    let dir = data_dir().join("cfg");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("не удалось создать директорию cfg {}: {e}", dir.display());
    }
    dir
}

/// Encrypted server file containing only uid/name/key as portable secrets.
/// Remains in the `data_dir` root (beside the exe on Windows), separate from settings.
pub fn servers_path() -> PathBuf {
    data_dir().join("servers.enc")
}

/// Remaining config (groups, etc.) as plaintext TOML without secrets.
pub fn settings_path() -> PathBuf {
    cfg_dir().join("settings.toml")
}

/// Chart theme in a separate portable file that can be shared.
pub fn theme_path() -> PathBuf {
    cfg_dir().join("theme.toml")
}

/// Order-line style in a separate portable file.
pub fn orders_path() -> PathBuf {
    cfg_dir().join("orders.toml")
}

/// Window layout (positions/sizes/minimized state/active tab + detached windows) in a
/// separate portable file.
pub fn layout_path() -> PathBuf {
    cfg_dir().join("layout.toml")
}

/// GPUI shell dock layout (DockAreaState per group) as separate JSON. gpui-component defines
/// the structure, so it is not TOML; see moon-ui-gpui.
pub fn docks_path() -> PathBuf {
    cfg_dir().join("docks.json")
}

/// Detached GPUI shell dock panels (panel, source group, and window geometry) as separate JSON.
/// Detached windows are restored at startup.
pub fn detached_path() -> PathBuf {
    cfg_dir().join("detached.json")
}

/// Chart-tab state (per-tab scale + detached-tab window geometry) as JSON. Detached tabs are
/// restored empty at startup, showing only the logo until a detect arrives.
pub fn charts_path() -> PathBuf {
    cfg_dir().join("charts.json")
}

/// User-defined chart figures (drawing layer; key = core+coin) as JSON.
pub fn figures_path() -> PathBuf {
    cfg_dir().join("figures.json")
}

/// Detect-type badges (code + per-strategy-type colors, per theme) as portable JSON.
pub fn badges_path() -> PathBuf {
    cfg_dir().join("badges.json")
}

/// Local per-tag colours for the News panel, in a small portable JSON file.
pub fn news_tags_path() -> PathBuf {
    cfg_dir().join("news_tags.json")
}

/// Detect-tape presentation (dimensions/chart/rail/size slots, per group) in a separate
/// portable file that can be shared through Copy/Paste in the ⚙ popup.
pub fn detects_view_path() -> PathBuf {
    cfg_dir().join("detects_view.toml")
}

/// Hotkeys and mouse gestures in a separate portable file that can be shared. Before schema v13,
/// they lived in a settings.toml section and are migrated once during load.
pub fn hotkeys_path() -> PathBuf {
    cfg_dir().join("hotkeys.toml")
}

/// Database directory (reports/klines). On Windows, use `data/` beside the exe to avoid
/// cluttering the portable root with large files. On macOS/Linux, `data_dir` is already a
/// dedicated application directory (Application Support/.config), so databases live there.
/// One-time migration renames the old root `reports.sqlite` plus -wal/-shm; rename is instant
/// on one volume and moving wal/shm together preserves database consistency.
pub fn db_dir() -> PathBuf {
    #[cfg(windows)]
    let dir = data_dir().join("data");
    #[cfg(not(windows))]
    let dir = data_dir();
    static MIGRATED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    MIGRATED.get_or_init(|| {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::warn!("не удалось создать директорию БД {}: {e}", dir.display());
            return;
        }
        for name in ["reports.sqlite", "reports.sqlite-wal", "reports.sqlite-shm"] {
            let old = data_dir().join(name);
            let new = dir.join(name);
            if old != new && old.exists() && !new.exists() {
                if let Err(e) = std::fs::rename(&old, &new) {
                    log::warn!(
                        "миграция {} → {} не удалась: {e}",
                        old.display(),
                        new.display()
                    );
                }
            }
        }
    });
    dir
}

/// SQLite report DB (typed `orders_rep` replica; legacy `closed_sell_reports` read-only).
pub fn reports_db_path() -> PathBuf {
    db_dir().join("reports.sqlite")
}

/// SQLite database for the local kline cache (see `market::kline_cache`), separate from reports.
pub fn klines_db_path() -> PathBuf {
    db_dir().join("klines.sqlite")
}

/// SQLite database for strategies and their versions (`strat_db`). DELIBERATELY separate from
/// `reports.sqlite`: the report replica is a recoverable cache resynchronized from the core,
/// while strategy version history is the only copy and must survive replica resets/resyncs.
pub fn strategies_db_path() -> PathBuf {
    db_dir().join("strategies.sqlite")
}

/// Local storage settings for report/strategy databases. The Settings tab writes UI-managed
/// values, while system values such as the version ignore list are edited manually.
pub fn storage_path() -> PathBuf {
    cfg_dir().join("storage.toml")
}

/// Log directory for diagnostic core commands/reports.
pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Snapshot directory for irreplaceable config files; see `config::backup`.
///
/// Lives in the `data_dir` root beside `logs/` and `servers.enc`, NOT inside `cfg/`: a snapshot
/// contains a file FROM `cfg/`, so nesting it there would make the next snapshot copy its predecessor.
///
/// Unlike [`cfg_dir`], deliberately does NOT create the directory. On Windows, `data_dir` is beside
/// the executable and is often cloud-synchronized, so an empty `backups/` is unnecessary before
/// the first snapshot. `config::backup` creates it lazily only when there is something to write.
pub fn backups_dir() -> PathBuf {
    data_dir().join(BACKUPS_DIR_NAME)
}

/// Snapshot directory name, kept as a constant so a test can prove that migration lists exclude it.
/// See `the_migration_lists_never_carry_the_backup_directory`.
const BACKUPS_DIR_NAME: &str = "backups";

/// Legacy combined encrypted config for one-time migration. Read from beside the executable,
/// where older builds left it.
pub fn legacy_enc_path() -> PathBuf {
    exe_dir().join("config.enc")
}

/// Oldest plaintext config used for one-time migration.
pub fn legacy_toml_path() -> PathBuf {
    PathBuf::from("config.toml")
}

/// One-time migration of user files from the bundle (beside the executable) to the user-data
/// directory. On Windows, `data_dir() == exe_dir()`, so this is a no-op. A file is copied only
/// when it exists beside the executable and is absent from its destination.
///
/// The encryption key lives in the OS keyring, so copied `servers.enc` remains readable and
/// decryptable without additional steps.
pub fn migrate_bundle_data() {
    let src_dir = exe_dir();
    let dst_dir = data_dir();
    if src_dir == dst_dir {
        return; // Windows or launch from the same directory: nothing to migrate.
    }
    // Older builds wrote all files flat beside the executable inside the bundle. Put each file
    // directly in its current location: secrets/databases in the data_dir root, settings/layout
    // in `cfg/`. This avoids orphaned root duplicates; otherwise a root `dst.exists()` would not
    // see a file already moved to `cfg/` and would copy it again on every launch. Legacy
    // config.enc/config.toml files migrate separately in AppConfig::load, which reads exe_dir directly.
    let cfg_root = cfg_dir();
    let targets = ROOT_FILES
        .iter()
        .map(|n| (*n, dst_dir.clone()))
        .chain(CFG_FILES.iter().map(|n| (*n, cfg_root.clone())));

    let mut migrated = false;
    for (name, dst_dir) in targets {
        let src = src_dir.join(name);
        let dst = dst_dir.join(name);
        if src.exists() && !dst.exists() {
            match std::fs::copy(&src, &dst) {
                Ok(_) => migrated = true,
                Err(e) => log::warn!("миграция {name} из бандла не удалась: {e}"),
            }
        }
    }
    if migrated {
        log::info!(
            "Migrated MoonTerminal config from app bundle to {}",
            dst_dir.display()
        );
    }
}

/// Files that remain in the `data_dir` root during migration from the application bundle.
///
/// Both lists (this one and [`CFG_FILES`]) contain ONLY file names. `migrate_bundle_data` and
/// `migrate_flat_to_cfg` use `fs::copy`/`fs::rename` and do not recurse into directories, but a
/// directory name here would not be a no-op: the directory and all its contents would move silently.
const ROOT_FILES: &[&str] = &["servers.enc", "reports.sqlite"];

/// Settings/layout files moved from the `data_dir` root to the `cfg/` subdirectory.
/// Move `.bak` too so the recovery copy of a corrupt settings.toml is not orphaned in the root.
const CFG_FILES: &[&str] = &[
    "settings.toml",
    "settings.toml.bak",
    "theme.toml",
    "orders.toml",
    "layout.toml",
    "docks.json",
    "detached.json",
    "charts.json",
    "badges.json",
    "hotkeys.toml",
];

/// One-time move of flat settings/layout files from the `data_dir` root to their new `cfg/`
/// location. Idempotent: move a file only when it exists in the root and is NOT already in
/// `cfg/`. Failures are non-fatal; the file remains in place, the error is logged, and the next
/// launch retries.
///
/// Call AFTER `migrate_bundle_data`, which places bundle files directly in their current
/// destinations, and BEFORE the first settings read.
pub fn migrate_flat_to_cfg() {
    let root = data_dir();
    let cfg = cfg_dir();
    let mut moved = 0u32;
    for name in CFG_FILES {
        let src = root.join(name);
        let dst = cfg.join(name);
        if src.exists() && !dst.exists() {
            match std::fs::rename(&src, &dst) {
                Ok(_) => moved += 1,
                Err(e) => log::warn!("перенос {name} в cfg/ не удался: {e}"),
            }
        }
    }
    if moved > 0 {
        log::info!("настройки перенесены в {}", cfg.display());
    }
}

#[cfg(test)]
mod tests;
