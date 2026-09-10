//! Who else answers this binding, which of them fires, and which therefore never does.
//!
//! The module is a transcription of two orders that live elsewhere, and it is worth saying why it
//! is a transcription rather than a rule of its own. The first version guessed: it assumed the
//! built-in keys sat below every configurable slot. That is true of eight of them and false of the
//! other thirty, so the page told traders that Panic Sell on Escape would take Escape away, when
//! Panic Sell on Escape in fact never fires at all. A caption that is confidently backwards is
//! worse than no caption, so both orders below are read off the dispatchers and pinned by tests
//! that read those dispatchers' own source.
//!
//! - **Keyboard.** [`RESOLVE_ORDER`] is `hotkeys::resolve_binding`'s chain of `if`s, built-in
//!   branches included, in its order. It returns on the first match, so the first holder of a
//!   keystroke fires and every later holder is dead — whatever either of them acts on. That is why
//!   the keyboard half needs no scope reasoning at all.
//! - **Mouse.** Each button's handler in `panels::chart::render_input` offers a press to its layers
//!   in a fixed order, and the order DIFFERS between buttons: the right button has no order
//!   placement layer at all, so a placement gesture bound to a right button reaches nothing.
//!   [`button_layers`] is that order, and it includes the layers this page has no row for — figure
//!   drawing, the figure menu, the order menu, the X-scale sync.
//!
//! A layer that answers only in a mode or over an object — drawing while a tool is armed, a context
//! menu over its object — does not kill what sits below it: the press falls through everywhere the
//! condition does not hold. Those two coexist, and the caption says so rather than crying wolf.
//!
//! Nothing here forbids anything. Every clash is reported and every binding stays storable.

use std::collections::HashMap;

use moon_core::config::{HotkeysConfig, MouseGestureBinding, MoveKind};
use rust_i18n::t;

use super::pull_gestures::{GestureTarget, local_gesture, target_label};
use super::{
    HotkeySlot, MouseSlot, MoveKindSlot, all_mouse_slots, move_kind_slot_value, parse_hotkey,
    slot_label, slot_value,
};

/// How badly the other holder gets in this row's way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Severity {
    /// This row still fires; something else answers the same binding beside or below it.
    Shares,
    /// This row never fires: something above it takes the press first.
    Shadowed,
}

/// One row's caption.
#[derive(Clone)]
pub(super) struct Clash {
    pub severity: Severity,
    /// Already localized — the row only has to print it.
    pub text: String,
}

/// One step of `hotkeys::resolve_binding`, in the order that function tests them.
///
/// Transcribed, not invented. `resolve_binding` returns on its first match, so this order alone
/// decides which holder of a keystroke is alive; `clash::tests` reads that function's source and
/// fails if the two ever disagree.
enum Step {
    Slot(HotkeySlot),
    /// A binding the user cannot edit: the keystrokes it answers, and the locale key naming it.
    Builtin(&'static [&'static str], &'static str),
}

const RESOLVE_ORDER: &[Step] = &[
    // The drawing layer is tested FIRST, which is the whole reason these eight can take a built-in
    // key away and the thirty below them cannot.
    Step::Slot(HotkeySlot::DrawHline),
    Step::Slot(HotkeySlot::DrawSegment),
    Step::Slot(HotkeySlot::DrawTriangle),
    Step::Slot(HotkeySlot::DrawChannel),
    Step::Slot(HotkeySlot::SwitchFigure),
    Step::Slot(HotkeySlot::FigDelete),
    Step::Slot(HotkeySlot::FigAlert),
    Step::Slot(HotkeySlot::FigUndo),
    // The built-ins sit HERE: below the figure slots, above everything else.
    Step::Builtin(&["shift-escape"], "hotkeys.clash.builtin.close_all"),
    Step::Builtin(&["escape"], "hotkeys.clash.builtin.esc_close"),
    Step::Builtin(&["ctrl-shift-f10"], "hotkeys.clash.builtin.reset_windows"),
    Step::Builtin(&["tab", "delete"], "hotkeys.clash.builtin.cancel_hover"),
    Step::Slot(HotkeySlot::ScalePlus),
    Step::Slot(HotkeySlot::ScaleMinus),
    Step::Slot(HotkeySlot::ChartShot),
    Step::Slot(HotkeySlot::OrderSize(0)),
    Step::Slot(HotkeySlot::OrderSize(1)),
    Step::Slot(HotkeySlot::OrderSize(2)),
    Step::Slot(HotkeySlot::OrderSize(3)),
    Step::Slot(HotkeySlot::OrderSize(4)),
    Step::Slot(HotkeySlot::OrderSize(5)),
    Step::Slot(HotkeySlot::SellPreset(0)),
    Step::Slot(HotkeySlot::SellPreset(1)),
    Step::Slot(HotkeySlot::SellPreset(2)),
    Step::Slot(HotkeySlot::SellPreset(3)),
    Step::Slot(HotkeySlot::SellPreset(4)),
    Step::Slot(HotkeySlot::SellPreset(5)),
    Step::Slot(HotkeySlot::CancelBuy),
    Step::Slot(HotkeySlot::CancelAllBuys),
    Step::Slot(HotkeySlot::PanicSell),
    Step::Slot(HotkeySlot::PanicSellOne),
    Step::Slot(HotkeySlot::JoinSells),
    Step::Slot(HotkeySlot::SplitOrder),
    Step::Slot(HotkeySlot::SplitOrderX),
    Step::Slot(HotkeySlot::SellsToRect),
    Step::Slot(HotkeySlot::NewLong),
    Step::Slot(HotkeySlot::NewShort),
    Step::Slot(HotkeySlot::ShiftBuyUp),
    Step::Slot(HotkeySlot::ShiftBuyDown),
    Step::Slot(HotkeySlot::ShiftSellUp),
    Step::Slot(HotkeySlot::ShiftSellDown),
    Step::Slot(HotkeySlot::SwitchCharts),
    Step::Slot(HotkeySlot::ManualStrategy(0)),
    Step::Slot(HotkeySlot::ManualStrategy(1)),
    Step::Slot(HotkeySlot::ManualStrategy(2)),
    Step::Slot(HotkeySlot::ManualStrategy(3)),
    Step::Slot(HotkeySlot::ManualStrategy(4)),
    Step::Slot(HotkeySlot::ManualStrategy(5)),
    Step::Slot(HotkeySlot::ManualStrategy(6)),
    Step::Slot(HotkeySlot::ManualStrategy(7)),
    Step::Slot(HotkeySlot::ManualStrategy(8)),
    Step::Slot(HotkeySlot::ManualStrategy(9)),
];

/// Every slot, in the order the dispatcher tests it.
///
/// The one list. It was two for a while — this order, and a hand-written one beside the enum — and
/// the second existed only so a test could walk every slot. Deriving it here means a slot that is
/// added to the enum but forgotten in this order is not merely unchecked: it also gets no clash
/// detection, which is a loud enough consequence to notice.
#[cfg(test)]
pub(super) fn slots_in_resolve_order() -> Vec<HotkeySlot> {
    RESOLVE_ORDER
        .iter()
        .filter_map(|step| match step {
            Step::Slot(slot) => Some(*slot),
            Step::Builtin(..) => None,
        })
        .collect()
}

/// A layer of one mouse button's handler: a row of this page, or something the chart does that has
/// no row here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layer {
    /// Arms and extends a figure — only while a drawing tool is selected.
    Draw,
    /// Deletes the figure under the pointer — only over a figure.
    FigDelete,
    /// Places an order. Absent from the right button entirely.
    Place,
    /// Moves a side's orders onto the clicked price.
    Move,
    /// The figure's context menu — only over a figure.
    FigMenu,
    /// The order's context menu — only over an order line.
    OrderMenu,
    /// Syncs the X scale across the window.
    XScale,
}

impl Layer {
    /// Whether the layer answers every press it is offered, or only in a mode or over an object.
    ///
    /// A conditional layer does not kill what sits below it: the press falls through wherever its
    /// condition does not hold, so both live.
    fn unconditional(self) -> bool {
        matches!(self, Self::Place | Self::Move | Self::XScale)
    }

    /// The locale key naming a layer that owns no row on this page, or `None` for one that does.
    fn layer_name(self) -> Option<&'static str> {
        Some(match self {
            Self::Draw => "hotkeys.clash.layer.draw",
            Self::FigMenu => "hotkeys.clash.layer.fig_menu",
            Self::OrderMenu => "hotkeys.clash.layer.order_menu",
            Self::XScale => "hotkeys.clash.layer.x_scale",
            Self::FigDelete | Self::Place | Self::Move => return None,
        })
    }
}

/// The layers one button's handler offers a press to, in its order.
///
/// Transcribed from `panels::chart::render_input`, whose order is itself pinned by
/// `panels::chart::tests`. The right button is the one that surprises: it carries no placement
/// layer, so an order-placing gesture bound to a right button fires nowhere.
fn button_layers(gesture: MouseGestureBinding) -> &'static [Layer] {
    use MouseGestureBinding as G;
    match gesture {
        G::None => &[],
        G::LeftDouble
        | G::LeftCtrl
        | G::LeftShift
        | G::LeftAlt
        | G::LeftCtrlDouble
        | G::LeftShiftDouble
        | G::LeftAltDouble => &[Layer::Draw, Layer::FigDelete, Layer::Place, Layer::Move],
        G::Middle | G::MiddleCtrl | G::MiddleShift | G::MiddleAlt => {
            &[Layer::FigDelete, Layer::Place, Layer::Move, Layer::XScale]
        }
        G::RightDouble | G::RightCtrl | G::RightShift | G::RightAlt => &[
            Layer::FigDelete,
            Layer::FigMenu,
            Layer::Move,
            Layer::OrderMenu,
        ],
    }
}

/// Which layer a row belongs to.
fn slot_layer(slot: MouseSlot) -> Layer {
    match slot {
        MouseSlot::FigDelete => Layer::FigDelete,
        MouseSlot::BuySet
        | MouseSlot::ShortSet
        | MouseSlot::PendingLong
        | MouseSlot::PendingShort => Layer::Place,
        _ => Layer::Move,
    }
}

/// Whether a rowless chart layer actually answers THIS gesture.
///
/// Drawing is claimed only for the Ctrl-left gestures — `try_fig_click` gates on the platform's
/// secondary modifier — while the X-scale sync is Shift+Middle alone. Both context menus answer any
/// press of their button over their object.
fn ownerless_layer(gesture: MouseGestureBinding, layer: Layer) -> bool {
    use MouseGestureBinding as G;
    match layer {
        Layer::Draw => matches!(gesture, G::LeftCtrl | G::LeftCtrlDouble),
        Layer::XScale => gesture == G::MiddleShift,
        Layer::FigMenu | Layer::OrderMenu => true,
        Layer::FigDelete | Layer::Place | Layer::Move => false,
    }
}

/// A keystroke reduced to what the dispatcher compares: modifiers and key, nothing else.
///
/// `hotkeys::pressed` compares those two rather than the whole `Keystroke`, so two slots that spell
/// one combination differently are one binding to it and must be one binding here. An unparseable
/// string yields `None` on both sides, which is the right reading: such a slot can never fire, so it
/// can never collide either.
fn normalise(raw: &str) -> Option<KeyId> {
    let k = parse_hotkey(raw)?;
    Some((k.modifiers, k.key))
}

type KeyId = (gpui::Modifiers, String);

/// One holder of a keystroke: a row of this page, or a built-in that owns it.
#[derive(Clone, Copy)]
enum Holder {
    Slot(HotkeySlot),
    Builtin(&'static str),
}

impl Holder {
    fn label(self) -> String {
        match self {
            Self::Slot(slot) => slot_label(slot),
            Self::Builtin(name) => t!(name).to_string(),
        }
    }
}

/// Who holds what, built once per group render.
pub(super) struct Clashes {
    /// Keystroke -> its holders in [`RESOLVE_ORDER`]; the first of them is the one that fires.
    keys: HashMap<KeyId, Vec<Holder>>,
    /// Gesture -> the rows holding it. A move row whose kind is `None` is left out: the dispatcher
    /// steps past such a row rather than letting it silence the next one.
    gestures: HashMap<MouseGestureBinding, Vec<MouseSlot>>,
}

impl Clashes {
    pub(super) fn build(hotkeys: &HotkeysConfig) -> Self {
        let mut keys: HashMap<KeyId, Vec<Holder>> = HashMap::new();
        for step in RESOLVE_ORDER {
            match step {
                Step::Slot(slot) => {
                    if let Some(id) = normalise(slot_value(hotkeys, *slot)) {
                        keys.entry(id).or_default().push(Holder::Slot(*slot));
                    }
                }
                Step::Builtin(strokes, name) => {
                    for raw in *strokes {
                        if let Some(id) = normalise(raw) {
                            keys.entry(id).or_default().push(Holder::Builtin(name));
                        }
                    }
                }
            }
        }
        let mut gestures: HashMap<MouseGestureBinding, Vec<MouseSlot>> = HashMap::new();
        for slot in all_mouse_slots() {
            // What the terminal FIRES for this row, not what the field holds: with the mirror
            // switch on, a short row's stored value is not read at all, and indexing it would put a
            // rival nothing dispatches into every other row's caption.
            let gesture = local_gesture(hotkeys, slot);
            if gesture != MouseGestureBinding::None && !inert_move_row(hotkeys, slot) {
                gestures.entry(gesture).or_default().push(slot);
            }
        }
        Self { keys, gestures }
    }

    /// The caption for one keyboard row.
    ///
    /// Direction is the whole point: the row that resolves FIRST is told what it is taking, and the
    /// rows below it are told they are dead. Handing both the same sentence — which is what the
    /// first version did — is how a working binding gets reported as broken.
    pub(super) fn key(&self, hotkeys: &HotkeysConfig, slot: HotkeySlot) -> Option<Clash> {
        let id = normalise(slot_value(hotkeys, slot))?;
        let holders = self.keys.get(&id)?;
        if holders.len() < 2 {
            return None;
        }
        let mine = holders.iter().position(|h| match h {
            Holder::Slot(other) => *other == slot,
            Holder::Builtin(_) => false,
        })?;
        if mine == 0 {
            let losers: Vec<String> = holders[1..]
                .iter()
                .filter(|other| !documented_fallback(slot, **other))
                .map(|h| h.label())
                .collect();
            if losers.is_empty() {
                return None;
            }
            return Some(Clash {
                severity: Severity::Shares,
                text: t!("hotkeys.clash.wins", rows = losers.join(", ")).to_string(),
            });
        }
        Some(Clash {
            severity: Severity::Shadowed,
            text: t!("hotkeys.clash.key", rows = holders[0].label()).to_string(),
        })
    }

    /// The caption for one gesture row.
    pub(super) fn mouse(&self, hotkeys: &HotkeysConfig, slot: MouseSlot) -> Option<Clash> {
        let gesture = local_gesture(hotkeys, slot);
        if gesture == MouseGestureBinding::None {
            return None;
        }
        let layers = button_layers(gesture);
        let mine = slot_layer(slot);
        // A placement gesture on the right button reaches no placement layer at all.
        if !layers.contains(&mine) {
            return Some(Clash {
                severity: Severity::Shadowed,
                text: t!("hotkeys.clash.no_layer").to_string(),
            });
        }
        let rows = self
            .gestures
            .get(&gesture)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut kills: Vec<String> = Vec::new();
        let mut beside: Vec<String> = Vec::new();
        for layer in layers {
            let holders = layer_holders(*layer, rows, slot, gesture);
            if holders.is_empty() {
                continue;
            }
            match layer_position(layers, *layer, mine) {
                // Above and unconditional: it takes every press and this row is dead.
                Position::Above if layer.unconditional() => kills.extend(holders),
                // Above but conditional, or below: both live.
                Position::Above | Position::Below => beside.extend(holders),
                // The same layer, and the handler asks it once: the first row in it answers.
                Position::Same => kills.extend(holders),
            }
        }
        if !kills.is_empty() {
            return Some(Clash {
                severity: Severity::Shadowed,
                text: t!("hotkeys.clash.gesture", rows = kills.join(", ")).to_string(),
            });
        }
        if beside.is_empty() {
            return None;
        }
        Some(Clash {
            severity: Severity::Shares,
            text: t!("hotkeys.clash.shares", rows = beside.join(", ")).to_string(),
        })
    }
}

/// Where one layer sits relative to the row's own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    Above,
    Same,
    Below,
}

fn layer_position(layers: &[Layer], layer: Layer, mine: Layer) -> Position {
    let ix = |l: Layer| layers.iter().position(|x| *x == l).unwrap_or(usize::MAX);
    match ix(layer).cmp(&ix(mine)) {
        std::cmp::Ordering::Less => Position::Above,
        std::cmp::Ordering::Equal => Position::Same,
        std::cmp::Ordering::Greater => Position::Below,
    }
}

/// Everything that answers this gesture at one layer, other than the row asking.
///
/// The two halves of ONE move row are not counted against each other:
/// `HotkeysConfig::resolve_move_gesture` tests each side separately and answers `MoveSide::Both`
/// when the press hits both, rather than picking a winner. The shipped defaults arrive there
/// without the mirror switch even being on, which is how this exemption was found.
fn layer_holders(
    layer: Layer,
    rows: &[MouseSlot],
    asking: MouseSlot,
    gesture: MouseGestureBinding,
) -> Vec<String> {
    if let Some(name) = layer.layer_name() {
        return if ownerless_layer(gesture, layer) {
            vec![t!(name).to_string()]
        } else {
            Vec::new()
        };
    }
    rows.iter()
        .filter(|other| **other != asking && slot_layer(**other) == layer)
        .filter(|other| !same_move_row(asking, **other))
        .map(|other| target_label(GestureTarget::Gesture(*other)))
        .collect()
}

/// The one pairing that is not a shadow although the order says it is.
///
/// `fig_delete` ships on Delete, above the built-in cancel — and `Shell::on_hotkey` hands the press
/// on to that cancel whenever no figure is selected, which is exactly why it may hold the key. The
/// exemption is for THAT pair only: the same slot on Escape or Ctrl+Shift+F10 has no such
/// fall-through and really does end them.
fn documented_fallback(slot: HotkeySlot, other: Holder) -> bool {
    matches!(
        (slot, other),
        (
            HotkeySlot::FigDelete,
            Holder::Builtin("hotkeys.clash.builtin.cancel_hover")
        )
    )
}

fn same_move_row(a: MouseSlot, b: MouseSlot) -> bool {
    move_pair(a).is_some() && move_pair(a) == move_pair(b)
}

fn move_pair(slot: MouseSlot) -> Option<u8> {
    Some(match slot {
        MouseSlot::BuyMove | MouseSlot::ShortBuyMove => 0,
        MouseSlot::SellMove | MouseSlot::ShortSellMove => 1,
        MouseSlot::BuyMove2 | MouseSlot::ShortBuyMove2 => 2,
        MouseSlot::SellMove2 | MouseSlot::ShortSellMove2 => 3,
        _ => return None,
    })
}

/// A move row whose kind is `None` sends nothing and does not stop the next row.
///
/// `resolve_move_gesture` steps past such a row on purpose — "another slot may hold the same
/// binding WITH a kind, and giving up here would let a disabled row silence a working one" — so
/// counting it as a holder would report a shadow that never happens.
fn inert_move_row(hotkeys: &HotkeysConfig, slot: MouseSlot) -> bool {
    let kind_slot = match move_pair(slot) {
        Some(0) => MoveKindSlot::BuyMove,
        Some(1) => MoveKindSlot::SellMove,
        Some(2) => MoveKindSlot::BuyMove2,
        Some(3) => MoveKindSlot::SellMove2,
        _ => return false,
    };
    move_kind_slot_value(hotkeys, kind_slot) == MoveKind::None
}

#[cfg(test)]
mod tests;
