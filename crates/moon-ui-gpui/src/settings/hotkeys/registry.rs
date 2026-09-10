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
//! A row can carry a key, a gesture, or BOTH — the figure-delete row is the first with two editors,
//! and the shape every other action will take when it gets a mouse half. Its two facts are then
//! the join of its halves' facts.

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
}

impl HotkeyGroup {
    pub(in crate::settings) const ALL: [Self; 7] = [
        Self::Presets,
        Self::Trading,
        Self::Chart,
        Self::Draw,
        Self::OrderMove,
        Self::Mouse,
        Self::ManualStrategy,
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
        }
        .to_string()
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
    /// The origin is the key's when there are two: a test holds the halves to the same answer, so
    /// the day a row gets a key that travels and a gesture that does not, the mark has to be
    /// redesigned rather than quietly answer for one half. The scope is `Scope::join`, which reads
    /// the containment the surfaces document.
    pub fn meta(&self) -> SlotMeta {
        match self.editors {
            Editors::Key(key) => meta::key_slot_meta(key),
            Editors::Mouse(mouse) => meta::gesture_slot_meta(mouse),
            Editors::Both(key, mouse) => {
                let k = meta::key_slot_meta(key);
                let m = meta::gesture_slot_meta(mouse);
                SlotMeta {
                    origin: k.origin,
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
    /// The "pull layout from core" section, closing the manual-strategy page.
    CorePull,
}

impl Row {
    /// The sub-tab this row is on.
    pub fn group(&self) -> HotkeyGroup {
        match self {
            Self::Slot(spec) => spec.group,
            Self::SplitParts => HotkeyGroup::Trading,
            Self::SameForMove => HotkeyGroup::Mouse,
            Self::CorePull => HotkeyGroup::ManualStrategy,
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
    let key = |group, key| {
        Row::Slot(SlotSpec {
            group,
            editors: Editors::Key(key),
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
        key(G::Draw, KeySlot::DrawSegment),
        key(G::Draw, KeySlot::DrawTriangle),
        key(G::Draw, KeySlot::DrawChannel),
        // One action, two editors: the key deletes the selected figure, the gesture the one under
        // the pointer. They were two rows while the gesture was the only mouse slot outside the
        // trading page; §3.4 of the plan is one row per action, and this is its first.
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

/// The page id of a keyboard slot: its stem with hyphens, and the index for a preset.
pub(super) fn key_id(slot: KeySlot) -> String {
    let stem = slot.stem().replace('_', "-");
    match slot.index() {
        Some(i) => format!("{stem}-{i}"),
        None => stem,
    }
}

/// The page id of a gesture slot: its stem with hyphens.
pub(super) fn gesture_id(slot: GestureSlot) -> String {
    slot.stem().replace('_', "-")
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
