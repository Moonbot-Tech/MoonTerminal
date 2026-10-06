//! Manual-trading state types and pure timing and price helpers.

use moon_core::config::GroupTradeSettings;
use moon_core::feed::percentage_stop_price;
use std::time::{Duration, Instant};

/// What the manual-strategy settle pass has already examined for one core: the strategy revision,
/// the client-settings revision, and whether those settings were stale at the time.
///
/// The staleness flag belongs in the key because it clears without moving either revision, so a
/// core examined while stale would never be examined again.
pub(crate) type SettleKey = (u64, u64, bool);

/// Which of the terminal's OWN manual-trading generations (sizes, TP/SL, sell presets) the toolbar
/// is showing.
///
/// Both arms are local config this terminal owns and delivers with the order; neither reads values
/// back out of a core, so neither can be "awaiting" anything. `CoreOwn` is reached when the
/// displayed core keeps its own generation ([`ServerConfig::own_trade_config`]), `GroupLocal`
/// otherwise — including when no chart core resolved at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ManualSource {
    GroupLocal,
    CoreOwn,
}

/// How long a requested `ignore_strat_sell_price` outranks the core's own value.
///
/// Covers the whole slow-channel budget: `SharedConfigSequence` allows three attempts at a
/// ten-second echo timeout each, so a shorter window would snap the checkbox back while the write
/// was still legitimately in flight, and a longer one would keep asserting a value the core has
/// provably refused.
pub(crate) const IGNORE_SELL_LOCAL_TTL: Duration = Duration::from_secs(35);

/// One in-flight request for a core's `ignore_strat_sell_price`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IgnoreSellLocal {
    /// The value the trader asked for.
    pub want: bool,
    /// When it was queued, for the TTL above.
    pub at: Instant,
}

/// Ordinal of the Manual kind in the Moonbot strategy schema; see `strat_kind_name`.
pub(crate) const MANUAL_STRATEGY_KIND: u8 = 12;

/// Strategy field naming the MoonHook strategy a strategy defers its exits to; empty means none.
///
/// Wire text, so it is spelled once: it is both read (the exits a manual order will carry) and
/// written (the picker in the manual-strategy popup).
pub(crate) const FIELD_USE_HOOK_STRATEGY: &str = "UseHookStrategy";

/// Ordinal of the MoonHook kind, which a manual strategy can hand its whole exit set to.
///
/// `UseHookStrategy` names one of these BY NAME, and moonproto builds that field's picklist from
/// exactly this filter (`StrategyDynamicPicklist::HookStrategies`: an empty item, then every local
/// MoonHook strategy), so the terminal offers the same list the core's own editor does.
pub(crate) const HOOK_STRATEGY_KIND: u8 = 20;

/// Minimum spacing between two panic-sell hotkey presses on the same `(core, market)` before the
/// later one is treated as a deliberate reversal rather than an impatient re-jab.
///
/// 500 ms sits above the impatient-burst band (re-jabs run 100-300 ms apart; OS key repeat is
/// already excluded before this point) and at or below the fastest deliberate reversal, which
/// requires reading a changed label and choosing to undo (~500-700 ms).
pub(crate) const PANIC_TOGGLE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Whether a panic-sell hotkey press arriving `now` falls inside the debounce window opened by
/// `last`, and so must be absorbed as a no-op rather than toggling anything.
///
/// Every press restarts the window, absorbed or not: the absorbed press is itself the evidence
/// that the user is still inside the burst. This deliberately diverges from the house pacing idiom
/// of anchoring to the last *accepted* event — those are rate limiters, where dropping is free
/// because the value is idempotent; this is an ambiguity guard, where the suppressed press is the
/// signal that a re-anchor to the last executed press would defeat: a burst of four presses 160 ms
/// apart would otherwise absorb three and then execute the fourth at 500 ms, reproducing the very
/// disarm this guard exists to remove.
///
/// Args:
///     last: Time of the preceding hotkey press for this target.
///     now: Time of the press being considered.
///
/// Returns:
///     `true` when the press falls inside the debounce window.
pub(super) fn panic_press_absorbed(last: Option<Instant>, now: Instant) -> bool {
    last.is_some_and(|last| now.duration_since(last) < PANIC_TOGGLE_DEBOUNCE)
}

/// What a core's own manual-trading generation must hold when its switch is turned ON.
///
/// `Some` seeds it from the group, which is what keeps the numbers on screen from moving at the
/// moment of the flip. `None` leaves an existing generation alone: a core that was switched off and
/// on again must come back to ITS OWN values, not to whatever the group holds now — otherwise the
/// switch would quietly discard the per-core set every time it was toggled.
pub(super) fn seed_on_enable(
    existing: Option<&GroupTradeSettings>,
    group: &GroupTradeSettings,
) -> Option<GroupTradeSettings> {
    existing.is_none().then(|| group.clone())
}

/// Absolute stop price for one order, from the visible percentage.
///
/// A long stops below its entry and a short above it, at the price
/// [`percentage_stop_price`] returns: `entry * (1 - pct/100)` and `entry / (1 - pct/100)`.
/// The toolbar's percentage is signed, so only its magnitude is used. `None` when no usable
/// stop can be computed. A zero percentage is that case too: the helper would answer with the
/// entry itself, and this write must not turn "no stop" into a price at the fill. The order
/// then keeps whatever the core would have applied.
///
/// Args:
///     entry: Price the order is placed at.
///     pct: Visible stop-loss percentage, signed.
///     short: Whether the position is short.
///
/// Returns:
///     The absolute stop price.
pub(super) fn stop_price(entry: f64, pct: f64, short: bool) -> Option<f64> {
    let pct = pct.abs();
    if !(pct.is_finite() && pct > 0.0) {
        return None;
    }
    percentage_stop_price(entry, pct, short)
}

/// Who owns the stop the toolbar is showing, and what it is — see [`Backend::manual_stop`].
///
/// Three states, not two: "the strategy owns it" and "the strategy owns it and this terminal cannot
/// read it" have to be told apart, because the second one must reach the screen as a dash. Folding
/// it into the first would print the saved generation as though it were the strategy's, under a
/// locked control, while the order carried something else entirely.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ManualStop {
    /// The terminal's own value governs: the mode is off, nothing is selected, or the trader turned
    /// Moonbot's rule off for this core.
    Free,
    /// The strategy (or the hook it defers to) owns the stop, and this is what the core will apply.
    Strategy { on: bool, pct: f32 },
    /// The strategy owns the stop and its value cannot be read: the schema has not arrived, or the
    /// hook it names is not in this snapshot.
    Unknown,
}

impl ManualStop {
    /// Whether the stop is the strategy's rather than this terminal's.
    ///
    /// On the enum so the toolbar, the popup guard and the edit absorber all read ONE spelling of
    /// it; three copies of `!matches!(.., Free)` is how they come to disagree.
    pub(crate) fn locked(self) -> bool {
        !matches!(self, Self::Free)
    }

    /// Whether a stop is enabled, given what the caller shows when the terminal still owns it.
    ///
    /// `Unknown` answers `false`: with no readable value there is nothing to enable, and a control
    /// lit from a stop nobody can state is worse than a dark one.
    pub(crate) fn stop_on(self, free: bool) -> bool {
        match self {
            Self::Strategy { on, .. } => on,
            Self::Unknown => false,
            Self::Free => free,
        }
    }
}

/// Exit values shown while a manual strategy owns them, seeded from that strategy.
///
/// Deliberately NOT written to the saved generation: switching charts, switching strategies, or
/// turning MS off must all show what the trader saved for that core or group, not what a strategy
/// left behind.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MsExitOverlay {
    /// Strategy these values were read FROM, which is the selected one or the MoonHook it defers
    /// to (`Backend::exit_source_strategy`).
    ///
    /// Kept so the seed can tell "already seeded" from "seeded off a source that no longer
    /// applies": pointing a manual strategy at a different hook replaces its whole exit set, and
    /// without this the first seed would hold the previous hook's numbers on screen forever.
    pub source: u64,
    /// Whether the stop is enabled (`UseStopLoss` at seed time).
    pub stop_on: bool,
    /// Stop loss as the toolbar states it: signed percent.
    pub stop_pct: f32,
    /// Take profit in percent (`SellPrice` at seed time), or `None` where the strategy has none to
    /// read and the trader's own saved take profit stays in force.
    ///
    /// `None` is the hooked case, and it is not the same as zero: a MoonHook carries NO `SellPrice`
    /// field at all — its sell lives in `HookSellLevel`/`HookSellFixed`, different fields with
    /// different semantics (checked against 290 real MoonHook rows in two core dumps: `SellPrice`
    /// present on none of them, `UseStopLoss`/`StopLoss` on all). Reading its absence as 0% would
    /// send an order with no sell target whatsoever.
    pub take_profit_pct: Option<f64>,
}

/// How long a queued per-order stop waits for its order to appear before it is abandoned.
///
/// Long enough for a placement round trip on a WAN-hosted core, short enough that a stop meant for
/// one order cannot land on an unrelated one placed minutes later.
pub(crate) const PENDING_STOP_TTL: Duration = Duration::from_secs(15);

/// A visible stop waiting for the order it belongs to.
#[derive(Clone, Debug)]
pub(crate) struct PendingStop {
    /// Orders already on the market when the placement was sent; a uid outside this set is a
    /// candidate for the order this stop belongs to.
    pub before_uids: std::collections::HashSet<u64>,
    /// Position side of the order this stop was computed for.
    ///
    /// A PROPERTY of the order rather than another exclusion flag: the uid diff alone cannot name
    /// our own order — moonproto's `client_order_id` is outbound and never echoed
    /// (`docs/trade_actions.md`) — so every new row in the market is a candidate, and the stop is an
    /// absolute price computed for THIS side. A hedged market worked from two windows, or a pending
    /// that triggers inside the TTL, both produce a rival row; matching the side rules out the ones
    /// that were never this stop's order.
    pub short: bool,
    /// The stop to apply, as an absolute price.
    pub form: moon_core::feed::OrderStopsForm,
    /// When the placement was sent, for [`PENDING_STOP_TTL`].
    pub at: Instant,
}
