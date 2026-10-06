//! Move kinds, gestures and the other hotkey vocabulary types.

use super::*;

/// Moonbot's "Move kind": WHICH orders a move gesture addresses, and how the core lays them out.
///
/// The gesture names a destination price; this names the set and the arrangement. Both halves go on
/// the wire in one command and the CORE does the work — the terminal computes no layout of its own,
/// which is why these variants are a transcription of Moonbot's dropdown rather than a design.
///
/// Delphi calls the four settings behind it `ReplaceBuyKind`, `ReplaceSellKind` and their `2`
/// twins; moonproto calls the wire enum `BulkMoveKind`. The lists match value for value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MoveKind {
    /// Moonbot: `None` — the gesture is recognised and sends nothing.
    None,
    /// Moonbot: "Parallel Shift to cursor". The line nearest the click lands on it and the rest
    /// keep their spacing. Moonbot's own default, and the arrangement the desk trades with.
    #[default]
    ParallelShift,
    /// Moonbot: "Top Vol first".
    TopVolume,
    /// Moonbot: "Low Vol first".
    LowVolume,
    /// Moonbot: "Top Profit first".
    TopProfit,
    /// Moonbot: "All to 1 price" — every addressed order onto the clicked price.
    AllToOnePrice,
    /// Moonbot: "Last Set".
    LastSet,
    /// Moonbot: "Last Moved".
    LastMoved,
}

impl MoveKind {
    /// Every kind in Moonbot's own dropdown order, for the settings selector.
    pub const ALL: [Self; 8] = [
        Self::None,
        Self::ParallelShift,
        Self::TopVolume,
        Self::LowVolume,
        Self::TopProfit,
        Self::AllToOnePrice,
        Self::LastSet,
        Self::LastMoved,
    ];

    /// Locale key of this kind's own name, so the three surfaces that draw the list cannot come
    /// to look it up under different keys.
    pub fn locale_key(self) -> String {
        format!("hotkeys.move_kind.{}", self.id())
    }

    /// Stable identifier for locale keys and for the settings selector's element ids.
    pub fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ParallelShift => "parallel-shift",
            Self::TopVolume => "top-volume",
            Self::LowVolume => "low-volume",
            Self::TopProfit => "top-profit",
            Self::AllToOnePrice => "all-to-one-price",
            Self::LastSet => "last-set",
            Self::LastMoved => "last-moved",
        }
    }
}

/// What a recognised move gesture sends: Moonbot's `MoveAllBuys` / `MoveAllSells` in three fields.
///
/// The destination price is not here — it comes from where the pointer was, which the config layer
/// knows nothing about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MoveGestureCommand {
    /// Whether the sell side is addressed (`MoveAllSells`) rather than the buy side.
    pub sell: bool,
    /// Which orders to take and how to lay them out.
    pub kind: MoveKind,
    /// Which position side the orders belong to.
    pub side: MoveSide,
}

/// Which side's orders a bulk move addresses — Moonbot's Long and Short gesture columns.
///
/// `Both` means the press claimed both slots and says nothing about the side by itself, which is
/// the shipped case: `same_hotkeys_for_move` copies the long gestures onto the short ones. A caller
/// that can see the market narrows it to the side actually open there before sending — on a hedged
/// market `Both` would reprice the other position's orders too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoveSide {
    Long,
    Short,
    Both,
}

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

    /// How a menu names this gesture: the readable form with Moonbot's own name beside it.
    ///
    /// Both surfaces that offer the list draw it this way — a trader reads one of them beside
    /// Moonbot's dialog, where `Ctrl+Left` alone does not match `CTRL_Click` on sight. Built here
    /// rather than at each call site so the two cannot drift.
    pub fn menu_label(self) -> String {
        format!("{} ({})", self.label(), self.moonbot_name())
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

/// Which modifier turns the mouse wheel over a chart label column into a scroll of that column.
///
/// Not a keyboard slot and not a click gesture: the wheel reaches the chart through one entry
/// point, so the binding is read there directly instead of going through the hotkey resolver.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WheelModifier {
    /// Never scrolls the column.
    None,
    /// The wheel with no modifier held.
    Plain,
    #[default]
    Alt,
    Ctrl,
    Shift,
}

impl WheelModifier {
    pub const ALL: [Self; 5] = [Self::None, Self::Plain, Self::Alt, Self::Ctrl, Self::Shift];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Plain => "plain",
            Self::Alt => "alt",
            Self::Ctrl => "ctrl",
            Self::Shift => "shift",
        }
    }

    /// Whether a wheel event carrying exactly these modifiers is this binding; `None` never is.
    pub fn matches(self, ctrl: bool, shift: bool, alt: bool) -> bool {
        match self {
            Self::None => false,
            Self::Plain => !ctrl && !shift && !alt,
            Self::Alt => alt && !ctrl && !shift,
            Self::Ctrl => ctrl && !shift && !alt,
            Self::Shift => shift && !ctrl && !alt,
        }
    }
}
