//! What a hotkey slot IS, beyond the key stored in it: WHERE its binding acts, and whether its
//! value travels between Moonbot and this terminal.
//!
//! Both facts already existed in the tree, scattered and unattached. The surface lives as runtime
//! predicates inside the routers — `Shell::on_hotkey` resolves a target as "the chart under the
//! pointer if it belongs to this window's group, else the window's main chart"
//! (`shell/actions.rs::select_hotkey_target`), `ChartPanel::place_order_at_pos` picks the book strip
//! or the whole pane. The travel side lives as the presence or absence of one mapping arm in
//! `settings::hotkeys::pull`'s action-to-slot map and
//! `moon_core::config::moonbot_import::plan::action_target`.
//! Neither was ever a property of the slot, so the settings page could not say "this one stays
//! yours when you paste a Moonbot config" or "these two both answer a click in the book", and every
//! such question had to be answered by reading the routers again.
//!
//! This module is that attachment and nothing else — a pure lookup with one arm per slot, so the
//! compiler refuses a new slot that does not state both facts. It sits HERE, beside the dispatcher
//! it describes, and not in the settings page that draws the marks: the fact is owned where the
//! behavior is. It could not until the slot types moved down to `moon_core::config` — a table
//! cannot sit below its own key type — which is why it spent its first months inside the page.
//!
//! It computes no conflicts, and it turned out not to be what the conflict captions needed:
//! `settings::hotkeys::clash` answers them from the dispatchers' own ORDER — `hotkeys::DISPATCH` for
//! the keys, the per-button layer lists for the mouse — because the first version, which reasoned about
//! where each binding acts, was backwards on thirty slots out of thirty-eight. So [`Scope`] is a
//! row's label and not a rule, and [`Scope::intersects`] has no caller outside its own tests.

use moon_core::config::{GestureSlot, KeySlot};
use rust_i18n::t;

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
pub struct Scope(u8);

impl Scope {
    /// The whole application, whatever holds the keyboard and wherever the pointer is: the drawing
    /// tools and the sells-zone mode arm state every chart then reads.
    pub const APP: Self = Self(1 << 0);
    /// This window and its group — its main chart, its group's exits and sizes, or its active
    /// trading core. Never another window's, and never decided by the pointer.
    pub const WINDOW: Self = Self(1 << 1);
    /// The chart the pointer rests on: `shell/actions.rs::select_hotkey_target` prefers it over the
    /// window's own chart whenever it belongs to the same group, and `hotkeys::pre_dispatch` uses
    /// nothing else. A key carries no position, so these read the cursor instead.
    pub const CURSOR: Self = Self(1 << 2);
    /// The selected figure, wherever it was drawn (`Backend::fig_selected`).
    pub const SELECTION: Self = Self(1 << 3);
    /// The order-book strip — the trading surface while "separate control zones" is on.
    pub const BOOK: Self = Self(1 << 4);
    /// The chart's price field, which is the trading surface while separate zones are off.
    pub const PLOT: Self = Self(1 << 5);
    /// A figure under the pointer, within the hit threshold.
    pub const FIGURE: Self = Self(1 << 6);

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
    pub const fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The set with these surfaces taken out.
    const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// The surfaces of a row that carries two slots — the union, read through the containment the
    /// constants document.
    ///
    /// [`Self::or`] is the raw union and the right thing for ONE slot's two alternatives (`BOOK`
    /// or `PLOT`, whichever the zones setting picks). A row that joins a key acting on the cursor
    /// with a gesture acting on the figure under it is a different case: "cursor / figure" names
    /// the same place twice, in a column with no room to. So a set that already says `CURSOR` drops
    /// the places the pointer could be, and a set that says `APP` says only that.
    pub fn join(self, other: Self) -> Self {
        let both = self.or(other);
        if both.intersects(Self::APP) {
            Self::APP
        } else if both.intersects(Self::CURSOR) {
            both.without(Self::BOOK.or(Self::PLOT).or(Self::FIGURE))
        } else {
            both
        }
    }

    /// Whether the two sets share a surface — the question a conflict caption turns on: two
    /// bindings that never meet on the same surface are not in each other's way at all.
    pub fn intersects(self, other: Self) -> bool {
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
    pub fn resolved(self, separate_zones: bool) -> Self {
        if separate_zones && self.intersects(Self::BOOK) && self.intersects(Self::PLOT) {
            return self.without(Self::PLOT);
        }
        self
    }

    /// Localized name of the surface, or of every surface in the set, for the row's mark.
    pub fn label(self) -> String {
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
/// import writes this field" has exactly one, and `settings::hotkeys::pull` and
/// `moon_core::config::moonbot_import` are where it is written down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// The value crosses over: pasting a Moonbot configuration writes this slot
    /// (`moonbot_import::plan::action_target`), or "pull layout from core" reconciles it against
    /// the core's own layout (`settings::hotkeys::pull` for the keys,
    /// `settings::hotkeys::pull_gestures` for the twelve order gestures), or both.
    Shared,
    /// Nothing writes it. A Moonbot paste and a core pull both leave this row exactly as it was.
    Local,
}

impl Origin {
    /// The origin of a row that carries two slots: shared as soon as either half is — a row
    /// whose key a paste overwrites is a row a paste overwrites, whatever its click half does.
    pub fn join(self, other: Self) -> Self {
        if self == Self::Shared || other == Self::Shared {
            Self::Shared
        } else {
            Self::Local
        }
    }
}

/// One slot's two facts, as the settings row shows them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SlotMeta {
    pub origin: Origin,
    pub scope: Scope,
}

const fn meta(origin: Origin, scope: Scope) -> SlotMeta {
    SlotMeta { origin, scope }
}

/// The mirror switch's facts. Not a slot either, and the widest blast radius on the page: a pull
/// writes it, and it decides whether the four short rows follow the long ones at all.
pub const SAME_FOR_MOVE: SlotMeta = meta(Origin::Shared, Scope::BOOK.or(Scope::PLOT));

/// The two facts about one keyboard slot.
///
/// Surfaces are read off the routers, not chosen here. The one that decides most rows is
/// `shell/actions.rs::select_hotkey_target`: an action that takes a target acts on the chart under
/// the POINTER when that chart is in this window's group, and on the window's main chart otherwise
/// — so it carries both surfaces, and a detached window (`chart_tabs::detached_host`, which uses its
/// own chart and no hover) is the case where only the second one is left.
pub fn key_slot_meta(slot: KeySlot) -> SlotMeta {
    use KeySlot as S;
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
        S::DrawHline | S::DrawHorizontalRay | S::DrawSegment | S::DrawTriangle | S::DrawChannel => {
            meta(Local, Scope::APP)
        }
        // Deletes the selected figure — and with NOTHING selected the same press falls through to
        // cancelling the order under the pointer (`shell/actions.rs`), which is the second surface
        // and the destructive one.
        S::FigDelete => meta(Local, Scope::SELECTION.or(Scope::CURSOR)),
        S::FigAlert => meta(Local, Scope::SELECTION),
    }
}

/// The two facts about one gesture slot.
///
/// The twelve trading gestures travel: the core carries them in `feed::GestureSettings` and
/// `settings::hotkeys::pull_gestures` reads them into this terminal's one layout, so a pull
/// overwrites these rows exactly as it overwrites a key. Their surface is the trading surface: the
/// book strip under "separate control zones" and the whole pane without it.
///
/// A key half acts wherever its key acts — it performs the same action through the same dispatch —
/// and nothing writes it: Moonbot has no gesture for these, so a paste and a pull leave the table
/// alone even where they overwrite the key beside it.
pub fn gesture_slot_meta(slot: GestureSlot) -> SlotMeta {
    use GestureSlot as S;
    match slot {
        S::ForKey(key) => meta(Origin::Local, key_slot_meta(key).scope),
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
