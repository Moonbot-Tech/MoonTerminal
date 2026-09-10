//! Public configuration for hotkeys and mouse gestures.
//!
//! Keyboard shortcuts are stored in `gpui::Keystroke::parse` format (`ctrl-r`,
//! `shift-f7`, `ctrl-delete`). An empty string means the action has no hotkey.
//! Mouse gestures mirror Delphi's `TOrderReplaceClick`.

use serde::{Deserialize, Serialize};

use super::paths;

#[cfg(test)]
mod tests;

pub const ORDER_SIZE_KEYS: usize = 6;
pub const SELL_PRESET_KEYS: usize = 6;
pub const MANUAL_STRATEGY_KEYS: usize = 10;

/// Current `hotkeys.toml` generation. Bump only together with a new arm in
/// [`HotkeysConfig::fill_unbound_slots`].
///
/// 1: backfilled the slots that shipped unbound. 2: cleared `chart_shot` where the user had
/// already given Ctrl+F10 to something else. 3: the same for `fig_undo` on Ctrl+Z. 4: cleared the
/// two pending-order GESTURES, which stopped being inert and started placing live orders. 5: the
/// figure-delete gesture yields its Middle default to a trading gesture already on Middle.
const SCHEMA: u8 = 5;

/// Parts produced by the plain Split Order action, matching Moonbot, where that action always
/// splits a sell order into three. The configurable count belongs to `Split N` instead.
pub const SPLIT_ORDER_PARTS: i32 = 3;

/// Percent one press of the order-shift hotkeys moves a market's orders by, as WHOLE percent.
///
/// Moonbot names the actions "Shift buys +1%" / "-1%", and whole percent is what the command takes:
/// moonproto's own wire test for this payload builds it with `percent: 3.5`
/// (`commands/trade/order_v2.rs::move_all_percent_has_no_side_byte_on_protocol_v4_wire`), a value
/// that as a fraction would be 350%. The SIGN is inferred rather than documented — the payload
/// carries a raw signed f64 and moonproto states no convention, so positive-is-up comes from
/// Moonbot's own +/- pair of actions.
pub const SHIFT_PERCENT: f64 = 1.0;
/// Bounds for the configurable `Split N` count (Moonbot `Hotkeys.SplitParts`). Fewer than two
/// parts is not a split, and the upper bound keeps a mistyped import from shredding a position.
pub const SPLIT_PARTS_MIN: u8 = 2;
pub const SPLIT_PARTS_MAX: u8 = 10;

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

/// One editable keyboard slot of [`HotkeysConfig`].
///
/// The slot identity lives HERE, beside the struct it addresses, and not in the settings page that
/// draws it. That is the whole point of the type: the same twenty-nine slots were enumerated by
/// hand in half a dozen places — this file's own collision list, the page's field map, its id and
/// label tables, the clash order, the row layout — and each list could forget a slot on its own.
/// A list that cannot reach the data is the reason they could not be merged: `bound_keys` is in
/// this crate and the page's tables are not.
///
/// The three preset families are indexed rather than spelled out, because they are arrays in the
/// config and the page numbers them (`F1`..`F6`); an index outside its family names no slot and is
/// answered with an empty binding rather than a panic.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum KeySlot {
    /// Manual order size `F1`-`F6`, `0`-based.
    OrderSize(usize),
    /// Fixed sell `S1`-`S6`, `0`-based.
    SellPreset(usize),
    /// Manual strategy button `1`-`10`, `0`-based.
    ManualStrategy(usize),
    CancelBuy,
    PanicSell,
    PanicSellOne,
    CancelAllBuys,
    JoinSells,
    SwitchCharts,
    NewLong,
    NewShort,
    SplitOrder,
    SplitOrderX,
    SellsToRect,
    ShiftBuyUp,
    ShiftBuyDown,
    ShiftSellUp,
    ShiftSellDown,
    ScalePlus,
    ScaleMinus,
    SwitchFigure,
    ChartShot,
    DrawHline,
    DrawSegment,
    DrawTriangle,
    DrawChannel,
    FigDelete,
    FigAlert,
    FigUndo,
}

impl KeySlot {
    /// Every slot that is one named field rather than a member of an indexed family.
    ///
    /// The compiler cannot check this against the enum — a new variant left out simply goes
    /// unenumerated. What checks it is a test that serializes the config with every slot written a
    /// marker and looks for a stored keystroke that kept its own value: the STRUCT is the reference,
    /// never this list, because a test that walks this list to verify this list proves nothing.
    pub const NAMED: [Self; 26] = [
        Self::CancelBuy,
        Self::PanicSell,
        Self::PanicSellOne,
        Self::CancelAllBuys,
        Self::JoinSells,
        Self::SwitchCharts,
        Self::NewLong,
        Self::NewShort,
        Self::SplitOrder,
        Self::SplitOrderX,
        Self::SellsToRect,
        Self::ShiftBuyUp,
        Self::ShiftBuyDown,
        Self::ShiftSellUp,
        Self::ShiftSellDown,
        Self::ScalePlus,
        Self::ScaleMinus,
        Self::SwitchFigure,
        Self::ChartShot,
        Self::DrawHline,
        Self::DrawSegment,
        Self::DrawTriangle,
        Self::DrawChannel,
        Self::FigDelete,
        Self::FigAlert,
        Self::FigUndo,
    ];

    /// Every slot the file holds a key for, presets first.
    ///
    /// The ORDER is this list's own and means nothing to a dispatcher: which of two holders of one
    /// keystroke actually fires is `hotkeys::resolve_binding`'s question, transcribed by the
    /// settings page's clash index. Callers here only ever ask "what is bound at all".
    pub fn all() -> Vec<Self> {
        (0..ORDER_SIZE_KEYS)
            .map(Self::OrderSize)
            .chain((0..SELL_PRESET_KEYS).map(Self::SellPreset))
            .chain((0..MANUAL_STRATEGY_KEYS).map(Self::ManualStrategy))
            .chain(Self::NAMED)
            .collect()
    }

    /// The one name every spelling of this slot derives from: its field in `hotkeys.toml` —
    /// `cancel_buy`, or the array `order_size` for a preset family.
    ///
    /// The import's ids (`hotkey.cancel_buy`), the settings page's element ids (`cancel-buy`) and
    /// its locale keys (`hotkeys.cancel_buy`) are all this stem in a different dressing. Deriving
    /// them here is what stops a slot from being spelled three ways in three tables and missing
    /// from one of them; a test pins the stem to the field the file actually writes.
    pub fn stem(self) -> &'static str {
        match self {
            Self::OrderSize(_) => "order_size",
            Self::SellPreset(_) => "sell_preset",
            Self::ManualStrategy(_) => "manual_strategy",
            Self::CancelBuy => "cancel_buy",
            Self::PanicSell => "panic_sell",
            Self::PanicSellOne => "panic_sell_one",
            Self::CancelAllBuys => "cancel_all_buys",
            Self::JoinSells => "join_sells",
            Self::SwitchCharts => "switch_charts",
            Self::NewLong => "new_long",
            Self::NewShort => "new_short",
            Self::SplitOrder => "split_order",
            Self::SplitOrderX => "split_order_x",
            Self::SellsToRect => "sells_to_rect",
            Self::ShiftBuyUp => "shift_buy_up",
            Self::ShiftBuyDown => "shift_buy_down",
            Self::ShiftSellUp => "shift_sell_up",
            Self::ShiftSellDown => "shift_sell_down",
            Self::ScalePlus => "scale_plus",
            Self::ScaleMinus => "scale_minus",
            Self::SwitchFigure => "switch_figure",
            Self::ChartShot => "chart_shot",
            Self::DrawHline => "draw_hline",
            Self::DrawSegment => "draw_segment",
            Self::DrawTriangle => "draw_triangle",
            Self::DrawChannel => "draw_channel",
            Self::FigDelete => "fig_delete",
            Self::FigAlert => "fig_alert",
            Self::FigUndo => "fig_undo",
        }
    }

    /// Position inside a preset family, or `None` for a slot that is one named field.
    pub fn index(self) -> Option<usize> {
        match self {
            Self::OrderSize(i) | Self::SellPreset(i) | Self::ManualStrategy(i) => Some(i),
            _ => None,
        }
    }
}

/// One editable mouse-gesture slot of [`HotkeysConfig`] — a `<row>_click` field.
///
/// Beside [`KeySlot`] for the same reason that one is here: the page that draws the row, its clash
/// index, the core pull and the two dispatchers all need one list of the gesture rows, and a list
/// can only be shared from where the data is. Before this the same thirteen rows were enumerated by
/// hand in the page's field macro, its id table, its locale-key table, its short-row and move-pair
/// tables and the pull's own copies of those.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum GestureSlot {
    BuySet,
    ShortSet,
    PendingLong,
    PendingShort,
    BuyMove,
    SellMove,
    BuyMove2,
    SellMove2,
    ShortBuyMove,
    ShortSellMove,
    ShortBuyMove2,
    ShortSellMove2,
    /// Deletes the figure under the pointer. The one gesture that is not a trading gesture: a
    /// figure is pointed at, and only a click carries the position that says which one.
    FigDelete,
}

/// What one placement gesture places.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Placement {
    /// Position side: `false` Long, `true` Short.
    pub short: bool,
    /// Whether the clicked price is a pending TRIGGER rather than an entry price.
    pub pending: bool,
}

/// One half of a Moonbot move row: the row, and whether this is its short half.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MoveHalf {
    pub row: MoveKindSlot,
    pub short: bool,
}

impl GestureSlot {
    /// Every gesture slot, in the order the settings page lists them.
    ///
    /// The first four are the placement rows, and their order HERE is the order a press is tried
    /// against them: `placement_intent` walks this list and the first match fires. The compiler
    /// cannot check the list against the enum; a test writes every slot a distinct gesture and
    /// looks for a stored field that kept its default.
    pub const ALL: [Self; 13] = [
        Self::BuySet,
        Self::ShortSet,
        Self::PendingLong,
        Self::PendingShort,
        Self::BuyMove,
        Self::SellMove,
        Self::BuyMove2,
        Self::SellMove2,
        Self::ShortBuyMove,
        Self::ShortSellMove,
        Self::ShortBuyMove2,
        Self::ShortSellMove2,
        Self::FigDelete,
    ];

    /// The name the settings page's element ids (`buy-move2`) and locale keys
    /// (`hotkeys.mouse.buy_move2`) derive from.
    ///
    /// NOT the field name, unlike [`KeySlot::stem`]: the secondary rows are stored as
    /// `buy_move_click2`, Moonbot's own spelling, and the page has always numbered them the other
    /// way round. The field is reached through [`HotkeysConfig::gesture`] instead.
    pub fn stem(self) -> &'static str {
        match self {
            Self::BuySet => "buy_set",
            Self::ShortSet => "short_set",
            Self::PendingLong => "pending_long",
            Self::PendingShort => "pending_short",
            Self::BuyMove => "buy_move",
            Self::SellMove => "sell_move",
            Self::BuyMove2 => "buy_move2",
            Self::SellMove2 => "sell_move2",
            Self::ShortBuyMove => "short_buy_move",
            Self::ShortSellMove => "short_sell_move",
            Self::ShortBuyMove2 => "short_buy_move2",
            Self::ShortSellMove2 => "short_sell_move2",
            Self::FigDelete => "fig_delete",
        }
    }

    /// What a placement row places, or `None` for a move or figure row.
    pub fn placement(self) -> Option<Placement> {
        let (short, pending) = match self {
            Self::BuySet => (false, false),
            Self::ShortSet => (true, false),
            Self::PendingLong => (false, true),
            Self::PendingShort => (true, true),
            _ => return None,
        };
        Some(Placement { short, pending })
    }

    /// The move row this slot is one half of, or `None` for a placement or figure row.
    pub fn move_half(self) -> Option<MoveHalf> {
        let (row, short) = match self {
            Self::BuyMove => (MoveKindSlot::BuyMove, false),
            Self::SellMove => (MoveKindSlot::SellMove, false),
            Self::BuyMove2 => (MoveKindSlot::BuyMove2, false),
            Self::SellMove2 => (MoveKindSlot::SellMove2, false),
            Self::ShortBuyMove => (MoveKindSlot::BuyMove, true),
            Self::ShortSellMove => (MoveKindSlot::SellMove, true),
            Self::ShortBuyMove2 => (MoveKindSlot::BuyMove2, true),
            Self::ShortSellMove2 => (MoveKindSlot::SellMove2, true),
            _ => return None,
        };
        Some(MoveHalf { row, short })
    }

    /// Whether this is the short half of a move row — one of the four the mirror switch owns.
    pub fn is_short_move(self) -> bool {
        self.move_half().is_some_and(|half| half.short)
    }

    /// The short row that follows this long row while `same_hotkeys_for_move` is set.
    ///
    /// Only the four long move rows have one; every other slot stands alone, which is why the
    /// settings page shows a kind column on those four and greys the short ones out.
    pub fn short_twin(self) -> Option<Self> {
        let half = self.move_half()?;
        (!half.short).then(|| half.row.half(true))
    }

    /// The "move kind" this row carries a selector for: the long move rows only. Moonbot keeps a
    /// single kind per row and lets the Long and Short columns share it.
    pub fn kind(self) -> Option<MoveKindSlot> {
        let half = self.move_half()?;
        (!half.short).then_some(half.row)
    }
}

/// The four Moonbot move rows, each with the one "move kind" its long and short halves share.
///
/// Named for the kind field the row owns (`buy_move_kind`), because that is what a caller asks for
/// through it; the gestures of either half come back through [`Self::half`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum MoveKindSlot {
    BuyMove,
    SellMove,
    BuyMove2,
    SellMove2,
}

impl MoveKindSlot {
    /// The four rows in the order [`HotkeysConfig::resolve_move_gesture`] tries them: the first row
    /// whose gesture matches a press answers, so a gesture put on two rows resolves the same way
    /// on every press.
    pub const ALL: [Self; 4] = [
        Self::BuyMove,
        Self::SellMove,
        Self::BuyMove2,
        Self::SellMove2,
    ];

    /// The row for one of Moonbot's four buckets: entry or exit legs, primary or secondary.
    pub fn row(entry: bool, second: bool) -> Self {
        match (entry, second) {
            (true, false) => Self::BuyMove,
            (false, false) => Self::SellMove,
            (true, true) => Self::BuyMove2,
            (false, true) => Self::SellMove2,
        }
    }

    /// Whether the row moves the entry leg (Buy) rather than the exit legs.
    pub fn entry(self) -> bool {
        matches!(self, Self::BuyMove | Self::BuyMove2)
    }

    /// Whether the row is the secondary one, under Moonbot's "additional commands".
    pub fn second(self) -> bool {
        matches!(self, Self::BuyMove2 | Self::SellMove2)
    }

    /// The gesture slot of one half of the row.
    pub fn half(self, short: bool) -> GestureSlot {
        match (self, short) {
            (Self::BuyMove, false) => GestureSlot::BuyMove,
            (Self::BuyMove, true) => GestureSlot::ShortBuyMove,
            (Self::SellMove, false) => GestureSlot::SellMove,
            (Self::SellMove, true) => GestureSlot::ShortSellMove,
            (Self::BuyMove2, false) => GestureSlot::BuyMove2,
            (Self::BuyMove2, true) => GestureSlot::ShortBuyMove2,
            (Self::SellMove2, false) => GestureSlot::SellMove2,
            (Self::SellMove2, true) => GestureSlot::ShortSellMove2,
        }
    }
}

impl HotkeysConfig {
    /// The keystroke stored for one slot, or `""` for a slot that binds nothing.
    ///
    /// An index outside its family answers `""` rather than panicking: the families are arrays, and
    /// this is reached from a page that builds indices in loops and from a config file a user can
    /// hand-edit.
    pub fn key(&self, slot: KeySlot) -> &str {
        match slot {
            KeySlot::OrderSize(i) => self.order_size.get(i).map_or("", String::as_str),
            KeySlot::SellPreset(i) => self.sell_preset.get(i).map_or("", String::as_str),
            KeySlot::ManualStrategy(i) => self.manual_strategy.get(i).map_or("", String::as_str),
            KeySlot::CancelBuy => &self.cancel_buy,
            KeySlot::PanicSell => &self.panic_sell,
            KeySlot::PanicSellOne => &self.panic_sell_one,
            KeySlot::CancelAllBuys => &self.cancel_all_buys,
            KeySlot::JoinSells => &self.join_sells,
            KeySlot::SwitchCharts => &self.switch_charts,
            KeySlot::NewLong => &self.new_long,
            KeySlot::NewShort => &self.new_short,
            KeySlot::SplitOrder => &self.split_order,
            KeySlot::SplitOrderX => &self.split_order_x,
            KeySlot::SellsToRect => &self.sells_to_rect,
            KeySlot::ShiftBuyUp => &self.shift_buy_up,
            KeySlot::ShiftBuyDown => &self.shift_buy_down,
            KeySlot::ShiftSellUp => &self.shift_sell_up,
            KeySlot::ShiftSellDown => &self.shift_sell_down,
            KeySlot::ScalePlus => &self.scale_plus,
            KeySlot::ScaleMinus => &self.scale_minus,
            KeySlot::SwitchFigure => &self.switch_figure,
            KeySlot::ChartShot => &self.chart_shot,
            KeySlot::DrawHline => &self.draw_hline,
            KeySlot::DrawSegment => &self.draw_segment,
            KeySlot::DrawTriangle => &self.draw_triangle,
            KeySlot::DrawChannel => &self.draw_channel,
            KeySlot::FigDelete => &self.fig_delete,
            KeySlot::FigAlert => &self.fig_alert,
            KeySlot::FigUndo => &self.fig_undo,
        }
    }

    /// Write one slot's keystroke, answering whether anything actually changed.
    ///
    /// The change answer is what keeps a settings render from marking the config dirty on every
    /// keystroke that re-selects what was already there. An index outside its family writes nothing
    /// and reports no change, matching [`Self::key`]'s reading of the same index.
    pub fn set_key(&mut self, slot: KeySlot, value: String) -> bool {
        match self.key_mut(slot) {
            Some(target) => assign_if_changed(target, value),
            None => false,
        }
    }

    /// The stored field for one slot, or `None` for an index outside its family.
    fn key_mut(&mut self, slot: KeySlot) -> Option<&mut String> {
        Some(match slot {
            KeySlot::OrderSize(i) => self.order_size.get_mut(i)?,
            KeySlot::SellPreset(i) => self.sell_preset.get_mut(i)?,
            KeySlot::ManualStrategy(i) => self.manual_strategy.get_mut(i)?,
            KeySlot::CancelBuy => &mut self.cancel_buy,
            KeySlot::PanicSell => &mut self.panic_sell,
            KeySlot::PanicSellOne => &mut self.panic_sell_one,
            KeySlot::CancelAllBuys => &mut self.cancel_all_buys,
            KeySlot::JoinSells => &mut self.join_sells,
            KeySlot::SwitchCharts => &mut self.switch_charts,
            KeySlot::NewLong => &mut self.new_long,
            KeySlot::NewShort => &mut self.new_short,
            KeySlot::SplitOrder => &mut self.split_order,
            KeySlot::SplitOrderX => &mut self.split_order_x,
            KeySlot::SellsToRect => &mut self.sells_to_rect,
            KeySlot::ShiftBuyUp => &mut self.shift_buy_up,
            KeySlot::ShiftBuyDown => &mut self.shift_buy_down,
            KeySlot::ShiftSellUp => &mut self.shift_sell_up,
            KeySlot::ShiftSellDown => &mut self.shift_sell_down,
            KeySlot::ScalePlus => &mut self.scale_plus,
            KeySlot::ScaleMinus => &mut self.scale_minus,
            KeySlot::SwitchFigure => &mut self.switch_figure,
            KeySlot::ChartShot => &mut self.chart_shot,
            KeySlot::DrawHline => &mut self.draw_hline,
            KeySlot::DrawSegment => &mut self.draw_segment,
            KeySlot::DrawTriangle => &mut self.draw_triangle,
            KeySlot::DrawChannel => &mut self.draw_channel,
            KeySlot::FigDelete => &mut self.fig_delete,
            KeySlot::FigAlert => &mut self.fig_alert,
            KeySlot::FigUndo => &mut self.fig_undo,
        })
    }

    /// The gesture stored for one slot.
    pub fn gesture(&self, slot: GestureSlot) -> MouseGestureBinding {
        match slot {
            GestureSlot::BuySet => self.buy_set_click,
            GestureSlot::ShortSet => self.short_set_click,
            GestureSlot::PendingLong => self.pending_long_click,
            GestureSlot::PendingShort => self.pending_short_click,
            GestureSlot::BuyMove => self.buy_move_click,
            GestureSlot::SellMove => self.sell_move_click,
            GestureSlot::BuyMove2 => self.buy_move_click2,
            GestureSlot::SellMove2 => self.sell_move_click2,
            GestureSlot::ShortBuyMove => self.short_buy_move_click,
            GestureSlot::ShortSellMove => self.short_sell_move_click,
            GestureSlot::ShortBuyMove2 => self.short_buy_move_click2,
            GestureSlot::ShortSellMove2 => self.short_sell_move_click2,
            GestureSlot::FigDelete => self.fig_delete_click,
        }
    }

    /// Write one slot's gesture and NOTHING else, answering whether anything changed.
    ///
    /// No mirroring, whatever `same_hotkeys_for_move` says: the settings editor carries the mirror
    /// itself, and a layout transfer must not — with the flag on locally and off at the core, a
    /// mirrored write of the long row would overwrite a short value the transfer had decided to
    /// leave alone.
    pub fn set_gesture(&mut self, slot: GestureSlot, value: MouseGestureBinding) -> bool {
        assign_if_changed(self.gesture_mut(slot), value)
    }

    fn gesture_mut(&mut self, slot: GestureSlot) -> &mut MouseGestureBinding {
        match slot {
            GestureSlot::BuySet => &mut self.buy_set_click,
            GestureSlot::ShortSet => &mut self.short_set_click,
            GestureSlot::PendingLong => &mut self.pending_long_click,
            GestureSlot::PendingShort => &mut self.pending_short_click,
            GestureSlot::BuyMove => &mut self.buy_move_click,
            GestureSlot::SellMove => &mut self.sell_move_click,
            GestureSlot::BuyMove2 => &mut self.buy_move_click2,
            GestureSlot::SellMove2 => &mut self.sell_move_click2,
            GestureSlot::ShortBuyMove => &mut self.short_buy_move_click,
            GestureSlot::ShortSellMove => &mut self.short_sell_move_click,
            GestureSlot::ShortBuyMove2 => &mut self.short_buy_move_click2,
            GestureSlot::ShortSellMove2 => &mut self.short_sell_move_click2,
            GestureSlot::FigDelete => &mut self.fig_delete_click,
        }
    }

    /// The gesture the terminal actually FIRES for one slot.
    ///
    /// A move row resolves through [`Self::move_gestures`], the one reader of
    /// `same_hotkeys_for_move`: with the mirror set the short field is not what fires, and showing
    /// or indexing it would put a binding nothing executes into a preview or a clash caption. The
    /// placement and figure rows have no mirror, so the field is what fires.
    pub fn gesture_in_effect(&self, slot: GestureSlot) -> MouseGestureBinding {
        match slot.move_half() {
            Some(half) => {
                self.move_gestures(half.row.entry(), half.short)[usize::from(half.row.second())]
            }
            None => self.gesture(slot),
        }
    }

    /// The "move kind" of one move row.
    pub fn move_kind(&self, row: MoveKindSlot) -> MoveKind {
        match row {
            MoveKindSlot::BuyMove => self.buy_move_kind,
            MoveKindSlot::SellMove => self.sell_move_kind,
            MoveKindSlot::BuyMove2 => self.buy_move_kind2,
            MoveKindSlot::SellMove2 => self.sell_move_kind2,
        }
    }

    /// Write one row's move kind, answering whether anything changed.
    pub fn set_move_kind(&mut self, row: MoveKindSlot, value: MoveKind) -> bool {
        let target = match row {
            MoveKindSlot::BuyMove => &mut self.buy_move_kind,
            MoveKindSlot::SellMove => &mut self.sell_move_kind,
            MoveKindSlot::BuyMove2 => &mut self.buy_move_kind2,
            MoveKindSlot::SellMove2 => &mut self.sell_move_kind2,
        };
        assign_if_changed(target, value)
    }
}

/// Writes `value` into `target` and answers whether that was a change.
///
/// The change answer is what keeps a settings render from marking the config dirty on every
/// keystroke that re-selects what was already there; every slot setter answers through this one.
fn assign_if_changed<T: PartialEq>(target: &mut T, value: T) -> bool {
    if *target == value {
        return false;
    }
    *target = value;
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HotkeysConfig {
    /// File generation, for one-time fills of slots that shipped unbound.
    ///
    /// Zero is a file written before this existed. See [`HotkeysConfig::fill_unbound_slots`]: a slot the user
    /// deliberately cleared must not come back on every launch, so the backfill runs once and the
    /// generation records that it did.
    #[serde(default)]
    pub schema: u8,
    /// Manual order size F1-F6 (`HotkeysConfig.OKeys` in Moonbot).
    #[serde(default = "default_order_size_keys")]
    pub order_size: [String; ORDER_SIZE_KEYS],
    /// Fixed sell S1-S6 (`HotkeysConfig.SKeys` in Moonbot).
    #[serde(default = "default_sell_preset_keys")]
    pub sell_preset: [String; SELL_PRESET_KEYS],
    /// Manual strategy buttons 1-10 (`ManualStratsConfig.hotKeys` in Moonbot).
    #[serde(default = "default_manual_strategy_keys")]
    pub manual_strategy: [String; MANUAL_STRATEGY_KEYS],

    // Keyboard defaults below are Moonbot's own, read off its Hotkeys page: a user coming from it
    // finds the keys where they left them. `Alt` combinations are deliberate and do reach us: the
    // Windows fork routes WM_SYSKEYDOWN through the same `WM_GPUI_KEYDOWN` path as WM_KEYDOWN
    // (`moon-gpui-windows/src/platform.rs::translate_accelerator`), so nothing is eaten by the
    // window menu.
    #[serde(default = "default_cancel_buy")]
    pub cancel_buy: String,
    #[serde(default = "default_panic_sell")]
    pub panic_sell: String,
    #[serde(default = "default_panic_sell_one")]
    pub panic_sell_one: String,
    #[serde(default = "default_cancel_all_buys")]
    pub cancel_all_buys: String,
    #[serde(default = "default_join_sells")]
    pub join_sells: String,
    #[serde(default = "default_switch_charts")]
    pub switch_charts: String,
    #[serde(default = "default_new_long")]
    pub new_long: String,
    #[serde(default = "default_new_short")]
    pub new_short: String,
    #[serde(default = "default_split_order")]
    pub split_order: String,
    /// Moonbot's "Split to N (click to set)": splits into [`HotkeysConfig::split_n_parts`] parts
    /// instead of the fixed three.
    #[serde(default = "default_split_order_x")]
    pub split_order_x: String,
    /// Moonbot's "Sells to rectangle": toggles a zone-drawing mode in which every pair of clicks
    /// gives the band the market's sells are spread across.
    #[serde(default = "default_sells_to_rect")]
    pub sells_to_rect: String,
    /// Part count for `Split N` (Moonbot `Hotkeys.SplitParts`), read through
    /// [`HotkeysConfig::split_n_parts`] so a hand-edited or imported value cannot leave its range.
    #[serde(default = "default_split_parts")]
    pub split_parts: u8,
    /// Shifts the active chart market's orders by [`SHIFT_PERCENT`], as Moonbot's ±1% does: the
    /// buy phase or the sell phase, up or down.
    #[serde(default = "default_shift_buy_up")]
    pub shift_buy_up: String,
    #[serde(default = "default_shift_buy_down")]
    pub shift_buy_down: String,
    #[serde(default = "default_shift_sell_up")]
    pub shift_sell_up: String,
    #[serde(default = "default_shift_sell_down")]
    pub shift_sell_down: String,

    // Moonbot hotkeys with no send command to call (reload book/chart, spy, show charts, fit
    // sells, broadcast, sell +/-) were removed completely on 2026-07-10 (configuration + tab +
    // dispatcher); serde silently ignores their keys in old hotkeys.toml files. Restore them from
    // git history as commands turn up: `Sells to rectangle` came back that way on 2026-08-15, on
    // `move_all_sells`, whose `percent` form now also drives the order shifts and whose
    // `replace_kind` form is still unused — so "no command" means "not on this list, check
    // moonproto first".
    //
    // `Make shot` left that list on 2026-08-18 by a different route: it never needed a command at
    // all, only a way to read the chart's own pixels, so it is `chart_shot` above rather than a
    // restoration from history. A Moonbot slot can therefore return either way.
    #[serde(default = "default_scale_plus")]
    pub scale_plus: String,
    #[serde(default = "default_scale_minus")]
    pub scale_minus: String,
    #[serde(default = "default_switch_figure")]
    pub switch_figure: String,

    /// Copies an image of the active chart — plot, order book and the coin caption — to the
    /// system clipboard. Moonbot's "make shot", back on a command the Terminal can serve.
    ///
    /// Nothing reaches the disk: the clipboard is the whole deliverable.
    ///
    /// Serde's default already gives every existing `hotkeys.toml` this key on load, so the field
    /// needs no BACKFILL. What it does need is a COLLISION check: a user who had given Ctrl+F10 to
    /// another action now holds it twice, and the duplicate resolves by branch order in the
    /// dispatcher — silently shadowing whatever sits lower, which includes the trading actions.
    /// Generation 2 of [`HotkeysConfig::fill_unbound_slots`] clears this slot in that case, exactly
    /// as generation 1 did for `sells_to_rect`.
    #[serde(default = "default_chart_shot")]
    pub chart_shot: String,

    /// Figure drawing layer: arms a tool. Pressing the same hotkey again disarms it, leaving the
    /// drawn figures in place. Defaults are on Ctrl because Moonbot has no drawing hotkeys to
    /// inherit, not because Alt is unavailable — it reaches the handler on both platforms.
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
    /// Deletes the LAST figure drawn on the chart the pointer rests on.
    ///
    /// Moonbot's own Ctrl+Z, which removes a drawn element — it is NOT an undo stack there and is
    /// not one here: nothing brings the figure back, and an edit or a move is not what it reverts.
    /// The key is free on every Moonbot default and on ours, and a text field keeps it: an input
    /// with the keyboard resolves Ctrl+Z as its own Undo before the window root is reached.
    #[serde(default = "default_fig_undo")]
    pub fig_undo: String,
    /// Registry keys ([`crate::figures::ToolDef::key`]) of the drawing tools left OUT of the
    /// [`Self::switch_figure`] cycle — Moonbot's `HotKey` checkbox, which sits in its pencil panel
    /// beside the line kind and says whether the selected tool takes part in the switching.
    ///
    /// An EXCLUSION list rather than an inclusion one, and that is the whole reason it can carry a
    /// bare serde default: an absent or empty list means every tool participates, which is what a
    /// fresh install means AND what every file written before this field existed meant. A tool
    /// added to the registry later therefore takes part without being written into anybody's file.
    ///
    /// An unknown key is inert rather than an error — it is how a tool retired in a later build
    /// leaves a file behind, and dropping it on load would rewrite a file the other build still
    /// reads.
    #[serde(default)]
    pub switch_figure_skip: Vec<String>,

    /// Mouse gesture that deletes the figure UNDER THE CURSOR, the pointing counterpart of the
    /// [`Self::fig_delete`] key.
    ///
    /// A key has no position, so it can only act on the selected figure; a click carries one, so it
    /// deletes what it points at and needs no selection first. Settable to any gesture, or to
    /// `None`, which turns the gesture off and leaves the key.
    ///
    /// Middle by default because that is the button Moonbot deletes with — from its UI, not from
    /// the wire: `SharedConfig`'s hotkey block carries no figure gesture at all (`switch_figure` is
    /// the only drawing entry there), so unlike every other gesture in this struct there is nothing
    /// for "pull layout from core" to reconcile this against, and the default is ours to keep.
    #[serde(default = "default_middle")]
    pub fig_delete_click: MouseGestureBinding,

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
    /// Delphi `ReplaceBuyKind`: how the primary Move Open gesture lays out what it moves.
    #[serde(default)]
    pub buy_move_kind: MoveKind,
    /// Delphi `ReplaceSellKind`: the same for the primary Move TP gesture.
    #[serde(default)]
    pub sell_move_kind: MoveKind,
    /// Delphi `ReplaceBuyKind2`: the secondary Move Open gesture's kind.
    #[serde(default)]
    pub buy_move_kind2: MoveKind,
    /// Delphi `ReplaceSellKind2`: the secondary Move TP gesture's kind.
    #[serde(default)]
    pub sell_move_kind2: MoveKind,
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
            schema: SCHEMA,
            order_size: default_order_size_keys(),
            sell_preset: default_sell_preset_keys(),
            manual_strategy: default_manual_strategy_keys(),
            cancel_buy: default_cancel_buy(),
            panic_sell: default_panic_sell(),
            panic_sell_one: default_panic_sell_one(),
            cancel_all_buys: default_cancel_all_buys(),
            join_sells: default_join_sells(),
            switch_charts: default_switch_charts(),
            new_long: default_new_long(),
            new_short: default_new_short(),
            split_order: default_split_order(),
            split_order_x: default_split_order_x(),
            sells_to_rect: default_sells_to_rect(),
            split_parts: default_split_parts(),
            shift_buy_up: default_shift_buy_up(),
            shift_buy_down: default_shift_buy_down(),
            shift_sell_up: default_shift_sell_up(),
            shift_sell_down: default_shift_sell_down(),
            scale_plus: default_scale_plus(),
            scale_minus: default_scale_minus(),
            switch_figure: default_switch_figure(),
            chart_shot: default_chart_shot(),
            draw_hline: default_draw_hline(),
            draw_segment: default_draw_segment(),
            draw_triangle: default_draw_triangle(),
            draw_channel: default_draw_channel(),
            fig_delete: default_fig_delete(),
            fig_alert: default_fig_alert(),
            fig_undo: default_fig_undo(),
            switch_figure_skip: Vec::new(),
            fig_delete_click: default_middle(),
            buy_set_click: default_left_double(),
            short_set_click: MouseGestureBinding::None,
            pending_long_click: MouseGestureBinding::None,
            pending_short_click: MouseGestureBinding::None,
            buy_move_click: default_left_shift(),
            sell_move_click: default_left_ctrl(),
            buy_move_click2: MouseGestureBinding::None,
            sell_move_click2: MouseGestureBinding::None,
            buy_move_kind: MoveKind::default(),
            sell_move_kind: MoveKind::default(),
            buy_move_kind2: MoveKind::default(),
            sell_move_kind2: MoveKind::default(),
            same_hotkeys_for_move: default_same_hotkeys_for_move(),
            short_buy_move_click: default_left_shift(),
            short_sell_move_click: default_left_ctrl(),
            short_buy_move_click2: MouseGestureBinding::None,
            short_sell_move_click2: MouseGestureBinding::None,
        }
    }
}

impl HotkeysConfig {
    /// Return the primary and secondary move gestures for one order line.
    ///
    /// `entry` selects the Buy leg against the exit legs (sell, stop, trailing, take profit), and
    /// `short` the position direction — Moonbot's four buckets. This is the ONE place that reads
    /// `same_hotkeys_for_move`: the settings panel also mirrors long values into the short fields as
    /// they are edited, but a shared or hand-edited file can carry the flag with stale short values,
    /// and the flag is what the user sees.
    pub fn move_gestures(&self, entry: bool, short: bool) -> [MouseGestureBinding; 2] {
        let short = short && !self.same_hotkeys_for_move;
        [false, true].map(|second| self.gesture(MoveKindSlot::row(entry, second).half(short)))
    }

    /// What one recognised move gesture has to send.
    ///
    /// Args:
    ///     matches: Whether a press being examined satisfies one binding. The caller owns the
    ///         platform's modifier type, so the comparison stays in the UI and only the ANSWER
    ///         comes back here.
    ///
    /// Returns:
    ///     The side of the book to move, the layout to move it into and the position side it
    ///     addresses, or `None` when no slot claims the press — including a slot whose kind is
    ///     `None`, which is Moonbot's way of leaving a bound gesture inert. Rows are examined in
    ///     [`MoveKindSlot::ALL`]'s order — the order the settings page lists them — so a gesture
    ///     the user put on two of them resolves the same way twice rather than by whichever
    ///     branch happened to run first.
    pub fn resolve_move_gesture(
        &self,
        matches: impl Fn(MouseGestureBinding) -> bool,
    ) -> Option<MoveGestureCommand> {
        for row in MoveKindSlot::ALL {
            let entry = row.entry();
            let kind = self.move_kind(row);
            // Both sides come from `move_gestures`, which is the one place that reads
            // `same_hotkeys_for_move`: with the mirror on it hands back the long gesture for the
            // short side too, so one press claims both and the core is told `Both`.
            let ix = usize::from(row.second());
            let long = self.move_gestures(entry, false)[ix];
            let short = self.move_gestures(entry, true)[ix];
            let hit_long = long != MouseGestureBinding::None && matches(long);
            let hit_short = short != MouseGestureBinding::None && matches(short);
            let side = match (hit_long, hit_short) {
                (true, true) => MoveSide::Both,
                (true, false) => MoveSide::Long,
                (false, true) => MoveSide::Short,
                (false, false) => continue,
            };
            // Moonbot's way of switching one gesture off without clearing its binding. `continue`
            // rather than `return`: another slot may hold the same binding WITH a kind, and giving
            // up here would let a disabled row silence a working one.
            if kind == MoveKind::None {
                continue;
            }
            return Some(MoveGestureCommand {
                sell: !entry,
                kind,
                side,
            });
        }
        None
    }

    /// Part count for the `Split N` action, clamped to [`SPLIT_PARTS_MIN`]..=[`SPLIT_PARTS_MAX`].
    ///
    /// Callers use this instead of the raw field: the value reaches a live trading command, and
    /// both a hand-edited `hotkeys.toml` and a Moonbot import can carry anything a `u8` holds.
    pub fn split_n_parts(&self) -> i32 {
        i32::from(self.split_parts.clamp(SPLIT_PARTS_MIN, SPLIT_PARTS_MAX))
    }

    /// Reads `hotkeys.toml`. `None` means the file does not exist yet (first launch after moving
    /// hotkeys out of settings.toml; the caller migrates the legacy section and writes the file).
    /// A corrupt file yields the default (and logs internally), NOT `None`; otherwise the corrupt
    /// file would be silently overwritten by the stale legacy copy from settings.toml.
    pub fn load() -> Option<Self> {
        let path = paths::hotkeys_path();
        if !path.exists() {
            return None;
        }
        let mut cfg: Self = super::toml_io::load_or_default(&path, "hotkeys.toml", |_| {});
        // Persist the stamp right here, or "runs once" is a promise the next launch breaks: the
        // generation would live in memory until some unrelated settings save happened to write it,
        // and until then every launch would refill a slot the user cleared.
        if cfg.fill_unbound_slots() {
            if let Err(error) = cfg.save() {
                log::warn!("hotkeys.toml migration not persisted: {error:#}");
            }
        }
        Some(cfg)
    }

    /// Brings a file written by an older build up to [`SCHEMA`], one generation at a time.
    ///
    /// Generation 0 → 1: the actions Moonbot binds by default shipped here UNBOUND, so a file from
    /// that build has empty strings where a new install now has Moonbot's key. Those empties are
    /// filled ONCE. It has to be once: a user is free to clear a hotkey, and a fill that ran on
    /// every load would hand it back on the next launch. Only empty slots are touched, so a key the
    /// user chose is never overwritten — and a shipped key ALREADY IN USE elsewhere in this file is
    /// skipped rather than duplicated, because a duplicate resolves by branch order in the
    /// dispatcher and would silently turn, say, a manual-strategy Alt+1 into a live long order.
    ///
    /// Generation 1 → 2: clear `chart_shot` where Ctrl+F10 was already the user's key for something
    /// else. A NEW field never needs backfilling — serde's default fills it — but it does need that
    /// collision check, and running it must NOT drag generation 1 along behind it, which is why
    /// each arm is gated on its own predecessor rather than on the aggregate.
    ///
    /// Generation 2 → 3: the same check for `fig_undo`, which arrives on Ctrl+Z the same way.
    ///
    /// Generation 3 → 4: the first arm about a GESTURE rather than a key, and about meaning rather
    /// than collision — the two pending-order gestures went from saved-but-inert to placing a live
    /// order, so a value chosen while the row said it did nothing is cleared.
    ///
    /// Generation 4 → 5: the collision check generations 2 and 3 run for an arriving key, run for
    /// an arriving gesture — `fig_delete_click` yields its Middle default where Middle already
    /// trades. Its own generation rather than a widening of 4, because files stamped 4 exist.
    ///
    /// Returns whether anything changed, so the caller can persist the stamp.
    pub(super) fn fill_unbound_slots(&mut self) -> bool {
        if self.schema >= SCHEMA {
            return false;
        }
        // Each generation is gated on its OWN predecessor, never on `schema < SCHEMA` as a whole:
        // a file already at generation 1 must NOT have the empty-slot backfill run over it again,
        // or every key its owner has deliberately cleared since comes back on the next launch.
        if self.schema < 1 {
            self.fill_generation_1();
        }
        if self.schema < 2 {
            self.clear_generation_2_collisions();
        }
        if self.schema < 3 {
            self.clear_generation_3_collisions();
        }
        if self.schema < 4 {
            self.clear_generation_4_pending_gestures();
        }
        if self.schema < 5 {
            self.clear_generation_5_figure_gesture();
        }
        self.schema = SCHEMA;
        true
    }

    /// Generation 0 -> 1: backfill the slots that shipped unbound.
    ///
    /// Returns:
    ///     Nothing; updates only slots that were empty in a generation-0 file.
    fn fill_generation_1(&mut self) {
        let defaults = Self::default();
        let taken = self.bound_keys();
        // `count` is how many slots already hold this key. A candidate for an EMPTY slot may hold
        // none; a slot serde has already filled from a NEW field's default holds one — its own —
        // and anything above that is a real collision.
        let occurrences = |key: &str| taken.iter().filter(|held| held.as_str() == key).count();
        clear_if_duplicate(&taken, &mut self.sells_to_rect, "Sells to rectangle");
        // ONLY the slots that shipped unbound. A slot that always had a key (panic_sell_one,
        // cancel_all_buys, switch_figure) is empty for exactly one reason — the user cleared it —
        // and filling it would take that choice back. Those keep their old value; the Moonbot key
        // is what a fresh install gets.
        for (slot, shipped) in [
            (&mut self.cancel_buy, defaults.cancel_buy),
            (&mut self.panic_sell, defaults.panic_sell),
            (&mut self.join_sells, defaults.join_sells),
            (&mut self.switch_charts, defaults.switch_charts),
            (&mut self.new_long, defaults.new_long),
            (&mut self.new_short, defaults.new_short),
            (&mut self.split_order, defaults.split_order),
            (&mut self.split_order_x, defaults.split_order_x),
            (&mut self.sells_to_rect, defaults.sells_to_rect),
            (&mut self.shift_buy_up, defaults.shift_buy_up),
            (&mut self.shift_buy_down, defaults.shift_buy_down),
            (&mut self.shift_sell_up, defaults.shift_sell_up),
            (&mut self.shift_sell_down, defaults.shift_sell_down),
        ] {
            if slot.trim().is_empty() && occurrences(&shipped) == 0 {
                *slot = shipped;
            }
        }
    }

    /// Generation 1 -> 2: `chart_shot` arrives pre-filled by its serde default, so it never reaches
    /// generation 1's empty-slot loop and would keep Ctrl+F10 even where the user had already given
    /// that keystroke to another action.
    ///
    /// The duplicate is not harmless: `resolve_binding` answers the FIRST matching branch, and the
    /// chart shot is resolved above every trading action, so the shipped default would quietly take
    /// a key that used to send an order. Clearing the NEW slot rather than the old one keeps the
    /// user's own choice, which is the same trade generation 1 made for `sells_to_rect`.
    ///
    /// Returns:
    ///     Nothing; clears only the new chart-shot slot when its default collides.
    fn clear_generation_2_collisions(&mut self) {
        // Recomputed rather than reused: generation 1 may have just filled slots above.
        let taken = self.bound_keys();
        clear_if_duplicate(&taken, &mut self.chart_shot, "Make Shot");
    }

    /// Generation 2 -> 3: `fig_undo` ships on Ctrl+Z through its serde default, so it reaches an
    /// existing file already filled and never passes through generation 1's empty-slot loop.
    ///
    /// Ctrl+Z is free on every default we and Moonbot ship, but nothing stops a user from having
    /// given it to another action — and the figure layer resolves ABOVE the trading actions, so the
    /// arriving default would quietly take a key that used to send an order. The NEW slot is the
    /// one cleared, keeping the user's own choice, exactly as generations 1 and 2 did.
    ///
    /// Returns:
    ///     Nothing; clears only the new figure-undo slot when its default collides.
    fn clear_generation_3_collisions(&mut self) {
        // Recomputed rather than reused: the generations above may have just changed slots.
        let taken = self.bound_keys();
        clear_if_duplicate(&taken, &mut self.fig_undo, "Delete last figure");
    }

    /// Generation 3 -> 4: the two pending-order gestures start FIRING.
    ///
    /// They shipped editable and saved, with the row itself saying the terminal did not send them
    /// yet — so anyone who set one was told, on that screen, that it did nothing. It now places a
    /// live pending order on the selected core. A value chosen under that promise is not a choice to
    /// trade, and the two are cleared once rather than waking up as a trading gesture; both ship
    /// unset, so this touches nobody who did not deliberately set one.
    ///
    /// Returns:
    ///     Nothing; clears only the two pending gesture slots.
    fn clear_generation_4_pending_gestures(&mut self) {
        for (slot, label) in [
            (&mut self.pending_long_click, "Pending Long"),
            (&mut self.pending_short_click, "Pending Short"),
        ] {
            if *slot != MouseGestureBinding::None {
                log::warn!(
                    "hotkeys.toml: {label} now places a real pending order; its gesture \
                     {slot:?} was cleared, set it again if that is what you want"
                );
                *slot = MouseGestureBinding::None;
            }
        }
    }

    /// Generation 4 -> 5: the arriving figure-delete gesture yields to a trading gesture the file
    /// already fires on the same button.
    ///
    /// `fig_delete_click` is the gesture counterpart of `chart_shot` and `fig_undo` in generations 2
    /// and 3: it reaches an existing file already filled by its serde default (Middle), and the
    /// figure layer is offered a press ABOVE every trading layer on that button. A user whose file
    /// already moves or places orders on Middle would find those presses deleting figures instead
    /// wherever one sits under the pointer. The NEW slot yields, exactly as the keys do, through
    /// [`Self::bound_gestures`] — the mirror of what generations 2 and 3 do through `bound_keys`.
    ///
    /// Returns:
    ///     Nothing; clears only the figure gesture, and only on a collision.
    fn clear_generation_5_figure_gesture(&mut self) {
        // Recomputed here rather than reused, like every key generation recomputes `bound_keys`:
        // generation 4 may have just cleared slots above.
        let taken = self.bound_gestures();
        let arriving = self.fig_delete_click;
        if arriving != MouseGestureBinding::None
            && taken.iter().filter(|held| **held == arriving).count() > 1
        {
            log::warn!(
                "hotkeys.toml: {arriving:?} is already a trading gesture, \
                 Delete figure was left without one"
            );
            self.fig_delete_click = MouseGestureBinding::None;
        }
    }

    /// Every gesture this file fires, for collision checks — the counterpart of
    /// [`Self::bound_keys`].
    ///
    /// The gestures IN EFFECT rather than the fields: with the mirror switch set a short field is
    /// not what fires, and counting it would report a collision nobody can press. A gesture held by
    /// two slots appears twice, which is what makes a duplicate visible to the caller. Unlike the
    /// keys, these compare exactly — a gesture is an enum, not a string with spellings.
    pub fn bound_gestures(&self) -> Vec<MouseGestureBinding> {
        GestureSlot::ALL
            .into_iter()
            .map(|slot| self.gesture_in_effect(slot))
            .filter(|gesture| *gesture != MouseGestureBinding::None)
            .collect()
    }

    /// Every keystroke this file already binds, for collision checks.
    ///
    /// Includes the preset and manual-strategy arrays: those are exactly where a user's own
    /// `alt-1` is most likely to sit. A key held by two slots appears twice, which is what makes a
    /// duplicate visible to the caller.
    ///
    /// Raw strings, exactly as stored — this crate cannot parse them and deliberately does not try.
    /// A caller that must decide whether two of these are the SAME PRESS compares them through
    /// `crate::hotkeys::binding_id` on the UI side, as the core pull's conflict gate does; a caller
    /// that compares them literally, as [`clear_if_duplicate`] does, misses a differently-spelled
    /// duplicate and says so where it is written.
    pub fn bound_keys(&self) -> Vec<String> {
        KeySlot::all()
            .into_iter()
            .map(|slot| self.key(slot).trim().to_string())
            .filter(|key| !key.is_empty())
            .collect()
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
            // Pasted text is a file like any other and gets the same one-time fills: a set shared
            // from an older build otherwise arrives with its new slots unbound.
            let mut cfg: Self = toml::from_str(text).ok()?;
            cfg.fill_unbound_slots();
            Some(cfg)
        } else {
            None
        }
    }
}

/// Clear `field` when the keystroke it holds is ALREADY bound elsewhere in this file.
///
/// Every generation of [`HotkeysConfig::fill_unbound_slots`] needs this and for the same reason: a
/// field ADDED in that generation arrives pre-filled by its serde default, so it never reaches the
/// empty-slot loop and would keep a keystroke its owner has given to something else. A duplicate
/// resolves by branch order in the dispatcher, so the shipped default would silently shadow the
/// user's own binding — which is why the NEW slot is the one that yields, never the old one.
///
/// `taken` is a [`HotkeysConfig::bound_keys`] snapshot, in which the field's OWN key already counts
/// once; anything above one occurrence is a real collision. An empty field binds nothing and is
/// left alone.
///
/// STILL NOT REDUNDANT, now that the settings page captions duplicates by name. The two answer
/// different questions and only one of them is about the user: the page reports a duplicate the USER
/// created and leaves it alone, because their binding is their business; this clears a default WE
/// ship into a slot that did not exist in their file yet, which they never chose and would only
/// discover by noticing an old key had stopped working. A caption cannot serve the second case — it
/// is only read by someone who opens the page, and by then the key is already stolen.
///
/// LIMIT, and it is a real one: this compares the STRINGS, while the dispatcher compares the press
/// (`crate::hotkeys::binding_id` in the UI crate is the definition). `Keystroke::parse` is
/// case-insensitive, takes the modifiers in any order, and reads `cmd`/`super`/`win` as one
/// modifier, so a file whose key is spelled `Ctrl-F10` or `ctrl-alt-win-shift-k` hides a real
/// collision from this check and the shipped default takes the key after all. Not fixed here on
/// purpose: this crate has no keystroke parser, and writing a second one to compare with would
/// invite exactly the drift it is meant to catch. The honest fix is to hand the comparison in from
/// the crate that owns the parser; `docs-internal/HOTKEYS_UNIFIED_PLAN.md` carries it as debt.
///
/// Args:
///     taken: Snapshot of all already-bound, non-empty keystrokes.
///     field: Newly introduced binding that yields to an existing collision.
///     label: User-facing name included in the collision warning.
///
/// Returns:
///     Nothing; clears `field` only when its key occurs more than once in `taken`.
fn clear_if_duplicate(taken: &[String], field: &mut String, label: &str) {
    let key = field.trim();
    if key.is_empty() || taken.iter().filter(|held| held.as_str() == key).count() <= 1 {
        return;
    }
    log::warn!(
        "hotkeys.toml: {} is already taken, {} was left without a key",
        field,
        label
    );
    field.clear();
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

/// Moonbot ships `SplitParts = 2`, which also keeps `Split N` distinct from the fixed three-part
/// Split Order until the user (or an import) sets their own count.
fn default_split_parts() -> u8 {
    2
}

// The drawing tools are the Terminal's own — Moonbot has no equivalent to inherit a key from — so
// these defaults are chosen here, on Ctrl, next to the other letter bindings. They use the literal
// `ctrl-` on BOTH platforms, matching how Moonbot treats Mac. Keys without a modifier (function
// keys, delete) remain as-is.
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

/// Moonbot's own key for removing a drawn element, and free on every shipped default here.
///
/// Returns:
///     The default GPUI keystroke for deleting the last drawn figure.
fn default_fig_undo() -> String {
    "ctrl-z".into()
}

fn default_scale_plus() -> String {
    "ctrl-q".into()
}

fn default_scale_minus() -> String {
    "ctrl-w".into()
}

fn default_switch_figure() -> String {
    "alt-d".into()
}

/// Ctrl+F10, next to the built-in Ctrl+Shift+F10 that resets window positions but never colliding
/// with it: the resolver matches on the WHOLE modifier set, so the two are distinct keystrokes.
/// Free on every shipped default and on Moonbot's own Hotkeys page.
///
/// Returns:
///     The default GPUI keystroke for copying the active chart.
fn default_chart_shot() -> String {
    "ctrl-f10".into()
}

// Moonbot's own bindings, taken from its Hotkeys page. `scale_plus`/`scale_minus` (Ctrl+Q/Ctrl+W)
// and `sell_preset`/`order_size` already matched; these are the rest of the set that has a Terminal
// action behind it. Moonbot entries with no command here — Reload Book/Chart, screenshots, Center
// Chart, Show\Hide Charts, Hide Balance, Open coin in all bots — stay absent, as they were.
// Two have come back since, each on a command that turned up later: `Sells to rectangle` on
// 2026-08-15 (`move_all_sells`), and Moonbot's screenshot on 2026-08-18 — that one needs no
// protocol command at all, only a way to read the chart's own pixels (`chart_shot`). So
// "no command" still means "not on this list, check moonproto first", and sometimes it means
// the action was never remote to begin with.
fn default_cancel_buy() -> String {
    "alt-z".into()
}

fn default_panic_sell() -> String {
    "alt-6".into()
}

fn default_join_sells() -> String {
    "alt-e".into()
}

fn default_switch_charts() -> String {
    "alt-f".into()
}

fn default_new_long() -> String {
    "alt-1".into()
}

fn default_new_short() -> String {
    "alt-3".into()
}

fn default_split_order() -> String {
    "alt-c".into()
}

fn default_split_order_x() -> String {
    "ctrl-x".into()
}

fn default_sells_to_rect() -> String {
    "ctrl-s".into()
}

fn default_shift_buy_up() -> String {
    "shift-up".into()
}

fn default_shift_buy_down() -> String {
    "shift-down".into()
}

fn default_shift_sell_up() -> String {
    "alt-up".into()
}

fn default_shift_sell_down() -> String {
    "alt-down".into()
}

fn default_panic_sell_one() -> String {
    "alt-5".into()
}

fn default_cancel_all_buys() -> String {
    "alt-a".into()
}

fn default_middle() -> MouseGestureBinding {
    MouseGestureBinding::Middle
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
