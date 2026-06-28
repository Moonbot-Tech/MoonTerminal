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

    // Стиль линий ордеров (цвета/толщины/маркеры) вынесен в отдельный orders.toml
    // (см. config::orders::OrdersStyle) — не дублируем его в теме.

    // --- Панели (egui-хром: тулбар, панель ордера, док ордеров, статус) ---
    /// Фон панелей (sRGB).
    pub panel_bg: [u8; 3],
}

impl Default for ChartTheme {
    fn default() -> Self {
        Self {
            bg: [29, 27, 25],   // тёплый тёмный фон чарта
            grid: [29, 30, 32], // едва заметная сетка
            grid_alpha: 1.0,
            background_opacity: 0.18,
            label_font_delta: -1.5,
            cross: [76, 54, 29], // приглушённый янтарный
            cross_alpha: 0.5,
            cross_thickness: 1.0,
            book_bg: [29, 27, 25],     // как фон чарта
            book_bid: palette::GREEN,  // --long (зелёный)
            book_ask: palette::ORANGE, // --short (оранжевый)
            book_level_alpha: 0.5,
            panel_bg: [24, 25, 27],
        }
    }
}

impl ChartTheme {
    /// Прочитать theme.toml рядом с exe. Нет файла или битый → дефолт (не падаем).
    pub fn load() -> Self {
        super::toml_io::load_or_default(&paths::theme_path(), "theme.toml", |_| {})
    }

    /// Записать theme.toml (открытый человекочитаемый TOML — можно делиться).
    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::theme_path(), self, "theme.toml")
    }
}
