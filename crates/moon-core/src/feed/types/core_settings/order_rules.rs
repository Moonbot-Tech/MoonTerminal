//! Order rules and mirrored move-gesture settings.

/// The rows of Moonbot's "Основные" page the compact gear popup does not draw.
///
/// A SEPARATE area from [`crate::feed::GeneralSettings`] although it is the same Moonbot page, and the reason is
/// the rule an area exists to serve: a surface may write only what it DREW. The compact popup draws
/// the exits and the blacklist and nothing else, so a mask naming its page must not carry these
/// seven — its OK would stamp them back from a frozen draft over whatever Moonbot changed
/// meanwhile. The expert window draws the whole page and names both areas.
///
/// So the boundary here is which surface draws a row, not what the row means, which is why two of
/// the seven are not order rules at all: the fresh-coin hold sits in Moonbot's risk frame and the
/// startup analysis across the top of the page. It is the one place this module's "an area is a
/// PAGE" model bends, and it bends towards the rule the model is FOR.
#[derive(Debug, Clone)]
pub struct OrderRulesSettings {
    /// `trading.trailing_float`: how much the trailing distance widens per per cent of price move.
    ///
    /// Moonbot's row reads "Добавить к трейлингу +X% за каждый % цены" and the wire calls the field
    /// the "trailing-stop floating percentage". The section's other trailing number,
    /// `trailing_drop`, is the distance itself and is already [`crate::feed::GeneralSettings::trailing_pct`], so
    /// this is the only candidate left for a row that ADDS to it.
    pub trailing_float: f64,
    /// `trading.auto_sell_partial`: per cent of a buy that has to be filled before the sell goes
    /// out. The wire's own default is 100, which it glosses as "wait for the whole fill" — a
    /// boundary Moonbot's caption ("продавать, если куплена часть > X%") words as a strict
    /// inequality. The caption is ported as Moonbot writes it; the wire's gloss is recorded here.
    pub auto_sell_partial: i32,
    /// `trading.auto_cancel_buy_order`: how long an unfilled buy is left standing.
    ///
    /// A COMPOSITE scale, and the one field on this page whose number is not what it reads as: the
    /// wire documents values below 30 as seconds and 30 and above as `value - 29` MINUTES, so 29 is
    /// twenty-nine seconds and 30 is one minute. The page prints the unit; nothing converts the
    /// value, which travels exactly as the core holds it. The scale can therefore express no delay
    /// between 30 and 59 seconds — that is the wire's own gap, not the control's.
    ///
    /// Its neighbour `trading.auto_cancel_lower_buy` is a second auto-cancel, and this is the one
    /// Moonbot's plain "Авто отмена покупки" row means: the neighbour is qualified ("a buy order
    /// placed BELOW the current price") and is counted in plain minutes, while this one carries the
    /// composite scale Moonbot's own control shows.
    pub auto_cancel_buy_order: i32,
    /// `trading.cancel_buy_on_sell_fill`: drop the standing buy once a sell of the same position
    /// fills.
    ///
    /// (Decoding of the field above lives on [`Self::auto_cancel_delay`].)
    pub cancel_buy_on_sell_fill: bool,
    /// `trading.dont_buy_new_coins`: minutes a freshly listed coin is left alone.
    pub dont_buy_new_coins: i32,
    /// `trading.deltas_by_trades`: compute the deltas from the trade stream rather than the book.
    ///
    /// Two halves, like [`crate::feed::GeneralSettings::exclude_blacklisted_from_deltas`]: moonproto applies its
    /// own copy to the terminal's retained analytics without asking the core
    /// (`streams().set_deltas_by_trades`), so committing this field must drive both or the two
    /// disagree until a restart.
    ///
    /// It is also a TAIL field of the `trading` section: a core built before it existed sends a
    /// shorter block and moonproto fills the default, so on such a core TICKING this row makes the
    /// echo unmatchable and the write exhausts its retry budget. Leaving it untouched costs
    /// nothing — the value sent is then the value read.
    ///
    /// The client half is applied locally either way, the same asymmetry
    /// [`crate::feed::GeneralSettings::exclude_blacklisted_from_deltas`] has and for the same reason: it is
    /// issued once the page has gone out, so the two halves cannot diverge the other way round.
    pub deltas_by_trades: bool,
    /// `signals.load_deep_history`: analyse every market's candle history when the core starts.
    ///
    /// The one field of this area outside `trading`. Moonbot draws this switch across the top of
    /// its "Основные" page, above the two columns.
    pub analyze_on_start: bool,
}

/// One row of Moonbot's Move grid — the four the "same hotkeys" switch governs.
///
/// Named rather than addressed by field so the mirror rule below can be written once instead of
/// once per row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveRow {
    /// "Move Order", the primary row.
    OpenPrimary,
    /// "Move TP", the primary row.
    TpPrimary,
    /// "Move Order" under "Дополнительные команды".
    OpenSecondary,
    /// "Move TP" under "Дополнительные команды".
    TpSecondary,
}

impl GestureSettings {
    /// The gesture one Move row actually fires on.
    ///
    /// With `same_hotkeys_for_move` set the short side follows the long one, so the SHORT field is
    /// not what the core acts on and must not be what a surface shows: a core can hold the flag
    /// with a divergent short value, and printing it would state a binding that never fires. The
    /// same resolution `config::HotkeysConfig::move_gestures` performs for the terminal's own copy.
    pub fn move_gesture(&self, row: MoveRow, short: bool) -> u8 {
        match (row, short && !self.same_hotkeys_for_move) {
            (MoveRow::OpenPrimary, false) => self.buy_move_click,
            (MoveRow::OpenPrimary, true) => self.short_buy_move_click,
            (MoveRow::TpPrimary, false) => self.sell_move_click,
            (MoveRow::TpPrimary, true) => self.short_sell_move_click,
            (MoveRow::OpenSecondary, false) => self.buy_move_click_2,
            (MoveRow::OpenSecondary, true) => self.short_buy_move_click_2,
            (MoveRow::TpSecondary, false) => self.sell_move_click_2,
            (MoveRow::TpSecondary, true) => self.short_sell_move_click_2,
        }
    }

    /// Set one Move row's gesture, carrying the mirror the flag demands.
    ///
    /// Writing the long side while `same_hotkeys_for_move` is set writes the short side too, which
    /// is what Moonbot's dialog does and what `moon-ui-gpui`'s own hotkeys settings tab does with
    /// the terminal's copy. It REPAIRS a divergence rather than preventing one: a core can already
    /// hold the flag over stale short values, and only an actual edit rewrites them.
    ///
    /// Writing the SHORT side while that flag is set does nothing, deliberately: [`Self::move_gesture`]
    /// could never return such a value, and a setter whose write its own reader cannot see is a trap
    /// for the next caller. Surfaces disable that control instead — this is the guard behind them.
    pub fn set_move_gesture(&mut self, row: MoveRow, short: bool, value: u8) {
        if short && self.same_hotkeys_for_move {
            return;
        }
        let mirror = !short && self.same_hotkeys_for_move;
        match row {
            MoveRow::OpenPrimary => {
                if short {
                    self.short_buy_move_click = value;
                } else {
                    self.buy_move_click = value;
                }
                if mirror {
                    self.short_buy_move_click = value;
                }
            }
            MoveRow::TpPrimary => {
                if short {
                    self.short_sell_move_click = value;
                } else {
                    self.sell_move_click = value;
                }
                if mirror {
                    self.short_sell_move_click = value;
                }
            }
            MoveRow::OpenSecondary => {
                if short {
                    self.short_buy_move_click_2 = value;
                } else {
                    self.buy_move_click_2 = value;
                }
                if mirror {
                    self.short_buy_move_click_2 = value;
                }
            }
            MoveRow::TpSecondary => {
                if short {
                    self.short_sell_move_click_2 = value;
                } else {
                    self.sell_move_click_2 = value;
                }
                if mirror {
                    self.short_sell_move_click_2 = value;
                }
            }
        }
    }

    /// Turn the "one set for Long and Short" switch, copying the long gestures onto the short ones
    /// when it goes on — the same thing Moonbot's own checkbox does.
    pub fn set_same_hotkeys(&mut self, on: bool) {
        self.same_hotkeys_for_move = on;
        self.normalize();
    }

    /// Restore the block's one cross-field rule: with the mirror on, every short gesture equals
    /// its long twin. A no-op while the mirror is off.
    ///
    /// The rule is kept by [`Self::set_move_gesture`] and [`Self::set_same_hotkeys`] as long as
    /// the block is only ever changed through them; a field-by-field copy from another core
    /// (`CoreChangeSet::overlay`) is not, and calls this afterwards.
    pub fn normalize(&mut self) {
        if self.same_hotkeys_for_move {
            self.short_buy_move_click = self.buy_move_click;
            self.short_sell_move_click = self.sell_move_click;
            self.short_buy_move_click_2 = self.buy_move_click_2;
            self.short_sell_move_click_2 = self.sell_move_click_2;
        }
    }
}

impl OrderRulesSettings {
    /// The auto-cancel delay decoded from its composite scale, as `(amount, in minutes)`.
    ///
    /// Beside the field rather than in the page that prints it: the scale is a property of the wire
    /// value, and a second surface showing this number would otherwise reinvent it.
    pub fn auto_cancel_delay(&self) -> (i32, bool) {
        if self.auto_cancel_buy_order < 30 {
            (self.auto_cancel_buy_order, false)
        } else {
            (self.auto_cancel_buy_order - 29, true)
        }
    }
}

/// Hand-written for the reason [`crate::feed::GeneralSettings`]'s is: `trailing_float` comes off the wire as
/// `f64`.
impl PartialEq for OrderRulesSettings {
    fn eq(&self, other: &Self) -> bool {
        // Destructured for the reason [`crate::feed::GeneralSettings`]'s is.
        let Self {
            trailing_float,
            auto_sell_partial,
            auto_cancel_buy_order,
            cancel_buy_on_sell_fill,
            dont_buy_new_coins,
            deltas_by_trades,
            analyze_on_start,
        } = self;
        trailing_float.total_cmp(&other.trailing_float).is_eq()
            && *auto_sell_partial == other.auto_sell_partial
            && *auto_cancel_buy_order == other.auto_cancel_buy_order
            && *cancel_buy_on_sell_fill == other.cancel_buy_on_sell_fill
            && *dont_buy_new_coins == other.dont_buy_new_coins
            && *deltas_by_trades == other.deltas_by_trades
            && *analyze_on_start == other.analyze_on_start
    }
}

/// Moonbot's Hotkeys page, "Orders Controls" tab: which mouse gesture places, moves and repositions
/// an order, and how a bulk move lays out what it addresses.
///
/// An area is a PAGE — see [`crate::feed::InterfaceSettings`] — but this one covers a BLOCK of its page rather
/// than the whole of it. The rest of the Hotkeys page mirrors [`crate::feed::ManualSettings`], which no OK from
/// a settings surface may write (see `feed::live::shared_config`'s module doc), so those rows stay
/// read-only while these are live.
///
/// Every field is a raw Delphi ordinal, and the terminal already carries both lists: a gesture is
/// `config::MouseGestureBinding::ALL` indexed by the byte — moonproto's own defaults annotate
/// `buy_set_click: 1` as `Dbl_Click` and `sell_move_click: 2` as `CTRL_Click`, which is that list
/// at 1 and 2 — and a move kind is `config::MoveKind::ALL` indexed the same way, against
/// moonproto's `ReplaceMultiKind` (`TReplaceMultiKind`, Vars.pas:37), whose constants run None=0,
/// Shift=1, TopVol=2, LowVol=3, TopProfit=4, All=5, LastSet=6, LastMoved=7.
///
/// The ordinals are kept as bytes rather than decoded here for the reason the rest of this module
/// keeps wire shapes: a core holding a value this build has no name for must survive the round trip
/// untouched, and an enum would have to invent a variant for it or drop it.
///
/// `trading` carries a SECOND, single-order set of the same idea — `order_set_click`,
/// `order_replace_click_buy`, `order_replace_click_sell` — which this block does not write. The
/// evidence that Moonbot's page edits the multi-order set is structural: the page has a long
/// column, a short column and a whole second row of "additional commands", and only
/// `multi_orders` carries short and secondary twins at all. The legacy trio has none, so it cannot
/// be what those columns write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GestureSettings {
    /// `trading.multi_orders.buy_set_click`: places a long at the clicked price.
    pub buy_set_click: u8,
    /// `trading.multi_orders.short_set_click`: places a short at the clicked price.
    pub short_set_click: u8,
    /// `trading.pending_order_set_click`: places a pending long.
    ///
    /// The one field of this block outside the `multi_orders` sub-record, and not by choice:
    /// `multi_orders` carries `pending_short_set_click` and no long counterpart, so Moonbot's own
    /// pair of rows straddles the two records. The asymmetry is the wire's.
    pub pending_order_set_click: u8,
    /// `trading.multi_orders.pending_short_set_click`: places a pending short.
    pub pending_short_set_click: u8,
    /// `trading.multi_orders.same_hotkeys_for_move`: the short columns follow the long ones.
    pub same_hotkeys_for_move: bool,
    /// The primary Move Order row, as `trading.multi_orders.buy_move_click` /
    /// `short_buy_move_click` / `replace_buy_kind`: its long gesture, its short gesture, and which
    /// orders the pair addresses. Read the pair of gestures through [`Self::move_gesture`] rather
    /// than directly — the short one is not always what fires.
    pub buy_move_click: u8,
    /// Short half of the primary Move Order row.
    pub short_buy_move_click: u8,
    /// Which orders the primary Move Order row addresses, and how the core lays them out.
    pub replace_buy_kind: u8,
    /// Long half of the primary Move TP row.
    pub sell_move_click: u8,
    /// Short half of the primary Move TP row.
    pub short_sell_move_click: u8,
    /// Which orders the primary Move TP row addresses.
    pub replace_sell_kind: u8,
    /// Long half of the secondary Move Order row — Moonbot's "Дополнительные команды".
    pub buy_move_click_2: u8,
    /// Short half of the secondary Move Order row.
    pub short_buy_move_click_2: u8,
    /// Which orders the secondary Move Order row addresses.
    pub replace_buy_kind_2: u8,
    /// Long half of the secondary Move TP row.
    pub sell_move_click_2: u8,
    /// Short half of the secondary Move TP row.
    pub short_sell_move_click_2: u8,
    /// Which orders the secondary Move TP row addresses.
    pub replace_sell_kind_2: u8,
}
