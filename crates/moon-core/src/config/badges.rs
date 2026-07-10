//! Бейджи типов детектов — переносимый JSON-файл `badges.json` (по образцу
//! `orders.toml`/`figures.json`). Для каждого вида стратегии (`StrategyKind`
//! ordinal) задаём: рисовать ли бейдж (`active`), короткий код (≤3 символа, не
//! обязательно буквы) РАЗДЕЛЬНО под long/short (если включено `distinguish_dir`) и
//! цвет РАЗДЕЛЬНО под тёмную/светлую тему. Обводка — ПЕР-СТРОКА: галка `outline` +
//! свои цвета long/short на тему.
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

// Дефолтные цвета обводки направления (long = зелёный, short = красный) на тему.
const OUT_LONG_DARK: [u8; 3] = c(0x2FA85C);
const OUT_LONG_LIGHT: [u8; 3] = c(0x168A49);
const OUT_SHORT_DARK: [u8; 3] = c(0xFF4A4A);
const OUT_SHORT_LIGHT: [u8; 3] = c(0xD92D3A);

/// Один бейдж типа детекта.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BadgeEntry {
    /// Ordinal вида стратегии (`StrategyKind`). Ключ соответствия детекту.
    pub ordinal: u8,
    /// Человекочитаемое имя вида (для UI-строки настроек).
    pub name: String,
    /// Рисовать бейдж для этого типа детекта. Выкл = бейдж не показываем.
    pub active: bool,
    /// Код бейджа (для лонга и для обоих направлений, если `distinguish_dir=false`). ≤3.
    pub code: String,
    /// Различать код по направлению: при true у шорта свой код `code_short`.
    pub distinguish_dir: bool,
    /// Код бейджа для ШОРТА (используется только при `distinguish_dir`). ≤3.
    pub code_short: String,
    /// Цвет бейджа в тёмной теме.
    pub color_dark: [u8; 3],
    /// Цвет бейджа в светлой теме.
    pub color_light: [u8; 3],
    /// Рисовать обводку бейджа (пер-строка). При true — цвета обводки по направлению.
    pub outline: bool,
    pub outline_long_dark: [u8; 3],
    pub outline_long_light: [u8; 3],
    pub outline_short_dark: [u8; 3],
    pub outline_short_light: [u8; 3],
}

impl Default for BadgeEntry {
    fn default() -> Self {
        Self {
            ordinal: 0,
            name: String::new(),
            active: true,
            code: "UNK".to_string(),
            distinguish_dir: false,
            code_short: String::new(),
            color_dark: MUTED_DARK,
            color_light: MUTED_LIGHT,
            outline: false,
            outline_long_dark: OUT_LONG_DARK,
            outline_long_light: OUT_LONG_LIGHT,
            outline_short_dark: OUT_SHORT_DARK,
            outline_short_light: OUT_SHORT_LIGHT,
        }
    }
}

impl BadgeEntry {
    /// Цвет бейджа под активную тему.
    pub fn color(&self, is_light: bool) -> [u8; 3] {
        if is_light {
            self.color_light
        } else {
            self.color_dark
        }
    }

    /// Код бейджа под направление: при `distinguish_dir` и шорте — `code_short`
    /// (если он непустой), иначе основной `code`.
    pub fn code_for(&self, is_short: bool) -> &str {
        if self.distinguish_dir && is_short && !self.code_short.is_empty() {
            &self.code_short
        } else {
            &self.code
        }
    }

    /// Цвет обводки под направление и тему (если обводка включена).
    pub fn outline_color(&self, is_short: bool, is_light: bool) -> Option<[u8; 3]> {
        if !self.outline {
            return None;
        }
        Some(match (is_short, is_light) {
            (true, false) => self.outline_short_dark,
            (true, true) => self.outline_short_light,
            (false, false) => self.outline_long_dark,
            (false, true) => self.outline_long_light,
        })
    }
}

/// Конфиг бейджей детектов (переносимый `badges.json`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BadgesConfig {
    /// Записи по видам стратегий (ordinal → код/цвета/обводка).
    pub entries: Vec<BadgeEntry>,
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
                    ..BadgeEntry::default()
                })
                .collect(),
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

    /// Текст в формате badges.json — для «Копировать» в Настройках (= содержимое файла).
    pub fn to_share_string(&self) -> Option<String> {
        serde_json::to_string_pretty(self).ok()
    }

    /// Разобрать текст badges.json (вставка из буфера / содержимое файла). Требуем
    /// массив `entries` — иначе любой JSON молча дал бы дефолт (serde(default)).
    /// `None` = это не бейджи.
    pub fn parse_share(text: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        v.get("entries")?.as_array()?;
        serde_json::from_str(text).ok()
    }

    /// Запись по ordinal (первое совпадение).
    pub fn entry(&self, ordinal: u8) -> Option<&BadgeEntry> {
        self.entries.iter().find(|e| e.ordinal == ordinal)
    }

    /// Рисовать ли бейдж для вида (ненастроенный вид → да, покажем UNK).
    pub fn active(&self, ordinal: u8) -> bool {
        self.entry(ordinal).map(|e| e.active).unwrap_or(true)
    }

    /// Код бейджа для вида под направление (фолбэк `UNK`).
    pub fn code(&self, ordinal: u8, is_short: bool) -> &str {
        self.entry(ordinal).map(|e| e.code_for(is_short)).unwrap_or("UNK")
    }

    /// Цвет бейджа для вида под активную тему (фолбэк — нейтраль).
    pub fn color(&self, ordinal: u8, is_light: bool) -> [u8; 3] {
        self.entry(ordinal).map(|e| e.color(is_light)).unwrap_or(if is_light {
            MUTED_LIGHT
        } else {
            MUTED_DARK
        })
    }

    /// Цвет обводки бейджа под направление/тему (None = обводки нет).
    pub fn outline_color(&self, ordinal: u8, is_short: bool, is_light: bool) -> Option<[u8; 3]> {
        self.entry(ordinal).and_then(|e| e.outline_color(is_short, is_light))
    }
}
