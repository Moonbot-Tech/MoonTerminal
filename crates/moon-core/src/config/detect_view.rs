//! Настройки отображения ленты детектов: у КАЖДОГО размера карточки (мини/средний/
//! крупный) свои габариты, тип графика, цветная полоска сервера (rail) и СЛОТОВАЯ
//! раскладка полей — в слот назначается поле + флаги «поверх графика»/«вправо»/«под
//! графиком». Persist — ОТДЕЛЬНЫЙ переносимый `detects_view.toml` (per-group, ключ —
//! имя группы; как theme.toml/badges.json можно копировать между пользователями).
//! Старые записи `layout.toml::detect_view_by_group` (галочная схема) не мигрируются —
//! отсутствие записи даёт дефолт по дизайн-макету.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{paths, toml_io};

/// Размер карточки детекта.
pub const DETECT_SIZE_MINI: u8 = 0;
pub const DETECT_SIZE_MEDIUM: u8 = 1;
pub const DETECT_SIZE_LARGE: u8 = 2;

/// Максимум слотов (крупный). Мини/средний используют первые 4/6 слотов того же массива —
/// один тип конфига на все размеры (и `Copy` без Vec).
pub const DETECT_MAX_SLOTS: usize = 9;

/// Сколько слотов у размера: мини 4 (2 ряда × 2), средний 6 (2 ряда × 3), крупный 9.
pub fn detect_slot_count(size: u8) -> usize {
    match size {
        DETECT_SIZE_MINI => 4,
        DETECT_SIZE_MEDIUM => 6,
        _ => DETECT_MAX_SLOTS,
    }
}

/// Ограничение ширины цветной полоски сервера (rail), px.
pub const DETECT_RAIL_MAX: u8 = 5;

/// Тип мини-графика карточки.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectChart {
    /// Без графика.
    None,
    /// Замороженные 5м-свечи (~2ч).
    #[default]
    Candles,
    /// Линия цены 24ч.
    Line,
}

/// Поле, назначаемое в слот карточки.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectField {
    /// Слот пуст.
    #[default]
    None,
    /// Токен монеты (крупная моно-подпись).
    Coin,
    /// Обратный отсчёт («Ns»).
    Time,
    /// Бейдж типа детекта.
    Badge,
    /// Бейдж имени ядра.
    Core,
    /// Дельта 24ч, %.
    Delta24h,
    /// Дельта 1ч, %.
    Delta1h,
    /// Название биржи.
    Exchange,
    /// Тип биржи (спот/фьючи/…).
    ExchangeKind,
}

impl DetectField {
    /// Все назначаемые поля (порядок выпадашки слота; `None` = «—»).
    pub const ALL: [DetectField; 9] = [
        DetectField::None,
        DetectField::Coin,
        DetectField::Time,
        DetectField::Badge,
        DetectField::Core,
        DetectField::Delta24h,
        DetectField::Delta1h,
        DetectField::Exchange,
        DetectField::ExchangeKind,
    ];
}

/// Один слот раскладки. Позиция ряда (верх/низ) у мини/среднего задана ИНДЕКСОМ слота
/// (первая половина — верх, вторая — низ); у крупного ряд выбирается флагом `below`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectSlot {
    /// Что показывать (None = слот пуст).
    pub field: DetectField,
    /// Рисовать поверх графика (с подложкой), а не в текстовой зоне. Действует только
    /// при включённом графике.
    pub over: bool,
    /// Прижать к правому краю своей зоны (иначе к левому).
    pub right: bool,
    /// Крупный размер: под графиком (иначе над). Мини/средний игнорируют.
    pub below: bool,
}

impl DetectSlot {
    const fn new(field: DetectField, over: bool, right: bool, below: bool) -> Self {
        Self {
            field,
            over,
            right,
            below,
        }
    }
}

/// Настройки ОДНОГО размера карточки.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectSizeCfg {
    /// Габариты карточки, лог. px (до масштаба UI).
    pub w: u16,
    pub h: u16,
    /// Тип мини-графика (мини-размер график не рисует независимо от значения).
    pub chart: DetectChart,
    /// Ширина цветной полоски сервера слева, px (0 = выкл, макс 5).
    pub rail_w: u8,
    /// Ширина градиента-фейда от полоски, px (0 = выкл, макс = ширина карточки).
    pub rail_grad: u16,
    /// Слоты раскладки (используются первые [`detect_slot_count`] штук).
    pub slots: [DetectSlot; DETECT_MAX_SLOTS],
}

impl Default for DetectSizeCfg {
    fn default() -> Self {
        // Прямо не используется (у каждого размера свой дефолт в DetectViewCfg), но нужен
        // для serde(default) при частично заполненных toml-записях.
        Self {
            w: 184,
            h: 44,
            chart: DetectChart::Candles,
            rail_w: 3,
            rail_grad: 20,
            slots: [DetectSlot::default(); DETECT_MAX_SLOTS],
        }
    }
}

impl DetectSizeCfg {
    /// Ширина полоски с клампом 0..=5.
    pub fn rail_w_clamped(&self) -> u8 {
        self.rail_w.min(DETECT_RAIL_MAX)
    }

    /// Ширина градиента с клампом 0..=w.
    pub fn rail_grad_clamped(&self) -> u16 {
        self.rail_grad.min(self.w)
    }
}

const F: fn(DetectField, bool, bool, bool) -> DetectSlot = DetectSlot::new;
const EMPTY: DetectSlot = DetectSlot::new(DetectField::None, false, false, false);

// Дефолты = рабочий набор пользователя от 2026-07-15 (снят с его detects_view.toml).

fn default_mini() -> DetectSizeCfg {
    DetectSizeCfg {
        w: 100,
        h: 40,
        chart: DetectChart::None,
        rail_w: 3,
        rail_grad: 30,
        // Верхний ряд: монета слева, время справа; нижний: бейдж слева, ядро справа.
        slots: [
            F(DetectField::Coin, false, false, false),
            F(DetectField::Time, false, true, false),
            F(DetectField::Badge, false, false, false),
            F(DetectField::Core, false, true, false),
            EMPTY,
            EMPTY,
            EMPTY,
            EMPTY,
            EMPTY,
        ],
    }
}

fn default_medium() -> DetectSizeCfg {
    DetectSizeCfg {
        w: 210,
        h: 44,
        chart: DetectChart::Line,
        rail_w: 3,
        rail_grad: 30,
        // Верх: монета, время слева, Δ24 справа; низ: бейдж, ядро слева, Δ1 справа.
        slots: [
            F(DetectField::Coin, false, false, false),
            F(DetectField::Time, false, false, false),
            F(DetectField::Delta24h, false, true, false),
            F(DetectField::Badge, false, false, false),
            F(DetectField::Core, false, false, false),
            F(DetectField::Delta1h, false, true, false),
            EMPTY,
            EMPTY,
            EMPTY,
        ],
    }
}

fn default_large() -> DetectSizeCfg {
    DetectSizeCfg {
        w: 140,
        h: 100,
        chart: DetectChart::Candles,
        rail_w: 3,
        rail_grad: 30,
        // Над графиком: монета+бейдж слева, Δ24 справа, время поверх; под:
        // ядро слева, Δ1 справа.
        slots: [
            F(DetectField::Coin, false, false, false),
            F(DetectField::Badge, false, false, false),
            F(DetectField::Delta24h, false, true, false),
            F(DetectField::Time, true, false, false),
            F(DetectField::None, true, false, false),
            EMPTY,
            F(DetectField::Core, false, false, true),
            F(DetectField::None, false, false, true),
            F(DetectField::Delta1h, false, true, true),
        ],
    }
}

/// Отображение ленты детектов одной группы: активный размер + настройки всех трёх.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectViewCfg {
    /// Активный размер карточки: 0=мини, 1=средний, 2=крупный.
    pub size: u8,
    /// Знаков после запятой у дельт (Δ24ч/Δ1ч), 0..=2 — ОДНА настройка на все размеры.
    pub delta_decimals: u8,
    pub mini: DetectSizeCfg,
    pub medium: DetectSizeCfg,
    pub large: DetectSizeCfg,
}

impl Default for DetectViewCfg {
    fn default() -> Self {
        Self {
            size: DETECT_SIZE_MEDIUM,
            delta_decimals: 1,
            mini: default_mini(),
            medium: default_medium(),
            large: default_large(),
        }
    }
}

impl DetectViewCfg {
    /// Нормализованный размер (клампится к валидному диапазону).
    pub fn size_clamped(&self) -> u8 {
        self.size.min(DETECT_SIZE_LARGE)
    }

    /// Знаков после запятой у дельт с клампом 0..=2.
    pub fn delta_decimals_clamped(&self) -> usize {
        self.delta_decimals.min(2) as usize
    }

    /// Настройки конкретного размера.
    pub fn size_cfg(&self, size: u8) -> &DetectSizeCfg {
        match size {
            DETECT_SIZE_MINI => &self.mini,
            DETECT_SIZE_MEDIUM => &self.medium,
            _ => &self.large,
        }
    }

    pub fn size_cfg_mut(&mut self, size: u8) -> &mut DetectSizeCfg {
        match size {
            DETECT_SIZE_MINI => &mut self.mini,
            DETECT_SIZE_MEDIUM => &mut self.medium,
            _ => &mut self.large,
        }
    }

    /// Настройки активного размера.
    pub fn active(&self) -> &DetectSizeCfg {
        self.size_cfg(self.size_clamped())
    }

    /// Текст в формате detects_view.toml одной группы — для «Копировать» в попапе.
    pub fn to_share_string(&self) -> Option<String> {
        toml::to_string_pretty(self).ok()
    }

    /// Разобрать текст «Вставить» (конфиг одной группы). Чужой/битый текст → None.
    pub fn parse_share(text: &str) -> Option<Self> {
        toml::from_str::<Self>(text).ok()
    }
}

/// Файл `detects_view.toml`: настройки лент детектов по группам (ключ — имя группы).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectViewFile {
    pub groups: HashMap<String, DetectViewCfg>,
}

impl DetectViewFile {
    pub fn load() -> Self {
        toml_io::load_or_default(&paths::detects_view_path(), "detects_view.toml", |_| {})
    }

    pub fn save(&self) {
        if let Err(e) = toml_io::save(&paths::detects_view_path(), self, "detects_view.toml") {
            log::warn!("не записал detects_view.toml: {e:#}");
        }
    }

    /// Настройки группы (или дефолт по макету).
    pub fn group(&self, group: &str) -> DetectViewCfg {
        self.groups.get(group).copied().unwrap_or_default()
    }

    pub fn set_group(&mut self, group: &str, cfg: DetectViewCfg) {
        self.groups.insert(group.to_string(), cfg);
    }
}

#[cfg(test)]
mod tests;
