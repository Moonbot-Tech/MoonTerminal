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
//!   in a fixed order, and the order DIFFERS between buttons: the right button offers placement
//!   LAST, under both context menus, where the left and middle buttons offer it above the move
//!   layer. [`button_layers`] is that order, and it includes the layers this page has no row for —
//!   figure drawing, the figure menu, the order menu, the X-scale sync. This half was guessed too,
//!   and the guess said the right button placed nothing at all — a red "will not fire" on a gesture
//!   that opens a live position.
//! - **Within one layer** the order is a third thing again, and [`same_layer_rank`] is it: placement
//!   asks in `all_mouse_slots` order, the move rows in `resolve_move_gesture`'s pair order.
//!
//! A layer that answers only in a mode or over an object — drawing while a tool is armed, a context
//! menu over its object — does not kill what sits below it: the press falls through everywhere the
//! condition does not hold. Those two coexist, and the caption says so rather than crying wolf.
//!
//! Nothing here forbids anything. Every clash is reported and every binding stays storable.

use std::collections::HashMap;

use moon_core::config::{HotkeysConfig, MouseGestureBinding, MoveKind};
use rust_i18n::t;

use crate::hotkeys::{BindingId, binding_id};

use super::pull_gestures::{GestureTarget, local_gesture, target_label};
use super::{
    HotkeySlot, MouseSlot, MoveKindSlot, all_mouse_slots, move_kind_slot_value, slot_label,
    slot_value,
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
    /// Places an order. Offered by every button, but LAST on the right, under both menus.
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
    ///
    /// The degenerate form of a question that wants a SET: "everywhere" and "somewhere" are the two
    /// ends of "on which surfaces", and a boolean cannot say that the somewhere of the layer above is
    /// the same somewhere as the row below — which is the one case a conditional layer does kill, and
    /// why [`dead_fig_delete_gesture`] exists as a list beside it. `meta::Scope` is the set.
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
/// `panels::chart::tests`. The right button is the one that surprises: placement sits LAST there,
/// under both context menus, so a right-button placement gesture works — but only where no menu
/// claims the press first. The earlier reading, that the right button places nothing at all, put a
/// red "will not fire" on a gesture that opens a live position: `mouse_down_right` really does
/// reach `try_place_order_click`.
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
            Layer::Place,
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
/// Drawing is claimed for both Ctrl-left gestures, and the DOUBLE one needs saying: a Ctrl double
/// click is two presses, and only the second carries `click_count == 2`. Press one reaches
/// `try_fig_click` like any single Ctrl press and grabs the figure under the pointer; press two
/// skips the drawing layer (`mouse_down_left` gates it on `e.click_count <= 1 || starting_band`) and
/// reaches the rows below. So a row bound to the double gesture SHARES the sequence with drawing
/// rather than losing it — which is what the caption says, and why this arm covers both.
///
/// The X-scale sync is Shift+Middle alone. Both context menus answer any press of their button over
/// their object.
fn ownerless_layer(gesture: MouseGestureBinding, layer: Layer) -> bool {
    use MouseGestureBinding as G;
    match layer {
        Layer::Draw => matches!(gesture, G::LeftCtrl | G::LeftCtrlDouble),
        Layer::XScale => gesture == G::MiddleShift,
        Layer::FigMenu | Layer::OrderMenu => true,
        Layer::FigDelete | Layer::Place | Layer::Move => false,
    }
}

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
    keys: HashMap<BindingId, Vec<Holder>>,
    /// Gesture -> the rows holding it. A move row whose kind is `None` is left out: the dispatcher
    /// steps past such a row rather than letting it silence the next one.
    gestures: HashMap<MouseGestureBinding, Vec<MouseSlot>>,
}

impl Clashes {
    pub(super) fn build(hotkeys: &HotkeysConfig) -> Self {
        let mut keys: HashMap<BindingId, Vec<Holder>> = HashMap::new();
        for step in RESOLVE_ORDER {
            match step {
                Step::Slot(slot) => {
                    if let Some(id) = binding_id(slot_value(hotkeys, *slot)) {
                        keys.entry(id).or_default().push(Holder::Slot(*slot));
                    }
                }
                Step::Builtin(strokes, name) => {
                    for raw in *strokes {
                        if let Some(id) = binding_id(raw) {
                            keys.entry(id).or_default().push(Holder::Builtin(name));
                        }
                    }
                }
            }
        }
        let mut gestures: HashMap<MouseGestureBinding, Vec<MouseSlot>> = HashMap::new();
        for slot in all_mouse_slots() {
            if let Some(gesture) = firing_gesture(hotkeys, slot) {
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
        let id = binding_id(slot_value(hotkeys, slot))?;
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

    /// The captions for one gesture row, in the order they should be printed.
    ///
    /// A list rather than one line: a row can both TAKE its binding from a row below it and SHARE
    /// it with a layer that only answers over an object, and those are two different sentences
    /// about two different rivals. `row_head` prints one line per entry.
    pub(super) fn mouse(&self, hotkeys: &HotkeysConfig, slot: MouseSlot) -> Vec<Clash> {
        // Nothing this row dispatches, nothing to caption. The index above uses the same reading, so
        // the two cannot disagree about which rows are even in the running — and they used to: a
        // caption was once handed to a row whose Move kind is `None`, telling it that it was taking
        // the binding from the row that is in fact the only one firing.
        let Some(gesture) = firing_gesture(hotkeys, slot) else {
            return Vec::new();
        };
        let layers = button_layers(gesture);
        let mine = slot_layer(slot);
        let rows = self
            .gestures
            .get(&gesture)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        // Two values of the figure-delete dropdown never fire and no layer ORDER says so, so the
        // walk below cannot find them; seeded here and emitted by the same tail as every other kill.
        let mut kills: Vec<String> = match mine {
            Layer::FigDelete => dead_fig_delete_gesture(gesture)
                .and_then(Layer::layer_name)
                .map(|name| vec![t!(name).to_string()])
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let mut wins: Vec<String> = Vec::new();
        let mut beside: Vec<String> = Vec::new();
        for layer in layers {
            let position = layer_position(layers, *layer, mine);
            if position == Position::Same {
                // The same layer, asked once: whichever row its own dispatcher reaches first
                // answers, and the others never do. Direction is the whole point here, exactly as
                // `Self::key` states beside it — handing both rows "will not fire" reports the one
                // that DOES fire as broken.
                let rank = same_layer_rank(slot);
                for other in layer_rows(*layer, rows, slot) {
                    let label = target_label(GestureTarget::Gesture(other));
                    if same_layer_rank(other) < rank {
                        kills.push(label);
                    } else {
                        wins.push(label);
                    }
                }
                continue;
            }
            let holders = layer_holders(*layer, rows, slot, gesture);
            match position {
                // Above and unconditional: it takes every press and this row is dead.
                Position::Above if layer.unconditional() => kills.extend(holders),
                // BELOW, and this row takes every press that layer would have seen. Two ways to
                // do that, and the second is easy to miss: my layer answers unconditionally, OR we
                // both need the same object and I am asked first — over that object the press never
                // reaches them, and away from it neither of us answers.
                //
                // The first case has to be told from this side too: `buy_set_click` on Shift+Left
                // sits above the move layer, which holds the same gesture by default — the move row
                // read "will not fire" while this one read "both work", so one collision described
                // itself two contradictory ways.
                Position::Below
                    if mine.unconditional()
                        || (layer_object(mine).is_some()
                            && layer_object(mine) == layer_object(*layer)) =>
                {
                    wins.extend(holders)
                }
                // Above but conditional, or below a row that is itself conditional: both live.
                _ => beside.extend(holders),
            }
        }
        // A row that never answers has nothing else worth saying: the other captions describe where
        // it still works, and it does not.
        if !kills.is_empty() {
            return vec![Clash {
                severity: Severity::Shadowed,
                text: t!("hotkeys.clash.gesture", rows = kills.join(", ")).to_string(),
            }];
        }
        let mut notes = Vec::new();
        if !wins.is_empty() {
            notes.push(Clash {
                severity: Severity::Shares,
                text: t!("hotkeys.clash.wins", rows = wins.join(", ")).to_string(),
            });
        }
        if !beside.is_empty() {
            notes.push(Clash {
                severity: Severity::Shares,
                text: t!("hotkeys.clash.shares", rows = beside.join(", ")).to_string(),
            });
        }
        notes
    }
}

/// What has to be under the pointer for a layer to answer at all.
///
/// The missing half of [`Layer::unconditional`]: that one asks whether a layer answers EVERY press,
/// which is enough to know when an upper layer kills a lower one, and not enough for the reverse.
/// Two conditional layers that need the SAME object do not coexist — whichever is offered the press
/// first takes every press the other would ever have seen. A figure-delete gesture on the right
/// button reads exactly that way: the delete layer is offered the press before the figure menu, and
/// over a figure it consumes it, so "both work" was false for the one gesture the row is bound to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Object {
    /// A drawn figure within the hit threshold.
    Figure,
    /// An order line under the pointer.
    OrderLine,
}

/// The object a layer needs, or `None` for one that answers wherever it is offered the press.
///
/// `Draw`'s entry is the NARROW half of its condition and is why this is not the whole story:
/// `try_fig_click` also answers on empty plot while a tool is armed, because that is where a new
/// figure starts. It costs nothing today — no settings row belongs to the drawing layer, so this is
/// never asked about `Draw` as the ROW's own layer, only as a rival above one — but a row that ever
/// does will need the armed-tool mode here as well, which is the `Scope` model the plan carries.
fn layer_object(layer: Layer) -> Option<Object> {
    match layer {
        Layer::Draw | Layer::FigDelete | Layer::FigMenu => Some(Object::Figure),
        Layer::OrderMenu => Some(Object::OrderLine),
        Layer::Place | Layer::Move | Layer::XScale => None,
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

/// A gesture the figure-delete row can hold and never fire on, and the layer that takes it.
///
/// Neither is a matter of layer ORDER — the figure-delete layer even sits ABOVE the menu on the right
/// button — so [`button_layers`] cannot express them and the caption used to read "both work" about a
/// gesture that is simply dead. Both are already written down in
/// `panels::chart::figures::erase`'s own module header:
///
/// - `Ctrl+Left` is taken by the drawing layer, which grabs an existing figure with the SAME hit
///   predicate and threshold this row would delete by. Sharing the object is what makes it a kill
///   rather than a coexistence: there is nowhere left for the press to fall through to. The DOUBLE
///   variant is NOT dead and must not be listed here — `mouse_down_left` gates the figure layer on
///   `e.click_count <= 1 || starting_band`, so press two of a Ctrl double click arrives at this row
///   (press one still goes to drawing, which is a shared sequence, not a kill).
/// - a right DOUBLE click never arrives — press one opens the figure menu and its overlay eats press
///   two — even though the figure-delete layer sits ABOVE the menu on that button.
///
/// Windows and Linux. On macOS the secondary modifier is Command, so `Ctrl+Left` does arrive there;
/// that is the same platform simplification [`ownerless_layer`] already makes for the drawing layer,
/// and the row it mislabels is one, on one platform, against a caption that is backwards on every.
///
/// A LIST, where the general rule is a set relation: both arms are "the layer above shares this
/// row's own object", which is what `meta::Scope`'s documented containment — `FIGURE` as a narrower
/// form of `CURSOR` — exists to express and has no caller yet. Two hand-written arms are the right
/// size for fixing one live wrong caption; a third belongs in that model instead. The plan carries it.
fn dead_fig_delete_gesture(gesture: MouseGestureBinding) -> Option<Layer> {
    use MouseGestureBinding as G;
    match gesture {
        G::LeftCtrl => Some(Layer::Draw),
        G::RightDouble => Some(Layer::FigMenu),
        _ => None,
    }
}

/// The gesture this row actually dispatches on, or `None` for a row that dispatches nothing.
///
/// Two ways to hold no press, and both must read the same to the index and to the caption: an unset
/// dropdown, and a move row whose kind is `None`, which `resolve_move_gesture` steps past on purpose.
/// It also answers what the terminal FIRES rather than what the field holds — with the mirror switch
/// on, a short row's stored value is not read at all, and indexing it would put a rival nothing
/// dispatches into every other row's caption.
fn firing_gesture(hotkeys: &HotkeysConfig, slot: MouseSlot) -> Option<MouseGestureBinding> {
    let gesture = local_gesture(hotkeys, slot);
    if gesture == MouseGestureBinding::None || inert_move_row(hotkeys, slot) {
        return None;
    }
    Some(gesture)
}

/// Where a row stands in its OWN layer's dispatch order, lowest answering first.
///
/// Neither layer is asked in this page's list order, and that is why this exists:
/// - placement — `panels::chart::trade::placement_intent` tries the four rows in
///   [`all_mouse_slots`] order and returns the FIRST match;
/// - move — `HotkeysConfig::resolve_move_gesture` walks four PAIRS, buy, sell, buy2, sell2, testing
///   each pair's long and short halves together, so `ShortBuyMove` answers before `SellMove2`
///   although this page lists it four rows later.
///
/// Ranks are only ever compared within one layer, so the two families may reuse the same numbers.
/// The halves of one move pair share a rank and are never compared: `same_move_row` exempts them.
fn same_layer_rank(slot: MouseSlot) -> u8 {
    match slot {
        MouseSlot::BuySet => 0,
        MouseSlot::ShortSet => 1,
        MouseSlot::PendingLong => 2,
        MouseSlot::PendingShort => 3,
        // The move pairs are numbered ONCE, by `move_pair`, which `same_move_row` also reads: a
        // second copy of that table here would let the rank that picks a winner disagree with the
        // exemption that decides whether the two rows are rivals at all. `FigDelete` is alone on
        // its layer, so its number is never compared with anything.
        other => move_pair(other).unwrap_or(0),
    }
}

/// The ROWS that answer this gesture at one layer, other than the row asking.
///
/// The two halves of ONE move row are not counted against each other:
/// `HotkeysConfig::resolve_move_gesture` tests each side separately and answers `MoveSide::Both`
/// when the press hits both, rather than picking a winner. The shipped defaults arrive there
/// without the mirror switch even being on, which is how this exemption was found.
///
/// Rows rather than labels because one caller ranks them and the other only names them, and the
/// exemption above must hold for both — it used to be written out twice.
fn layer_rows<'a>(
    layer: Layer,
    rows: &'a [MouseSlot],
    asking: MouseSlot,
) -> impl Iterator<Item = MouseSlot> + 'a {
    rows.iter()
        .copied()
        .filter(move |other| *other != asking && slot_layer(*other) == layer)
        .filter(move |other| !same_move_row(asking, *other))
}

/// Everything that answers this gesture at one layer, named for a caption.
///
/// A layer with no row of its own — drawing, either context menu, the X-scale sync — is named by
/// the layer itself, and only when it really claims this gesture.
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
    layer_rows(layer, rows, asking)
        .map(|other| target_label(GestureTarget::Gesture(other)))
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
