//! Описание одного ядра MoonBot (сервера) и группы.

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
    /// Рантайм-id ядра (CoreId): позиционный, переназначается при каждой загрузке.
    /// Используется для привязки панелей/данных/БД в пределах сессии.
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
    /// Base64-ключ MoonBot. Внутри зашиты host/port/transport — отдельных полей нет.
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
