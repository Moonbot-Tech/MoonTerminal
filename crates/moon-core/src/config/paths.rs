//! Пути конфигов рядом с исполняемым файлом.

use std::path::PathBuf;

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Зашифрованный файл серверов: только name/ip/port/key (переносимый секрет).
pub fn servers_path() -> PathBuf {
    exe_dir().join("servers.enc")
}

/// Остальная конфигурация (группы и пр.) — открытый toml, без секретов.
pub fn settings_path() -> PathBuf {
    exe_dir().join("settings.toml")
}

/// Тема оформления чарта — отдельный переносимый файл (можно делиться).
pub fn theme_path() -> PathBuf {
    exe_dir().join("theme.toml")
}

/// Стиль линий ордеров — отдельный переносимый файл рядом с exe.
pub fn orders_path() -> PathBuf {
    exe_dir().join("orders.toml")
}

/// Раскладка окон (позиции/размеры/свёрнутость/активная вкладка + откреплённые
/// окна) — отдельный переносимый файл рядом с exe.
pub fn layout_path() -> PathBuf {
    exe_dir().join("layout.toml")
}

/// Раскладка доков GPUI-оболочки (DockAreaState по группам) — отдельный JSON рядом
/// с exe (структура задаётся gpui-component, потому не toml; см. moon-ui-gpui).
pub fn docks_path() -> PathBuf {
    exe_dir().join("docks.json")
}

/// Откреплённые dock-панели GPUI-оболочки (какая панель, из какой группы, геометрия
/// окна) — отдельный JSON рядом с exe. На старте окна открепления восстанавливаются.
pub fn detached_path() -> PathBuf {
    exe_dir().join("detached.json")
}

/// Состояние чарт-вкладок (масштаб по вкладке + геометрия откреп-окон вкладок) — JSON рядом
/// с exe. На старте откреп-вкладки восстанавливаются пустыми (только лого), ждут детект.
pub fn charts_path() -> PathBuf {
    exe_dir().join("charts.json")
}

/// SQLite-БД с отчётами по закрытым ордерам (`ClosedSellOrderReport`).
pub fn reports_db_path() -> PathBuf {
    exe_dir().join("reports.sqlite")
}

/// Папка логов рядом с exe (команды/отчёты ядра для диагностики).
pub fn logs_dir() -> PathBuf {
    exe_dir().join("logs")
}

/// Старый объединённый зашифрованный конфиг (для одноразовой миграции).
pub fn legacy_enc_path() -> PathBuf {
    exe_dir().join("config.enc")
}

/// Совсем старый открытый конфиг (для одноразовой миграции).
pub fn legacy_toml_path() -> PathBuf {
    PathBuf::from("config.toml")
}
