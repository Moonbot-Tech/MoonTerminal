//! Низкоуровневое чтение/запись файлов конфига: шифрование servers.enc и
//! открытый settings.toml. Тут НЕТ доменной логики (слияние/uid — в `reconcile`).
//!
//! Важно: битый settings.toml не теряем молча — уводим в `.bak` и продолжаем с
//! дефолта. Иначе одно неверное значение обнулило бы все группы и галки серверов.

use std::path::Path;

use anyhow::Context;

use super::crypto;
use super::paths;
use super::schema::{ServersFile, SettingsFile};
use super::toml_io::ConfigLoad;

/// Расшифровать и разобрать servers.enc. Ошибка чтения/дешифровки — фатальна
/// (это секреты пользователя, молча терять нельзя).
pub fn read_servers() -> anyhow::Result<ServersFile> {
    let bytes = std::fs::read(paths::servers_path()).context("чтение servers.enc")?;
    let plain = crypto::decrypt(&bytes)?;
    let sf = toml::from_str(std::str::from_utf8(&plain)?).context("разбор servers.enc")?;
    Ok(sf)
}

/// Зашифровать и записать servers.enc.
pub fn write_servers(sf: &ServersFile) -> anyhow::Result<()> {
    let enc = crypto::encrypt(toml::to_string(sf)?.as_bytes())?;
    super::toml_io::write_atomic(&paths::servers_path(), &enc, "servers.enc")
        .context("запись servers.enc")?;
    Ok(())
}

/// Прочитать settings.toml вместе со СТАТУСОМ чтения. Нет файла → дефолт (первый
/// запуск). Битый файл → увести в `.bak`, залогировать и вернуть дефолт (данные не
/// теряются молча). Нечитаемый файл → дефолт + [`ConfigLoad::Unreadable`].
///
/// Статус обязателен именно здесь: `AppConfig::load` автоматически пере-сохраняет
/// конфиг, когда версия схемы устарела, и без этого различия одна временная ошибка
/// чтения приводила бы к перезаписи живого конфига дефолтами.
pub fn read_settings() -> (SettingsFile, ConfigLoad) {
    super::toml_io::load_or_default_status(&paths::settings_path(), "settings.toml", backup_corrupt)
}

/// Записать settings.toml (открытый, человекочитаемый TOML, без секретов).
pub fn write_settings(sf: &SettingsFile) -> anyhow::Result<()> {
    super::toml_io::save(&paths::settings_path(), sf, "settings.toml")
}

/// Переименовать битый settings.toml → settings.toml.bak (не затираем молча).
fn backup_corrupt(path: &Path) {
    let bak = path.with_extension("toml.bak");
    if let Err(e) = std::fs::rename(path, &bak) {
        log::warn!("не удалось увести битый settings.toml в .bak: {e}");
    }
}
