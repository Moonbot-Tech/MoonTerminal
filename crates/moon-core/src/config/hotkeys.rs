//! Открытая конфигурация горячих клавиш и мышиных жестов.
//!
//! Клавиатура хранится в формате `gpui::Keystroke::parse` (`ctrl-r`,
//! `shift-f7`, `ctrl-delete`). Пустая строка = действие без хоткея.
//! Мышиные жесты повторяют Delphi `TOrderReplaceClick`.

use serde::{Deserialize, Serialize};

use super::paths;

pub const ORDER_SIZE_KEYS: usize = 6;
pub const SELL_PRESET_KEYS: usize = 6;
pub const MANUAL_STRATEGY_KEYS: usize = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MouseGestureBinding {
    /// Delphi: `None_Click`.
    #[default]
    None,
    /// Delphi: `Dbl_Click` — double left click without modifiers.
    LeftDouble,
    /// Delphi: `CTRL_Click`.
    LeftCtrl,
    /// Delphi: `Shift_Click`.
    LeftShift,
    /// Delphi: `Alt_Click`.
    LeftAlt,
    /// Delphi: `Mid_Click`.
    Middle,
    /// Delphi: `CTRL_Mid`.
    MiddleCtrl,
    /// Delphi: `Shift_Mid`.
    MiddleShift,
    /// Delphi: `Alt_Mid`.
    MiddleAlt,
    /// Delphi: `Dbl_Right` — double right click without modifiers.
    RightDouble,
    /// Delphi: `CTRL_Right`.
    RightCtrl,
    /// Delphi: `Shift_Right`.
    RightShift,
    /// Delphi: `Alt_Right`.
    RightAlt,
    /// Delphi: `CTRL_Dbl`.
    LeftCtrlDouble,
    /// Delphi: `Shift_Dbl`.
    LeftShiftDouble,
    /// Delphi: `Alt_Dbl`.
    LeftAltDouble,
}

impl MouseGestureBinding {
    pub const ALL: [Self; 16] = [
        Self::None,
        Self::LeftDouble,
        Self::LeftCtrl,
        Self::LeftShift,
        Self::LeftAlt,
        Self::Middle,
        Self::MiddleCtrl,
        Self::MiddleShift,
        Self::MiddleAlt,
        Self::RightDouble,
        Self::RightCtrl,
        Self::RightShift,
        Self::RightAlt,
        Self::LeftCtrlDouble,
        Self::LeftShiftDouble,
        Self::LeftAltDouble,
    ];

    pub fn moonbot_name(self) -> &'static str {
        match self {
            Self::None => "None_Click",
            Self::LeftDouble => "Dbl_Click",
            Self::LeftCtrl => "CTRL_Click",
            Self::LeftShift => "Shift_Click",
            Self::LeftAlt => "Alt_Click",
            Self::Middle => "Mid_Click",
            Self::MiddleCtrl => "CTRL_Mid",
            Self::MiddleShift => "Shift_Mid",
            Self::MiddleAlt => "Alt_Mid",
            Self::RightDouble => "Dbl_Right",
            Self::RightCtrl => "CTRL_Right",
            Self::RightShift => "Shift_Right",
            Self::RightAlt => "Alt_Right",
            Self::LeftCtrlDouble => "CTRL_Dbl",
            Self::LeftShiftDouble => "Shift_Dbl",
            Self::LeftAltDouble => "Alt_Dbl",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::LeftDouble => "Left dbl",
            Self::LeftCtrl => "Ctrl+Left",
            Self::LeftShift => "Shift+Left",
            Self::LeftAlt => "Alt+Left",
            Self::Middle => "Middle",
            Self::MiddleCtrl => "Ctrl+Middle",
            Self::MiddleShift => "Shift+Middle",
            Self::MiddleAlt => "Alt+Middle",
            Self::RightDouble => "Right dbl",
            Self::RightCtrl => "Ctrl+Right",
            Self::RightShift => "Shift+Right",
            Self::RightAlt => "Alt+Right",
            Self::LeftCtrlDouble => "Ctrl+Left dbl",
            Self::LeftShiftDouble => "Shift+Left dbl",
            Self::LeftAltDouble => "Alt+Left dbl",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::LeftDouble => "left-double",
            Self::LeftCtrl => "left-ctrl",
            Self::LeftShift => "left-shift",
            Self::LeftAlt => "left-alt",
            Self::Middle => "middle",
            Self::MiddleCtrl => "middle-ctrl",
            Self::MiddleShift => "middle-shift",
            Self::MiddleAlt => "middle-alt",
            Self::RightDouble => "right-double",
            Self::RightCtrl => "right-ctrl",
            Self::RightShift => "right-shift",
            Self::RightAlt => "right-alt",
            Self::LeftCtrlDouble => "left-ctrl-double",
            Self::LeftShiftDouble => "left-shift-double",
            Self::LeftAltDouble => "left-alt-double",
        }
    }

    pub fn from_config_value(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|gesture| gesture.config_value() == value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HotkeysConfig {
    /// Размер ручного ордера F1-F6 (`HotkeysConfig.OKeys` в Moonbot).
    #[serde(default = "default_order_size_keys")]
    pub order_size: [String; ORDER_SIZE_KEYS],
    /// Fixed sell S1-S6 (`HotkeysConfig.SKeys` в Moonbot).
    #[serde(default = "default_sell_preset_keys")]
    pub sell_preset: [String; SELL_PRESET_KEYS],
    /// Manual strategy buttons 1-10 (`ManualStratsConfig.hotKeys` в Moonbot).
    #[serde(default = "default_manual_strategy_keys")]
    pub manual_strategy: [String; MANUAL_STRATEGY_KEYS],

    #[serde(default)]
    pub cancel_buy: String,
    #[serde(default)]
    pub panic_sell: String,
    #[serde(default = "default_panic_sell_one")]
    pub panic_sell_one: String,
    #[serde(default = "default_cancel_all_buys")]
    pub cancel_all_buys: String,
    #[serde(default)]
    pub join_sells: String,
    #[serde(default)]
    pub switch_charts: String,
    #[serde(default)]
    pub new_long: String,
    #[serde(default)]
    pub new_short: String,
    #[serde(default)]
    pub split_order: String,
    #[serde(default)]
    pub split_order_x: String,
    /// Сдвиг ордеров рынка активного чарта на шаг цены (`move_order`): вход (buy, пока не
    /// залит) / выход (sell) вверх-вниз.
    #[serde(default)]
    pub shift_buy_up: String,
    #[serde(default)]
    pub shift_buy_down: String,
    #[serde(default)]
    pub shift_sell_up: String,
    #[serde(default)]
    pub shift_sell_down: String,

    // Moonbot-хоткеи, под которые в moonproto НЕТ send-команд (reload book/chart,
    // make shot, spy, show charts, fit sells, broadcast, sell +/-), удалены целиком
    // 2026-07-10 (конфиг+вкладка+диспетчер); serde молча игнорирует их ключи в старых
    // hotkeys.toml. Появится команда — возвращать по git-истории.
    #[serde(default = "default_scale_plus")]
    pub scale_plus: String,
    #[serde(default = "default_scale_minus")]
    pub scale_minus: String,
    #[serde(default = "default_switch_figure")]
    pub switch_figure: String,

    /// Слой рисования фигур (карандаш): тоггл инструментов. Повторное нажатие того же
    /// хоткея (или Esc) выключает режим. Дефолты на Ctrl (Alt-сочетания Windows перехватывает под меню окна — до обработчика не доходят, только системный звук).
    #[serde(default = "default_draw_hline")]
    pub draw_hline: String,
    #[serde(default = "default_draw_segment")]
    pub draw_segment: String,
    #[serde(default = "default_draw_triangle")]
    pub draw_triangle: String,
    #[serde(default = "default_draw_channel")]
    pub draw_channel: String,
    /// Удалить выделенную фигуру.
    #[serde(default = "default_fig_delete")]
    pub fig_delete: String,
    /// Тоггл галки «Alert» у выделенной фигуры (заармить/разоружить chart-алерт).
    #[serde(default = "default_fig_alert")]
    pub fig_alert: String,

    /// Живой Moonbot-путь MultiOrders: поставить long по стакану.
    #[serde(default = "default_left_double")]
    pub buy_set_click: MouseGestureBinding,
    /// Живой Moonbot-путь MultiOrders: поставить short по стакану.
    #[serde(default)]
    pub short_set_click: MouseGestureBinding,
    /// Живой Moonbot-путь: поставить pending long.
    #[serde(default)]
    pub pending_long_click: MouseGestureBinding,
    /// Живой Moonbot-путь MultiOrders: поставить pending short.
    #[serde(default)]
    pub pending_short_click: MouseGestureBinding,
    /// Живой Moonbot-путь MultiOrders: двигать open/buy long.
    #[serde(default = "default_left_shift")]
    pub buy_move_click: MouseGestureBinding,
    /// Живой Moonbot-путь MultiOrders: двигать TP/sell long.
    #[serde(default = "default_left_ctrl")]
    pub sell_move_click: MouseGestureBinding,
    /// Живой Moonbot-путь MultiOrders: второй жест движения open/buy long.
    #[serde(default)]
    pub buy_move_click2: MouseGestureBinding,
    /// Живой Moonbot-путь MultiOrders: второй жест движения TP/sell long.
    #[serde(default)]
    pub sell_move_click2: MouseGestureBinding,
    /// Delphi `SameHotkeysForMove`: short move жесты повторяют long move.
    #[serde(default = "default_same_hotkeys_for_move")]
    pub same_hotkeys_for_move: bool,
    #[serde(default = "default_left_shift")]
    pub short_buy_move_click: MouseGestureBinding,
    #[serde(default = "default_left_ctrl")]
    pub short_sell_move_click: MouseGestureBinding,
    #[serde(default)]
    pub short_buy_move_click2: MouseGestureBinding,
    #[serde(default)]
    pub short_sell_move_click2: MouseGestureBinding,
}

impl Default for HotkeysConfig {
    fn default() -> Self {
        Self {
            order_size: default_order_size_keys(),
            sell_preset: default_sell_preset_keys(),
            manual_strategy: default_manual_strategy_keys(),
            cancel_buy: String::new(),
            panic_sell: String::new(),
            panic_sell_one: default_panic_sell_one(),
            cancel_all_buys: default_cancel_all_buys(),
            join_sells: String::new(),
            switch_charts: String::new(),
            new_long: String::new(),
            new_short: String::new(),
            split_order: String::new(),
            split_order_x: String::new(),
            shift_buy_up: String::new(),
            shift_buy_down: String::new(),
            shift_sell_up: String::new(),
            shift_sell_down: String::new(),
            scale_plus: default_scale_plus(),
            scale_minus: default_scale_minus(),
            switch_figure: default_switch_figure(),
            draw_hline: default_draw_hline(),
            draw_segment: default_draw_segment(),
            draw_triangle: default_draw_triangle(),
            draw_channel: default_draw_channel(),
            fig_delete: default_fig_delete(),
            fig_alert: default_fig_alert(),
            buy_set_click: default_left_double(),
            short_set_click: MouseGestureBinding::None,
            pending_long_click: MouseGestureBinding::None,
            pending_short_click: MouseGestureBinding::None,
            buy_move_click: default_left_shift(),
            sell_move_click: default_left_ctrl(),
            buy_move_click2: MouseGestureBinding::None,
            sell_move_click2: MouseGestureBinding::None,
            same_hotkeys_for_move: default_same_hotkeys_for_move(),
            short_buy_move_click: default_left_shift(),
            short_sell_move_click: default_left_ctrl(),
            short_buy_move_click2: MouseGestureBinding::None,
            short_sell_move_click2: MouseGestureBinding::None,
        }
    }
}

impl HotkeysConfig {
    /// Прочитать `hotkeys.toml`. `None` = файла ещё нет (первый запуск после переезда
    /// хоткеев из settings.toml — вызывающий мигрирует legacy-секцию и записывает файл).
    /// Битый файл → дефолт (лог внутри), а НЕ `None` — иначе битый файл молча
    /// перезаписался бы устаревшей legacy-копией из settings.toml.
    pub fn load() -> Option<Self> {
        let path = paths::hotkeys_path();
        if !path.exists() {
            return None;
        }
        Some(super::toml_io::load_or_default(
            &path,
            "hotkeys.toml",
            |_| {},
        ))
    }

    /// Записать `hotkeys.toml` (открытый человекочитаемый TOML — можно делиться).
    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::hotkeys_path(), self, "hotkeys.toml")
    }

    /// Текст в формате hotkeys.toml — для «Копировать» в Настройках (= содержимое файла).
    pub fn to_share_string(&self) -> Option<String> {
        toml::to_string_pretty(self).ok()
    }

    /// Разобрать текст hotkeys.toml (вставка из буфера / содержимое файла). Валидируем по
    /// характерным ключам — serde игнорирует незнакомые поля и на чужом файле молча дал бы
    /// дефолт. `None` = это не хоткеи.
    pub fn parse_share(text: &str) -> Option<Self> {
        const KEYS: [&str; 4] = ["order_size", "sell_preset", "buy_set_click", "draw_hline"];
        let v: toml::Value = toml::from_str(text).ok()?;
        if v.as_table()
            .is_some_and(|t| KEYS.iter().any(|k| t.contains_key(*k)))
        {
            toml::from_str(text).ok()
        } else {
            None
        }
    }
}

fn default_order_size_keys() -> [String; ORDER_SIZE_KEYS] {
    std::array::from_fn(|i| format!("f{}", i + 1))
}

fn default_sell_preset_keys() -> [String; SELL_PRESET_KEYS] {
    std::array::from_fn(|i| format!("shift-f{}", i + 7))
}

fn default_manual_strategy_keys() -> [String; MANUAL_STRATEGY_KEYS] {
    std::array::from_fn(|_| String::new())
}

// Клавиатурные дефолты — буквальный `ctrl-` на ОБЕИХ платформах (паритет с Moonbot: на маке
// управление тоже через Ctrl). На маке физический Control для клавиатурных хоткеев доходит
// нормально (в отличие от Ctrl+ЛКМ, который ОС превращает в правый клик — поэтому мышиный
// жест РИСОВАНИЯ остаётся на `secondary()`/Cmd, это отдельный код, не дефолт хоткея).
// Клавиши без модификатора (F-клавиши, delete) — как есть.
fn default_draw_hline() -> String {
    "ctrl-h".into()
}

fn default_draw_segment() -> String {
    "ctrl-l".into()
}

fn default_draw_triangle() -> String {
    "ctrl-t".into()
}

fn default_draw_channel() -> String {
    "ctrl-k".into()
}

fn default_fig_delete() -> String {
    "delete".into()
}

fn default_fig_alert() -> String {
    "ctrl-b".into()
}

fn default_scale_plus() -> String {
    "ctrl-q".into()
}

fn default_scale_minus() -> String {
    "ctrl-w".into()
}

fn default_switch_figure() -> String {
    "ctrl-f".into()
}

fn default_panic_sell_one() -> String {
    "ctrl-f1".into()
}

fn default_cancel_all_buys() -> String {
    "ctrl-delete".into()
}

fn default_left_double() -> MouseGestureBinding {
    MouseGestureBinding::LeftDouble
}

fn default_left_shift() -> MouseGestureBinding {
    MouseGestureBinding::LeftShift
}

fn default_left_ctrl() -> MouseGestureBinding {
    MouseGestureBinding::LeftCtrl
}

fn default_same_hotkeys_for_move() -> bool {
    true
}
