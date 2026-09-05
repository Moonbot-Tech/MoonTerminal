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
/// already given Ctrl+F10 to something else.
const SCHEMA: u8 = 3;

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
        match (entry, short && !self.same_hotkeys_for_move) {
            (true, false) => [self.buy_move_click, self.buy_move_click2],
            (true, true) => [self.short_buy_move_click, self.short_buy_move_click2],
            (false, false) => [self.sell_move_click, self.sell_move_click2],
            (false, true) => [self.short_sell_move_click, self.short_sell_move_click2],
        }
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
    ///     `None`, which is Moonbot's way of leaving a bound gesture inert. Slots are examined in
    ///     the order the settings page lists them, so a gesture the user put on two of them
    ///     resolves the same way twice rather than by whichever branch happened to run first.
    pub fn resolve_move_gesture(
        &self,
        matches: impl Fn(MouseGestureBinding) -> bool,
    ) -> Option<MoveGestureCommand> {
        for (entry, second, kind) in [
            (true, false, self.buy_move_kind),
            (false, false, self.sell_move_kind),
            (true, true, self.buy_move_kind2),
            (false, true, self.sell_move_kind2),
        ] {
            // Both sides come from `move_gestures`, which is the one place that reads
            // `same_hotkeys_for_move`: with the mirror on it hands back the long gesture for the
            // short side too, so one press claims both and the core is told `Both`.
            let ix = usize::from(second);
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

    /// Every keystroke this file already binds, for collision checks.
    ///
    /// Includes the preset and manual-strategy arrays: those are exactly where a user's own
    /// `alt-1` is most likely to sit. A key held by two slots appears twice, which is what makes a
    /// duplicate visible to the caller.
    pub fn bound_keys(&self) -> Vec<String> {
        let named = [
            &self.cancel_buy,
            &self.panic_sell,
            &self.panic_sell_one,
            &self.cancel_all_buys,
            &self.join_sells,
            &self.switch_charts,
            &self.new_long,
            &self.new_short,
            &self.split_order,
            &self.split_order_x,
            &self.sells_to_rect,
            &self.shift_buy_up,
            &self.shift_buy_down,
            &self.shift_sell_up,
            &self.shift_sell_down,
            &self.scale_plus,
            &self.scale_minus,
            &self.switch_figure,
            &self.chart_shot,
            &self.draw_hline,
            &self.draw_segment,
            &self.draw_triangle,
            &self.draw_channel,
            &self.fig_delete,
            &self.fig_alert,
            &self.fig_undo,
        ];
        named
            .into_iter()
            .chain(self.order_size.iter())
            .chain(self.sell_preset.iter())
            .chain(self.manual_strategy.iter())
            .map(|key| key.trim().to_string())
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
        "hotkeys.toml: {} уже занят, {} оставлен без клавиши",
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
