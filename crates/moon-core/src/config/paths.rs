//! Пути конфигов и пользовательских данных.
//!
//! - **Windows**: files live beside the executable for a portable installation.
//! - **macOS / Linux**: файлы лежат в пользовательской writable-директории *вне*
//!   бандла приложения, чтобы обновление (замена `.app` через DMG / пакета) не
//!   удаляло ядра и настройки:
//!     - macOS: `~/Library/Application Support/com.moonbot.moonterminal/`
//!     - Linux: `~/.config/com.moonbot.moonterminal/`
//!   Ключ шифрования `servers.enc` хранится в OS keyring (см. `crypto.rs`).
//!
//! Все пути считаются от `data_dir()`; на Windows `data_dir() == exe_dir()`.
//!
//! Настройки и раскладка UI (открытые `settings.toml`/`theme.toml`/`orders.toml`/
//! `layout.toml`/`docks.json`/`detached.json`/`charts.json`) лежат в подпапке
//! `cfg/` внутри `data_dir`. В корне остаются только секрет `servers.enc`, БД
//! report database `reports.sqlite`, logs in `logs/`, and config snapshots in `backups/`; see
//! `config::backup`. Flat files in the root are moved to `cfg/` once at startup; see
//! `migrate_flat_to_cfg`.

use std::path::PathBuf;

/// Идентификатор приложения для пользовательских директорий вне бандла
/// (совпадает с `CFBundleIdentifier` в `.github/scripts/make-dmg.sh`).
/// На Windows не используется — там данные лежат рядом с exe.
#[cfg_attr(windows, allow(dead_code))]
const APP_ID: &str = "com.moonbot.moonterminal";

/// Папка рядом с исполняемым файлом. На Windows это и есть директория данных;
/// на macOS/Linux — лишь источник одноразовой миграции (внутри бандла).
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Директория пользовательских данных без побочного создания (для сравнения путей
/// в миграции). Логику расположения держим тут, в одном месте.
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
        // Linux и прочие unix: ~/.config
        dirs::config_dir()
            .map(|d| d.join(APP_ID))
            .unwrap_or_else(exe_dir)
    }
}

/// Директория пользовательских данных/конфигов. Создаётся при первом обращении
/// (на Windows это exe_dir и уже существует — `create_dir_all` no-op).
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

/// Подпапка `cfg/` внутри данных — настройки и раскладка UI (открытые TOML/JSON).
/// Everything moves here EXCEPT the `servers.enc` secret, `reports.sqlite` report database,
/// `logs/`, and `backups/`, which remain in the `data_dir` root.
/// Created on first access.
pub fn cfg_dir() -> PathBuf {
    let dir = data_dir().join("cfg");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("не удалось создать директорию cfg {}: {e}", dir.display());
    }
    dir
}

/// Зашифрованный файл серверов: только name/ip/port/key (переносимый секрет).
/// Остаётся в корне `data_dir` (рядом с exe на Windows) — отдельно от настроек.
pub fn servers_path() -> PathBuf {
    data_dir().join("servers.enc")
}

/// Остальная конфигурация (группы и пр.) — открытый toml, без секретов.
pub fn settings_path() -> PathBuf {
    cfg_dir().join("settings.toml")
}

/// Тема оформления чарта — отдельный переносимый файл (можно делиться).
pub fn theme_path() -> PathBuf {
    cfg_dir().join("theme.toml")
}

/// Стиль линий ордеров — отдельный переносимый файл.
pub fn orders_path() -> PathBuf {
    cfg_dir().join("orders.toml")
}

/// Раскладка окон (позиции/размеры/свёрнутость/активная вкладка + откреплённые
/// окна) — отдельный переносимый файл.
pub fn layout_path() -> PathBuf {
    cfg_dir().join("layout.toml")
}

/// Раскладка доков GPUI-оболочки (DockAreaState по группам) — отдельный JSON
/// (структура задаётся gpui-component, потому не toml; см. moon-ui-gpui).
pub fn docks_path() -> PathBuf {
    cfg_dir().join("docks.json")
}

/// Откреплённые dock-панели GPUI-оболочки (какая панель, из какой группы, геометрия
/// окна) — отдельный JSON. На старте окна открепления восстанавливаются.
pub fn detached_path() -> PathBuf {
    cfg_dir().join("detached.json")
}

/// Состояние чарт-вкладок (масштаб по вкладке + геометрия откреп-окон вкладок) — JSON.
/// На старте откреп-вкладки восстанавливаются пустыми (только лого), ждут детект.
pub fn charts_path() -> PathBuf {
    cfg_dir().join("charts.json")
}

/// Пользовательские фигуры чарта (слой рисования; ключ = ядро+монета) — JSON.
pub fn figures_path() -> PathBuf {
    cfg_dir().join("figures.json")
}

/// Бейджи типов детектов (код+цвета по видам стратегий, на тему) — переносимый JSON.
pub fn badges_path() -> PathBuf {
    cfg_dir().join("badges.json")
}

/// Отображение лент детектов (габариты/график/rail/слоты по размерам, per-group) —
/// отдельный переносимый файл (можно делиться, Копировать/Вставить в попапе ⚙).
pub fn detects_view_path() -> PathBuf {
    cfg_dir().join("detects_view.toml")
}

/// Горячие клавиши и мышиные жесты — отдельный переносимый файл (можно делиться).
/// До schema v13 жили секцией внутри settings.toml (одноразовая миграция в load).
pub fn hotkeys_path() -> PathBuf {
    cfg_dir().join("hotkeys.toml")
}

/// Подпапка баз данных (reports/klines). На Windows — `data/` рядом с exe, чтобы не
/// захламлять портативный корень крупными файлами; на macOS/Linux `data_dir` и так
/// выделенная папка приложения (Application Support/.config) — кладём прямо в неё.
/// Разовая миграция: старый `reports.sqlite` (+ -wal/-shm) из корня переносится
/// rename-ом (мгновенно на одном томе; wal/shm едут вместе — консистентность БД).
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

/// SQLite-БД с отчётами по закрытым ордерам (`ClosedSellOrderReport`).
pub fn reports_db_path() -> PathBuf {
    db_dir().join("reports.sqlite")
}

/// SQLite-БД локального kline-кэша (см. `market::kline_cache`) — отдельная от отчётов.
pub fn klines_db_path() -> PathBuf {
    db_dir().join("klines.sqlite")
}

/// SQLite-БД стратегий и их версий (`strat_db`). НАРОЧНО отдельный файл от
/// `reports.sqlite`: реплика отчётов — восстановимый кэш (пересинхронизируется
/// с ядра), а история версий стратегий — единственный экземпляр и должна
/// переживать любые сбросы/пересинхроны реплики.
pub fn strategies_db_path() -> PathBuf {
    db_dir().join("strategies.sqlite")
}

/// Настройки локального хранилища (БД отчётов/стратегий): UI-часть пишется из
/// вкладки Настроек, системная (игнор-лист версий и пр.) правится руками.
pub fn storage_path() -> PathBuf {
    cfg_dir().join("storage.toml")
}

/// Папка логов (команды/отчёты ядра для диагностики).
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

/// Старый объединённый зашифрованный конфиг (для одноразовой миграции). Читаем из
/// каталога рядом с exe — там его оставляли прежние сборки.
pub fn legacy_enc_path() -> PathBuf {
    exe_dir().join("config.enc")
}

/// Совсем старый открытый конфиг (для одноразовой миграции).
pub fn legacy_toml_path() -> PathBuf {
    PathBuf::from("config.toml")
}

/// Одноразовая миграция пользовательских файлов из бандла (рядом с exe) в
/// пользовательскую директорию данных. На Windows `data_dir() == exe_dir()` —
/// no-op. Копируем файл только если в data_dir его ещё нет, а рядом с exe есть.
///
/// Ключ шифрования лежит в OS keyring, поэтому после копирования `servers.enc`
/// читается/расшифровывается без дополнительных действий.
pub fn migrate_bundle_data() {
    let src_dir = exe_dir();
    let dst_dir = data_dir();
    if src_dir == dst_dir {
        return; // Windows / запуск из той же папки — мигрировать нечего.
    }
    // Прежние сборки писали все файлы плоско рядом с exe (внутри бандла). Кладём их
    // сразу в актуальное расположение: секрет/БД — в корень data_dir, настройки и
    // раскладку — в `cfg/`. Так не появляется осиротевших дублей в корне (иначе
    // `dst.exists()` по корню «не видел» бы уже переехавший в `cfg/` файл и копировал
    // бы заново на каждом старте). legacy config.enc/config.toml мигрируются отдельно
    // в AppConfig::load — они читаются из exe_dir напрямую.
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

/// Файлы настроек/раскладки, переехавшие из корня `data_dir` в подпапку `cfg/`.
/// `.bak` тоже переносим, чтобы аварийная копия битого settings.toml не осталась
/// сиротой в корне.
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

/// Одноразовый перенос плоских файлов настроек/раскладки из корня `data_dir` в
/// подпапку `cfg/` (новое расположение). Идемпотентно: переносим файл, только если
/// он есть в корне и ещё НЕ появился в `cfg/`. Не фатально — при ошибке оставляем
/// файл на месте и логируем (на следующем старте попробуем снова).
///
/// Вызывать ПОСЛЕ `migrate_bundle_data` (та кладёт файлы из бандла в корень, отсюда
/// они доезжают в `cfg/`) и ДО первого чтения настроек.
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
mod tests {
    //! Checks for one-time migration lists.

    use super::{BACKUPS_DIR_NAME, CFG_FILES, ROOT_FILES};

    /// No migration list may contain the snapshot DIRECTORY.
    ///
    /// `migrate_bundle_data` and `migrate_flat_to_cfg` walk these lists with `fs::copy` and
    /// `fs::rename`. They do not recurse, but they do not reject a directory either: on Windows,
    /// `fs::rename(data_dir/backups, cfg/backups)` successfully moves the entire tree somewhere
    /// `backups_dir()` does not search, making snapshots disappear from the application.
    ///
    /// The plausible breakage is seeing `settings.toml.bak` in `CFG_FILES` and adding `"backups"`
    /// beside it. This compiles, and the damage appears only on a machine that already has snapshots.
    ///
    /// The oracle is `BACKUPS_DIR_NAME` itself, which `backups_dir()` uses to build the path, so the
    /// test cannot drift from the actual directory name.
    #[test]
    fn the_migration_lists_never_carry_the_backup_directory() {
        for (label, list) in [("ROOT_FILES", ROOT_FILES), ("CFG_FILES", CFG_FILES)] {
            assert!(
                !list.contains(&BACKUPS_DIR_NAME),
                "{label} names the snapshot directory `{BACKUPS_DIR_NAME}`; the migration would \
                 move the whole backup tree out from under backups_dir()"
            );
        }
    }
}
