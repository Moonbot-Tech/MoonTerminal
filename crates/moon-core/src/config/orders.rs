//! Стиль линий ордеров на чарте — ОТДЕЛЬНЫЙ переносимый файл `orders.toml` рядом с
//! exe (как theme.toml). Для каждого вида линии (buy/sell/stop/trailing/tp/vstop/
//! cond/liq) задаются цвет, толщина, маркеры начала/конца (крест 45°), узелки
//! перестановок и их размеры. Глобально — прозрачность активных/закрытых линий и
//! cap хранения закрытых ордеров. Цвета в sRGB (linear считают шейдеры).

use serde::{Deserialize, Serialize};

use super::paths;
use crate::palette;

/// Стиль одного вида линии. `#[serde(default)]` — отсутствующие поля в présent
/// таблице берут generic-дефолт (см. `Default`), цвет лучше задавать явно.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LineStyle {
    /// Цвет линии и маркеров (sRGB).
    pub color: [u8; 3],
    /// Толщина линии, px.
    pub thickness: f32,
    /// Рисовать крест в точке начала (выставления).
    pub start_marker: bool,
    /// Рисовать крест в точке конца (закрытия/отмены).
    pub end_marker: bool,
    /// Полудлина «плеча» креста, px (тик ≈ 3.5 → ×3 ≈ 10).
    pub marker_size: f32,
    /// Толщина линий креста, px.
    pub marker_thickness: f32,
    /// Рисовать узелок-точку на каждой перестановке цены.
    pub knots: bool,
    /// Полуразмер узелка, px.
    pub knot_size: f32,
    /// Базовый пунктир линии (например, pending-условие).
    pub dashed: bool,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self {
            color: palette::TEXT_2,
            thickness: 1.5,
            start_marker: true,
            end_marker: true,
            marker_size: 4.0,
            marker_thickness: 1.5,
            knots: true,
            knot_size: 3.0,
            dashed: false,
        }
    }
}

impl LineStyle {
    fn with(color: [u8; 3]) -> Self {
        Self {
            color,
            ..Self::default()
        }
    }
    /// Тот же стиль, но без крестов начала/конца (для trailing/tp/vstop).
    fn no_markers(mut self) -> Self {
        self.start_marker = false;
        self.end_marker = false;
        self
    }
}

/// Стиль «пути» (trail) — змейка реального движения линии по истории перестановок.
/// Отдельно от основной (прямой) линии: своя галка показа, цвет, толщина, пунктир.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PathStyle {
    /// Показывать путь (змейку исторических позиций).
    pub show: bool,
    /// Цвет пути (sRGB).
    pub color: [u8; 3],
    /// Толщина пути, px.
    pub thickness: f32,
    /// Пунктир.
    pub dashed: bool,
}

impl Default for PathStyle {
    fn default() -> Self {
        Self {
            show: false,
            color: palette::TEXT_3,
            thickness: 1.0,
            dashed: true,
        }
    }
}

/// Полный стиль линий ордеров (orders.toml).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OrdersStyle {
    /// Линия цены покупки (buy_price). По умолчанию оранжевый.
    pub buy: LineStyle,
    /// Линия цены продажи (sell_price). По умолчанию синий.
    pub sell: LineStyle,
    /// Стоп-лосс. Красный.
    pub stop: LineStyle,
    /// Трейлинг-стоп. Светло-синий.
    pub trailing: LineStyle,
    /// Тейк-профит. Зелёный.
    pub take_profit: LineStyle,
    /// VStop. Фиолетовый.
    pub vstop: LineStyle,
    /// Pending-условие (BuyCondPrice). Серый, пунктир.
    pub pending_cond: LineStyle,
    /// Линия ликвидации. Красная, БЕЗ маркеров начала/конца (непрерывная).
    pub liq: LineStyle,
    /// Путь (trail) — змейка движения линий по истории перестановок (опц.).
    pub path: PathStyle,
    /// Прозрачность серверной трассы ордера (`CO_OrderLine.Thikness2` в MoonBot).
    pub trace_alpha: f32,

    /// Прозрачность активных линий, 0..1.
    pub active_alpha: f32,
    /// Прозрачность закрытых (отменённых/исполненных) линий, 0..1.
    pub closed_alpha: f32,
    /// Pending-ордер: линию входа рисовать пунктиром.
    pub pending_dashed: bool,
    /// Cap хранения закрытых ордеров на (ядро,рынок) — ограничение памяти.
    pub max_closed_orders: u32,
}

impl Default for OrdersStyle {
    fn default() -> Self {
        // Синий/светло-синий/фиолетовый литералами (нет в палитре).
        const BLUE: [u8; 3] = [0x5a, 0x96, 0xff];
        const LIGHT_BLUE: [u8; 3] = palette::TP;
        const PURPLE: [u8; 3] = [0xb4, 0x78, 0xff];

        let liq = LineStyle {
            start_marker: false,
            end_marker: false,
            knots: false,
            ..LineStyle::with(palette::RED)
        };
        let pending_cond = LineStyle {
            dashed: true,
            ..LineStyle::with(palette::TEXT_2)
        };
        Self {
            buy: LineStyle::with(palette::ORANGE),
            sell: LineStyle::with(BLUE),
            stop: LineStyle::with(palette::RED),
            trailing: LineStyle::with(LIGHT_BLUE).no_markers(),
            take_profit: LineStyle::with(palette::GREEN).no_markers(),
            vstop: LineStyle::with(PURPLE).no_markers(),
            pending_cond,
            liq,
            path: PathStyle::default(),
            trace_alpha: 0.4,
            active_alpha: 0.95,
            closed_alpha: 0.35,
            pending_dashed: true,
            max_closed_orders: 500,
        }
    }
}

impl OrdersStyle {
    /// Прочитать orders.toml рядом с exe. Нет файла → дефолт + досейв (чтобы у
    /// пользователя сразу был полный файл с правильными цветами для правки).
    /// Битый → дефолт (не падаем).
    pub fn load() -> Self {
        let path = paths::orders_path();
        if !path.exists() {
            let def = Self::default();
            let _ = def.save();
            return def;
        }
        super::toml_io::load_or_default(&path, "orders.toml", |_| {})
    }

    /// Записать orders.toml (открытый человекочитаемый TOML — можно делиться).
    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::orders_path(), self, "orders.toml")
    }
}
