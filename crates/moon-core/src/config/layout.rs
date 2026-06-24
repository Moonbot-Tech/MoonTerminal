//! Раскладка окон — отдельный переносимый `layout.toml` рядом с exe (как
//! `theme.toml`). Хранит позиции/размеры окон групп, свёрнут ли док и активную
//! вкладку, а также список откреплённых окон (какая вкладка, из какой группы,
//! геометрия). Общая на всех (один файл). Битый/отсутствующий файл → дефолт.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::paths;

/// Геометрия+состояние окна группы (ключ карты — имя группы).
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct GroupLayout {
    /// Внешняя позиция окна (физ. пиксели десктопа).
    pub x: i32,
    pub y: i32,
    /// Внутренний размер (физ. пиксели).
    pub w: u32,
    pub h: u32,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub collapsed: bool,
    /// Индекс активной вкладки дока (см. `DockTab::idx`).
    #[serde(default)]
    pub tab: u8,
    /// Высота развёрнутого дока (точки egui). 0 = не задано → дефолт.
    #[serde(default)]
    pub dock_h: f32,
    /// Сортировка ордеров: 0=по созданию, 1=Sell первые, 2=Buy первые.
    #[serde(default)]
    pub orders_primary: u8,
    /// Сортировка ордеров по времени: новые первыми.
    #[serde(default = "def_true")]
    pub orders_newest_first: bool,
    /// Фильтр ордеров «только текущий маркет».
    #[serde(default)]
    pub orders_only_current: bool,
    /// Фильтр типа ордеров: 0=все, 1=реальные, 2=эмуляторные.
    #[serde(default)]
    pub orders_kind: u8,
}

fn def_true() -> bool {
    true
}

/// Прямоугольник окна (внешняя позиция + внутренний размер, физ. пиксели).
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct GeomRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Одно откреплённое окно вкладки.
#[derive(Clone, Serialize, Deserialize)]
pub struct DetachedLayout {
    /// Индекс вкладки (см. `DockTab::idx`).
    pub tab: u8,
    /// Имя группы-владельца (для Orders — чьи ордера; для глобальных — откуда открыт).
    pub owner_group: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Полная раскладка окон.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct WindowLayout {
    /// Окна групп по имени группы.
    #[serde(default)]
    pub groups: HashMap<String, GroupLayout>,
    /// Открытые откреплённые окна вкладок.
    #[serde(default)]
    pub detached: Vec<DetachedLayout>,
    /// Запомненная геометрия окон открепления по ключу (даже после закрытия) —
    /// чтобы повторное открепление той же вкладки вставало на прежнее место.
    /// Ключ: `g:<idx>` для глобальных, `o:<idx>:<группа>` для Orders (см. App).
    #[serde(default)]
    pub detached_geom: HashMap<String, GeomRect>,
    /// Геометрия окна «Стратегии» (отдельное окно) — чтобы открывалось на прежнем месте.
    #[serde(default)]
    pub strategies_window: Option<GeomRect>,
    /// Геометрия глобального окна «Активы» (singleton) — чтобы открывалось на прежнем месте.
    #[serde(default)]
    pub assets_window: Option<GeomRect>,
    /// Геометрия окна «Настройки» (отдельное окно) — чтобы открывалось на прежнем месте.
    #[serde(default)]
    pub settings_window: Option<GeomRect>,
}

impl WindowLayout {
    /// Загрузить layout.toml. Нет файла → дефолт; битый → лог + дефолт.
    pub fn load() -> Self {
        super::toml_io::load_or_default(&paths::layout_path(), "layout.toml", |_| {})
    }

    /// Записать layout.toml (не фатально: при ошибке только лог).
    pub fn save(&self) {
        if let Err(e) = super::toml_io::save(&paths::layout_path(), self, "layout.toml") {
            log::warn!("{e:#}");
        }
    }
}
