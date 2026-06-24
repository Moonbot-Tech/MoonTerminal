//! Env-гейтнутая диагностика ПУТИ ДЕТЕКТОВ (железное правило: где рвётся — это РАНТАЙМ,
//! не гадаем по коду, а инструментируем и читаем лог). Цепь:
//!   feed.detects(flag) → Event::Detect → FeedMsg::Detects → store.detects_rev → ChartTabs::ingest
//! По умолчанию инертна в ЛЮБОЙ сборке (как `diag.rs`/`MOON_RENDER_DIAG`). Включается явно
//! `MOON_DETECT_DIAG=1` → строки дописываются в `detect_diag.log` в cwd. Публичная сборка чистая.
//!
//! Живёт в moon-core (нижний крейт), чтобы звать из обоих концов: feed/store (moon-core) и
//! ChartTabs (moon-ui-gpui, зависит от moon-core).

use std::sync::OnceLock;

/// Включено только при заданной env `MOON_DETECT_DIAG` (любое значение). Читается раз.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MOON_DETECT_DIAG").is_some())
}

/// Дописать строку в `detect_diag.log` (no-op без env). Без таймстампа — порядок строк = порядок
/// событий; для грубой привязки во времени достаточно секундного гранулирования из `log`-сборки.
pub fn line(msg: &str) {
    if !enabled() {
        return;
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("detect_diag.log")
    {
        let _ = writeln!(f, "{msg}");
    }
}
