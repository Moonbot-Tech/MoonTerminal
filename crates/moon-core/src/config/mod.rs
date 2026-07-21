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
//! - `migrate`   — одноразовые миграции со старых форматов;
//! - `backup`    — снимки обоих файлов в `backups/` перед миграцией и сохранением.

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

/// Удался ли снимок, сопровождавший сохранение.
///
/// Отдельно от `Result`: провал снимка НЕ отменяет запись конфига, но и не должен
/// теряться — «Сохранено» без копии для отката вводит пользователя в заблуждение.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// Копия снята, либо копировать было нечего (первый запуск).
    Ok,
    /// Копию снять не удалось. Конфиг записан, отката нет.
    Failed,
}

/// Снимать ли резервную копию конфига перед записью (см. `AppConfig::save_impl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapshotPolicy {
    /// Рутинная запись — снимок не нужен.
    No,
    /// Осознанное сохранение — снять копию перед перезаписью.
    Yes,
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
    /// How every core list in the app is ordered (settings.toml). Default `Name`.
    pub core_sort: CoreSortMode,
    /// Next uid high-water mark loaded from `SettingsFile::next_uid`.
    pub next_uid: u64,
    /// Горячие клавиши терминала (settings.toml, открытый формат).
    pub hotkeys: HotkeysConfig,
    /// Тема оформления чарта per-режим UI (тёмная/светлая) — отдельный переносимый theme.toml.
    pub theme: ChartThemeSet,
    /// Стили линий ордеров per-theme (тёмная/светлая) — отдельный переносимый orders.toml.
    pub orders: OrdersStyleSet,
    /// Бейджи типов детектов (код+цвета по видам, на тему) — отдельный переносимый badges.json.
    pub badges: BadgesConfig,
    /// Рантайм-флаг (НЕ сериализуется): `settings.toml` СУЩЕСТВУЕТ, но прочитать его не
    /// удалось (права, шара, невыгруженный облачный плейсхолдер), поэтому в памяти лежат
    /// ДЕФОЛТЫ, а не настройки пользователя.
    ///
    /// Пока флаг взведён, ЛЮБАЯ запись конфига запрещена — см. `save_impl`. Гасить только
    /// автоматический досейв на старте недостаточно: рутинный слив `config_dirty` из 100-мс
    /// цикла, запись на выходе из приложения и кнопка «Сохранить» в Настройках — три
    /// независимых пути, каждый из которых так же превратил бы временную ошибку чтения в
    /// безвозвратную замену живого конфига дефолтами.
    pub settings_unreadable: bool,
    /// Рантайм-флаг (НЕ сериализуется): конфиг загружен из версии < `COREID_UID_VERSION`,
    /// где `charts.json` хранил позиционные CoreId. UI на старте один раз перепривяжет их
    /// к стабильным uid (см. `chart_persist::remap_core_ids`). Дефолт false.
    pub chart_core_remap_needed: bool,
}

impl AppConfig {
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
        if paths::servers_path().exists() {
            let sf = store::read_servers()?;
            let (meta, meta_load) = store::read_settings();
            // Взято ДО того, как `merge` поглотит `meta`: `Merged` из-за этого не приходится
            // расширять полем, которое здесь и так уже известно.
            let schema_upgrade = meta.version < schema::SCHEMA_VERSION;
            // Снимок ДО любой записи в этом пути: пере-сохранение ниже заменит оба файла
            // атомарным переименованием, после чего до-миграционных байтов уже не достать.
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
                settings_unreadable: meta_load == toml_io::ConfigLoad::Unreadable,
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
            // FAIL CLOSED: при `settings_unreadable` сам `save` откажет (см. `save_impl`) —
            // здесь только явный лог, чтобы причина была видна в журнале, а не выглядела
            // как загадочный отказ записи.
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
            // Same reason as the fresh-config path: schema default `true`, derived `Default`
            // `false`, and the `save()` below would persist whichever one wins.
            cfg.separate_control_zones = servers::default_true();
            cfg.chart_memory_percent = schema::default_chart_memory_percent();
            cfg.hotkeys = hotkeys_file.unwrap_or_default();
            cfg.save()?;
            log::info!("мигрировано из config.enc → servers.enc + settings.toml");
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
            // Same reason as the fresh-config path: schema default `true`, derived `Default`
            // `false`, and the `save()` below would persist whichever one wins.
            cfg.separate_control_zones = servers::default_true();
            cfg.chart_memory_percent = schema::default_chart_memory_percent();
            cfg.hotkeys = hotkeys_file.unwrap_or_default();
            cfg.save()?;
            log::info!("мигрировано из config.toml → servers.enc + settings.toml");
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
            // Listed explicitly, unlike its `..Self::default()` neighbours, because its schema
            // default is `true`: the derived `Default` would hand back `false` and the first
            // settings save would persist that, silently inverting control zones on a fresh
            // install. Every other field this struct update covers defaults to zero anyway.
            separate_control_zones: servers::default_true(),
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
            // Плейнтекст-режим не читает settings.toml вовсе, писать нечего и нечем испортить.
            settings_unreadable: false,
            chart_core_remap_needed: false,
        }))
    }

    /// Сохраняет в два файла. Проставляет стабильные uid, валидирует уникальность
    /// имени и host:port. `&mut self` — т.к. может присвоить uid новым ядрам.
    ///
    /// БЕЗ снимка: этот путь зовёт рутинный слив `config_dirty` (100-мс цикл, выход из
    /// приложения, правки из шапки), который срабатывает на мелочах и за минуты вытеснил бы
    /// полезные снимки из хранилища. Осознанное сохранение — [`Self::save_with_snapshot`].
    pub fn save(&mut self) -> anyhow::Result<()> {
        self.save_impl(SnapshotPolicy::No).map(|_| ())
    }

    /// Как [`Self::save`], но сначала снимает копию текущих файлов в `backups/`.
    ///
    /// Для осознанных сохранений (окно Настроек), где пользователю может понадобиться откат.
    /// Имя описывает ПОВЕДЕНИЕ записи, а не UI-поверхность: `moon-core` не знает про окна.
    ///
    /// `Ok(SnapshotOutcome::Failed)` означает: конфиг ЗАПИСАН, но копии для отката нет.
    /// Вызывающий обязан это показать — иначе «Сохранено» соврёт о наличии защиты.
    pub fn save_with_snapshot(&mut self) -> anyhow::Result<SnapshotOutcome> {
        self.save_impl(SnapshotPolicy::Yes)
    }

    /// Общая реализация сохранения; `snapshot` решает, снимать ли копию перед записью.
    ///
    /// ЕДИНСТВЕННАЯ точка записи конфига, поэтому запрет на запись стоит здесь: так он
    /// накрывает и Настройки, и слив по таймеру, и запись на выходе, и миграцию — а не
    /// один путь, о котором вспомнили.
    fn save_impl(&mut self, snapshot: SnapshotPolicy) -> anyhow::Result<SnapshotOutcome> {
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
        // Снимок ПОСЛЕ валидации и ровно перед первой записью. Раньше — и каждое отклонённое
        // сохранение (кнопка активна всегда, а дубль имени ядра валится именно в `validate`)
        // тратило бы слот хранения, ничего при этом не записав: тридцати таких хватило бы,
        // чтобы вытеснить миграционный снимок.
        let outcome = if snapshot == SnapshotPolicy::Yes
            && !backup::snapshot(backup::Trigger::SettingsSave)
        {
            SnapshotOutcome::Failed
        } else {
            SnapshotOutcome::Ok
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
            // Sort mode is presentation-only. The servers Vec order stays in this signature
            // because `SessionManager::config_order` is built from it and decides where a
            // reactivated session lands — a session-layer concern, not a presentation one.
            CoreSortMode::default(),
            // The uid counter advances on save; it describes no structure of its own.
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
mod structural_sig_tests {
    //! Structural-signature tests for presentation-only and server-order changes.

    use super::{AppConfig, CoreSortMode, ServerConfig};

    /// Build an `AppConfig` with the selected order and servers.
    fn config(mode: CoreSortMode, servers: Vec<ServerConfig>) -> AppConfig {
        AppConfig {
            servers,
            core_sort: mode,
            ..Default::default()
        }
    }

    /// Build a minimal server fixture.
    fn server(id: u64, name: &str) -> ServerConfig {
        ServerConfig {
            id,
            uid: id,
            name: name.to_string(),
            ..toml::from_str("id = 0").expect("ServerConfig must deserialize from defaults")
        }
    }

    /// Protects `AppConfig::structural_sig`: a presentation-only sort change must not
    /// reconnect cores or rebuild group windows.
    #[test]
    fn changing_only_the_sort_mode_is_not_structural() {
        let servers = vec![server(1, "Alpha"), server(2, "Bravo")];
        let by_added = config(CoreSortMode::AddedOldest, servers.clone());
        let by_name = config(CoreSortMode::Name, servers);
        assert_eq!(
            by_added.structural_sig(),
            by_name.structural_sig(),
            "a sort-mode change must not trigger a session reconcile"
        );
    }

    /// Protects `AppConfig::structural_sig`: reordering the servers Vec remains structural
    /// because `SessionManager::config_order` is built from that order and decides where a
    /// reactivated session is inserted.
    #[test]
    fn a_server_list_reorder_stays_structural() {
        let forward = config(
            CoreSortMode::AddedOldest,
            vec![server(1, "Alpha"), server(2, "Bravo")],
        );
        let reversed = config(
            CoreSortMode::AddedOldest,
            vec![server(2, "Bravo"), server(1, "Alpha")],
        );
        assert_ne!(
            forward.structural_sig(),
            reversed.structural_sig(),
            "reordering servers must reach the session layer"
        );
    }
}
