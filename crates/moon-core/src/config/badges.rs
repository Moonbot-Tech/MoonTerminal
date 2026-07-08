//! Бейджи типов детектов — переносимый JSON-файл `badges.json` (по образцу
//! `orders.toml`/`figures.json`). Для каждого вида стратегии (`StrategyKind`
//! ordinal) задаём короткий код (≤3 символа, не обязательно буквы) и цвет
//! РАЗДЕЛЬНО под тёмную/светлую тему. Плюс глобальная опция «помечать направление
//! (isShort) обводкой» с настраиваемыми цветами short/long на тему.
//!
//! Список видов растёт со временем — новые добавляются в UI по ordinal-номеру;
//! ненастроенные виды в рантайме падают на нейтральный код `UNK`.

use serde::{Deserialize, Serialize};

use super::paths;
use super::write_file_atomic;

/// `0xRRGGBB` → sRGB `[u8;3]`.
const fn c(hex: u32) -> [u8; 3] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

/// Нейтральный (muted) цвет-фолбэк для ненастроенных видов.
const MUTED_DARK: [u8; 3] = c(0x97928A);
const MUTED_LIGHT: [u8; 3] = c(0x4F5B68);

/// Один бейдж типа детекта: код + цвет на каждую тему. Код и имя общие для обеих
/// тем — под тему различается только цвет.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BadgeEntry {
    /// Ordinal вида стратегии (`StrategyKind`). Ключ соответствия детекту.
    pub ordinal: u8,
    /// Человекочитаемое имя вида (для UI-строки настроек).
    pub name: String,
    /// Короткий код на бейдже — ≤3 символа (обрезается в UI), не обязательно буквы.
    pub code: String,
    /// Цвет бейджа в тёмной теме.
    pub color_dark: [u8; 3],
    /// Цвет бейджа в светлой теме.
    pub color_light: [u8; 3],
}

impl Default for BadgeEntry {
    fn default() -> Self {
        Self {
            ordinal: 0,
            name: String::new(),
            code: "UNK".to_string(),
            color_dark: MUTED_DARK,
            color_light: MUTED_LIGHT,
        }
    }
}

impl BadgeEntry {
    /// Цвет под активную тему.
    pub fn color(&self, is_light: bool) -> [u8; 3] {
        if is_light {
            self.color_light
        } else {
            self.color_dark
        }
    }
}

/// Конфиг бейджей детектов (переносимый `badges.json`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BadgesConfig {
    /// Записи по видам стратегий (ordinal → код/цвета).
    pub entries: Vec<BadgeEntry>,
    /// Помечать направление стратегии (isShort) цветной обводкой бейджа.
    pub mark_direction: bool,
    pub short_outline_dark: [u8; 3],
    pub short_outline_light: [u8; 3],
    pub long_outline_dark: [u8; 3],
    pub long_outline_light: [u8; 3],
}

/// Дефолтные 24 вида: `(ordinal, имя, код, цвет_тёмн, цвет_светл)`. Цвета — из
/// семейств `MoonTone` (см. свотч бейджей). Держим здесь как единый источник —
/// на первом запуске пишется в `badges.json`, дальше редактируется в Настройках.
#[rustfmt::skip]
const DEFAULTS: &[(u8, &str, &str, u32, u32)] = &[
    // Вход / «лунные» — Positive (green)
    (2,  "Drops",         "DRP", 0x2FA85C, 0x168A49),
    (6,  "MoonShot",      "SHT", 0x2FA85C, 0x168A49),
    (13, "MoonStrike",    "STR", 0x2FA85C, 0x168A49),
    (20, "Moon Hook",     "HOK", 0x2FA85C, 0x168A49),
    (14, "New Listing",   "NEW", 0x2FA85C, 0x168A49),
    // Импульс / объём — Warning (amber)
    (4,  "Volumes",       "VOL", 0xFFB347, 0xB97800),
    (5,  "PumpDetection", "PMP", 0xFFB347, 0xB97800),
    (8,  "Delta",         "DLT", 0xFFB347, 0xB97800),
    (9,  "Waves",         "WAV", 0xFFB347, 0xB97800),
    (21, "Activity",      "ACT", 0xFFB347, 0xB97800),
    // Стакан / структура — Info (blue)
    (3,  "Walls",         "WAL", 0x7FC9FF, 0x126CBF),
    (19, "Chart Wall",    "CHW", 0x7FC9FF, 0x126CBF),
    (18, "Spread",        "SPR", 0x7FC9FF, 0x126CBF),
    // Риск — Danger (red)
    (15, "Liquidations",  "LIQ", 0xFF4A4A, 0xD92D3A),
    // Индикатор / производные — Accent
    (17, "EMA",           "EMA", 0xD2691E, 0xB95C18),
    (7,  "V Lite",        "VLT", 0xD2691E, 0xB95C18),
    (16, "TopMarket",     "TOP", 0xD2691E, 0xB95C18),
    (10, "Combo",         "CMB", 0xD2691E, 0xB95C18),
    // Внешний сигнал — Notice (yellow)
    (1,  "Telegram",      "TLG", 0xFFD93D, 0xB48A00),
    (11, "UDP",           "UDP", 0xFFD93D, 0xB48A00),
    // Мета / служебное — Muted
    (12, "Manual",        "MAN", 0x97928A, 0x4F5B68),
    (22, "Alerts",        "ALR", 0x97928A, 0x4F5B68),
    (23, "Watcher",       "WCH", 0x97928A, 0x4F5B68),
    // Служебный
    (0,  "Unknown",       "UNK", 0xE8E4DC, 0x18202A),
];

impl Default for BadgesConfig {
    fn default() -> Self {
        Self {
            entries: DEFAULTS
                .iter()
                .map(|&(ordinal, name, code, cd, cl)| BadgeEntry {
                    ordinal,
                    name: name.to_string(),
                    code: code.to_string(),
                    color_dark: c(cd),
                    color_light: c(cl),
                })
                .collect(),
            mark_direction: false,
            // short = red, long = green (из палитры), под каждую тему.
            short_outline_dark: c(0xFF4A4A),
            short_outline_light: c(0xD92D3A),
            long_outline_dark: c(0x2FA85C),
            long_outline_light: c(0x168A49),
        }
    }
}

impl BadgesConfig {
    /// Прочитать `badges.json`. Нет файла/битый → дефолт (и записать его на диск,
    /// чтобы пользователю было что редактировать вручную) — как `OrdersStyleSet::load`.
    pub fn load() -> Self {
        let path = paths::badges_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<BadgesConfig>(&s) {
                Ok(cfg) => cfg,
                Err(e) => {
                    log::warn!("badges.json битый ({e}) — беру дефолт");
                    Self::default()
                }
            },
            Err(_) => {
                let cfg = Self::default();
                cfg.save();
                cfg
            }
        }
    }

    /// Записать `badges.json` (атомарно). Не фатально — при ошибке логируем.
    pub fn save(&self) {
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = write_file_atomic(&paths::badges_path(), s.as_bytes(), "badges.json")
                {
                    log::warn!("не записал badges.json: {e:#}");
                }
            }
            Err(e) => log::warn!("сериализация badges.json не удалась: {e}"),
        }
    }

    /// Запись по ordinal (первое совпадение).
    pub fn entry(&self, ordinal: u8) -> Option<&BadgeEntry> {
        self.entries.iter().find(|e| e.ordinal == ordinal)
    }

    /// Код бейджа для вида (фолбэк `UNK`).
    pub fn code(&self, ordinal: u8) -> &str {
        self.entry(ordinal).map(|e| e.code.as_str()).unwrap_or("UNK")
    }

    /// Цвет бейджа для вида под активную тему (фолбэк — нейтраль).
    pub fn color(&self, ordinal: u8, is_light: bool) -> [u8; 3] {
        self.entry(ordinal).map(|e| e.color(is_light)).unwrap_or(if is_light {
            MUTED_LIGHT
        } else {
            MUTED_DARK
        })
    }

    /// Цвет обводки направления под активную тему.
    pub fn outline(&self, is_short: bool, is_light: bool) -> [u8; 3] {
        match (is_short, is_light) {
            (true, false) => self.short_outline_dark,
            (true, true) => self.short_outline_light,
            (false, false) => self.long_outline_dark,
            (false, true) => self.long_outline_light,
        }
    }
}
