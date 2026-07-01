//! Тема оформления чарта (фон/сетка/перекрестие) — ОТДЕЛЬНЫЙ переносимый файл
//! `theme.toml` рядом с exe, чтобы темой можно было делиться (скопировал файл —
//! и оформление перенеслось). Цвета заданы в sRGB (как палитра/egui); в linear
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

    // --- Стакан ---
    /// Фон зоны стакана (sRGB).
    pub book_bg: [u8; 3],
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
    /// Alpha лёгкой плашки подписей ордеров/угловой подписи.
    pub readout_soft_bg_alpha: f32,
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
            grid: [54, 54, 54],
            grid_alpha: 1.0,
            background_opacity: 0.18,
            label_font_delta: -1.5,
            cross: [128, 128, 128],
            cross_alpha: 0.5,
            cross_thickness: 1.0,
            book_bg: [30, 30, 30],
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
            readout_border_alpha: 0.0,
            readout_border_px: 0.0,
            panel_bg: [24, 25, 27],
        }
    }
}

impl ChartTheme {
    /// Светлые MoonBot-дефолты для тех частей chart theme, которые нельзя тащить из dark.
    /// Фон/сетка дальше могут быть уточнены активной MoonUI-палитрой приложения.
    pub fn apply_light_defaults(&mut self) {
        self.bg = [255, 255, 255];
        self.grid = [211, 211, 211];
        self.cross = [128, 128, 128];
        self.book_bg = [255, 255, 255];
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
        self.readout_border_alpha = 0.0;
        self.readout_border_px = 0.0;
    }

    /// Прочитать theme.toml рядом с exe. Нет файла или битый → дефолт (не падаем).
    pub fn load() -> Self {
        super::toml_io::load_or_default(&paths::theme_path(), "theme.toml", |_| {})
    }

    /// Записать theme.toml (открытый человекочитаемый TOML — можно делиться).
    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::theme_path(), self, "theme.toml")
    }
}
