//! Связка файловых форматов (`schema`) с рантайм-`AppConfig` в обе стороны.
//!
//! Ключ привязки меты к серверу — стабильный `uid`. Для старых файлов без uid
//! один раз привязываемся по `name` и тут же проставляем свежий uid: после этого
//! переименование сервера больше НЕ теряет его галки (привязка идёт по uid).

use super::groups::GroupConfig;
use super::hotkeys::HotkeysConfig;
use super::lang::Language;
use super::schema::{
    clamp_chart_memory_percent, clamp_chart_stack_height, ServerEntry, ServerMeta, ServersFile,
    SettingsFile, UiThemeMode, COREID_UID_VERSION, SCHEMA_VERSION,
};
use super::servers;
use super::ServerConfig;
use crate::market::MarketDataMode;

/// Результат слияния двух файлов в рантайм.
pub struct Merged {
    pub servers: Vec<ServerConfig>,
    pub groups: Vec<GroupConfig>,
    /// Язык интерфейса из settings.toml (или системный дефолт).
    pub language: Language,
    /// Источник рыночных данных из settings.toml (или дефолт Dedup).
    pub market_mode: MarketDataMode,
    /// Отдельная чарт-вкладка на ядро (AddToChart).
    pub charts_split_by_core: bool,
    /// AddToChart-стек: вертикальный скролл (true) / делить высоту окна (false).
    pub charts_stack_scroll: bool,
    /// Скролл-стек: сжимать по заполнению (без скролла).
    pub charts_stack_compress: bool,
    /// Скролл-стек: высота одного графика (лог. px).
    pub chart_stack_height: u16,
    /// Раздельные зоны управления (ордера/линии только в зоне стакана).
    pub separate_control_zones: bool,
    /// Авто-закрытие графиков Main при неактивности окна, сек (0 = выключено).
    pub main_idle_close_secs: u32,
    /// Писать лог в файлы logs/.
    pub log_to_file: bool,
    /// Срок хранения файлов лога (дней; 0 = хранить всё).
    pub log_retention_days: u32,
    /// Прибавка к базовым размерам UI-шрифтов в logical px.
    pub ui_font_delta: f32,
    /// Тёмная/светлая тема MoonUI.
    pub ui_theme_mode: UiThemeMode,
    /// Общий масштаб геометрии UI.
    pub ui_scale: f32,
    /// Множитель бюджета retained chart history.
    pub chart_memory_percent: u16,
    /// Горячие клавиши терминала.
    pub hotkeys: HotkeysConfig,
    /// Нужно пере-сохранить на диск: присвоены новые uid и/или версия схемы
    /// устарела (надо дослоить дефолты новых полей в settings.toml).
    pub dirty: bool,
    /// Конфиг был версии < `COREID_UID_VERSION` → `charts.json` хранит ПОЗИЦИОННЫЕ
    /// CoreId, их надо один раз перепривязать к стабильным uid (делает UI на старте,
    /// т.к. формат `charts.json` живёт в UI-крейте). Одноразово: после досейва версия
    /// поднимется и флаг больше не взведётся.
    pub chart_core_remap_needed: bool,
}

/// servers.enc + settings.toml → рантайм-серверы. Привязка меты по uid,
/// с одноразовым fallback на имя для старых файлов без uid.
pub fn merge(sf: ServersFile, meta: SettingsFile) -> Merged {
    let mut next_uid = next_free_uid(&sf, &meta);
    let mut dirty = meta.version < SCHEMA_VERSION;
    // До v11 рантайм-CoreId был позиционным → charts.json хранит позиционные id.
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
    let ui_font_delta = meta.ui_font_delta;
    let ui_theme_mode = meta.ui_theme_mode;
    let ui_scale = meta.ui_scale;
    let chart_memory_percent = clamp_chart_memory_percent(meta.chart_memory_percent);
    let hotkeys = meta.hotkeys;

    let servers = sf
        .servers
        .into_iter()
        .map(|e| {
            // Привязка меты: по uid, иначе (старый файл) по имени.
            let m = if e.uid != 0 {
                meta.servers.iter().find(|m| m.uid == e.uid)
            } else {
                meta.servers.iter().find(|m| m.name == e.name)
            };
            // Стабильный uid: из файла либо свежий (тогда конфиг «грязный» → досейв).
            let uid = if e.uid != 0 {
                e.uid
            } else {
                dirty = true;
                let u = next_uid;
                next_uid += 1;
                u
            };
            ServerConfig {
                // Рантайм-CoreId = стабильный uid (НЕ позиция): переживает добавление/
                // удаление/перепорядок серверов, поэтому окна/подписки/раскладку не
                // приходится пересоздавать при изменении набора ядер.
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
        hotkeys,
        dirty,
        chart_core_remap_needed,
    }
}

/// Рантайм-`AppConfig` → два файловых формата (для записи).
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
    hotkeys: HotkeysConfig,
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
        hotkeys,
        groups: groups.to_vec(),
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
            })
            .collect(),
    };
    (sf, meta)
}

/// Проставить стабильный uid всем серверам без него (uid == 0). Вызывается перед
/// записью, чтобы у только что добавленных в UI ядер сразу был стабильный id.
pub fn ensure_uids(servers: &mut [ServerConfig]) {
    let mut next = servers.iter().map(|s| s.uid).max().unwrap_or(0) + 1;
    for s in servers.iter_mut() {
        if s.uid == 0 {
            s.uid = next;
            next += 1;
        }
    }
}

/// Первый свободный uid с учётом обоих файлов (чтобы не выдать занятый).
fn next_free_uid(sf: &ServersFile, meta: &SettingsFile) -> u64 {
    let from_entries = sf.servers.iter().map(|e| e.uid).max().unwrap_or(0);
    let from_meta = meta.servers.iter().map(|m| m.uid).max().unwrap_or(0);
    from_entries.max(from_meta) + 1
}
