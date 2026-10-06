//! Public configuration for hotkeys and mouse gestures.
//!
//! Keyboard shortcuts are stored in `gpui::Keystroke::parse` format (`ctrl-r`,
//! `shift-f7`, `ctrl-delete`). An empty string means the action has no hotkey.
//! Mouse gestures mirror Delphi's `TOrderReplaceClick`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::paths;

mod access;
mod persist;
mod ruler;
mod slots;
#[cfg(test)]
mod tests;
mod vocab;

pub use ruler::RulerDrag;
pub use slots::*;
pub use vocab::*;

use super::toml_io;

pub const ORDER_SIZE_KEYS: usize = 6;
pub const SELL_PRESET_KEYS: usize = 6;
pub const MANUAL_STRATEGY_KEYS: usize = 10;

/// Current `hotkeys.toml` generation. Bump only together with a new arm in
/// [`HotkeysConfig::fill_unbound_slots`].
///
/// 1: backfilled the slots that shipped unbound. 2: cleared `chart_shot` where the user had
/// already given Ctrl+F10 to something else. 3: the same for `fig_undo` on Ctrl+Z. 4: cleared the
/// two pending-order GESTURES, which stopped being inert and started placing live orders. 5: the
/// figure-delete gesture yields its Middle default to a trading gesture already on Middle. 6: the
/// same as 2 and 3 for `center_chart` on Ctrl+Right. 7: the same for `toggle_live` on Space.
const SCHEMA: u8 = 7;

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
/// Twenty since 2026-09-12, at the user's request; the core lays the parts out itself and the
/// wire carries the count as a plain `i32`, so nothing else bounds it.
pub const SPLIT_PARTS_MIN: u8 = 2;
pub const SPLIT_PARTS_MAX: u8 = 20;

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
    /// Moonbot's "Cancel buys in all bots": `cancel_all_buys` on every connected core at once.
    ///
    /// Unbound by default, as in Moonbot, so it needs neither a backfill nor a collision
    /// generation: serde's default leaves it empty in every existing file.
    #[serde(default)]
    pub cancel_all_buys_all_cores: String,
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
    /// Local time-axis super zoom; no Moonbot import slot or default binding.
    #[serde(default)]
    pub super_zoom_in: String,
    /// Local time-axis zoom out; no Moonbot import slot or default binding.
    #[serde(default)]
    pub super_zoom_out: String,
    /// Moonbot's "Center chart" (its built-in Ctrl+Right, hence no import slot): every chart drops
    /// its manual Y view, puts the last price at the centre and returns to the live edge — out of
    /// an explicit Pause too — with neither scale touched.
    ///
    /// Arrives pre-filled through its serde default like `chart_shot`, so it needs the same
    /// collision check and no backfill: generation 6 of [`HotkeysConfig::fill_unbound_slots`].
    #[serde(default = "default_center_chart")]
    pub center_chart: String,
    /// Toggle the application-wide Live/Pause flag, the same as the toolbar button.
    ///
    /// Space is free on every shipped default, the Moonbot import already spells it, and
    /// `is_bare_alnum` leaves it out of the amber clash hint. Generation 7 of
    /// [`HotkeysConfig::fill_unbound_slots`] yields the default where Space is already taken.
    #[serde(default = "default_toggle_live")]
    pub toggle_live: String,
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
    /// Optional shortcut for the one-click horizontal ray; absent means unassigned.
    #[serde(default)]
    pub draw_horizontal_ray: String,
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
    #[serde(default = "default_middle", deserialize_with = "tolerant_middle")]
    pub fig_delete_click: MouseGestureBinding,

    /// Live Moonbot MultiOrders path: places a long from the order book.
    #[serde(
        default = "default_left_double",
        deserialize_with = "tolerant_left_double"
    )]
    pub buy_set_click: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: places a short from the order book.
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub short_set_click: MouseGestureBinding,
    /// Live Moonbot path: places a pending long.
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub pending_long_click: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: places a pending short.
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub pending_short_click: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: moves an open/buy long.
    #[serde(
        default = "default_left_shift",
        deserialize_with = "tolerant_left_shift"
    )]
    pub buy_move_click: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: moves a TP/sell long.
    #[serde(default = "default_left_ctrl", deserialize_with = "tolerant_left_ctrl")]
    pub sell_move_click: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: secondary gesture for moving an open/buy long.
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub buy_move_click2: MouseGestureBinding,
    /// Live Moonbot MultiOrders path: secondary gesture for moving a TP/sell long.
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub sell_move_click2: MouseGestureBinding,
    /// Delphi `ReplaceBuyKind`: how the primary Move Open gesture lays out what it moves.
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub buy_move_kind: MoveKind,
    /// Delphi `ReplaceSellKind`: the same for the primary Move TP gesture.
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub sell_move_kind: MoveKind,
    /// Delphi `ReplaceBuyKind2`: the secondary Move Open gesture's kind.
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub buy_move_kind2: MoveKind,
    /// Delphi `ReplaceSellKind2`: the secondary Move TP gesture's kind.
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub sell_move_kind2: MoveKind,
    /// Delphi `SameHotkeysForMove`: short-move gestures mirror long-move gestures.
    #[serde(default = "default_same_hotkeys_for_move")]
    pub same_hotkeys_for_move: bool,
    #[serde(
        default = "default_left_shift",
        deserialize_with = "tolerant_left_shift"
    )]
    pub short_buy_move_click: MouseGestureBinding,
    #[serde(default = "default_left_ctrl", deserialize_with = "tolerant_left_ctrl")]
    pub short_sell_move_click: MouseGestureBinding,
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub short_buy_move_click2: MouseGestureBinding,
    #[serde(default, deserialize_with = "crate::config::tolerant::or_default")]
    pub short_sell_move_click2: MouseGestureBinding,

    /// The mouse half of every keyboard slot that has one, keyed by [`KeySlot::name`].
    ///
    /// One table rather than forty-six fields: a slot's gesture is looked up by the same name the
    /// Moonbot import and the settings page derive from its stem, so there is nothing to keep in
    /// step. Absent means unset, and an unset gesture is removed rather than stored as `none`, so
    /// a file that never used one carries no table at all — and an older build, which does not know
    /// the table, ignores it and keeps every other value. A name this build does not know is kept
    /// through a load-and-save and never offered as a slot; a gesture VALUE this build cannot read drops
    /// that one entry on load, so the next save does not write it back.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::config::tolerant::map_values"
    )]
    pub action_clicks: BTreeMap<String, MouseGestureBinding>,
    /// The modifier that makes the wheel scroll a chart label column instead of the chart.
    #[serde(
        default = "default_label_scroll_wheel",
        deserialize_with = "tolerant_label_scroll_wheel"
    )]
    pub label_scroll_wheel: WheelModifier,
    /// The modifier that turns a left drag over the chart plot into the percent ruler.
    #[serde(
        default = "ruler::default_ruler_drag",
        deserialize_with = "ruler::tolerant_ruler_drag"
    )]
    pub ruler_drag: RulerDrag,
}

impl Default for HotkeysConfig {
    /// Seed the standard bindings, leaving terminal-only super zoom explicitly unbound.
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
            cancel_all_buys_all_cores: String::new(),
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
            super_zoom_in: String::new(),
            super_zoom_out: String::new(),
            center_chart: default_center_chart(),
            toggle_live: default_toggle_live(),
            switch_figure: default_switch_figure(),
            chart_shot: default_chart_shot(),
            draw_hline: default_draw_hline(),
            draw_horizontal_ray: String::new(),
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
            action_clicks: BTreeMap::new(),
            label_scroll_wheel: default_label_scroll_wheel(),
            ruler_drag: ruler::default_ruler_drag(),
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

/// Moonbot's own key for "Center chart" — built in there rather than on its Hotkeys page, which is
/// why the import never carries it. Free on every shipped default; MoonUI's text fields bind
/// Ctrl+Right to a word jump, but a modified press reaches the bindings before the field on
/// purpose (`hotkeys::belongs_to_the_field`), the same trade Ctrl+Z and Ctrl+H already make.
///
/// Returns:
///     The default GPUI keystroke for centring every chart on its price.
fn default_center_chart() -> String {
    "ctrl-right".into()
}

/// Space is free on every shipped default, accepted by the Moonbot import parser, and excluded
/// from the bare-alnum clash hint. A focused text field still types a space
/// (`hotkeys::belongs_to_the_field`).
///
/// Returns:
///     The default GPUI keystroke for toggling Live/Pause.
fn default_toggle_live() -> String {
    "space".into()
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

fn default_label_scroll_wheel() -> WheelModifier {
    WheelModifier::Alt
}

fn default_same_hotkeys_for_move() -> bool {
    true
}

// Field-default twins of the `default_*` gestures above for `tolerant::or_else`: an unknown
// gesture a newer build wrote falls back to the same binding an absent key gets.
fn tolerant_middle<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<MouseGestureBinding, D::Error> {
    super::tolerant::or_else(d, default_middle)
}

fn tolerant_left_double<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<MouseGestureBinding, D::Error> {
    super::tolerant::or_else(d, default_left_double)
}

fn tolerant_left_shift<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<MouseGestureBinding, D::Error> {
    super::tolerant::or_else(d, default_left_shift)
}

fn tolerant_left_ctrl<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<MouseGestureBinding, D::Error> {
    super::tolerant::or_else(d, default_left_ctrl)
}

fn tolerant_label_scroll_wheel<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<WheelModifier, D::Error> {
    super::tolerant::or_else(d, default_label_scroll_wheel)
}
