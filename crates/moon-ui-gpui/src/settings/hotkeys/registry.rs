//! The Hotkeys tab as DATA: every row it shows, in order, and what each one edits.
//!
//! One list, read by everything that used to keep its own: the row builder walked a four-hundred
//! line `match` that named every slot with its two locale keys; the pull preview named the same
//! rows again to title them; the clash captions named them a third time; the marks test walked yet
//! another. A slot added to one list and forgotten in another was not a compile error — it was a
//! row without a caption, or a preview line without a title.
//!
//! Nothing here is spelled twice. A row's id, title and hint all come off the slot's own stem
//! (`KeySlot::stem`, `GestureSlot::stem`), dressed with hyphens for an element id and with the
//! `hotkeys.` prefix for the locale; a test checks that every dressing resolves to real text. Which
//! side of the page a row is on is the one fact stated here and nowhere else.
//!
//! A row can carry a key, a gesture, or BOTH. Every keyboard slot that can carry a mouse half
//! (`KeySlot::has_mouse_half`) is a two-editor row whose gesture is `GestureSlot::ForKey`; the
//! figure-delete row pairs its key with the gesture field it had before the table existed. A row's
//! two facts are then the join of its halves' facts.

use std::sync::LazyLock;

use moon_core::config::{
    GestureSlot, KeySlot, MANUAL_STRATEGY_KEYS, MoveKindSlot, ORDER_SIZE_KEYS, SELL_PRESET_KEYS,
    SPLIT_ORDER_PARTS,
};
use rust_i18n::t;

use crate::hotkeys::meta::{self, SlotMeta};

/// Hotkey groups shown as sub-tabs below the built-in block, matching Moonbot's hotkey pages.
/// Built-ins are not a group and remain visible above the switcher.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::settings) enum HotkeyGroup {
    Presets,
    Trading,
    Chart,
    Draw,
    OrderMove,
    Mouse,
    ManualStrategy,
    /// Not a hotkey page: the "pull layout from core" preview, which compares every slot the core
    /// carries against the local one. A tab of its own so the manual-strategy page ends where its
    /// rows end.
    CorePull,
}

impl HotkeyGroup {
    pub(in crate::settings) const ALL: [Self; 8] = [
        Self::Presets,
        Self::Trading,
        Self::Chart,
        Self::Draw,
        Self::OrderMove,
        Self::Mouse,
        Self::ManualStrategy,
        Self::CorePull,
    ];

    pub(in crate::settings) fn title(self) -> String {
        match self {
            Self::Presets => t!("hotkeys.group.presets"),
            Self::Trading => t!("hotkeys.group.trading"),
            Self::Chart => t!("hotkeys.group.chart"),
            Self::Draw => t!("hotkeys.group.draw"),
            Self::OrderMove => t!("hotkeys.group.order_move"),
            Self::Mouse => t!("hotkeys.group.mouse"),
            Self::ManualStrategy => t!("hotkeys.group.manual_strategy"),
            Self::CorePull => t!("hotkeys.pull.title"),
        }
        .to_string()
    }

    /// Whether the page is the hotkey table — with its header — rather than the pull preview.
    pub(in crate::settings) fn is_table(self) -> bool {
        self != Self::CorePull
    }

    /// Returns the hint shown above the active sub-tab's rows.
    pub(in crate::settings) fn hint(self) -> String {
        match self {
            Self::Presets => t!("hotkeys.group.presets_hint"),
            Self::Trading => t!("hotkeys.group.trading_hint"),
            Self::Chart => t!("hotkeys.group.chart_hint"),
            Self::Draw => t!("hotkeys.group.draw_hint"),
            Self::OrderMove => t!("hotkeys.group.order_move_hint"),
            Self::Mouse => t!("hotkeys.group.mouse_hint"),
            Self::ManualStrategy => t!("hotkeys.group.manual_strategy_hint"),
            Self::CorePull => t!("hotkeys.pull.hint"),
        }
        .to_string()
    }
}

/// The editors one row carries. At least one, by construction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Editors {
    Key(KeySlot),
    Mouse(GestureSlot),
    Both(KeySlot, GestureSlot),
}

/// One action the page lets the user bind: which sub-tab it is on and which slots it edits.
///
/// Everything else a row shows — id, title, hint, marks, whether the mirror switch owns it, which
/// kind selector it carries — is derived from those two facts, so there is nothing here to keep in
/// step with anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct SlotSpec {
    pub group: HotkeyGroup,
    editors: Editors,
}

impl SlotSpec {
    /// The keyboard slot this row edits, if it has one.
    pub fn key(&self) -> Option<KeySlot> {
        match self.editors {
            Editors::Key(key) | Editors::Both(key, _) => Some(key),
            Editors::Mouse(_) => None,
        }
    }

    /// The gesture slot this row edits, if it has one.
    pub fn mouse(&self) -> Option<GestureSlot> {
        match self.editors {
            Editors::Mouse(mouse) | Editors::Both(_, mouse) => Some(mouse),
            Editors::Key(_) => None,
        }
    }

    /// The "move kind" selector this row carries — the long move rows only.
    pub fn kind(&self) -> Option<MoveKindSlot> {
        self.mouse()?.kind()
    }

    /// Whether the mirror switch owns this row: a short move row is inert while
    /// `same_hotkeys_for_move` is set, following its long twin instead of its own field.
    pub fn follows_mirror(&self) -> bool {
        self.mouse().is_some_and(GestureSlot::is_short_move)
    }

    /// The row's title: the localized action, or the preset's own identity (`F3`, `S2`).
    pub fn title(&self) -> String {
        match self.editors {
            Editors::Key(key) | Editors::Both(key, _) => key_title(key),
            Editors::Mouse(mouse) => mouse_only_title(mouse),
        }
    }

    /// Whether the title is an identity like `F3` rather than a phrase — the preset rows, which the
    /// page sets in mono for that reason.
    pub fn title_is_identity(&self) -> bool {
        matches!(
            self.key(),
            Some(KeySlot::OrderSize(_) | KeySlot::SellPreset(_))
        )
    }

    /// The localized explanation under the title.
    pub fn hint(&self) -> String {
        match self.editors {
            Editors::Key(key) | Editors::Both(key, _) => match key {
                KeySlot::OrderSize(i) => t!("hotkeys.order_size", n = i + 1).to_string(),
                KeySlot::SellPreset(i) => t!("hotkeys.sell_preset", n = i + 1).to_string(),
                KeySlot::ManualStrategy(i) => {
                    t!("hotkeys.manual_strategy_hint", n = i + 1).to_string()
                }
                // The one hint with a number in it that is not an index.
                KeySlot::SplitOrder => {
                    t!("hotkeys.split_order_hint", n = SPLIT_ORDER_PARTS).to_string()
                }
                named => {
                    let key = format!("hotkeys.{}_hint", named.stem());
                    t!(&key).to_string()
                }
            },
            Editors::Mouse(mouse) => {
                let key = format!("hotkeys.mouse.{}_hint", mouse.stem());
                t!(&key).to_string()
            }
        }
    }

    /// The row's two facts — where it acts and whether Moonbot writes it — joined across its
    /// editors.
    ///
    /// The origin says whether ANY half travels: for almost every two-editor row that is the key
    /// (a Moonbot paste or a core pull writes it) while the gesture is ours, and the MB column's
    /// tooltip says so in as many words. The scope is `Scope::join`, which reads the containment
    /// the surfaces document.
    pub fn meta(&self) -> SlotMeta {
        match self.editors {
            Editors::Key(key) => meta::key_slot_meta(key),
            Editors::Mouse(mouse) => meta::gesture_slot_meta(mouse),
            Editors::Both(key, mouse) => {
                let k = meta::key_slot_meta(key);
                let m = meta::gesture_slot_meta(mouse);
                SlotMeta {
                    origin: k.origin.join(m.origin),
                    scope: k.scope.join(m.scope),
                }
            }
        }
    }
}

/// One row of the tab. Three rows edit something that is not a slot and are placed by the same
/// list, so the tab has no order of its own to keep.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Row {
    Slot(SlotSpec),
    /// The part count `Split N` reads, after the two split rows it serves.
    SplitParts,
    /// The mirror switch, between the long move rows and the short ones it owns.
    SameForMove,
    /// The "pull layout from core" preview — the whole of its own page.
    CorePull,
}

impl Row {
    /// The sub-tab this row is on.
    pub fn group(&self) -> HotkeyGroup {
        match self {
            Self::Slot(spec) => spec.group,
            Self::SplitParts => HotkeyGroup::Trading,
            Self::SameForMove => HotkeyGroup::Mouse,
            Self::CorePull => HotkeyGroup::CorePull,
        }
    }
}

/// Every row of the tab, in page order.
///
/// Built once: the list holds slot identities only, nothing a locale or a config could change, and
/// the titles that name a rival in a caption look a row up per call — a list rebuilt for each of
/// those would have made one render of the Mouse page quadratic in its own length.
pub(super) fn rows() -> &'static [Row] {
    static ROWS: LazyLock<Vec<Row>> = LazyLock::new(build_rows);
    &ROWS
}

fn build_rows() -> Vec<Row> {
    use HotkeyGroup as G;
    // A keyboard slot brings its mouse half along wherever it can carry one, so the page has no
    // list of "which actions also take a click" to keep — the config's own rule decides.
    let key = |group, key: KeySlot| {
        Row::Slot(SlotSpec {
            group,
            editors: match key.mouse_half() {
                Some(half) => Editors::Both(key, half),
                None => Editors::Key(key),
            },
        })
    };
    let mouse = |group, mouse| {
        Row::Slot(SlotSpec {
            group,
            editors: Editors::Mouse(mouse),
        })
    };
    let both = |group, key, mouse| {
        Row::Slot(SlotSpec {
            group,
            editors: Editors::Both(key, mouse),
        })
    };

    let mut rows = Vec::new();
    rows.extend((0..ORDER_SIZE_KEYS).map(|i| key(G::Presets, KeySlot::OrderSize(i))));
    rows.extend((0..SELL_PRESET_KEYS).map(|i| key(G::Presets, KeySlot::SellPreset(i))));
    rows.extend([
        key(G::Trading, KeySlot::CancelBuy),
        key(G::Trading, KeySlot::PanicSell),
        key(G::Trading, KeySlot::PanicSellOne),
        key(G::Trading, KeySlot::CancelAllBuys),
        key(G::Trading, KeySlot::JoinSells),
        key(G::Trading, KeySlot::NewLong),
        key(G::Trading, KeySlot::NewShort),
        key(G::Trading, KeySlot::SplitOrder),
        key(G::Trading, KeySlot::SplitOrderX),
        Row::SplitParts,
        key(G::Trading, KeySlot::SellsToRect),
        key(G::Chart, KeySlot::SwitchCharts),
        key(G::Chart, KeySlot::ScalePlus),
        key(G::Chart, KeySlot::ScaleMinus),
        key(G::Chart, KeySlot::ChartShot),
        key(G::Draw, KeySlot::SwitchFigure),
        key(G::Draw, KeySlot::DrawHline),
        key(G::Draw, KeySlot::DrawHorizontalRay),
        key(G::Draw, KeySlot::DrawSegment),
        key(G::Draw, KeySlot::DrawTriangle),
        key(G::Draw, KeySlot::DrawChannel),
        // The key deletes the selected figure, the gesture the one under the pointer: the one
        // two-editor row whose gesture has a field of its own, from before the table existed.
        both(G::Draw, KeySlot::FigDelete, GestureSlot::FigDelete),
        key(G::Draw, KeySlot::FigAlert),
        key(G::Draw, KeySlot::FigUndo),
        key(G::OrderMove, KeySlot::ShiftBuyUp),
        key(G::OrderMove, KeySlot::ShiftBuyDown),
        key(G::OrderMove, KeySlot::ShiftSellUp),
        key(G::OrderMove, KeySlot::ShiftSellDown),
        mouse(G::Mouse, GestureSlot::BuySet),
        mouse(G::Mouse, GestureSlot::ShortSet),
        mouse(G::Mouse, GestureSlot::PendingLong),
        mouse(G::Mouse, GestureSlot::PendingShort),
        mouse(G::Mouse, GestureSlot::BuyMove),
        mouse(G::Mouse, GestureSlot::SellMove),
        mouse(G::Mouse, GestureSlot::BuyMove2),
        mouse(G::Mouse, GestureSlot::SellMove2),
        Row::SameForMove,
        mouse(G::Mouse, GestureSlot::ShortBuyMove),
        mouse(G::Mouse, GestureSlot::ShortSellMove),
        mouse(G::Mouse, GestureSlot::ShortBuyMove2),
        mouse(G::Mouse, GestureSlot::ShortSellMove2),
    ]);
    rows.extend(
        (0..MANUAL_STRATEGY_KEYS).map(|i| key(G::ManualStrategy, KeySlot::ManualStrategy(i))),
    );
    rows.push(Row::CorePull);
    rows
}

/// Every slot row, in page order.
pub(super) fn slots() -> impl Iterator<Item = SlotSpec> {
    rows().iter().filter_map(|row| match row {
        Row::Slot(spec) => Some(*spec),
        _ => None,
    })
}

/// The page id of a keyboard slot: its full name with hyphens — `cancel-buy`, `order-size-2`.
pub(super) fn key_id(slot: KeySlot) -> String {
    slot.name().replace(['_', '.'], "-")
}

/// The page id of a gesture slot: its stem with hyphens; a key half carries its key's id.
pub(super) fn gesture_id(slot: GestureSlot) -> String {
    match slot {
        GestureSlot::ForKey(key) => key_id(key),
        own => own.stem().replace('_', "-"),
    }
}

/// The title of the row that edits one keyboard slot — `F3`, `S2`, or the localized action.
///
/// Read off the slot rather than off its row, because the two agree by construction (a row titles
/// itself this way) and the callers that need it — the pull preview, a clash caption naming a
/// rival — hold a slot, not a row.
pub(super) fn key_title(slot: KeySlot) -> String {
    match slot {
        KeySlot::OrderSize(i) => format!("F{}", i + 1),
        KeySlot::SellPreset(i) => format!("S{}", i + 1),
        KeySlot::ManualStrategy(i) => t!("hotkeys.manual_strategy", n = i + 1).to_string(),
        named => {
            let key = format!("hotkeys.{}", named.stem());
            t!(&key).to_string()
        }
    }
}

/// The title of the row that edits one gesture slot.
///
/// A gesture that shares its row with a key is titled by that key, so a caption naming the
/// figure-delete gesture names the row the reader will find it on. The lookup is a scan of the
/// registry, which is why a row that KNOWS it has no key goes to [`mouse_only_title`] directly.
pub(super) fn gesture_title(slot: GestureSlot) -> String {
    // A key half names its key in the variant; only a gesture with a field of its own has to be
    // looked up to learn whether it shares a row.
    if let GestureSlot::ForKey(key) = slot {
        return key_title(key);
    }
    let shared_row_key = slots().find_map(|spec| match (spec.key(), spec.mouse()) {
        (Some(key), Some(mouse)) if mouse == slot => Some(key),
        _ => None,
    });
    match shared_row_key {
        Some(key) => key_title(key),
        None => mouse_only_title(slot),
    }
}

/// The title of a gesture that has a row of its own.
fn mouse_only_title(slot: GestureSlot) -> String {
    let key = format!("hotkeys.mouse.{}", slot.stem());
    t!(&key).to_string()
}

#[cfg(test)]
mod tests;
