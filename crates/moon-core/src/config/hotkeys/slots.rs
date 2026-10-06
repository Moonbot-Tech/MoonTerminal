//! Editable keyboard and gesture slots of the hotkey config.

use super::*;

/// One editable keyboard slot of [`HotkeysConfig`].
///
/// The slot identity lives HERE, beside the struct it addresses, and not in the settings page that
/// draws it. That is the whole point of the type: the same named slots were enumerated by
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
    /// Moonbot's "Cancel buys in all bots": what `CancelAllBuys` does, on every connected core.
    CancelAllBuysAllCores,
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
    /// Zoom the time axis in to the three-second floor.
    SuperZoomIn,
    /// Zoom the time axis out while respecting the three-second floor.
    SuperZoomOut,
    /// Moonbot's built-in Ctrl+Right, "Center chart": back to the price and the live edge.
    CenterChart,
    /// Toggle the toolbar Live/Pause flag. Default Space; no Moonbot import slot.
    ToggleLive,
    SwitchFigure,
    ChartShot,
    DrawHline,
    /// Optional one-click horizontal ray shortcut.
    DrawHorizontalRay,
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
    pub const NAMED: [Self; 32] = [
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
        Self::SuperZoomIn,
        Self::SuperZoomOut,
        Self::CenterChart,
        Self::ToggleLive,
        Self::SwitchFigure,
        Self::ChartShot,
        Self::DrawHline,
        Self::DrawHorizontalRay,
        Self::DrawSegment,
        Self::DrawTriangle,
        Self::DrawChannel,
        Self::FigDelete,
        Self::FigAlert,
        Self::FigUndo,
        // Last, like its step in the UI's key dispatch: `HotkeysConfig::action_for_gesture` walks
        // this order, so a click bound here AND to another slot fires the other one, never the
        // widest action there is.
        Self::CancelAllBuysAllCores,
    ];

    /// Every slot the file holds a key for, presets first.
    ///
    /// The ORDER is this list's own and means nothing to a dispatcher: which of two holders of one
    /// keystroke actually fires is `hotkeys::DISPATCH`'s question, in the UI crate, and the settings
    /// page's clash index reads that list. Callers here only ever ask "what is bound at all".
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
            Self::CancelAllBuysAllCores => "cancel_all_buys_all_cores",
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
            Self::SuperZoomIn => "super_zoom_in",
            Self::SuperZoomOut => "super_zoom_out",
            Self::CenterChart => "center_chart",
            Self::ToggleLive => "toggle_live",
            Self::SwitchFigure => "switch_figure",
            Self::ChartShot => "chart_shot",
            Self::DrawHline => "draw_hline",
            Self::DrawHorizontalRay => "draw_horizontal_ray",
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

    /// The slot's full name: the stem, with the index after a dot for a family member —
    /// `cancel_buy`, `order_size.2`. The key of its row in `[action_clicks]`, and what the Moonbot
    /// import dresses with `hotkey.`.
    pub fn name(self) -> String {
        match self.index() {
            Some(i) => format!("{}.{i}", self.stem()),
            None => self.stem().to_string(),
        }
    }

    /// The slot one full name spells, or `None` for a name this build does not know — a removed
    /// slot, a hand-edit, an index past its family.
    pub fn for_name(name: &str) -> Option<Self> {
        // Split once instead of spelling forty-eight names to compare against: an index past its
        // family is still refused, because no slot has that stem AND that index.
        let (stem, index) = match name.rsplit_once('.') {
            Some((stem, index)) => (stem, Some(index.parse::<usize>().ok()?)),
            None => (name, None),
        };
        Self::all()
            .into_iter()
            .find(|slot| slot.stem() == stem && slot.index() == index)
    }

    /// Whether the slot can carry a mouse gesture beside its key.
    ///
    /// Almost every one can: an action that needs a position gets it from the click, an action
    /// that needs the hovered market gets it from the chart the click landed on, and an action
    /// that needs neither simply does not use it. The two that cannot: `sells_to_rect` ARMS a mode
    /// the next clicks then drive, so a click to arm it would be the first click of the band; and
    /// `fig_delete` has had its own gesture field since before this table existed.
    pub fn has_mouse_half(self) -> bool {
        !matches!(self, Self::SellsToRect | Self::FigDelete)
    }

    /// The slot's mouse half, or `None` for the two that have none — the one place a
    /// `GestureSlot::ForKey` is made, so no reader has to re-check the rule.
    pub fn mouse_half(self) -> Option<GestureSlot> {
        self.has_mouse_half().then_some(GestureSlot::ForKey(self))
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
    /// The mouse half of a keyboard slot: a click that performs the same action as the key, stored
    /// in `[action_clicks]` under the slot's name rather than in a field of its own.
    ForKey(KeySlot),
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
    /// Every gesture that has a FIELD of its own, in the order the settings page lists them.
    ///
    /// The first four are the placement rows, and their order HERE is the order a press is tried
    /// against them: `placement_intent` walks this list and the first match fires. The compiler
    /// cannot check the list against the enum; a test writes every slot a distinct gesture and
    /// looks for a stored field that kept its default. [`Self::all`] adds the key halves.
    pub const OWN: [Self; 13] = [
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

    /// Every gesture slot: the thirteen with a field of their own, then the mouse half of every
    /// keyboard slot that can carry one, in `KeySlot::all` order.
    ///
    /// The order within the key halves is the order a press is tried against them by the chart's
    /// action layer, and the first match fires — the same rule the placement rows follow.
    pub fn all() -> Vec<Self> {
        Self::OWN
            .into_iter()
            .chain(KeySlot::all().into_iter().filter_map(KeySlot::mouse_half))
            .collect()
    }

    /// The name the settings page's element ids (`buy-move2`) and locale keys
    /// (`hotkeys.mouse.buy_move2`) derive from; the key's own stem for a key half.
    ///
    /// NOT the field name, unlike [`KeySlot::stem`]: the secondary rows are stored as
    /// `buy_move_click2`, Moonbot's own spelling, and the page has always numbered them the other
    /// way round. The field is reached through [`HotkeysConfig::gesture`] instead.
    pub fn stem(self) -> &'static str {
        match self {
            Self::ForKey(key) => key.stem(),
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
