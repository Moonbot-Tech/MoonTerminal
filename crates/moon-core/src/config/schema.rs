//! Форматы файлов конфига на диске (serde). Здесь — ТОЛЬКО структуры данных:
//! без чтения/записи (см. `store`) и без слияния с рантаймом (см. `reconcile`).
//!
//! Forward-compat: каждое новое поле помечаем `#[serde(default = …)]`, тогда старый
//! файл без него читается без ошибки (поле получает дефолт), а `version` ниже
//! позволяет один раз дослоить эти дефолты обратно на диск (см. `AppConfig::load`).

use serde::{Deserialize, Serialize};

use super::groups::GroupConfig;
use super::hotkeys::HotkeysConfig;
use super::lang::Language;
use super::secrets::Secret;
use super::servers::{self, FeedFlags};
use crate::market::MarketDataMode;

/// Текущая версия схемы settings.toml. Поднимай на +1, когда добавил новое поле
/// и хочешь, чтобы старые файлы один раз пере-сохранились с его дефолтом.
/// v2: добавлено поле `language`. v3: добавлено `market_mode`.
/// v4: добавлено `charts_split_by_core`. v5: добавлены `log_to_file` + `log_retention_days`.
/// v6: добавлены `ui_font_delta` + `ui_scale`.
/// v7: добавлен `chart_memory_percent`. v8: добавлен per-server `chart_bundle`.
/// v9: добавлен `charts_stack_scroll`. v10: добавлен блок `hotkeys`.
pub const SCHEMA_VERSION: u32 = 10;

/// Старые файлы без поля `version` читаются как 0 → меньше SCHEMA_VERSION →
/// триггерят досейв с дослоением новых дефолтов.
pub fn default_version() -> u32 {
    0
}

pub fn default_ui_font_delta() -> f32 {
    2.0
}

pub fn default_ui_scale() -> f32 {
    1.0
}

pub fn default_chart_memory_percent() -> u16 {
    100
}

pub fn clamp_chart_memory_percent(value: u16) -> u16 {
    value.clamp(100, 800)
}

pub fn default_chart_stack_height() -> u16 {
    360
}

pub fn clamp_chart_stack_height(value: u16) -> u16 {
    value.clamp(120, 2000)
}

/// Запись сервера в servers.enc (секрет + стабильный uid).
///
/// host/port НЕ храним: они зашиты в самом ключе MoonBot (см. `parse_key_info` в
/// feed/live.rs). Старые servers.enc с полями host/port читаются без ошибки —
/// неизвестные поля serde просто игнорирует, подключение пойдёт по ключу.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerEntry {
    /// Стабильный идентификатор ядра (см. `ServerConfig::uid`). 0 в старых файлах →
    /// присваивается при первой загрузке (см. `reconcile::merge`).
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

/// По-серверная мета в settings.toml (открытая, без секретов).
/// Привязка к серверу — по `uid` (стабильно); для старых файлов без uid
/// один раз привязываемся по `name` (см. `reconcile::merge`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerMeta {
    #[serde(default)]
    pub uid: u64,
    /// Дублируется из servers.enc — для читаемости открытого файла и legacy-привязки.
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
    /// Имя чарт-связки AddToChart (см. `ServerConfig::chart_bundle`). Пусто = по
    /// глобальной настройке. Старые файлы → пустая строка (дефолт).
    #[serde(default)]
    pub chart_bundle: String,
    /// 6 пресетов размера ручного ордера (F1-F6) в базовой монете. `None`/старые файлы →
    /// дефолт по базе ядра (см. `ServerConfig::order_sizes`).
    #[serde(default)]
    pub order_sizes: Option<[f64; 6]>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct SettingsFile {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Язык интерфейса. Отсутствует в старых файлах → serde-дефолт = системная локаль.
    #[serde(default)]
    pub language: Language,
    /// Источник рыночных данных (дедуп по провайдеру / по ядрам). Старые файлы → дефолт.
    #[serde(default)]
    pub market_mode: MarketDataMode,
    /// Отдельная чарт-вкладка на каждое ядро (AddToChart): true = 1-HL-ядро,
    /// false = все ядра в одной вкладке 1-HL. Старые файлы → дефолт true.
    #[serde(default = "servers::default_true")]
    pub charts_split_by_core: bool,
    /// AddToChart-вкладка с несколькими графиками: true = вертикальный скролл (фикс. высота
    /// каждого графика), false = делить высоту окна (как раньше — масштаб по вертикали).
    /// Старые файлы → дефолт false.
    #[serde(default)]
    pub charts_stack_scroll: bool,
    /// Скролл-режим: сжимать по заполнению — скролл не появляется, графики рисуются заданной
    /// высоты, пока не упрутся в конец окна, затем сжимаются (как без скролла). Дефолт false.
    #[serde(default)]
    pub charts_stack_compress: bool,
    /// Скролл-режим: высота одного графика в логических px. Дефолт 360.
    #[serde(default = "default_chart_stack_height")]
    pub chart_stack_height: u16,
    /// Раздельные зоны управления: true = ставить ордера и двигать линии ТОЛЬКО в зоне стакана;
    /// false = по всей области графика. Дефолт false.
    #[serde(default)]
    pub separate_control_zones: bool,
    /// Авто-закрытие графиков Main при неактивности окна, сек. 0 = выключено. Дефолт 0.
    #[serde(default)]
    pub main_idle_close_secs: u32,
    /// Писать лог (приложения и ядер) в файлы logs/<дата>_<источник>.log. Дефолт on.
    #[serde(default = "servers::default_true")]
    pub log_to_file: bool,
    /// Сколько дней хранить файлы лога; старее — удаляются. 0 = хранить всё. Дефолт 14.
    #[serde(default = "servers::default_log_retention_days")]
    pub log_retention_days: u32,
    /// Прибавка к базовым размерам UI-шрифтов в logical px. Дефолт +2: на 1x
    /// дизайнерский 10px текст становится 12px, без полного zoom интерфейса.
    #[serde(default = "default_ui_font_delta")]
    pub ui_font_delta: f32,
    /// Общий масштаб геометрии UI. Пока без публичной ручки, но хранится рядом с
    /// font_delta, чтобы компонентная тема имела один источник правды.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// Множитель бюджета retained chart history относительно RAM-based базы.
    /// 100 = авто-база, 800 = 8x, как Delphi UseMemForCharts.
    #[serde(default = "default_chart_memory_percent")]
    pub chart_memory_percent: u16,
    /// Горячие клавиши терминала. Открытый формат, без секретов.
    #[serde(default)]
    pub hotkeys: HotkeysConfig,
    #[serde(default)]
    pub groups: Vec<GroupConfig>,
    #[serde(default)]
    pub servers: Vec<ServerMeta>,
}
