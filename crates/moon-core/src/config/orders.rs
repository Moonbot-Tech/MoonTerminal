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
    /// Прозрачность линий ВЫСТАВЛЕННОГО, но ещё не исполненного ордера (fill=0), 0..1.
    /// Применяется на уровне ордера по его входной линии: `buy` (лонг) или `buy_short`
    /// (шорт). После исполнения линии рисуются на `active_alpha`. Остальные линии поле
    /// игнорируют (значимо только для `buy`/`buy_short`).
    pub pending_alpha: f32,
    /// Цвет входной линии ВЫСТАВЛЕННОГО, но ещё не исполненного ордера (fill=0).
    /// `None` = брать основной `color` (т.е. выставленный = исполненный, только бледнее).
    /// Значимо только для `buy`/`buy_short`; после фила берётся `color`.
    #[serde(default)]
    pub pending_color: Option<[u8; 3]>,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self {
            color: palette::TEXT_2,
            thickness: 1.0,
            start_marker: true,
            end_marker: true,
            marker_size: 4.0,
            marker_thickness: 1.5,
            knots: true,
            knot_size: 3.0,
            dashed: false,
            pending_alpha: 0.65,
            pending_color: None,
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
    /// Толщина линии (билдер).
    fn t(mut self, thickness: f32) -> Self {
        self.thickness = thickness;
        self
    }
    /// Цвет невыставленного (выставлен, не исполнен) — билдер.
    fn pending(mut self, color: [u8; 3]) -> Self {
        self.pending_color = Some(color);
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
            show: true,
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
    /// Линия входа ЛОНГ-ордера (buy_price). По умолчанию оранжевый.
    pub buy: LineStyle,
    /// Линия входа ШОРТ-ордера — отдельный цвет/стиль от лонга (как long/short в MoonBot).
    /// Применяется к линии входа, кресту и подписи размера, когда ордер шорт.
    pub buy_short: LineStyle,
    /// Линия цены продажи (sell_price) ЛОНГ-ордера. По умолчанию синий.
    pub sell: LineStyle,
    /// Линия цены продажи ШОРТ-ордера — отдельный цвет/стиль от лонга (как long/short в
    /// MoonBot: BuyShort/SellShort). Применяется к sell-линии, когда ордер шорт.
    pub sell_short: LineStyle,
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
    /// Дефолт = ТЁМНЫЙ набор пользователя (бейк из его `orders.toml`).
    fn default() -> Self {
        let liq = LineStyle {
            start_marker: false,
            end_marker: false,
            knots: false,
            thickness: 1.5,
            ..LineStyle::with([255, 48, 0])
        };
        let pending_cond = LineStyle {
            dashed: true,
            ..LineStyle::with([151, 146, 138])
        };
        Self {
            buy: LineStyle::with([255, 179, 71]).t(1.2).pending([255, 217, 61]),
            buy_short: LineStyle::with([255, 179, 71])
                .t(1.2)
                .pending([232, 228, 220]),
            sell: LineStyle::with([143, 208, 240]).t(1.2),
            sell_short: LineStyle::with([127, 201, 255]).t(1.2),
            stop: LineStyle::with([255, 74, 74]).t(1.2),
            trailing: LineStyle::with([92, 111, 224]).no_markers(),
            take_profit: LineStyle::with([47, 168, 92]).no_markers(),
            vstop: LineStyle::with([180, 120, 255]).no_markers(),
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
    /// Дефолт СВЕТЛОГО набора (бейк из `[light]` в `orders.toml` пользователя).
    fn default_light() -> Self {
        let liq = LineStyle {
            start_marker: false,
            end_marker: false,
            knots: false,
            thickness: 1.5,
            ..LineStyle::with([255, 0, 0])
        };
        let pending_cond = LineStyle {
            dashed: true,
            ..LineStyle::with([151, 146, 138])
        };
        Self {
            buy: LineStyle::with([0, 0, 0]),
            buy_short: LineStyle::with([144, 0, 160]),
            sell: LineStyle::with([0, 0, 255]),
            sell_short: LineStyle::with([139, 0, 0]),
            stop: LineStyle::with([255, 74, 74]),
            trailing: LineStyle::with([0, 0, 128]).no_markers(),
            take_profit: LineStyle::with([47, 168, 92]).no_markers(),
            vstop: LineStyle::with([180, 120, 255]).no_markers(),
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

/// Стили линий ОТДЕЛЬНО для тёмной и светлой темы (per-theme). Хранится в одном `orders.toml`
/// двумя таблицами `[dark]` / `[light]`. Активный набор выбирается по текущей теме приложения.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OrdersStyleSet {
    pub dark: OrdersStyle,
    pub light: OrdersStyle,
}

impl Default for OrdersStyleSet {
    fn default() -> Self {
        // Дефолты = текущие наборы пользователя: тёмный и СВОЙ светлый (бейк из orders.toml).
        Self {
            dark: OrdersStyle::default(),
            light: OrdersStyle::default_light(),
        }
    }
}

impl OrdersStyleSet {
    /// Набор для активной темы: `light=true` → светлый, иначе тёмный.
    pub fn get(&self, light: bool) -> &OrdersStyle {
        if light { &self.light } else { &self.dark }
    }
    pub fn get_mut(&mut self, light: bool) -> &mut OrdersStyle {
        if light { &mut self.light } else { &mut self.dark }
    }

    /// Прочитать `orders.toml`. Новый формат — таблицы `[dark]`/`[light]`. СТАРЫЙ плоский
    /// `OrdersStyle` (без них) мигрируем в ОБА набора и сразу пере-сохраняем. Нет файла → дефолт
    /// + досейв; битый → дефолт (не падаем).
    pub fn load() -> Self {
        let path = paths::orders_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            let def = Self::default();
            let _ = def.save();
            return def;
        };
        // Новый формат: присутствует таблица темы.
        if text.contains("[dark") || text.contains("[light") {
            return toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("orders.toml повреждён ({e}); беру дефолт");
                Self::default()
            });
        }
        // Старый плоский файл → один и тот же стиль в оба набора + миграция формата на диск.
        let flat: OrdersStyle = toml::from_str(&text).unwrap_or_default();
        let set = Self {
            light: flat.clone(),
            dark: flat,
        };
        let _ = set.save();
        set
    }

    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::orders_path(), self, "orders.toml")
    }
}
