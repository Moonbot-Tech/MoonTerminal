//! What a hotkey slot IS, beyond the key stored in it: WHERE its binding acts, and whether its
//! value travels between Moonbot and this terminal.
//!
//! Both facts already existed in the tree, scattered and unattached. The surface lives as runtime
//! predicates inside the routers — `Shell::on_hotkey` resolves a target as "the chart under the
//! pointer if it belongs to this window's group, else the window's main chart"
//! (`shell/actions.rs::select_hotkey_target`), `ChartPanel::place_order_at_pos` picks the book strip
//! or the whole pane. The travel side lives as the presence or absence of one mapping arm in
//! `super::pull`'s action-to-slot map and
//! `moon_core::config::moonbot_import::plan::action_target`.
//! Neither was ever a property of the slot, so the settings page could not say "this one stays
//! yours when you paste a Moonbot config" or "these two both answer a click in the book", and every
//! such question had to be answered by reading the routers again.
//!
//! This module is that attachment and nothing else — a pure lookup with one arm per slot, so the
//! compiler refuses a new slot that does not state both facts. It computes no conflicts yet; the
//! yellow/red captions planned on top of it need exactly the overlap [`Scope::intersects`] answers,
//! and when they are written [`Scope`] should move down beside the routers it describes
//! (`crate::hotkeys`) so the rule can refine `HotkeysConfig::bound_keys` rather than disagree with
//! it.

use rust_i18n::t;

use super::{HotkeySlot, MouseSlot};

/// The surfaces a binding acts on, as a SET rather than one value.
///
/// A set because a slot's surface is genuinely plural. Two reasons, both from the routers: a
/// keyboard action that reads a target gets the chart under the POINTER when that chart belongs to
/// the window's group and the window's own chart otherwise, so both are true of it; and the trading
/// gestures read the order-book strip when "separate control zones" is on and the whole chart pane
/// when it is off. An overlap test that saw only one half would call a real collision safe.
///
/// [`Self::BOOK`], [`Self::PLOT`] and [`Self::FIGURE`] are narrower forms of [`Self::CURSOR`] —
/// each names a place the pointer has to be — and [`Self::APP`] contains everything. The conflict
/// rule that comes next needs that containment; [`Self::intersects`] alone does not encode it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Scope(u8);

impl Scope {
    /// The whole application, whatever holds the keyboard and wherever the pointer is: the drawing
    /// tools and the sells-zone mode arm state every chart then reads.
    pub(super) const APP: Self = Self(1 << 0);
    /// This window and its group — its main chart, its group's exits and sizes, or its active
    /// trading core. Never another window's, and never decided by the pointer.
    pub(super) const WINDOW: Self = Self(1 << 1);
    /// The chart the pointer rests on: `shell/actions.rs::select_hotkey_target` prefers it over the
    /// window's own chart whenever it belongs to the same group, and `hotkeys::pre_dispatch` uses
    /// nothing else. A key carries no position, so these read the cursor instead.
    pub(super) const CURSOR: Self = Self(1 << 2);
    /// The selected figure, wherever it was drawn (`Backend::fig_selected`).
    pub(super) const SELECTION: Self = Self(1 << 3);
    /// The order-book strip — the trading surface while "separate control zones" is on.
    pub(super) const BOOK: Self = Self(1 << 4);
    /// The chart's price field, which is the trading surface while separate zones are off.
    pub(super) const PLOT: Self = Self(1 << 5);
    /// A figure under the pointer, within the hit threshold.
    pub(super) const FIGURE: Self = Self(1 << 6);

    /// Every surface in display order, with the locale key naming it.
    const NAMED: [(Self, &'static str); 7] = [
        (Self::APP, "hotkeys.scope.app"),
        (Self::WINDOW, "hotkeys.scope.window"),
        (Self::CURSOR, "hotkeys.scope.cursor"),
        (Self::SELECTION, "hotkeys.scope.selection"),
        (Self::BOOK, "hotkeys.scope.book"),
        (Self::PLOT, "hotkeys.scope.plot"),
        (Self::FIGURE, "hotkeys.scope.figure"),
    ];

    /// Both surfaces of one slot, for the slots that have two.
    const fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether the two sets share a surface — the question a conflict caption turns on: two
    /// bindings that never meet on the same surface are not in each other's way at all.
    pub(super) fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Narrow the pair that "separate control zones" decides, but only when one answer is true of
    /// EVERY chart.
    ///
    /// The setting is not the whole rule: `ChartPanel::separate_zones` returns true unconditionally
    /// for a numbered AddToChart or Custom panel, which always keeps its book on the right. So with
    /// the setting ON the book is the trading surface everywhere and the row can say so; with it OFF
    /// a Main chart trades from the whole pane while those panels still trade from the book, and
    /// naming one of them would be a lie about the other.
    ///
    /// The SET is what the model keeps regardless — the setting can be flipped, and a conflict rule
    /// has to see both halves. Every other set is left alone.
    pub(super) fn resolved(self, separate_zones: bool) -> Self {
        if separate_zones && self.intersects(Self::BOOK) && self.intersects(Self::PLOT) {
            return Self(self.0 & !Self::PLOT.0);
        }
        self
    }

    /// Localized name of the surface, or of every surface in the set, for the row's mark.
    pub(super) fn label(self) -> String {
        Self::NAMED
            .into_iter()
            .filter(|(flag, _)| self.intersects(*flag))
            .map(|(_, key)| t!(key).to_string())
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

/// Whether this row's value travels between Moonbot and here — the fact a user needs BEFORE pasting
/// a Moonbot configuration over their own.
///
/// Deliberately about the VALUE and not about the action. "Moonbot can also draw a channel" does
/// not help anyone decide what a paste will overwrite, and it has no checkable answer; "the config
/// import writes this field" has exactly one, and [`super::pull`] and
/// `moon_core::config::moonbot_import` are where it is written down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Origin {
    /// The value crosses over: pasting a Moonbot configuration writes this slot
    /// (`moonbot_import::plan::action_target`), or "pull layout from core" reconciles it against
    /// the core's own layout (`super::pull` for the keys, `super::pull_gestures` for the twelve
    /// order gestures), or both.
    Shared,
    /// Nothing writes it. A Moonbot paste and a core pull both leave this row exactly as it was.
    Local,
}

impl Origin {
    /// Short mark shown on the row.
    pub(super) fn label(self) -> String {
        match self {
            Self::Shared => t!("hotkeys.origin.shared"),
            Self::Local => t!("hotkeys.origin.local"),
        }
        .to_string()
    }
}

/// One slot's two facts, as the settings row shows them.
#[derive(Clone, Copy)]
pub(super) struct SlotMeta {
    pub origin: Origin,
    pub scope: Scope,
}

const fn meta(origin: Origin, scope: Scope) -> SlotMeta {
    SlotMeta { origin, scope }
}

/// The mirror switch's facts. Not a slot either, and the widest blast radius on the page: a pull
/// writes it, and it decides whether the four short rows follow the long ones at all.
pub(super) const SAME_FOR_MOVE: SlotMeta = meta(Origin::Shared, Scope::BOOK.or(Scope::PLOT));

/// The part-count row's facts. Not a [`HotkeySlot`] — it stores a number, not a key — but a Moonbot
/// paste rewrites it (`Hotkeys.SplitParts`), so it belongs in this table rather than stated inline
/// in the renderer where nothing checks it.
pub(super) const SPLIT_PARTS: SlotMeta = meta(Origin::Shared, Scope::WINDOW);

/// The two facts about one keyboard slot.
///
/// Surfaces are read off the routers, not chosen here. The one that decides most rows is
/// `shell/actions.rs::select_hotkey_target`: an action that takes a target acts on the chart under
/// the POINTER when that chart is in this window's group, and on the window's main chart otherwise
/// — so it carries both surfaces, and a detached window (`chart_tabs::detached_host`, which uses its
/// own chart and no hover) is the case where only the second one is left.
pub(super) fn key_slot_meta(slot: HotkeySlot) -> SlotMeta {
    use HotkeySlot as S;
    use Origin::*;
    // Every action that reads `hotkeys::apply`'s `target`, and therefore follows the pointer first.
    const AIMED: Scope = Scope::CURSOR.or(Scope::WINDOW);
    match slot {
        // Presets act on the window GROUP (sizes, exits) or on its active trading core (manual
        // strategy), never on a pointed-at chart. `OKeys` and `SKeys` travel both ways; the manual
        // strategy keys travel on the core pull alone (`ManualSettings::strat_buttons`).
        S::OrderSize(_) | S::SellPreset(_) | S::ManualStrategy(_) => meta(Shared, Scope::WINDOW),
        // Trading actions that address one market: the pointer picks it, the window's chart is the
        // fallback.
        S::CancelBuy
        | S::PanicSell
        | S::PanicSellOne
        | S::JoinSells
        | S::ShiftBuyUp
        | S::ShiftBuyDown
        | S::ShiftSellUp
        | S::ShiftSellDown => meta(Shared, AIMED),
        // Both split slots resolve to the SAME `HotkeyAction::SplitOrder` (`hotkeys.rs`), which
        // `pre_dispatch` offers to the hovered ORDER before any market-level split.
        S::SplitOrder | S::SplitOrderX => meta(Shared, AIMED),
        // Placed at the pointer's price, and refused unless the pointer is inside the trading
        // surface — the book strip with separate control zones on, the whole pane with it off
        // (`ChartPanel::place_order_at_pos`).
        S::NewLong | S::NewShort => meta(Shared, Scope::BOOK.or(Scope::PLOT)),
        // Addresses the core, not a chart: every market of the window's active core at once.
        S::CancelAllBuys => meta(Shared, Scope::WINDOW),
        // Group-owned: the group's charts and the group's price scale.
        S::SwitchCharts | S::ScalePlus | S::ScaleMinus => meta(Shared, Scope::WINDOW),
        // Shoots the last chart the pointer visited IN THIS WINDOW (`panels::chart::shot`), so it is
        // window-bounded and pointer-chosen at once.
        S::ChartShot => meta(Shared, AIMED),
        // Arms a tool in the application-wide figure state; the core reconciles the key.
        S::SwitchFigure => meta(Shared, Scope::APP),
        // The core's own slot for this ordinal is "Fit sells to orderbook", a different action from
        // the price-band spread this performs, which is why the pull deliberately drops it
        // (`moonbot_import::plan::action_target`). Nothing writes this row.
        S::SellsToRect => meta(Local, Scope::APP),
        // Moonbot's own Ctrl+Z, hardwired there rather than configurable, so no slot carries it.
        // Addressed here by the pointer alone (`hotkeys::pre_dispatch`).
        S::FigUndo => meta(Local, Scope::CURSOR),
        // The drawing layer is the Terminal's: no core slot, no imported field.
        S::DrawHline | S::DrawSegment | S::DrawTriangle | S::DrawChannel => meta(Local, Scope::APP),
        // Deletes the selected figure — and with NOTHING selected the same press falls through to
        // cancelling the order under the pointer (`shell/actions.rs`), which is the second surface
        // and the destructive one.
        S::FigDelete => meta(Local, Scope::SELECTION.or(Scope::CURSOR)),
        S::FigAlert => meta(Local, Scope::SELECTION),
    }
}

/// The two facts about one mouse slot.
///
/// The twelve trading gestures travel: the core carries them in `feed::GestureSettings` and
/// [`super::pull_gestures`] reads them into this terminal's one layout, so a pull overwrites these
/// rows exactly as it overwrites a key. Their surface is the trading surface: the book strip under
/// "separate control zones" and the whole pane without it.
pub(super) fn mouse_slot_meta(slot: MouseSlot) -> SlotMeta {
    use MouseSlot as S;
    match slot {
        S::BuySet
        | S::ShortSet
        | S::PendingLong
        | S::PendingShort
        | S::BuyMove
        | S::SellMove
        | S::BuyMove2
        | S::SellMove2
        | S::ShortBuyMove
        | S::ShortSellMove
        | S::ShortBuyMove2
        | S::ShortSellMove2 => meta(Origin::Shared, Scope::BOOK.or(Scope::PLOT)),
        // The one gesture with no counterpart anywhere: `feed::GestureSettings` carries the twelve
        // order gestures and no figure gesture at all, so there is nothing to import.
        S::FigDelete => meta(Origin::Local, Scope::FIGURE),
    }
}

#[cfg(test)]
mod tests;
