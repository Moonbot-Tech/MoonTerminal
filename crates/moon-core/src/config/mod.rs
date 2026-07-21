//! Конфиг приложения в ДВУХ файлах рядом с exe:
//! - `servers.enc` (зашифрован): uid/name/key — переносимый секрет (скопировал
//!   файл — и ключи на месте). host/port/transport зашиты в самом ключе Moonbot.
//! - `settings.toml` (открытый): версия схемы + группы + по-серверная мета
//!   (галки active/show_window/feed, группа, рынок, цвет). Привязка к серверу — по uid.
//!
//! Обновление версии программы: старый settings.toml без новых полей читается
//! без потерь (serde-дефолты), а `version` < `SCHEMA_VERSION` запускает один
//! досейв — новые галки дописываются в файл с дефолтами, старые сохраняются.
//!
//! Раскладка по модулям (не валим всё в один файл):
//! - `schema`    — структуры файлов на диске (serde) + версия схемы;
//! - `store`     — чтение/запись файлов (шифрование, бэкап битого settings.toml);
//! - `reconcile` — слияние файлов ↔ рантайм + стабильные uid;
//! - `migrate`   — one-time migrations from legacy formats;
//! - `backup`    — snapshots of both files in `backups/` before migration and saving.

pub mod badges;
pub mod crypto;
pub mod detect_view;
pub mod groups;
pub mod hotkeys;
pub mod lang;
pub mod layout;
pub mod moonbot_import;
pub mod orders;
pub mod paths;
pub mod secrets;
pub mod servers;
pub mod storage;
pub mod theme;

mod backup;
mod migrate;
mod reconcile;
mod schema;
mod store;
mod toml_io;

pub use badges::{BadgeEntry, BadgesConfig};
pub use detect_view::{
    detect_slot_count, DetectChart, DetectField, DetectSizeCfg, DetectSlot, DetectViewCfg,
    DetectViewFile, DETECT_RAIL_MAX, DETECT_SIZE_LARGE, DETECT_SIZE_MEDIUM, DETECT_SIZE_MINI,
};
pub use groups::GroupConfig;
pub use hotkeys::{
    HotkeysConfig, MouseGestureBinding, MANUAL_STRATEGY_KEYS, ORDER_SIZE_KEYS, SELL_PRESET_KEYS,
};
pub use lang::Language;
pub use layout::{DetachedLayout, GeomRect, GroupLayout, WindowLayout};
pub use orders::{LineStyle, OrdersStyle, OrdersStyleSet};
pub use schema::UiThemeMode;
pub use secrets::Secret;
pub use servers::{ChartBucket, CoreSortMode, FeedFlags, ServerConfig};
pub use theme::{ChartTheme, ChartThemeSet};

use std::collections::HashSet;
use std::path::Path;

use crate::market::MarketDataMode;

/// Атомарная запись пользовательского файла конфигурации/раскладки: временный sibling + rename.
/// Используется и для TOML, и для JSON-персиста UI.
pub fn write_file_atomic(path: &Path, bytes: &[u8], label: &str) -> anyhow::Result<()> {
    toml_io::write_atomic(path, bytes, label)
}

/// Outcome of the snapshot accompanying a save.
///
/// Kept separate from `Result`: snapshot failure does NOT cancel the config write, but it must not
/// disappear either, because reporting success without a rollback copy would mislead the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// A copy was taken, or nothing existed to copy on first launch.
    Ok,
    /// The copy failed. The config was written, but no rollback is available.
    Failed,
}

/// Intent of a config write: a routine drain or a deliberate save.
///
/// Named after PURPOSE rather than mechanism ("whether to take a snapshot"): snapshots are the
/// current consequence of this distinction, not its essence. The core cannot infer intent, so the
/// caller supplies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveKind {
    /// Background write from the `config_dirty` drain, application exit, or a header edit.
    Routine,
    /// A deliberate user save for which rollback may be needed.
    Deliberate,
}

/// Рантайм-конфиг (смерженный из двух файлов).
#[derive(Clone, Debug, Default)]
pub struct AppConfig {
    pub servers: Vec<ServerConfig>,
    pub groups: Vec<GroupConfig>,
    /// Язык интерфейса (settings.toml). Дефолт — системная локаль.
    pub language: Language,
    /// Источник рыночных данных (settings.toml). Дефолт — Dedup (провайдер на биржу).
    pub market_mode: MarketDataMode,
    /// Отдельная чарт-вкладка на каждое ядро для AddToChart (settings.toml).
    pub charts_split_by_core: bool,
    /// AddToChart-стек: вертикальный скролл (true) / делить высоту окна (false, как раньше).
    pub charts_stack_scroll: bool,
    /// Скролл-стек: сжимать по заполнению (скролл не появляется). Дефолт false.
    pub charts_stack_compress: bool,
    /// Скролл-стек: высота одного графика (лог. px). Дефолт 360.
    pub chart_stack_height: u16,
    /// Раздельные зоны управления: ордера/линии только в зоне стакана (settings.toml). Дефолт true.
    pub separate_control_zones: bool,
    /// Авто-закрытие графиков Main при неактивности окна, сек (settings.toml). 0 = выключено.
    /// Неактивность = окно не в фокусе ЛИБО в фокусе, но мышь не двигается. Каждый график
    /// закрывается через N сек своей неактивности (новейший — последним), фулскрин тоже.
    pub main_idle_close_secs: u32,
    /// Писать лог (приложения и ядер) в файлы logs/ (settings.toml). Дефолт on.
    pub log_to_file: bool,
    /// Срок хранения файлов лога, дней; 0 = хранить всё (settings.toml). Дефолт 14.
    pub log_retention_days: u32,
    /// Прибавка к базовым размерам UI-шрифтов в logical px. Дефолт +2.
    pub ui_font_delta: f32,
    /// Тёмная/светлая тема MoonUI (settings.toml, открытый формат).
    pub ui_theme_mode: UiThemeMode,
    /// Общий масштаб геометрии UI. Дефолт 1.0.
    pub ui_scale: f32,
    /// Множитель RAM-budget для retained market history. 100 = авто-база, 800 = 8x.
    pub chart_memory_percent: u16,
    /// Order of every core list in the application (`settings.toml`). Defaults to `Name`.
    pub core_sort: CoreSortMode,
    /// Upper bound for the next uid from `SettingsFile::next_uid`.
    pub next_uid: u64,
    /// Горячие клавиши терминала (settings.toml, открытый формат).
    pub hotkeys: HotkeysConfig,
    /// Тема оформления чарта per-режим UI (тёмная/светлая) — отдельный переносимый theme.toml.
    pub theme: ChartThemeSet,
    /// Стили линий ордеров per-theme (тёмная/светлая) — отдельный переносимый orders.toml.
    pub orders: OrdersStyleSet,
    /// Бейджи типов детектов (код+цвета по видам, на тему) — отдельный переносимый badges.json.
    pub badges: BadgesConfig,
    /// Runtime flag (NOT serialized): `settings.toml` EXISTS but could not be read because of
    /// permissions, a share, or an unhydrated cloud placeholder, so memory holds DEFAULTS rather
    /// than the user's settings.
    ///
    /// While set, EVERY config write is forbidden; see `save_impl`. Suppressing only the automatic
    /// startup save is insufficient: the routine `config_dirty` drain from the 100-ms loop, the
    /// application-exit write, and the Save button in Settings are three independent paths, each of
    /// which could turn a temporary read failure into irreversible replacement of the live config
    /// with defaults.
    pub settings_unreadable: bool,
    /// Рантайм-флаг (НЕ сериализуется): конфиг загружен из версии < `COREID_UID_VERSION`,
    /// где `charts.json` хранил позиционные CoreId. UI на старте один раз перепривяжет их
    /// к стабильным uid (см. `chart_persist::remap_core_ids`). Дефолт false.
    pub chart_core_remap_needed: bool,
}

impl AppConfig {
    /// Load and merge server secrets, settings, and separate UI config files.
    ///
    /// The `settings.toml` read status is computed before choosing a load branch so every
    /// construction path sets `settings_unreadable` and blocks an unsafe write-back.
    pub fn load() -> anyhow::Result<Self> {
        // macOS/Linux: при первом запуске после переезда хранилища перенести
        // конфиги из бандла (рядом с exe) в пользовательскую директорию данных.
        // На Windows это no-op (data_dir == exe_dir).
        paths::migrate_bundle_data();
        // Настройки/раскладка переехали из корня данных в подпапку `cfg/` — один раз
        // переносим старые плоские файлы (на всех платформах, после bundle-миграции).
        paths::migrate_flat_to_cfg();
        // Тема и стиль линий ордеров — отдельные переносимые файлы, грузятся
        // независимо от серверов/групп.
        let theme = ChartThemeSet::load();
        let orders = OrdersStyleSet::load();
        let badges = BadgesConfig::load();
        // Хоткеи — отдельный переносимый файл (с v13); None = ещё не мигрировали.
        let hotkeys_file = HotkeysConfig::load();
        if let Some(cfg) = Self::load_plaintext_env(theme.clone(), orders.clone(), badges.clone())?
        {
            return Ok(cfg);
        }
        // Read ONCE and BEFORE branch selection: the settings.toml status decides whether ANY
        // branch below may write to disk, not only the primary branch. Reading it only inside the
        // `servers.enc` branch misses an absent servers.enc paired with an unreadable settings.toml
        // after partial sync, manual deletion, or restoration. Migration and fresh-config branches
        // would then leave the flag clear, allowing the next Settings save to replace unread
        // settings with defaults.
        let (meta, meta_load) = store::read_settings();
        let settings_unreadable = !meta_load.permits_overwrite();

        if paths::servers_path().exists() {
            let sf = store::read_servers()?;
            // Capture this BEFORE `merge` consumes `meta`, avoiding an extra `Merged` field for a
            // value already known here.
            let schema_upgrade = meta.version < schema::SCHEMA_VERSION;
            // Snapshot BEFORE any write on this path: the save below replaces both files by atomic
            // rename, after which the pre-migration bytes cannot be recovered.
            if schema_upgrade {
                backup::snapshot(backup::Trigger::SchemaMigration);
            }
            let merged = reconcile::merge(sf, meta);
            // hotkeys.toml приоритетен; нет файла → одноразовая миграция legacy-секции
            // из settings.toml (или дефолта) на диск в новый файл.
            let hotkeys = hotkeys_file.unwrap_or_else(|| {
                let h = merged.hotkeys.clone();
                if let Err(e) = h.save() {
                    log::warn!("не записал hotkeys.toml при миграции: {e:#}");
                }
                h
            });
            let mut cfg = Self {
                servers: merged.servers,
                groups: merged.groups,
                language: merged.language,
                market_mode: merged.market_mode,
                charts_split_by_core: merged.charts_split_by_core,
                charts_stack_scroll: merged.charts_stack_scroll,
                charts_stack_compress: merged.charts_stack_compress,
                chart_stack_height: merged.chart_stack_height,
                separate_control_zones: merged.separate_control_zones,
                main_idle_close_secs: merged.main_idle_close_secs,
                log_to_file: merged.log_to_file,
                log_retention_days: merged.log_retention_days,
                ui_font_delta: merged.ui_font_delta,
                ui_theme_mode: merged.ui_theme_mode,
                ui_scale: merged.ui_scale,
                chart_memory_percent: merged.chart_memory_percent,
                core_sort: merged.core_sort,
                next_uid: merged.next_uid,
                hotkeys,
                theme,
                orders,
                badges,
                settings_unreadable,
                chart_core_remap_needed: merged.chart_core_remap_needed,
            };
            log::info!(
                "конфиг: {} серверов, {} групп",
                cfg.servers.len(),
                cfg.groups.len()
            );
            // Дослоить новые дефолты / зафиксировать свежие uid на диск.
            // Не фатально: при ошибке продолжаем с тем, что уже в памяти.
            //
            // FAIL CLOSED: `save` itself rejects `settings_unreadable` (see `save_impl`). This
            // explicit log makes the reason visible instead of presenting a mysterious write
            // failure.
            if merged.dirty {
                if cfg.settings_unreadable {
                    log::error!(
                        "settings.toml не прочитался — запись конфига отключена на весь сеанс, \
                         чтобы не затереть настройки дефолтами"
                    );
                } else if let Err(e) = cfg.save() {
                    log::warn!("не удалось дослоить конфиг на диск: {e}");
                }
            }
            return Ok(cfg);
        }

        // Миграции со старых форматов (один раз → save() пишет новые файлы).
        if paths::legacy_enc_path().exists() {
            let mut cfg = migrate::from_legacy_enc()?;
            cfg.theme = theme;
            cfg.orders = orders;
            cfg.badges = badges.clone();
            cfg.charts_split_by_core = true;
            cfg.chart_stack_height = schema::default_chart_stack_height();
            cfg.log_to_file = true;
            cfg.log_retention_days = 14;
            cfg.ui_font_delta = schema::default_ui_font_delta();
            cfg.ui_theme_mode = UiThemeMode::default();
            cfg.ui_scale = schema::default_ui_scale();
            // As in the fresh-config branch, the serde default is `true` while derived `Default` is
            // `false`; `save()` below writes the selected value.
            cfg.separate_control_zones = servers::default_true();
            cfg.chart_memory_percent = schema::default_chart_memory_percent();
            cfg.hotkeys = hotkeys_file.unwrap_or_default();
            cfg.settings_unreadable = settings_unreadable;
            if settings_unreadable {
                log::error!(
                    "settings.toml есть, но не прочитался — миграция из config.enc НЕ записана"
                );
            } else {
                cfg.save()?;
                log::info!("мигрировано из config.enc → servers.enc + settings.toml");
            }
            return Ok(cfg);
        }
        if paths::legacy_toml_path().exists() {
            let mut cfg = migrate::from_legacy_toml()?;
            cfg.theme = theme;
            cfg.orders = orders;
            cfg.badges = badges.clone();
            cfg.charts_split_by_core = true;
            cfg.chart_stack_height = schema::default_chart_stack_height();
            cfg.log_to_file = true;
            cfg.log_retention_days = 14;
            cfg.ui_font_delta = schema::default_ui_font_delta();
            cfg.ui_theme_mode = UiThemeMode::default();
            cfg.ui_scale = schema::default_ui_scale();
            // As in the fresh-config branch, the serde default is `true` while derived `Default` is
            // `false`; `save()` below writes the selected value.
            cfg.separate_control_zones = servers::default_true();
            cfg.chart_memory_percent = schema::default_chart_memory_percent();
            cfg.hotkeys = hotkeys_file.unwrap_or_default();
            cfg.settings_unreadable = settings_unreadable;
            if settings_unreadable {
                log::error!(
                    "settings.toml есть, но не прочитался — миграция из config.toml НЕ записана"
                );
            } else {
                cfg.save()?;
                log::info!("мигрировано из config.toml → servers.enc + settings.toml");
            }
            return Ok(cfg);
        }

        log::warn!("конфиг не найден — добавь сервера в Настройках");
        Ok(Self {
            theme,
            orders,
            badges,
            charts_split_by_core: true, // дефолт — отдельная вкладка на ядро
            chart_stack_height: schema::default_chart_stack_height(),
            log_to_file: true,
            log_retention_days: 14,
            ui_font_delta: schema::default_ui_font_delta(),
            ui_theme_mode: UiThemeMode::default(),
            ui_scale: schema::default_ui_scale(),
            chart_memory_percent: schema::default_chart_memory_percent(),
            hotkeys: hotkeys_file.unwrap_or_default(),
            // Set explicitly instead of inheriting `..Self::default()` because the serde default is
            // `true`. Derived `Default` would return `false`, causing the first Settings save to
            // invert the control zones silently. The other fields introduced with this struct
            // update all have zero defaults.
            separate_control_zones: servers::default_true(),
            // Core files are absent, but settings.toml may still exist and be unreadable; this
            // branch must not overwrite it either.
            settings_unreadable,
            ..Self::default()
        })
    }

    fn load_plaintext_env(
        theme: ChartThemeSet,
        orders: OrdersStyleSet,
        badges: BadgesConfig,
    ) -> anyhow::Result<Option<Self>> {
        if std::env::var_os("MOON_CONFIG_PLAINTEXT").is_none() {
            return Ok(None);
        }

        let key = match std::env::var("MOON_CONFIG_PLAINTEXT_KEY") {
            Ok(key) if !key.trim().is_empty() => key,
            _ => {
                let path = std::env::var("MOON_CONFIG_PLAINTEXT_KEY_FILE").map_err(|_| {
                    anyhow::anyhow!(
                        "MOON_CONFIG_PLAINTEXT=1 задан, но нет MOON_CONFIG_PLAINTEXT_KEY \
                         или MOON_CONFIG_PLAINTEXT_KEY_FILE"
                    )
                })?;
                std::fs::read_to_string(&path).map_err(|e| {
                    anyhow::anyhow!("не прочитал MOON_CONFIG_PLAINTEXT_KEY_FILE {path}: {e}")
                })?
            }
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            anyhow::bail!("MOON_CONFIG_PLAINTEXT key пустой");
        }

        let name = std::env::var("MOON_CONFIG_PLAINTEXT_NAME").unwrap_or_else(|_| "default".into());
        let group = std::env::var("MOON_CONFIG_PLAINTEXT_GROUP")
            .unwrap_or_else(|_| servers::default_group());
        let market = std::env::var("MOON_CONFIG_PLAINTEXT_MARKET")
            .unwrap_or_else(|_| servers::default_market());

        log::warn!(
            "MOON_CONFIG_PLAINTEXT=1: тестовый plaintext-конфиг, servers.enc/keyring пропущены"
        );
        Ok(Some(Self {
            servers: vec![ServerConfig {
                id: 1,
                uid: 1,
                name,
                active: true,
                show_window: true,
                feed: FeedFlags::default(),
                key: Secret::new(key),
                group,
                market,
                color: servers::default_color(),
                synthetic: false,
                chart_bundle: String::new(),
                order_sizes: None,
                order_size_sel: None,
                default_alert_strategy: 0,
            }],
            groups: Vec::new(),
            language: Language::default(),
            market_mode: MarketDataMode::default(),
            charts_split_by_core: true,
            charts_stack_scroll: false,
            charts_stack_compress: false,
            chart_stack_height: schema::default_chart_stack_height(),
            separate_control_zones: true,
            main_idle_close_secs: 0,
            log_to_file: true,
            log_retention_days: servers::default_log_retention_days(),
            ui_font_delta: schema::default_ui_font_delta(),
            ui_theme_mode: UiThemeMode::default(),
            ui_scale: schema::default_ui_scale(),
            chart_memory_percent: schema::default_chart_memory_percent(),
            core_sort: CoreSortMode::default(),
            // The plaintext test config issues uid 1 above, so the counter starts at 2.
            next_uid: 2,
            hotkeys: HotkeysConfig::default(),
            theme,
            orders,
            badges,
            // Plaintext mode never reads settings.toml, so there is nothing to overwrite or damage.
            settings_unreadable: false,
            chart_core_remap_needed: false,
        }))
    }

    /// Сохраняет в два файла. Проставляет стабильные uid, валидирует уникальность
    /// имени и host:port. `&mut self` — т.к. может присвоить uid новым ядрам.
    ///
    /// WITHOUT a snapshot: this path serves the routine `config_dirty` drain (the 100-ms loop,
    /// application exit, and header edits), which fires for small changes and would evict useful
    /// snapshots from retention within minutes. Deliberate saves use [`Self::save_with_snapshot`].
    pub fn save(&mut self) -> anyhow::Result<()> {
        self.save_impl(SaveKind::Routine).map(|_| ())
    }

    /// Like [`Self::save`], but first copies the current files into `backups/`.
    ///
    /// Used for deliberate saves from Settings where the user may need a rollback. The name
    /// describes write BEHAVIOR rather than a UI surface because `moon-core` knows nothing about
    /// windows.
    ///
    /// `Ok(SnapshotOutcome::Failed)` means the config WAS WRITTEN but no rollback copy exists. The
    /// caller must surface this or a success message would falsely promise protection.
    pub fn save_with_snapshot(&mut self) -> anyhow::Result<SnapshotOutcome> {
        self.save_impl(SaveKind::Deliberate)
    }

    /// Shared save implementation; `kind` decides whether to snapshot before writing.
    ///
    /// This is the ONLY config write point, so the write block belongs here. It covers Settings,
    /// the timer drain, the exit write, and migration rather than only one remembered path.
    fn save_impl(&mut self, kind: SaveKind) -> anyhow::Result<SnapshotOutcome> {
        if self.settings_unreadable {
            anyhow::bail!(
                "settings.toml не был прочитан при старте — запись запрещена, чтобы не \
                 заменить настройки дефолтами; перезапустите приложение"
            );
        }
        reconcile::ensure_uids(&mut self.servers, &mut self.next_uid);
        self.prune_orphan_groups();
        self.validate()?;
        let (sf, meta) = reconcile::split(
            &self.servers,
            &self.groups,
            self.language,
            self.market_mode,
            self.charts_split_by_core,
            self.charts_stack_scroll,
            self.charts_stack_compress,
            self.chart_stack_height,
            self.separate_control_zones,
            self.main_idle_close_secs,
            self.log_to_file,
            self.log_retention_days,
            self.ui_font_delta,
            self.ui_theme_mode,
            self.ui_scale,
            self.chart_memory_percent,
            self.core_sort,
            self.next_uid,
        );
        // Snapshot AFTER validation and immediately before the first write. Taking it earlier
        // would consume a retention slot for every rejected save without writing anything: the
        // button is always enabled and duplicate core names fail only in `validate`. Thirty such
        // attempts would be enough to evict a migration snapshot.
        let outcome = match kind {
            SaveKind::Deliberate => backup::snapshot(backup::Trigger::SettingsSave),
            SaveKind::Routine => SnapshotOutcome::Ok,
        };
        store::write_servers(&sf)?;
        store::write_settings(&meta)?;
        // Тема, стиль линий, бейджи детектов и хоткеи — в свои переносимые файлы,
        // независимо от settings.toml.
        self.theme.save()?;
        self.orders.save()?;
        self.badges.save();
        self.hotkeys.save()?;
        Ok(outcome)
    }

    /// Тема чарта активного режима UI (тёмная/светлая — по `ui_theme_mode`).
    pub fn chart_theme(&self) -> &ChartTheme {
        self.theme.get(self.ui_theme_mode == UiThemeMode::Light)
    }

    /// Группа имеет смысл только пока на неё ссылается хоть одно ядро. Сироты
    /// (например, от промежуточных значений при наборе имени) не сохраняем.
    fn prune_orphan_groups(&mut self) {
        let used: HashSet<&str> = self.servers.iter().map(|s| s.group.as_str()).collect();
        self.groups.retain(|g| used.contains(g.name.as_str()));
    }

    /// Проверка уникальности имени сервера и ключа (endpoint теперь внутри ключа,
    /// поэтому одинаковый ключ = одно и то же ядро дважды). Пустые ключи не сравниваем
    /// — это недозаполненные строки в процессе редактирования.
    fn validate(&self) -> anyhow::Result<()> {
        let mut names = HashSet::new();
        let mut keys = HashSet::new();
        for s in &self.servers {
            // core i18n-агностичен: сообщения валидации — простым текстом. Раньше
            // было t!("err.dup_name"/"err.dup_key"); при желании UI перелокализует.
            if !names.insert(s.name.to_lowercase()) {
                anyhow::bail!("duplicate server name: {}", s.name);
            }
            if !s.key.is_empty() && !keys.insert(s.key.expose().to_owned()) {
                anyhow::bail!("duplicate API key (server: {})", s.name);
            }
        }
        Ok(())
    }

    /// Сигнатура «структурной» части конфига: серверы + группы, БЕЗ темы/языка/режима
    /// рынка/хоткеев. По ней App решает, нужен ли при сохранении настроек реконнект к ядрам и
    /// пересоздание окон. Тема меняется живо, язык, режим рынка и хоткеи — без реконнекта,
    /// поэтому их исключаем (нейтрализуем дефолтом).
    pub fn structural_sig(&self) -> String {
        // Связка чарт-вкладок (`chart_bundle`) и пресеты размера ордера (`order_sizes`) —
        // чисто UI/локальные настройки: их смена НЕ требует реконнекта ядер/ребилда сессий
        // (см. apply_settings). Нейтрализуем, чтобы не считать структурными.
        let servers: Vec<ServerConfig> = self
            .servers
            .iter()
            .map(|s| ServerConfig {
                chart_bundle: String::new(),
                order_sizes: None,
                order_size_sel: None,
                default_alert_strategy: 0,
                ..s.clone()
            })
            .collect();
        let (sf, meta) = reconcile::split(
            &servers,
            &self.groups,
            Language::default(),
            MarketDataMode::default(),
            true,  // нейтрализуем: тумблер чартов не влияет на структуру (без ребилда)
            false, // charts_stack_scroll — чисто визуальный, не структурный
            false, // charts_stack_compress — чисто визуальный
            schema::default_chart_stack_height(), // высота стека — не структурная
            false, // separate_control_zones — поведенческий, не структурный
            0,     // main_idle_close_secs — поведенческий, не структурный
            true,  // лог-настройки тоже не структурные (без реконнекта/ребилда)
            14,
            schema::default_ui_font_delta(),
            UiThemeMode::default(),
            schema::default_ui_scale(),
            schema::default_chart_memory_percent(),
            // Sort mode affects presentation only. The server Vec order remains in the signature:
            // it builds `SessionManager::config_order`, which determines a reactivated session's
            // position and therefore affects the session layer.
            CoreSortMode::default(),
            // The uid counter advances on save and does not describe structure by itself.
            0,
        );
        let a = toml::to_string(&sf).unwrap_or_default();
        let b = toml::to_string(&meta).unwrap_or_default();
        format!("{a}\n{b}")
    }

    /// Свойства группы по имени (существующие или дефолт).
    pub fn group(&self, name: &str) -> GroupConfig {
        self.groups
            .iter()
            .find(|g| g.name == name)
            .cloned()
            .unwrap_or_else(|| GroupConfig::new(name))
    }

    /// Настраивали ли уже конфиг: есть ли хоть один сервер с ключом. False = первый
    /// запуск (ещё ничего не вводили) — показываем только окно Настроек.
    pub fn has_keyed_server(&self) -> bool {
        self.servers.iter().any(|s| !s.key.is_empty())
    }
}

#[cfg(test)]
mod structural_sig_tests;
