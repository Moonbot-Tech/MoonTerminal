//! Caller-owned areas permitted in one shared-configuration edit.

use super::*;

/// Which areas of [`CoreConfig`] one queued edit actually asked to change, set by the CALLER at
/// enqueue time — the one moment the user's intent is known. Never derived by comparison: a
/// send-time diff against the latest retained snapshot picks up a concurrent core-side change as
/// "touched" and writes it back, the exact opposite of what a mask is for.
///
/// `apply_core_config` writes only the areas named here, so two edits queued before either's echo
/// arrives cannot restore each other's fields, and the gear popup's mask — naming only the five
/// rendered sections — cannot reach the manual block at all, checkbox on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldMask {
    /// Moonbot's autobuy page, which reaches into `signals`, its `signal_config` sub-record and two
    /// fields of `trading`. One area because it is one PAGE.
    pub(super) auto_buy: bool,
    pub(super) auto_start: bool,
    pub(super) btc_blink: bool,
    pub(super) general: bool,
    /// The mouse-gesture block of Moonbot's Hotkeys page: `trading.multi_orders` and one field of
    /// `trading` beside it. The rest of that page mirrors the manual block, which no mask reaches.
    pub(super) gestures: bool,
    /// Moonbot's own interface page, which reaches into `trading`, `visual`, `signals` AND `ui`.
    /// One area rather than four because it is one PAGE: what a surface draws is what it may write.
    pub(super) interface: bool,
    /// The rows of Moonbot's General page the COMPACT popup does not draw — the one place the
    /// "an area is a page" rule splits a page in two, because the rule it serves is that a surface
    /// writes only what it drew. See `feed::OrderRulesSettings`.
    pub(super) order_rules: bool,
    pub(super) leverage: bool,
    /// The `signals` section's two price-approach alerts. The FIRST field the terminal writes
    /// outside `trading`/`visual`; the send carries every section either way, so this narrows only
    /// when those six fields are overwritten, exactly as the four above do for theirs.
    pub(super) signals: bool,
    /// Moonbot's "Специальные" page: the engine switches, logging and the screenshot block.
    pub(super) special: bool,
    /// Moonbot's Telegram page: the signal channels, their rules, and the cloud blacklist flag.
    pub(super) telegram: bool,
    /// `trading.ignore_strat_sell_price`, the one manual-block field the terminal still WRITES.
    ///
    /// It is core behaviour, not a value the terminal can hold locally: it decides whether the core
    /// applies a manual strategy's own sell price or the global TP/S the toolbar edits. Everything
    /// else in the manual block is read-only here — see the module doc.
    pub(super) ignore_strat_sell_price: bool,
}

impl FieldMask {
    /// No fields touched. Base value for building a narrow mask with the `with_*` methods below.
    pub const EMPTY: Self = Self {
        auto_buy: false,
        auto_start: false,
        btc_blink: false,
        general: false,
        gestures: false,
        interface: false,
        order_rules: false,
        leverage: false,
        signals: false,
        special: false,
        telegram: false,
        ignore_strat_sell_price: false,
    };

    /// The five sections the COMPACT gear popup renders, and nothing else.
    ///
    /// Not "everything the terminal renders" any more: the expert window builds its own mask out of
    /// the FIELDS its user actually changed (each entry of `feed::CORE_FIELDS` carries its
    /// section's mask), and those reach sections this popup does not draw. Each surface names what
    /// it drew.
    ///
    /// The manual block is deliberately absent from BOTH: an OK may never change a manual-trading
    /// field, checkbox on or off — see `send_core_config`, the one applier they share.
    pub const RENDERED_SECTIONS: Self = Self {
        // NOT the autobuy page: the compact popup does not draw it.
        auto_buy: false,
        auto_start: true,
        btc_blink: true,
        general: true,
        // NOT the Hotkeys page: the compact popup does not draw it.
        gestures: false,
        // NOT the interface page: the compact popup does not draw it. The expert window names it
        // itself, through `with_interface`.
        interface: false,
        // NOT the seven General-page rows below the popup's own: it draws the exits and the
        // blacklist, and naming these would let its OK stamp them back from a frozen draft.
        order_rules: false,
        leverage: true,
        signals: true,
        // NOT the Special page: the compact popup does not draw it.
        special: false,
        // NOT the Telegram page: the compact popup does not draw it.
        telegram: false,
        ignore_strat_sell_price: false,
    };

    /// Whether this mask names the `general` section.
    ///
    /// For a caller that keeps a second, CLIENT-side copy of one of that section's fields: it must
    /// move only when the section itself does, or the two halves drift apart the first time a
    /// surface sends a mask without `general` in it.
    pub const fn writes_general(self) -> bool {
        self.general
    }

    /// Name the `general` section: the exits, the risk limits and the blacklist's delta filter.
    /// The blacklist itself rides the compact channel; see [`super::wire_writers::apply_general`].
    pub const fn with_general(mut self) -> Self {
        self.general = true;
        self
    }

    /// Name the `auto_start` section: what the core turns on, its loss caps and its watchdogs.
    pub const fn with_auto_start(mut self) -> Self {
        self.auto_start = true;
        self
    }

    /// Name the `signals` section's alert sounds.
    pub const fn with_signals(mut self) -> Self {
        self.signals = true;
        self
    }

    /// Name the `btc_blink` section: the BTC-rate highlight and its alarm.
    pub const fn with_btc_blink(mut self) -> Self {
        self.btc_blink = true;
        self
    }

    /// Name Moonbot's "Специальные" page — the engine switches, logging and screenshots.
    pub const fn with_special(mut self) -> Self {
        self.special = true;
        self
    }

    /// Name Moonbot's Telegram page — its signal channels and the rules over them.
    pub const fn with_telegram(mut self) -> Self {
        self.telegram = true;
        self
    }

    /// Name Moonbot's autobuy page — its signal sources and its message filter.
    pub const fn with_auto_buy(mut self) -> Self {
        self.auto_buy = true;
        self
    }

    /// Whether two projections agree on every area this mask names.
    ///
    /// The same comparison the echo path uses to decide "confirmed", "rejected" and "already
    /// satisfied" — shared with the SESSION store, which needs it to tell a retry of one edit from
    /// a different edit. Comparing whole projections there had the defect it had here: the areas an
    /// edit never named are free to drift, and a drift is not a different edit.
    ///
    /// Not a predicate about the mask alone, unlike its `writes_*` neighbours: it takes two
    /// configurations and answers about THEM.
    pub(crate) fn agrees_within(self, a: &CoreConfig, b: &CoreConfig) -> bool {
        rejection_within_mask(a, b, self).is_none()
    }

    /// Whether every area `other` names is also named here.
    ///
    /// A send's mask is the UNION of everything queued, so it NARROWS as entries leave — a batch
    /// whose head is confirmed re-sends the rest under a smaller mask. That is still the same work
    /// in flight, which is why the store asks for containment rather than equality.
    pub(crate) fn contains(self, other: Self) -> bool {
        self.union(other) == self
    }

    /// Whether this mask names the `order_rules` area.
    ///
    /// For the same caller [`Self::writes_general`] serves: `deltas_by_trades` also has a
    /// client-side half, and it must move only when its area does.
    pub const fn writes_order_rules(self) -> bool {
        self.order_rules
    }

    /// Name the General page's rows below the compact popup's own.
    pub const fn with_order_rules(mut self) -> Self {
        self.order_rules = true;
        self
    }

    /// Name the mouse-gesture block of Moonbot's Hotkeys page.
    pub const fn with_gestures(mut self) -> Self {
        self.gestures = true;
        self
    }

    /// Name Moonbot's interface page — its own windows, charts and order-book zones.
    pub const fn with_interface(mut self) -> Self {
        self.interface = true;
        self
    }

    /// Name the core's "ignore a manual strategy's own sell price" flag.
    pub fn with_ignore_strat_sell_price(mut self) -> Self {
        self.ignore_strat_sell_price = true;
        self
    }

    /// Every area either mask names.
    pub(crate) fn union(self, other: Self) -> Self {
        Self {
            auto_buy: self.auto_buy || other.auto_buy,
            auto_start: self.auto_start || other.auto_start,
            btc_blink: self.btc_blink || other.btc_blink,
            general: self.general || other.general,
            gestures: self.gestures || other.gestures,
            interface: self.interface || other.interface,
            order_rules: self.order_rules || other.order_rules,
            leverage: self.leverage || other.leverage,
            signals: self.signals || other.signals,
            special: self.special || other.special,
            telegram: self.telegram || other.telegram,
            ignore_strat_sell_price: self.ignore_strat_sell_price || other.ignore_strat_sell_price,
        }
    }
}
