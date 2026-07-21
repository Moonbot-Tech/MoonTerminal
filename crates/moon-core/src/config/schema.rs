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
use super::servers::{self, CoreSortMode, FeedFlags};
use crate::market::MarketDataMode;

/// Current `settings.toml` schema version.
///
/// Increment when persisted fields require a serde-default backfill save.
///
/// v15 retired the `"manual"` core-sort mode. The bump earns its keep twice: the backfill save
/// rewrites that dead code to the new default instead of leaving a meaningless token on disk,
/// and it makes `config::backup` take one snapshot for every existing user — captured before the
/// default-order change reshuffles their lists, so there is something to roll back to.
pub const SCHEMA_VERSION: u32 = 15;

/// Версия схемы, начиная с которой рантайм-`CoreId == uid` (стабильный). Конфиги
/// старее неё хранили в `charts.json` ПОЗИЦИОННЫЕ CoreId — их надо один раз
/// перепривязать к uid. Фиксированная (НЕ `SCHEMA_VERSION`), чтобы будущие bump'ы
/// схемы не запускали ремап повторно. См. `reconcile::merge`, `chart_persist::remap_core_ids`.
pub const COREID_UID_VERSION: u32 = 11;

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiThemeMode {
    Light,
    #[default]
    Dark,
}

/// Запись сервера в servers.enc (секрет + стабильный uid).
///
/// host/port НЕ храним: они зашиты в самом ключе Moonbot (см. `parse_key_info` в
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
    /// Последний выбранный пресет размера (индекс 0..=5), см. `ServerConfig::order_size_sel`.
    #[serde(default)]
    pub order_size_sel: Option<usize>,
    /// Стратегия алертов по умолчанию (id вида «Alerts»), см. `ServerConfig::default_alert_strategy`.
    #[serde(default)]
    pub default_alert_strategy: u64,
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
    /// false = по всей области графика. Дефолт true.
    #[serde(default = "servers::default_true")]
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
    /// Тёмная/светлая тема MoonUI. Открытая настройка: это не секрет и не chart theme.
    #[serde(default)]
    pub ui_theme_mode: UiThemeMode,
    /// Общий масштаб геометрии UI. Пока без публичной ручки, но хранится рядом с
    /// font_delta, чтобы компонентная тема имела один источник правды.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// Множитель бюджета retained chart history относительно RAM-based базы.
    /// 100 = авто-база, 800 = 8x, как Delphi UseMemForCharts.
    #[serde(default = "default_chart_memory_percent")]
    pub chart_memory_percent: u16,
    /// Legacy (schema < v13): хоткеи жили секцией здесь, теперь — в отдельном
    /// переносимом `hotkeys.toml`. Читаем для одноразовой миграции, не пишем.
    #[serde(default, skip_serializing)]
    pub hotkeys: HotkeysConfig,
    #[serde(default)]
    pub groups: Vec<GroupConfig>,
    /// How core lists are ordered app-wide; missing values default to `Name`.
    #[serde(default)]
    pub core_sort: CoreSortMode,
    /// Next uid to issue, persisted so deleted identities are not reused.
    ///
    /// Zero falls back to one past the highest surviving uid. This field is the only durable
    /// record of deleted high-water marks, so losing it also loses that history.
    #[serde(default)]
    pub next_uid: u64,
    #[serde(default)]
    pub servers: Vec<ServerMeta>,
}
