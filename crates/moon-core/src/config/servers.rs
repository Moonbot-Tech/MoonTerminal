//! Описание одного ядра Moonbot (сервера) и группы.

use serde::{Deserialize, Serialize};

use super::secrets::Secret;

/// Флаги приёма данных от ядра — чисто клиентский фильтр.
///
/// ВАЖНО: ядро всё равно шлёт эти доменные события всегда. Сброшенный флаг
/// означает «не читаем / не складываем / не рисуем» (экономим CPU, БД и окна),
/// но НЕ экономит сетевой трафик — серверного opt-out у этих категорий нет.
/// Стакан/лента сюда не входят: они chart-only и живут только при открытом окне.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FeedFlags {
    /// Открытые ордера ядра (нижний док).
    #[serde(default = "default_true")]
    pub orders: bool,
    /// Детекты / watcher-строки / chart-only / alert-fire (`DetectEvent`).
    #[serde(default = "default_true")]
    pub detects: bool,
    /// Отчёты по закрытым sell-ордерам (`ClosedSellOrderReport`) → SQLite.
    #[serde(default = "default_true")]
    pub reports: bool,
    /// Балансы и метаданные аккаунта.
    #[serde(default = "default_true")]
    pub balance: bool,
    /// Состояние стратегий (`Strat`).
    #[serde(default = "default_true")]
    pub strategies: bool,
    /// Серверный лог (`ServerLog`).
    #[serde(default = "default_true")]
    pub log: bool,
    /// Chart-алерты и chart-текст.
    #[serde(default = "default_true")]
    pub alerts: bool,
    /// Арбитраж (`Arb`).
    #[serde(default = "default_true")]
    pub arb: bool,
}

impl Default for FeedFlags {
    /// Дефолт = принимать всё (поведение как до введения флагов).
    fn default() -> Self {
        Self {
            orders: true,
            detects: true,
            reports: true,
            balance: true,
            strategies: true,
            log: true,
            alerts: true,
            arb: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Рантайм-id ядра (CoreId). С v11 схемы РАВЕН `uid` → стабилен между загрузками
    /// и при добавлении/удалении/перепорядке серверов (раньше был позиционным). По нему
    /// привязываются панели/данные/БД/подписки/раскладка в пределах сессии.
    pub id: u64,
    /// Стабильный идентификатор ядра. Переживает переименование и перепорядок —
    /// по нему мета из settings.toml привязывается к серверу из servers.enc.
    /// 0 = ещё не присвоен (старый файл / только что добавлен) → проставится при save.
    #[serde(default)]
    pub uid: u64,
    #[serde(default)]
    pub name: String,
    /// Активно ли ядро (галка в настройках). Неактивные не подключаются.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Рисовать ли окно/чарт ядра. Off + active = headless: тянем отчёты/детекты
    /// в БД/store без окна. Окно показываем только при active && show_window.
    #[serde(default = "default_true")]
    pub show_window: bool,
    /// Что принимаем от ядра (клиентский фильтр).
    #[serde(default)]
    pub feed: FeedFlags,
    /// Base64-ключ Moonbot. Внутри зашиты host/port/transport — отдельных полей нет.
    #[serde(default)]
    pub key: Secret,
    /// Группа = имя окна, куда попадает ядро. Цвет/иконка — на группе (GroupConfig).
    #[serde(default = "default_group")]
    pub group: String,
    /// Рынок по умолчанию (временно, до мульти-рынков на ядро).
    #[serde(default = "default_market")]
    pub market: String,
    /// Цвет сервера (RGB) — цвет детекта (используется позже).
    #[serde(default = "default_color")]
    pub color: [u8; 3],
    /// Синтетическое ядро бенчмарка (MOON_SYNTH): фид гонит synth::run вместо live::run.
    #[serde(default)]
    pub synthetic: bool,
    /// Имя чарт-связки для AddToChart. Пусто = по глобальной настройке
    /// (`charts_split_by_core`: своя вкладка на ядро / все ядра в одной). Непусто =
    /// ядра ОДНОЙ группы с этим же именем сводят свои AddToChart=N графики в ОДНУ
    /// вкладку, а имя связки идёт в её заголовок. Имя локально для группы.
    #[serde(default)]
    pub chart_bundle: String,
    /// 6 пресетов размера ручного ордера (кнопки F1-F6 тулбара), в БАЗОВОЙ монете ядра.
    /// `None` = не настроено → берём дефолт по базе ядра (`default_order_sizes`), т.к.
    /// для BTC-базы нужны ~0.01..0.5, а для USDT — крупные (~50..2500). В moonproto
    /// значений buy-size НЕТ (только sell-пресеты ClientSettings) — это локальный конфиг.
    #[serde(default)]
    pub order_sizes: Option<[f64; 6]>,
    /// Последний ВЫБРАННЫЙ пресет размера (индекс 0..=5 кнопки F1-F6) — восстановление
    /// выбора после перезапуска. `None` = не выбирали (дефолт F3). Как и значения
    /// пресетов — локальный конфиг (в moonproto выбора buy-size нет).
    #[serde(default)]
    pub order_size_sel: Option<usize>,
    /// Стратегия алертов по умолчанию (Def Strategy): id стратегии вида «Alerts» этого
    /// ядра, автоназначаемый новому алерту при постановке галки Alert. 0 = без.
    /// Локальный конфиг терминала (протокол дефолт-стратегию ядра не отдаёт).
    #[serde(default)]
    pub default_alert_strategy: u64,
}

/// Порядок всех списков ядер в приложении, выбранный пользователем в Настройках.
///
/// Выбор хранится глобально; ранжирование выполняет модуль `core_order` UI-крейта.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CoreSortMode {
    /// Лексикографически по Unicode-именам в нижнем регистре, с uid для разрешения равенства.
    #[default]
    Name,
    /// По порядку добавления, сначала старые (по `ServerConfig::uid`).
    AddedOldest,
    /// По порядку добавления, сначала новые (по `ServerConfig::uid`).
    AddedNewest,
}

impl CoreSortMode {
    /// Стабильный код для `settings.toml`.
    ///
    /// `AddedOldest` намеренно сериализуется как `"added"`: это закреплённое значение формата,
    /// которое означает порядок добавления от старых к новым.
    pub fn code(self) -> &'static str {
        match self {
            CoreSortMode::Name => "name",
            CoreSortMode::AddedOldest => "added",
            CoreSortMode::AddedNewest => "added_newest",
        }
    }

    /// Разобрать код из `settings.toml`; неизвестный код возвращает `None`.
    ///
    /// Неизвестные значения намеренно не приближаются к одному из режимов по содержимому:
    /// `Deserialize` консервативно сводит их к `Default` (= `Name`).
    pub fn from_code(s: &str) -> Option<Self> {
        match s {
            "name" => Some(CoreSortMode::Name),
            "added" => Some(CoreSortMode::AddedOldest),
            "added_newest" => Some(CoreSortMode::AddedNewest),
            _ => None,
        }
    }
}

impl Serialize for CoreSortMode {
    /// Сериализовать стабильный строчный код для `settings.toml`.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for CoreSortMode {
    /// Свести неизвестную строку, i64/u64/f64/bool/unit, последовательность или map к дефолту.
    ///
    /// Эти формы покрываются явно, чтобы косметическое поле не отвергало остальные настройки;
    /// прочие формы данных обрабатываются стандартным отказом `serde::de::Visitor`.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        /// Посетитель форм TOML, для которых задан безопасный дефолт режима сортировки.
        struct AnyScalar;

        impl<'de> serde::de::Visitor<'de> for AnyScalar {
            type Value = CoreSortMode;

            /// Описать допустимые строковые коды для диагностик serde.
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a core sort mode (name / added / added_newest)")
            }

            /// Разобрать строковый код, сводя неизвестный к дефолту.
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(CoreSortMode::from_code(v).unwrap_or_default())
            }

            // Недопустимые скалярные значения затрагивают только это косметическое поле.
            /// Свести знаковое 64-битное число к дефолту.
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(CoreSortMode::default())
            }

            /// Свести беззнаковое 64-битное число к дефолту.
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(CoreSortMode::default())
            }

            /// Свести число с плавающей точкой к дефолту.
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(CoreSortMode::default())
            }

            /// Свести логическое значение к дефолту.
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(CoreSortMode::default())
            }

            /// Свести unit/null к дефолту.
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(CoreSortMode::default())
            }

            // Вычитываем недопустимый контейнер целиком, чтобы десериализация не рассинхронизировалась.
            /// Вычитать последовательность целиком и вернуть дефолт.
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(CoreSortMode::default())
            }

            /// Вычитать map целиком и вернуть дефолт.
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                while map
                    .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(CoreSortMode::default())
            }
        }

        d.deserialize_any(AnyScalar)
    }
}

/// Ключ чарт-вкладки AddToChart внутри группы — куда сводить графики ядра.
/// Резолвится из `ServerConfig::chart_bucket` (см.). Сериализуется в charts.json.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ChartBucket {
    /// Все ядра группы в одной вкладке `N-группа` (глоб. split=off, связка пуста).
    Shared,
    /// Своя вкладка ядра `N-группа-ядро` (глоб. split=on, связка пуста).
    Core(crate::session::CoreId),
    /// Именованная связка `N-группа-имя` — подмножество ядер группы (переопределяет
    /// глобальный флаг). Имя попадает в заголовок вкладки.
    Bundle(String),
}

impl ServerConfig {
    /// Куда сводить AddToChart-графики этого ядра при текущем глобальном флаге
    /// `charts_split_by_core` (split). Непустая связка переопределяет флаг.
    pub fn chart_bucket(&self, split: bool) -> ChartBucket {
        if !self.chart_bundle.is_empty() {
            ChartBucket::Bundle(self.chart_bundle.clone())
        } else if split {
            ChartBucket::Core(self.id)
        } else {
            ChartBucket::Shared
        }
    }

    /// 6 пресетов размера ручного ордера для тулбара: настроенные (`order_sizes`) или
    /// дефолт по базовой монете ядра `base` ("BTC"/"USDT"/…). `base` UI берёт из
    /// `SessionManager::core_base`.
    pub fn order_sizes_or_default(&self, base: &str) -> [f64; 6] {
        self.order_sizes
            .unwrap_or_else(|| default_order_sizes(base))
    }
}

pub fn default_color() -> [u8; 3] {
    crate::palette::ACCENT
}

pub fn default_group() -> String {
    "default".to_string()
}

pub fn default_market() -> String {
    "BTCUSDT".to_string()
}

/// Дефолтные пресеты размера ордера (F1-F6) по базовой монете ядра. BTC-база → мелкие
/// (как было захардкожено в тулбаре); прочее (USDT/стейблы/альты) → крупные. Это лишь
/// стартовые значения — пользователь правит их в Настройках ядра (`order_sizes`).
pub fn default_order_sizes(base: &str) -> [f64; 6] {
    if base.eq_ignore_ascii_case("BTC") {
        [0.01, 0.025, 0.05, 0.10, 0.25, 0.50]
    } else {
        [50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0]
    }
}

pub fn default_true() -> bool {
    true
}

/// Дефолт срока хранения файлов лога (дней). См. SettingsFile::log_retention_days.
pub fn default_log_retention_days() -> u32 {
    14
}

#[cfg(test)]
mod core_sort_parse_tests {
    //! Проверки устойчивого разбора глобальной настройки порядка ядер.

    use super::CoreSortMode;
    use serde::Deserialize;

    /// Минимальная оболочка настроек для проверки сохранности соседнего поля при разборе.
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        core_sort: CoreSortMode,
        #[serde(default)]
        keep: String,
    }

    /// Проверяет `CoreSortMode::deserialize`: неверное слово или тип сбрасывает только это поле,
    /// сохраняя остальные данные `SettingsFile`.
    #[test]
    fn a_bad_core_sort_value_never_costs_the_rest_of_the_file() {
        for bad in [
            r#"core_sort = "typo""#,
            "core_sort = 1",
            "core_sort = 1.5",
            "core_sort = true",
            "core_sort = []",
            "core_sort = {}",
        ] {
            let toml = format!("{bad}\nkeep = \"server meta\"\n");
            let probe: Probe = toml::from_str(&toml)
                .unwrap_or_else(|e| panic!("`{bad}` must not fail the whole file: {e}"));
            assert_eq!(probe.core_sort, CoreSortMode::Name, "for `{bad}`");
            assert_eq!(probe.keep, "server meta", "for `{bad}`");
        }
    }

    /// Неподдерживаемый код режима сводится к `Name`, не затрагивая соседние поля.
    ///
    /// Возможная поломка: добавить особое отображение неизвестного кода в один из режимов
    /// добавления. Это дало бы произвольный результат вместо консервативного дефолта.
    #[test]
    fn a_retired_manual_setting_lands_on_the_new_default() {
        let probe: Probe = toml::from_str("core_sort = \"manual\"\nkeep = \"server meta\"\n")
            .expect("a retired code must not fail the file");
        assert_eq!(probe.core_sort, CoreSortMode::Name);
        assert_eq!(probe.keep, "server meta");
    }

    /// Коды на диске — закреплённые значения формата, а не свободные идентификаторы.
    ///
    /// Проверка round-trip ниже этого не ловит: она подаёт результат `code()` обратно в
    /// `from_code()`, поэтому синхронное изменение обеих сторон останется зелёным, но существующий
    /// `"added"` на диске начнёт сводиться к `Name`. Эти литералы независимо фиксируют контракт.
    #[test]
    fn the_on_disk_codes_are_frozen() {
        assert_eq!(CoreSortMode::Name.code(), "name");
        assert_eq!(CoreSortMode::AddedOldest.code(), "added");
        assert_eq!(CoreSortMode::AddedNewest.code(), "added_newest");
    }

    /// Проверяет отображение допустимых кодов, используемое сериализацией `CoreSortMode`.
    #[test]
    fn every_mode_round_trips_through_its_code() {
        for mode in [
            CoreSortMode::Name,
            CoreSortMode::AddedOldest,
            CoreSortMode::AddedNewest,
        ] {
            let toml = format!("core_sort = \"{}\"\nkeep = \"\"\n", mode.code());
            let probe: Probe = toml::from_str(&toml).expect("a valid code must parse");
            assert_eq!(probe.core_sort, mode);
        }
    }
}
