//! Связка файловых форматов (`schema`) с рантайм-`AppConfig` в обе стороны.
//!
//! Ключ привязки меты к серверу — стабильный `uid`. Для старых файлов без uid
//! один раз привязываемся по `name` и тут же проставляем свежий uid: после этого
//! переименование сервера больше НЕ теряет его галки (привязка идёт по uid).

use super::groups::GroupConfig;
use super::hotkeys::HotkeysConfig;
use super::lang::Language;
use super::schema::{
    clamp_chart_memory_percent, clamp_chart_stack_height, repair_ui_font_delta, repair_ui_scale,
    ServerEntry, ServerMeta, ServersFile, SettingsFile, UiThemeMode, COREID_UID_VERSION,
    SCHEMA_VERSION,
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
    /// Legacy-хоткеи из settings.toml (schema < v13) — только для одноразовой
    /// миграции в `hotkeys.toml`; при существующем hotkeys.toml игнорируются.
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
    let ui_font_delta = repair_ui_font_delta(meta.ui_font_delta);
    let ui_theme_mode = meta.ui_theme_mode;
    let ui_scale = repair_ui_scale(meta.ui_scale);
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
                order_size_sel: m.and_then(|m| m.order_size_sel),
                default_alert_strategy: m.map(|m| m.default_alert_strategy).unwrap_or(0),
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
        // Legacy-поле: с v13 живёт в hotkeys.toml, в settings.toml не сериализуется.
        hotkeys: HotkeysConfig::default(),
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
                order_size_sel: s.order_size_sel,
                default_alert_strategy: s.default_alert_strategy,
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

#[cfg(test)]
/// Repairs `merge` must apply to values coming off disk.
mod tests {
    use super::super::schema::{
        default_ui_font_delta, default_ui_scale, ServersFile, SettingsFile,
    };
    use super::{merge, Merged};

    /// Merge a settings file carrying nothing but the two scaling knobs.
    fn merged_with(ui_scale: f32, ui_font_delta: f32) -> Merged {
        merge(
            ServersFile::default(),
            SettingsFile {
                ui_scale,
                ui_font_delta,
                ..Default::default()
            },
        )
    }

    /// Pins the `repair_ui_scale` CALL inside [`merge`], not the repair itself — a pure repair
    /// function nobody invokes is exactly how this regresses. The plausible edit: someone reads
    /// `ui_scale` back as a plain passthrough, matching its unrepaired neighbours on either side.
    ///
    /// A stored `ui_scale = 0.0` is not hypothetical — it is what every `settings.toml` written
    /// before the loader applied schema defaults contains. `MoonThemeTokens::ui` floors the factor
    /// at `0.25`, so honouring the zero renders the whole interface at a quarter size: text still
    /// paints, every hit rectangle shrinks past the point where clicks land, and the Settings
    /// screen the user would repair it from is itself unusable. Loading has to fix it.
    #[test]
    fn a_degenerate_stored_ui_scale_is_repaired_on_load() {
        for broken in [0.0_f32, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                merged_with(broken, 0.0).ui_scale,
                default_ui_scale(),
                "a scale of {broken} cannot mean anything; loading must repair it, not pass it on"
            );
        }
    }

    /// The other half of the contract, and the half that is easy to break while "hardening" the
    /// first: repair must not become a range clamp.
    ///
    /// `ui_scale` has no settings-UI control, so hand-editing `settings.toml` is the only way to
    /// set it — and the loaded value is written straight back by the next `save()`. A clamp would
    /// therefore not just ignore an unusual choice, it would DESTROY it on disk, with nothing in
    /// the UI to restore it from. `0.25` is MoonUI's own floor in `MoonThemeTokens::ui` and `6.0`
    /// is far past any preset; both are usable, so both must survive untouched.
    #[test]
    fn an_unusual_but_usable_scale_survives_the_load() {
        for kept in [0.25_f32, 0.4, 6.0, 10.0] {
            assert_eq!(
                merged_with(kept, 0.0).ui_scale,
                kept,
                "a usable scale of {kept} must load verbatim; repair is not a clamp"
            );
        }
    }

    /// `ui_font_delta` splits the other way from `ui_scale`: `0.0` means "no adjustment" and is a
    /// real choice, while a non-finite value is not — TOML parses `inf`/`nan`, and MoonUI adds
    /// this delta directly into text metrics, where an infinity spreads into layout dimensions.
    #[test]
    fn a_non_finite_font_delta_is_repaired_while_zero_is_kept() {
        for broken in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            assert_eq!(
                merged_with(1.0, broken).ui_font_delta,
                default_ui_font_delta(),
                "a font delta of {broken} reaches MoonUI text metrics; it must be repaired"
            );
        }
        assert_eq!(
            merged_with(1.0, 0.0).ui_font_delta,
            0.0,
            "zero font delta is 'no adjustment', a legitimate choice — it must NOT be repaired"
        );
    }
}
