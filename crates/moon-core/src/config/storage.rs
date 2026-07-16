//! Настройки локального хранилища — `cfg/storage.toml`.
//!
//! Один файл на все БД (`data/*.sqlite`): UI-часть (вкладка «Хранилище» Настроек)
//! пишет свои ключи сюда же, системная часть (игнор-лист версий и пр.) правится
//! только руками. Нет файла → при первом чтении записываем дефолты, чтобы
//! системные ключи были видны и редактируемы без чтения исходников.

use serde::{Deserialize, Serialize};

use super::{paths, toml_io};

/// Поля стратегии, изменение которых НЕ создаёт новую версию (косметика/статус/
/// оформление). Перенесено из mb_ai (проверено там годом эксплуатации), плюс:
/// - `PreventWorkingUntil`: sgStop/sgStart — состояние, не правка параметров
///   (трекается в head.checked), иначе каждый стоп плодил бы версию;
/// - `OrderSize`: размер ордера крутится рутинно (в т.ч. хоткеями), версия
///   параметров от него не нужна (решение 2026-07-16).
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

/// Секция `[strategies]` — локальная БД стратегий и версий.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategiesStoreCfg {
    /// Вести локальную БД стратегий (head + версии). Выключение останавливает
    /// запись; уже накопленная история остаётся на диске.
    pub enabled: bool,
    /// Максимум версий на стратегию (0 = без лимита). Старые обрезаются.
    pub version_limit: u32,
    /// Поля, изменение которых не создаёт версию. Системный ключ: в UI не
    /// выносится, правится руками (случайно выкинутое поле = молча потерянная
    /// история правок).
    pub ignore_fields: Vec<String>,
}

impl Default for StrategiesStoreCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            version_limit: 0,
            ignore_fields: DEFAULT_IGNORE_FIELDS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Прочитать `storage.toml`; при отсутствии файла — записать дефолты (чтобы
/// системные ключи были на виду). Битый файл → лог + дефолт (не перетираем).
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

/// Сохранить настройки хранилища (вызывает вкладка «Хранилище»).
pub fn save(cfg: &StorageCfg) {
    if let Err(e) = toml_io::save(&paths::storage_path(), cfg, "storage.toml") {
        log::warn!("storage.toml: сохранение не удалось: {e:#}");
    }
}
