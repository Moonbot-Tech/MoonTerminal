//! Тема оформления чарта (фон/сетка/перекрестие) — ОТДЕЛЬНЫЙ переносимый файл
//! `theme.toml` рядом с exe, чтобы темой можно было делиться (скопировал файл —
//! и оформление перенеслось). Хранится ПАРОЙ наборов `[dark]`/`[light]` (как
//! `orders.toml`) — файл самодостаточен и переносится независимо от того, какой
//! режим UI выбран у получателя. Цвета заданы в sRGB (как палитра/egui); в linear
//! их конвертируют шейдеры (см. [[srgb-shader-colors]]).

use serde::{Deserialize, Serialize};

use super::paths;
use crate::palette;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChartTheme {
    // --- График: фон и сетка ---
    /// Фон чарта (sRGB).
    pub bg: [u8; 3],
    /// Цвет линий сетки (sRGB).
    pub grid: [u8; 3],
    /// Видимость сетки 0..1 (0 — скрыть).
    pub grid_alpha: f32,
    /// Непрозрачность фото-подложки 0..1 (0 — выключить).
    pub background_opacity: f32,
    /// Поправка к кеглю подписей ордер-линий И курсора на чарте (px, +/- от базы 11.5).
    /// Слайдер в Настройках/Интерфейс. На подписи осей и угловой тикер НЕ влияет.
    pub label_font_delta: f32,

    // --- График: перекрестие ---
    /// Цвет перекрестия (sRGB).
    pub cross: [u8; 3],
    /// Прозрачность линий перекрестия 0..1.
    pub cross_alpha: f32,
    /// Полутолщина линий перекрестия, px.
    pub cross_thickness: f32,

    // --- Свечи ---
    /// Цвет растущей свечи (close ≥ open), sRGB. Дефолт = зелёный крестов buy.
    pub candle_up: [u8; 3],
    /// Цвет падающей свечи, sRGB. Дефолт = оранжевый крестов sell.
    pub candle_down: [u8; 3],
    /// Нейтральный цвет свечей в зоне трейдов (галка «Нейтральный цвет в зоне»), sRGB.
    pub candle_neutral: [u8; 3],
    /// Непрозрачность заливки тела свечи 0..1 (контуры/фитили рисуются плотнее).
    pub candle_fill_alpha: f32,

    // --- Стакан ---
    /// Фон зоны стакана МЕЖДУ лучшими bid/ask (щель спреда), sRGB.
    pub book_bg: [u8; 3],
    /// Фон ask-половины стакана (выше лучшего ask), sRGB.
    pub book_bg_ask: [u8; 3],
    /// Фон bid-половины стакана (ниже лучшего bid), sRGB.
    pub book_bg_bid: [u8; 3],
    /// Цвет bid-стороны (покупки), sRGB.
    pub book_bid: [u8; 3],
    /// Цвет ask-стороны (продажи), sRGB.
    pub book_ask: [u8; 3],
    /// Яркость/opacity отдельных линий уровней стакана 0..1.
    pub book_level_alpha: f32,
    /// Толщина отдельных линий уровней стакана, physical px.
    pub book_level_width: f32,

    // --- Подписи на графике ---
    /// Цвет положительных значений в подписях (% профита, bid-depth), sRGB.
    pub label_positive: [u8; 3],
    /// Цвет отрицательных значений в подписях (% убытка, ask-depth), sRGB.
    pub label_negative: [u8; 3],
    /// Цвет нейтральных подписей, sRGB.
    pub label_neutral: [u8; 3],
    /// Цвет подписей осей цены/времени, sRGB.
    pub axis_label: [u8; 3],
    /// Цвет угловой подписи ядра/рынка, sRGB.
    pub caption_label: [u8; 3],
    /// Цвет нейтрального cursor/readout текста, sRGB.
    pub readout_label: [u8; 3],
    /// Alpha плотной плашки cursor/readout.
    pub readout_bg_alpha: f32,
    /// Alpha лёгкой плашки угловой подписи (имя ядра/тикер).
    pub readout_soft_bg_alpha: f32,
    /// Alpha плашки подписей ордер-линий. Полу-плотная: выглядит непрозрачной, но при наложении
    /// плашка старшей подписи ложится на младшую (та просвечивает → «заходит под», не исчезает).
    pub line_label_bg_alpha: f32,
    /// Alpha обводки плашки readout.
    pub readout_border_alpha: f32,
    /// Толщина обводки readout, px. 0 = без обводки.
    pub readout_border_px: f32,

    // Стиль линий ордеров (цвета/толщины/маркеры) вынесен в отдельный orders.toml
    // (см. config::orders::OrdersStyle) — не дублируем его в теме.

    // --- Панели (egui-хром: тулбар, панель ордера, док ордеров, статус) ---
    /// Фон панелей (sRGB).
    pub panel_bg: [u8; 3],
}

impl Default for ChartTheme {
    fn default() -> Self {
        Self {
            bg: [30, 30, 30],
            grid: [40, 40, 40],
            grid_alpha: 1.0,
            background_opacity: 0.18,
            label_font_delta: -1.5,
            cross: [128, 128, 128],
            cross_alpha: 0.5,
            cross_thickness: 1.0,
            // Свечи в цветах крестов трейдов (crosses.hlsl buy/sell), нейтраль — серый.
            candle_up: [47, 168, 92],
            candle_down: [255, 142, 90],
            candle_neutral: [128, 128, 128],
            candle_fill_alpha: 0.85,
            book_bg: [30, 30, 30],
            // Половины стакана слегка подкрашены своими сторонами; щель спреда между
            // лучшими bid/ask остаётся нейтральным book_bg.
            book_bg_ask: [42, 30, 27],
            book_bg_bid: [30, 36, 26],
            book_bid: [75, 86, 48],
            book_ask: [170, 73, 39],
            book_level_alpha: 0.5,
            book_level_width: 1.5,
            label_positive: palette::GREEN,
            label_negative: palette::RED,
            label_neutral: [211, 211, 211],
            axis_label: [211, 211, 211],
            caption_label: [211, 211, 211],
            readout_label: [211, 211, 211],
            readout_bg_alpha: 0.96,
            readout_soft_bg_alpha: 0.20,
            line_label_bg_alpha: 0.85,
            readout_border_alpha: 0.0,
            readout_border_px: 0.0,
            panel_bg: [24, 25, 27],
        }
    }
}

impl ChartTheme {
    /// Дефолт светлого набора: тёмный дефолт + светлые Moonbot-переопределения.
    fn default_light() -> Self {
        let mut t = Self::default();
        t.apply_light_defaults();
        t
    }

    /// Светлые Moonbot-дефолты. Фон/сетка = значения светлой MoonUI-палитры
    /// (chart_bg 0xFFFFFF / row_line 0xECEFF2) — раньше рендер перекрывал их палитрой
    /// на лету, теперь они просто дефолт светлого набора (и редактируются).
    fn apply_light_defaults(&mut self) {
        self.bg = [255, 255, 255];
        self.grid = [236, 239, 242];
        self.cross = [128, 128, 128];
        self.candle_up = [0, 128, 0];
        self.candle_down = [255, 0, 0];
        self.candle_neutral = [150, 150, 150];
        self.candle_fill_alpha = 0.85;
        self.book_bg = [255, 255, 255];
        self.book_bg_ask = [255, 244, 242];
        self.book_bg_bid = [243, 250, 242];
        self.book_bid = [0, 128, 0];
        self.book_ask = [255, 0, 0];
        self.book_level_alpha = 0.5;
        self.book_level_width = 1.5;
        self.label_positive = [0, 128, 0];
        self.label_negative = [255, 0, 0];
        self.label_neutral = [0, 0, 0];
        self.axis_label = [0, 0, 0];
        self.caption_label = [0, 0, 0];
        self.readout_label = [0, 0, 0];
        self.readout_bg_alpha = 0.96;
        self.readout_soft_bg_alpha = 0.20;
        self.line_label_bg_alpha = 0.85;
        self.readout_border_alpha = 0.0;
        self.readout_border_px = 0.0;
    }

}

/// Тема чарта ОТДЕЛЬНО для тёмного и светлого режима UI (per-theme, как
/// [`super::OrdersStyleSet`]). Хранится в одном `theme.toml` таблицами `[dark]`/`[light]`;
/// активный набор выбирается по `ui_theme_mode`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChartThemeSet {
    pub dark: ChartTheme,
    pub light: ChartTheme,
}

impl Default for ChartThemeSet {
    fn default() -> Self {
        Self {
            dark: ChartTheme::default(),
            light: ChartTheme::default_light(),
        }
    }
}

impl ChartThemeSet {
    /// Набор для активного режима: `light=true` → светлый, иначе тёмный.
    pub fn get(&self, light: bool) -> &ChartTheme {
        if light { &self.light } else { &self.dark }
    }
    pub fn get_mut(&mut self, light: bool) -> &mut ChartTheme {
        if light { &mut self.light } else { &mut self.dark }
    }

    /// Прочитать `theme.toml`. Новый формат — таблицы `[dark]`/`[light]`. СТАРЫЙ плоский
    /// `ChartTheme` (де-факто тёмная тема; в светлом режиме его перекрывали дефолты) →
    /// становится `dark`, `light` берёт светлый дефолт; сразу пере-сохраняем. Нет файла →
    /// дефолт + досейв; битый → дефолт (не падаем).
    pub fn load() -> Self {
        let path = paths::theme_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            let def = Self::default();
            let _ = def.save();
            return def;
        };
        if text.contains("[dark") || text.contains("[light") {
            return toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("theme.toml повреждён ({e}); беру дефолт");
                Self::default()
            });
        }
        let flat: ChartTheme = toml::from_str(&text).unwrap_or_default();
        let set = Self {
            dark: flat,
            light: ChartTheme::default_light(),
        };
        let _ = set.save();
        set
    }

    /// Записать theme.toml (открытый человекочитаемый TOML — можно делиться).
    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::theme_path(), self, "theme.toml")
    }

    /// Текст в формате theme.toml — для «Копировать» в Настройках (= содержимое файла).
    pub fn to_share_string(&self) -> Option<String> {
        toml::to_string_pretty(self).ok()
    }

    /// Разобрать текст theme.toml (вставка из буфера / содержимое файла). Валидируем по
    /// характерным ключам темы — serde игнорирует незнакомые поля и на чужом файле молча
    /// дал бы дефолт. Старый плоский `ChartTheme` (де-факто тёмный) → в `dark` поверх
    /// `current` (как миграция load). `None` = это не тема чарта.
    pub fn parse_share(text: &str, current: &Self) -> Option<Self> {
        const KEYS: [&str; 4] = ["bg", "cross", "book_bid", "panel_bg"];
        let v: toml::Value = toml::from_str(text).ok()?;
        let table_has = |name: &str| {
            v.get(name)
                .and_then(|x| x.as_table())
                .is_some_and(|t| KEYS.iter().any(|k| t.contains_key(*k)))
        };
        if table_has("dark") || table_has("light") {
            return toml::from_str(text).ok();
        }
        if v.as_table()
            .is_some_and(|t| KEYS.iter().any(|k| t.contains_key(*k)))
        {
            let flat: ChartTheme = toml::from_str(text).ok()?;
            return Some(Self {
                dark: flat,
                light: current.light.clone(),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HotkeysConfig, OrdersStyleSet};

    /// Round-trip: скопированный текст вкладки вставляется обратно 1:1.
    #[test]
    fn share_roundtrip() {
        let mut set = ChartThemeSet::default();
        set.dark.bg = [1, 2, 3];
        set.light.grid = [7, 8, 9];
        let text = set.to_share_string().unwrap();
        let parsed = ChartThemeSet::parse_share(&text, &ChartThemeSet::default()).unwrap();
        assert_eq!(parsed, set);
    }

    /// Старый плоский theme.toml → dark (light остаётся текущим).
    #[test]
    fn share_flat_legacy_goes_dark() {
        let mut flat = ChartTheme::default();
        flat.bg = [10, 20, 30];
        let text = toml::to_string_pretty(&flat).unwrap();
        let mut current = ChartThemeSet::default();
        current.light.bg = [200, 200, 200];
        let parsed = ChartThemeSet::parse_share(&text, &current).unwrap();
        assert_eq!(parsed.dark, flat);
        assert_eq!(parsed.light, current.light);
    }

    /// Чужие файлы (orders.toml/hotkeys.toml/мусор) НЕ проходят как тема — и наоборот.
    #[test]
    fn share_rejects_foreign_files() {
        let orders = OrdersStyleSet::default().to_share_string().unwrap();
        let hotkeys = HotkeysConfig::default().to_share_string().unwrap();
        let theme = ChartThemeSet::default().to_share_string().unwrap();
        let cur_t = ChartThemeSet::default();
        let cur_o = OrdersStyleSet::default();

        assert!(ChartThemeSet::parse_share(&orders, &cur_t).is_none());
        assert!(ChartThemeSet::parse_share(&hotkeys, &cur_t).is_none());
        assert!(OrdersStyleSet::parse_share(&theme, &cur_o).is_none());
        assert!(OrdersStyleSet::parse_share(&hotkeys, &cur_o).is_none());
        assert!(HotkeysConfig::parse_share(&theme).is_none());
        assert!(HotkeysConfig::parse_share(&orders).is_none());
        assert!(ChartThemeSet::parse_share("не toml вовсе {", &cur_t).is_none());

        // Свои файлы — проходят.
        assert!(OrdersStyleSet::parse_share(&orders, &cur_o).is_some());
        assert!(HotkeysConfig::parse_share(&hotkeys).is_some());
    }
}
