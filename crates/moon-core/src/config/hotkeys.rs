//! Public configuration for hotkeys and mouse gestures.
//!
//! Keyboard shortcuts are stored in `gpui::Keystroke::parse` format (`ctrl-r`,
//! `shift-f7`, `ctrl-delete`). An empty string means the action has no hotkey.
//! Mouse gestures mirror Delphi's `TOrderReplaceClick`.

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
    /// Manual order size F1-F6 (`HotkeysConfig.OKeys` in Moonbot).
    #[serde(default = "default_order_size_keys")]
    pub order_size: [String; ORDER_SIZE_KEYS],
    /// Fixed sell S1-S6 (`HotkeysConfig.SKeys` in Moonbot).
    #[serde(default = "default_sell_preset_keys")]
    pub sell_preset: [String; SELL_PRESET_KEYS],
    /// Manual strategy buttons 1-10 (`ManualStratsConfig.hotKeys` in Moonbot).
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
    /// Shifts orders for the active chart's market by one price step (`move_order`): entry
    /// (buy, while unfilled) / exit (sell) up or down.
    #[serde(default)]
    pub shift_buy_up: String,
    #[serde(default)]
    pub shift_buy_down: String,
    #[serde(default)]
    pub shift_sell_up: String,
    #[serde(default)]
    pub shift_sell_down: String,

    // Moonbot hotkeys that have NO corresponding send commands in moonproto (reload book/chart,
    // make shot, spy, show charts, fit sells, broadcast, sell +/-) were removed completely on
    // 2026-07-10 (configuration + tab + dispatcher); serde silently ignores their keys in old
    // hotkeys.toml files. Restore them from git history if a command becomes available.
    #[serde(default = "default_scale_plus")]
    pub scale_plus: String,
    #[serde(default = "default_scale_minus")]
    pub scale_minus: String,
    #[serde(default = "default_switch_figure")]
    pub switch_figure: String,

    /// Figure drawing layer: arms a tool. Pressing the same hotkey again disarms it, leaving the
    /// drawn figures in place. Defaults use Ctrl (Windows intercepts Alt combinations for the
    /// window menu, so they never reach the handler and produce only a system sound).
    #[serde(default = "default_draw_hline")]
    pub draw_hline: String,
    #[serde(default = "default_draw_segment")]
    pub draw_segment: String,
    #[serde(default = "default_draw_triangle")]
    pub draw_triangle: String,
    #[serde(default = "default_draw_channel")]
    pub draw_channel: String,
    /// Deletes the selected figure.
    #[serde(default = "default_fig_delete")]
    pub fig_delete: String,
    /// Toggles the "Alert" checkbox on the selected figure (arms/disarms the chart alert).
    #[serde(default = "default_fig_alert")]
    pub fig_alert: String,

    /// Live Moonbot MultiOrders path: places a long from the order book.
    #[serde(default = "default_left_double")]
    pub buy_set_click: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: places a short from the order book.
    #[serde(default)]
    pub short_set_click: MouseGestureBinding,
    /// Live Moonbot path: places a pending long.
    #[serde(default)]
    pub pending_long_click: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: places a pending short.
    #[serde(default)]
    pub pending_short_click: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: moves an open/buy long.
    #[serde(default = "default_left_shift")]
    pub buy_move_click: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: moves a TP/sell long.
    #[serde(default = "default_left_ctrl")]
    pub sell_move_click: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: secondary gesture for moving an open/buy long.
    #[serde(default)]
    pub buy_move_click2: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: secondary gesture for moving a TP/sell long.
    #[serde(default)]
    pub sell_move_click2: MouseGestureBinding,
    /// Delphi `SameHotkeysForMove`: short-move gestures mirror long-move gestures.
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
    /// Reads `hotkeys.toml`. `None` means the file does not exist yet (first launch after moving
    /// hotkeys out of settings.toml; the caller migrates the legacy section and writes the file).
    /// A corrupt file yields the default (and logs internally), NOT `None`; otherwise the corrupt
    /// file would be silently overwritten by the stale legacy copy from settings.toml.
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

    /// Writes `hotkeys.toml` (open, human-readable TOML that can be shared).
    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::hotkeys_path(), self, "hotkeys.toml")
    }

    /// Text in hotkeys.toml format for "Copy" in Settings (= file contents).
    pub fn to_share_string(&self) -> Option<String> {
        toml::to_string_pretty(self).ok()
    }

    /// Parses hotkeys.toml text (clipboard paste / file contents). Validates using distinctive
    /// keys; serde ignores unknown fields and would silently produce the default for a foreign file.
    /// `None` means the text is not a hotkey configuration.
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

// Keyboard defaults use the literal `ctrl-` on BOTH platforms (matching Moonbot: Mac controls
// also use Ctrl). On Mac, the physical Control key reaches keyboard-hotkey handling normally
// (unlike Ctrl+left click, which the OS turns into a right click; therefore the DRAWING mouse
// gesture remains on `secondary()`/Cmd, which is separate code rather than a hotkey default).
// Keys without a modifier (function keys, delete) remain as-is.
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
