//! Manual-trading settings, order terms, and core command helpers for [`Backend`].

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use moon_core::config::{
    DEFAULT_ORDER_SIZES_USD, GroupExitSettings, GroupTradeSettings, MANUAL_STRAT_SLOTS,
    ManualStratState, StratSlot, TakeProfitMode,
};
use moon_core::feed::{ClientSettingsEdit, FieldMask, StrategyRow};
use moon_core::market::MarketQuantityUnit;
use moon_core::session::CoreId;

use crate::Backend;

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

/// How long a fresh `PanicLocal` override outranks the core snapshot.
///
/// The override's only job is bridging one core round trip. 3 s is >= 3x the slowest in-app data
/// cadence (the 1000 ms background-panel floor) and covers a WAN round trip to a VPS-hosted core
/// plus one order-publish tick. Matches the in-repo `stop_overlay` TTL constant and now its
/// lifecycle too: on expiry we prefer the core's truth over our optimistic guess, which is
/// correct on the money path where the core is the authority.
pub(crate) const PANIC_LOCAL_TTL: Duration = Duration::from_secs(3);

/// Minimum spacing between two panic-sell hotkey presses on the same `(core, market)` before the
/// later one is treated as a deliberate reversal rather than an impatient re-jab.
///
/// 500 ms sits above the impatient-burst band (re-jabs run 100-300 ms apart; OS key repeat is
/// already excluded before this point) and at or below the fastest deliberate reversal, which
/// requires reading a changed label and choosing to undo (~500-700 ms).
pub(crate) const PANIC_TOGGLE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Optimistic Panic Sell override for one `(core, market)`.
///
/// It records both arm and disarm requests. The reconciliation tick drops it when the core agrees
/// or its TTL expires, returning authority to the retained core snapshot.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PanicLocal {
    /// The armed state this override asserts, pending core confirmation.
    pub want: bool,
    /// When this override was recorded, for TTL and settle comparisons.
    pub at: Instant,
}

/// Resolve the effective armed state from an optional fresh local override and the core snapshot.
///
/// `local` carries `(want, age)` when a `PanicLocal` exists. While `age < PANIC_LOCAL_TTL` the
/// override outranks the snapshot in both directions (arm and disarm); once stale, or absent, the
/// snapshot is authoritative. `snapshot_armed` is supplied LAZILY and is not evaluated at all while
/// a fresh override decides the answer: the caller is on the chart render path, and the snapshot
/// walk is `order_lines.iter_market`, so skipping it on the common post-press path matters.
///
/// Args:
///     local: Requested state and age for the optional local override.
///     snapshot_armed: Deferred lookup of the retained core state.
///
/// Returns:
///     The fresh local state when available, otherwise the retained core state.
fn effective_panic_armed(
    local: Option<(bool, Duration)>,
    snapshot_armed: impl FnOnce() -> bool,
) -> bool {
    match local {
        Some((want, age)) if age < PANIC_LOCAL_TTL => want,
        _ => snapshot_armed(),
    }
}

/// Whether a `PanicLocal` override has settled and may be dropped by the reconciliation tick.
///
/// Settled once the TTL has elapsed (the override can no longer influence `effective_panic_armed`)
/// or the moment the core snapshot agrees with what the override asserts -- dropping it as soon as
/// the core agrees, rather than only on the user's next press, is what stops a transient agreement
/// from being forgotten and turning an intended re-arm into a disarm.
///
/// Args:
///     want: Armed state asserted by the local override.
///     age: Time since the override was accepted.
///     snapshot_armed: Current state from the retained core snapshot.
///
/// Returns:
///     `true` when the override cannot change the effective state any longer.
fn panic_local_settled(want: bool, age: Duration, snapshot_armed: bool) -> bool {
    age >= PANIC_LOCAL_TTL || snapshot_armed == want
}

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
fn panic_press_absorbed(last: Option<Instant>, now: Instant) -> bool {
    last.is_some_and(|last| now.duration_since(last) < PANIC_TOGGLE_DEBOUNCE)
}

/// What a core's own manual-trading generation must hold when its switch is turned ON.
///
/// `Some` seeds it from the group, which is what keeps the numbers on screen from moving at the
/// moment of the flip. `None` leaves an existing generation alone: a core that was switched off and
/// on again must come back to ITS OWN values, not to whatever the group holds now — otherwise the
/// switch would quietly discard the per-core set every time it was toggled.
fn seed_on_enable(
    existing: Option<&GroupTradeSettings>,
    group: &GroupTradeSettings,
) -> Option<GroupTradeSettings> {
    existing.is_none().then(|| group.clone())
}

/// Absolute stop price for one order, from the visible percentage.
///
/// A long stops BELOW its entry and a short ABOVE it; the toolbar's percentage is signed, so only
/// its magnitude is used. `None` when no usable stop can be computed, which leaves the order on
/// whatever the core would have applied.
///
/// Args:
///     entry: Price the order is placed at.
///     pct: Visible stop-loss percentage, signed.
///     short: Whether the position is short.
///
/// Returns:
///     The absolute stop price.
fn stop_price(entry: f64, pct: f64, short: bool) -> Option<f64> {
    let pct = pct.abs();
    if !(entry.is_finite() && entry > 0.0 && pct.is_finite() && pct > 0.0 && pct < 100.0) {
        return None;
    }
    let level = if short {
        entry * (1.0 + pct / 100.0)
    } else {
        entry * (1.0 - pct / 100.0)
    };
    (level.is_finite() && level > 0.0).then_some(level)
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
    /// Orders already on the market when the placement was sent; the first uid outside this set is
    /// the order this stop belongs to.
    pub before_uids: std::collections::HashSet<u64>,
    /// The stop to apply, as an absolute price.
    pub form: moon_core::feed::OrderStopsForm,
    /// When the placement was sent, for [`PENDING_STOP_TTL`].
    pub at: Instant,
}

/// Resolve one strategy field from the snapshot, then from its kind's schema default.
pub(crate) fn strat_field_value(
    row: &StrategyRow,
    schema: Option<&moon_core::feed::StrategySchemaModel>,
    name: &str,
) -> Option<String> {
    if let Some((_, value)) = row.fields.iter().find(|(field, _)| field == name) {
        return Some(value.clone());
    }
    schema?
        .kinds
        .iter()
        .find(|kind| kind.ordinal == row.kind_ordinal)?
        .sections
        .iter()
        .flat_map(|section| section.fields.iter())
        .find(|field| field.name == name)?
        .default
        .clone()
}

/// The sell target one manual order carries, as a PRICE, or `None` when the order must not carry
/// one.
///
/// Moonbot's own model, and the reason this is computed per order rather than written anywhere:
/// the trader's TP or engaged S preset is a property of the ORDER (`planned_sell_price` on the wire
/// and on the retained row), not of the strategy — clicking a preset changes no strategy file, as
/// the core's own screen shows. A short sells BELOW its entry, so its target mirrors.
///
/// `None` while no percentage is set: zero would ask the core to sell at the entry price.
///
/// Args:
///     entry: Price the order is placed at.
///     pct: Effective take profit, in percent.
///     short: Whether the position is short.
///
/// Returns:
///     The absolute price to store with the order.
fn planned_sell_price(entry: f64, pct: f64, short: bool) -> Option<f64> {
    if !(entry.is_finite() && entry > 0.0 && pct.is_finite() && pct > 0.0) {
        return None;
    }
    let target = if short {
        entry * (1.0 - pct / 100.0)
    } else {
        entry * (1.0 + pct / 100.0)
    };
    (target.is_finite() && target > 0.0).then_some(target)
}

/// Resolve the sell-price flag to show: a fresh request the core has not answered yet, or the
/// core's own value.
///
/// Settles the moment the core AGREES rather than only on the TTL, exactly like
/// [`panic_local_settled`]: an override still asserting a value the core already holds would make
/// the next click — which asks for the opposite — look like the no-op this override exists to
/// prevent.
///
/// Args:
///     local: The requested value and how long ago it was queued, when one is in flight.
///     core_value: What the core's own configuration currently says.
///
/// Returns:
///     The value the checkbox must render.
fn effective_ignore_sell(local: Option<(bool, Duration)>, core_value: bool) -> bool {
    match local {
        Some((want, age)) if want != core_value && age < IGNORE_SELL_LOCAL_TTL => want,
        _ => core_value,
    }
}

/// Apply one visible toolbar edit with the same wire quantization used by MoonProto.
fn apply_group_exit_edit(exit: &mut GroupExitSettings, edit: ClientSettingsEdit) -> bool {
    match edit {
        ClientSettingsEdit::TakeProfit { pct, extended } => {
            let mode = if extended {
                TakeProfitMode::Extended
            } else {
                TakeProfitMode::Normal
            };
            let Some(pct) = mode.canonical_take_profit_pct(pct) else {
                return false;
            };
            exit.take_profit_mode = mode;
            exit.take_profit_pct = pct;
            for fixed_pct in &mut exit.fixed_sell_pcts {
                *fixed_pct = exit
                    .take_profit_mode
                    .canonical_fixed_sell_pct(*fixed_pct)
                    .unwrap_or_default();
            }
            exit.fixed_sell_slot = None;
        }
        ClientSettingsEdit::ScalpTakeProfit(pct) => {
            let Some(pct) = TakeProfitMode::Scalp.canonical_take_profit_pct(pct) else {
                return false;
            };
            exit.take_profit_mode = TakeProfitMode::Scalp;
            exit.take_profit_pct = pct;
            for fixed_pct in &mut exit.fixed_sell_pcts {
                *fixed_pct = exit
                    .take_profit_mode
                    .canonical_fixed_sell_pct(*fixed_pct)
                    .unwrap_or_default();
            }
            exit.fixed_sell_slot = None;
        }
        ClientSettingsEdit::StopLossPct(pct) => {
            let Some(pct) = GroupExitSettings::canonical_stop_loss_pct(pct) else {
                return false;
            };
            exit.stop_loss_pct = pct;
        }
        ClientSettingsEdit::SelectFixedSellSlot(slot) if (1..=6).contains(&slot) => {
            exit.fixed_sell_slot = Some(slot);
        }
        ClientSettingsEdit::EngageMainTakeProfit => exit.fixed_sell_slot = None,
        ClientSettingsEdit::SetFixedSellPct { slot, pct } if (1..=6).contains(&slot) => {
            let Some(pct) = exit.take_profit_mode.canonical_fixed_sell_pct(pct) else {
                return false;
            };
            exit.fixed_sell_pcts[slot - 1] = pct;
        }
        ClientSettingsEdit::UseStopMarket(on) => exit.use_stop_market = on,
        ClientSettingsEdit::PanicIfPriceDrop(on) => exit.stop_loss_enabled = on,
        _ => return false,
    }
    true
}

/// Mirror one live toolbar mutation into an open Settings preview without replacing draft fields.
fn update_group_trade_pair(
    live: &mut GroupTradeSettings,
    preview: Option<&mut GroupTradeSettings>,
    update: impl Fn(&mut GroupTradeSettings),
) {
    update(live);
    if let Some(preview) = preview {
        update(preview);
    }
}

/// Convert a positive USD equivalent to base quantity, rejecting unavailable or invalid rates.
fn usd_to_base_amount(usd: f64, rate: Option<f64>) -> Option<f64> {
    let rate = rate?;
    if !(usd.is_finite() && usd > 0.0 && rate.is_finite() && rate > 0.0) {
        return None;
    }
    let size = usd / rate;
    (size.is_finite() && size > 0.0).then_some(size)
}

/// Group-owned terms resolved before one manual order is submitted to a core.
pub(crate) struct ManualOrderTerms {
    /// Sell target to store with the order, when the terminal's own exit controls are the ones
    /// that apply. `None` leaves the field zero, which is what the core reads as "no target".
    pub(crate) planned_sell: Option<f64>,
    /// Whether the order must wait for the core to confirm the visible exit generation.
    ///
    /// `false` with a manual strategy selected: the core reads its sell from the strategy or from
    /// `planned_sell`, and its stop is applied to the order itself, so waiting for those settings
    /// only delays the order by a retry budget of round trips.
    pub(crate) sync_exit: bool,
    /// Quantity sent to the target core, in THAT MARKET's own unit: a coin amount on a linear or
    /// spot market, a contract count on a coin-settled one. See `manual_order_size_base`.
    pub(crate) size_base: f64,
    /// Visible USD equivalent, absent when an isolated FireTest overrides the base size.
    pub(crate) size_usd: Option<f64>,
    /// Complete visible exit generation serialized before the order.
    pub(crate) exit: GroupExitSettings,
    /// Manual strategy this order is placed on, sent as an explicit `StratID`.
    ///
    /// Explicit rather than `None`: a zero `StratID` asks the CORE to substitute whatever its own
    /// `use_manual_strategy` currently names, which makes the order depend on a switch this
    /// terminal does not own and another client can move. Naming the strategy makes the order say
    /// what it is, and leaves Moonbot's own screen alone.
    pub(crate) strategy_id: Option<u64>,
}

/// Whether the per-order stop write would say exactly what the core is going to do anyway.
///
/// True only when the strategy's own stop and the visible one agree, which is the case the write
/// exists to avoid: an identical second packet costs a round trip and makes the stop line jump from
/// the strategy's level to the same level again.
///
/// The strategy side is compared UNCLAMPED against a visible value that was stored through
/// `canonical_stop_loss_pct`. That asymmetry is the point: for a strategy stop outside the protocol
/// range the two must NOT agree, because the screen shows the clamped value while the core would
/// apply the raw one, and this write is the only thing that closes that gap. For an in-range
/// strategy the clamp is the identity and they compare equal as expected.
///
/// Args:
///     strategy: The strategy's own `UseStopLoss` and `StopLoss` (a positive distance).
///     visible: What the trader sees, as `(enabled, signed percent)`.
///
/// Returns:
///     Whether the per-order write can be skipped.
fn stop_write_is_redundant(strategy: (bool, f64), visible: (bool, f32)) -> bool {
    let (strategy_on, strategy_pct) = strategy;
    let (visible_on, visible_pct) = visible;
    // With both sides disabled there is no percentage on screen to differ.
    strategy_on == visible_on && (!visible_on || signed_stop_pct(Some(strategy_pct)) == visible_pct)
}

/// Resolve which strategy in one snapshot supplies the exits for `strategy_id`.
///
/// `hook` is that strategy's `UseHookStrategy`, already read and trimmed. Empty means it keeps its
/// own exits, so it is its own source. A named hook resolves ONLY against MoonHook-kind rows: the
/// field is a picklist over that kind, and a Manual strategy that happens to share the name is a
/// different strategy with different exits — resolving to it would put numbers on screen that no
/// order uses.
///
/// `None` means a hook is named and this snapshot does not have it, which is the one case the
/// caller must not guess at.
///
/// Args:
///     strategies: The core's retained strategy snapshot.
///     hook: `UseHookStrategy` of the selected strategy, trimmed.
///     strategy_id: The selected strategy.
///
/// Returns:
///     The id whose `UseStopLoss`/`StopLoss`/`SellPrice` the order will carry.
fn exit_source(strategies: &[StrategyRow], hook: &str, strategy_id: u64) -> Option<u64> {
    if hook.is_empty() {
        return Some(strategy_id);
    }
    strategies
        .iter()
        // A zero id is the "nothing selected" sentinel everywhere else here, so a row carrying one
        // cannot be an answer: handing it back would reveal nothing and price an order off a
        // strategy the rest of this file reads as absent.
        .find(|row| is_hook(row) && row.id != 0 && row.name.trim() == hook)
        .map(|row| row.id)
}

/// Whether a retained strategy row is one of the Manual-kind strategies this mode selects from.
fn is_manual(row: &StrategyRow) -> bool {
    row.kind_ordinal == MANUAL_STRATEGY_KIND
}

/// Whether a retained strategy row is a MoonHook — the only kind `UseHookStrategy` can name.
fn is_hook(row: &StrategyRow) -> bool {
    row.kind_ordinal == HOOK_STRATEGY_KIND
}

/// The MoonHook this row defers its exits to, trimmed; empty means none.
///
/// Takes the ROW rather than an id, so a caller holding one pays no lookup at all. The header does
/// still find its row per drawn button (measured at well under a tenth of a percent of a frame, and
/// the alternative — carrying the row through the slot tuple — pays for the buttons the fit ladder
/// clips as well).
pub(crate) fn hook_of(
    row: &StrategyRow,
    schema: Option<&moon_core::feed::StrategySchemaModel>,
) -> String {
    strat_field_value(row, schema, FIELD_USE_HOOK_STRATEGY)
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// Convert a strategy's own stop distance into the signed percentage the toolbar and the order use.
///
/// One producer on purpose: the strategy stores a POSITIVE distance while everything on this side
/// is signed, and the suppression check in [`Backend::queue_visible_stop`] is only correct because
/// the value it compares against was produced right here too.
fn signed_stop_pct(strategy_pct: Option<f64>) -> f32 {
    strategy_pct.map(|pct| -(pct.abs() as f32)).unwrap_or(0.0)
}

/// Resolve a Manual-kind strategy NAME to its id within one core's retained snapshot.
///
/// Free rather than a method so the header's quick-select path resolves a name exactly the way the
/// stored mode does — the two had already drifted apart on whether to trim.
pub(crate) fn manual_strategy_id(strategies: &[StrategyRow], name: &str) -> Option<u64> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    // Both sides trimmed: a core name carrying surrounding whitespace would otherwise never match
    // its own stored copy and would refuse every order on that core, permanently.
    strategies
        .iter()
        // A zero id is the sentinel for "nothing selected" everywhere else here, so a row carrying
        // one cannot be selected; returning it would read as no selection and place a bare order.
        .find(|s| is_manual(s) && s.id != 0 && s.name.trim() == name)
        .map(|s| s.id)
}

/// Whether a stored selection names a strategy this core cannot currently provide.
///
/// Three states have to stay apart here, and conflating any two of them has already produced a
/// live defect:
///
/// - an EMPTY list is the list not having ARRIVED. A selection cannot be resolved against it, and
///   an order must be refused rather than sent without the strategy the header still shows.
/// - a CONFIRMED list holding no Manual strategy is a core this mode does not apply to at all.
///   `effective_manual_strat_state` already reads it as off and the header hides the whole cluster,
///   so refusing orders would leave nothing on screen able to clear the refusal.
/// - a confirmed list that HAS Manual strategies but not this one is the broken selection this
///   answers `true` for: renamed, deleted, or not yet published.
///
/// Args:
///     strategies: The core's retained strategy snapshot.
///     stored: This core's stored manual-strategy selection.
///
/// Returns:
///     Whether an order on this core must be refused.
fn manual_selection_is_broken(strategies: &[StrategyRow], stored: &ManualStratState) -> bool {
    if !stored.on || stored.strategy.trim().is_empty() {
        return false;
    }
    if !strategies.is_empty() && !strategies.iter().any(is_manual) {
        return false;
    }
    resolve_manual_selection(strategies, stored).is_none()
}

/// Resolve a stored selection to the strategy id an order must actually be placed on.
///
/// The pinned id wins whenever the core still HAS that strategy: re-deriving the id from the name
/// on every order is what let a Moonbot hook substitution silently move a selection onto another
/// strategy, and with it onto another stop. The name is consulted only when the pinned id names
/// nothing any more — a strategy deleted and rebuilt keeps its name and loses its number, which is
/// the case the name is stored for.
///
/// Args:
///     strategies: The core's retained strategy snapshot.
///     stored: This core's stored manual-strategy selection.
///
/// Returns:
///     The id to place on, or `None` when neither the pinned id nor the name resolves.
///
/// The pin is trusted on identity alone, not re-checked against the name: a core-side RENAME must
/// keep firing the strategy the trader picked, which is the whole point of pinning. The residual
/// risk is a pin carried to a DIFFERENT Moonbot (ids are unique per host, not globally), which is
/// why re-keying a core clears the pin — see `settings::connections`.
fn resolve_manual_selection(strategies: &[StrategyRow], stored: &ManualStratState) -> Option<u64> {
    if stored.id != 0 && strategies.iter().any(|s| is_manual(s) && s.id == stored.id) {
        return Some(stored.id);
    }
    manual_strategy_id(strategies, &stored.strategy)
}

/// Decide the manual-strategy state a core should be seeded with from its own snapshot.
///
/// `None` means "cannot answer yet, ask again": an EMPTY strategy list, which is not the same as a
/// core with no Manual strategies. The feed republishes the list on its first poll whatever it
/// holds (`last_strat_sig` starts at `u64::MAX`), so the first publish can be empty at a non-zero
/// revision and a revision check alone would seed against nothing, permanently.
///
/// `None` also for a core reporting the mode ON whose selection does not resolve, for the same
/// reason: that is an incompletely read core, not a core with nothing selected. Answering it would
/// latch either a mode that is on and names no strategy, or an off state that discarded the
/// trader's selection — and a stored answer is what stops the seed from asking again.
///
/// Args:
///     core_mode_on: The core's own `use_manual_strategy`.
///     core_strategy_id: The core's own `manual_strategy_id`.
///     strategies: The core's retained strategy snapshot.
///
/// Returns:
///     The state to store, or `None` while the snapshot cannot answer.
fn manual_strat_seed(
    core_mode_on: bool,
    core_strategy_id: u64,
    strategies: &[StrategyRow],
) -> Option<ManualStratState> {
    if strategies.is_empty() {
        return None;
    }
    let strategy = strategies
        .iter()
        .find(|s| is_manual(s) && s.id == core_strategy_id)
        .map(|s| s.name.trim().to_string())
        .unwrap_or_default();
    let id = if strategy.is_empty() {
        0
    } else {
        core_strategy_id
    };
    // A core naming a selection that resolves to nothing has not been read completely: the strategy
    // list arrives in partial payloads, so the first NON-empty one can be a subset that does not
    // contain the selected row yet. Storing "nothing selected" here would throw away the very
    // selection this seed exists to carry across the upgrade, and the stored answer is what stops
    // it from ever being asked again. The mode being off does not make it safe — the selection is
    // still what the trader gets back when they switch the mode on.
    if core_strategy_id != 0 && strategy.is_empty() {
        return None;
    }
    Some(ManualStratState {
        on: core_mode_on,
        strategy,
        id,
        // A core adopted from its own snapshot starts on Moonbot's stop rule, which is what it was
        // running under a moment ago.
        ..ManualStratState::default()
    })
}

/// Resolve whether a raw manual-strategy state is usable with the retained strategy snapshot.
///
/// Args:
///     raw: State from the stored per-core mode.
///     strategies: Rows in the retained strategy snapshot.
///
/// Returns:
///     Raw state while the snapshot is pending or contains a Manual-kind row; otherwise an
///     effective disabled state that preserves the selected id.
fn effective_manual_strat_state(raw: (bool, u64), strategies: &[StrategyRow]) -> (bool, u64) {
    // A NON-EMPTY list is the confirmation, not a non-zero revision: the feed publishes an empty
    // list at revision 1 during initialization (`InitialStrategies::new(0, Vec::new())`), so a
    // revision test reads every fresh connection as "this core has no manual strategy" and hands
    // back a disabled state for a while after every connect and every reconnect.
    let confirmed_without_manual = !strategies.is_empty() && !strategies.iter().any(is_manual);
    if confirmed_without_manual {
        (false, raw.1)
    } else {
        raw
    }
}

impl Backend {
    /// Return the six USD-equivalent presets and selected slot for one window group.
    pub(crate) fn manual_order_size_state(&self, group: &str) -> ([f64; 6], usize) {
        self.config
            .group_ref(group)
            .map_or((DEFAULT_ORDER_SIZES_USD, 2), |group| {
                (
                    group.trade.order_sizes_usd,
                    group
                        .trade
                        .order_size_sel
                        .min(group.trade.order_sizes_usd.len() - 1),
                )
            })
    }

    /// Resolve `core`'s ten manual-strategy quick-select slots: what each button fires and what it
    /// says.
    ///
    /// The terminal's own slots win when it has them; otherwise the core's `manual_strats_names`
    /// stand in, so a terminal that has never assigned a button shows exactly what Moonbot shows.
    /// `None` means neither source exists yet — the core has not reported its config and nothing
    /// local was ever assigned — and the caller must draw no buttons rather than ten empty ones.
    pub(crate) fn strat_slots(&self, core: CoreId) -> Option<Vec<StratSlot>> {
        if let Some(local) = self.core_strat_slots(core) {
            return Some(local.to_vec());
        }
        Some(self.core_slots_from_config(core)?.to_vec())
    }

    /// Build the slot array the CORE describes: its names, and its own per-slot visibility.
    ///
    /// `use_buttons` is folded into every slot's `show` rather than kept beside it, because from
    /// here on visibility is one boolean per slot — the core's master switch has no counterpart in
    /// the terminal's own slots, and carrying it separately would leave two different answers to
    /// "is this button drawn".
    fn core_slots_from_config(&self, core: CoreId) -> Option<[StratSlot; MANUAL_STRAT_SLOTS]> {
        let manual = &self
            .session
            .store()
            .core(core)?
            .core_config
            .as_ref()?
            .manual;
        Some(std::array::from_fn(|i| StratSlot {
            strategy: manual.strat_names[i].trim().to_string(),
            show: manual.strat_buttons.use_buttons && manual.strat_buttons.show_button[i],
        }))
    }

    /// Whether ANY slot arrangement exists for `core` — its own or the core's.
    ///
    /// The hotkey path asks this before falling back to its pre-slot ordinal reading: with a slot
    /// table present, the slot is the authority and an empty one fires nothing; with none at all,
    /// the ordinal is the only thing left to go on.
    pub(crate) fn core_owns_strat_trade_slots(&self, core: CoreId) -> bool {
        self.strat_slots(core).is_some()
    }

    /// Show or hide one quick-select slot, taking this core's slots local in the process.
    pub(crate) fn set_strat_slot_show(&mut self, core: CoreId, ix: usize, show: bool) {
        self.update_strat_slot(core, ix, |slot| slot.show = show);
    }

    /// Replace this core's local slots with what the CORE currently describes — Moonbot's own
    /// names and per-button visibility.
    ///
    /// The popup's explicit "pull from the core" action, and the only way back to the core's
    /// arrangement once a slot has been assigned here. Captions are dropped with the rest: they
    /// name the core's strategies again, which is exactly what pulling asks for.
    pub(crate) fn pull_strat_slots_from_core(&mut self, core: CoreId) -> bool {
        let Some(slots) = self.core_slots_from_config(core) else {
            return false;
        };
        self.update_server(core, |server| server.strat_slots = Some(slots.clone()));
        true
    }

    /// Set the core's "ignore a manual strategy's own sell price" flag.
    ///
    /// The one manual-block field that is NOT local: it changes what the core does with the toolbar
    /// TP and S slots while a manual strategy is active, so it travels as a narrow shared-config
    /// write and is confirmed by the core's echo like every other core-owned setting.
    pub(crate) fn set_ignore_strat_sell_price(&mut self, core: CoreId, on: bool) {
        let Some(mut cfg) = self
            .session
            .store()
            .core(core)
            .and_then(|data| data.core_config.clone())
        else {
            log::warn!("ignore strat sell price: core={core} has not reported its configuration");
            return;
        };
        cfg.manual.ignore_strat_sell_price = on;
        if let Err(error) = self.session.edit_core_config(
            core,
            cfg,
            FieldMask::EMPTY.with_ignore_strat_sell_price(),
        ) {
            log::warn!("ignore strat sell price failed: core={core}: {error:#}");
            return;
        }
        // Optimistic, for the same reason the manual-strategy toggle and Panic Sell keep one: this
        // value travels on the SLOW channel (a whole safe-share packet, behind the compact-settings
        // gate, with an echo and three attempts), so a checkbox rendered from the core's own value
        // alone does not move when clicked and reads as broken. The override is time-boxed rather
        // than permanent — if the core never takes the value, the truth has to come back on its
        // own.
        log::info!("core {core} ignore strat sell price -> {on} (queued)");
        self.ignore_sell_local.insert(
            core,
            IgnoreSellLocal {
                want: on,
                at: Instant::now(),
            },
        );
    }

    /// The core's "ignore a manual strategy's own sell price" flag as the UI must show it: a fresh
    /// local request while one is in flight, the core's own value otherwise.
    ///
    /// `None` before the core has reported its configuration at all — there is nothing to show and
    /// nothing to write onto.
    pub(crate) fn ignore_strat_sell_price(&self, core: CoreId) -> Option<bool> {
        let core_value = self
            .session
            .store()
            .core(core)?
            .core_config
            .as_ref()?
            .manual
            .ignore_strat_sell_price;
        Some(effective_ignore_sell(
            self.ignore_sell_local
                .get(&core)
                .map(|local| (local.want, local.at.elapsed())),
            core_value,
        ))
    }

    /// This core's OWN slots, or `None` while it still follows the core's.
    fn core_strat_slots(&self, core: CoreId) -> Option<&[StratSlot; MANUAL_STRAT_SLOTS]> {
        self.config
            .servers
            .iter()
            .find(|server| server.id == core)
            .and_then(|server| server.strat_slots.as_ref())
    }

    /// Assign the strategy one slot fires, taking this core's slots local in the process.
    ///
    /// Args:
    ///     core: Core whose slot is being assigned.
    ///     ix: Zero-based slot.
    ///     strategy: Manual-kind strategy name, or empty to clear the slot.
    pub(crate) fn set_strat_slot_strategy(&mut self, core: CoreId, ix: usize, strategy: String) {
        self.update_strat_slot(core, ix, |slot| slot.strategy = strategy.clone());
    }

    /// Apply one mutation to a slot, seeding this core's whole slot array from whatever it was
    /// SHOWING first.
    ///
    /// Seeding from the shown values rather than from empties is what keeps the other nine buttons
    /// where they were the moment the first one is assigned; without it, taking a core local would
    /// blank every button that came from `manual_strats_names`.
    fn update_strat_slot(&mut self, core: CoreId, ix: usize, update: impl Fn(&mut StratSlot)) {
        if ix >= MANUAL_STRAT_SLOTS {
            return;
        }
        let seed: [StratSlot; MANUAL_STRAT_SLOTS] = self
            .strat_slots(core)
            .map(|slots| std::array::from_fn(|i| slots.get(i).cloned().unwrap_or_default()))
            .unwrap_or_default();
        self.update_server(core, |server| {
            let slots = server.strat_slots.get_or_insert_with(|| seed.clone());
            update(&mut slots[ix]);
        });
    }

    /// Seed the manual-strategy exit OVERLAY from a strategy's own values.
    ///
    /// An overlay rather than a write: while MS is on, the take profit and stop on screen belong to
    /// the SELECTED STRATEGY, and the group's (or the core's own) saved values must survive
    /// untouched underneath. Switch to another chart and those saved values are what show; switch
    /// MS off and they come back here too.
    ///
    /// Keyed by `(core, strategy)`, so returning to a strategy restores what was last used with it
    /// rather than re-reading the strategy over the trader's own adjustment.
    pub(crate) fn seed_exit_from_strategy(&mut self, core: CoreId, strategy_id: u64) -> bool {
        if strategy_id == 0 {
            return false;
        }
        // The strategy the CORE will read these from, which is the selected one unless it defers to
        // a MoonHook. `None` means a hook is named but its row is not here, so nothing on screen can
        // be made to agree with the order: leave the saved generation showing rather than display
        // the selected strategy's own numbers, which that order will not use.
        let Some(source) = self.exit_source_strategy(core, strategy_id) else {
            return false;
        };
        // Already holding this source's values, adjustments included — nothing to read. A DIFFERENT
        // source is a different exit set altogether (the hook was changed or cleared), and that one
        // is re-read over whatever is here.
        if self
            .ms_exit_local
            .get(&(core, strategy_id))
            .map(|ms| ms.source)
            == Some(source)
        {
            return false;
        }
        let Some((stop_on, stop_pct, sell_pct)) = self.strategy_exit(core, source) else {
            return false;
        };
        // Through the same clamp every other writer of this number uses, or a strategy carrying a
        // stop outside the protocol range would be displayed and priced at a value that silently
        // snaps to the boundary the first time the SL popup is touched.
        let stop_pct = GroupExitSettings::canonical_stop_loss_pct(signed_stop_pct(Some(stop_pct)))
            .unwrap_or(0.0);
        // The take profit comes from the SELECTED strategy or from nowhere. The stop above may be
        // the hook's — a hook has one — but a hook has no `SellPrice`, so a hooked strategy leaves
        // this unseeded and the trader's own take profit stays on screen and on the order.
        //
        // A sell distance is a percentage forward from entry; anything non-finite or negative is
        // not one, and `planned_sell_price` would turn it into a target below the buy.
        let take_profit_pct = (source == strategy_id).then(|| {
            if sell_pct.is_finite() && sell_pct >= 0.0 {
                sell_pct
            } else {
                0.0
            }
        });
        self.ms_exit_local.insert(
            (core, strategy_id),
            MsExitOverlay {
                source,
                stop_on,
                stop_pct,
                take_profit_pct,
            },
        );
        true
    }

    /// Seed the exit overlay for a core whose manual-strategy mode is ALREADY on.
    ///
    /// The overlay is process-lifetime and used to be filled only by the click that selected a
    /// strategy. After a restart there is no click: the toolbar showed the saved generation and the
    /// first order carried it, while the header named a strategy whose values were never loaded.
    /// This closes that window as soon as the strategy list makes the selection resolvable.
    ///
    /// Returns whether anything was seeded, so the caller knows to repaint.
    pub(crate) fn tick_manual_exit_seed(&mut self) -> bool {
        let mut pending: Vec<(CoreId, u64)> = Vec::new();
        let mut checked: Vec<(CoreId, (u64, u64))> = Vec::new();
        for server in &self.config.servers {
            // Cheapest tests first: only a core whose mode is on and which names a strategy can
            // need an overlay, and those are a handful even on a 200-core desk. Everything after
            // this point costs a store lookup and a scan of that core's strategy list.
            let Some(stored) = server.manual_strategy.as_ref() else {
                continue;
            };
            if !stored.on || stored.strategy.trim().is_empty() {
                continue;
            }
            let Some(data) = self.session.store().core(server.id) else {
                continue;
            };
            // Both revisions this seed reads: the strategy list it resolves the selection against,
            // and the schema without which a field left at its default cannot be told from one that
            // has not arrived. Until one of them moves the answer cannot change, and re-deriving it
            // ten times a second is the cost this gate exists to remove.
            let key = (data.strategies_rev, data.schema_rev);
            if self.manual_exit_checked.get(&server.id) == Some(&key) {
                continue;
            }
            checked.push((server.id, key));
            let Some(id) = resolve_manual_selection(&data.strategies, stored) else {
                continue;
            };
            // No "already has an overlay" test here: an entry seeded off a source that no longer
            // applies — the strategy was pointed at another hook — has to be re-read, and only
            // `seed_exit_from_strategy` knows which source produced it. It answers cheaply.
            pending.push((server.id, id));
        }
        for (core, key) in checked {
            self.manual_exit_checked.insert(core, key);
        }
        let mut seeded = false;
        for (core, id) in pending {
            // Reports what it STORED, not what was attempted: a strategy whose fields have not
            // arrived seeds nothing, and treating that as work done would repaint at 10 Hz forever.
            seeded |= self.seed_exit_from_strategy(core, id);
        }
        seeded
    }

    /// The exit the toolbar must show and the order must use, while MS owns it.
    ///
    /// `None` whenever the saved generation is the one in force: MS off, no strategy selected, or
    /// no overlay seeded for it yet.
    pub(crate) fn manual_exit_overlay(&self, core: CoreId) -> Option<MsExitOverlay> {
        let strategy_id = self.manual_strat_active(core)?;
        self.ms_exit_local.get(&(core, strategy_id)).copied()
    }

    /// Apply an exit edit to the manual-strategy overlay instead of the saved generation.
    ///
    /// Returns whether the edit was absorbed here; `false` leaves it to the ordinary group/core
    /// write, which is what happens with MS off.
    fn edit_manual_exit_overlay(&mut self, group: &str, edit: ClientSettingsEdit) -> bool {
        let Some(core) = self.active_trade_core(group) else {
            return false;
        };
        let Some(strategy_id) = self.manual_strat_active(core) else {
            return false;
        };
        // Moonbot's own rule, when this core follows it: the stop belongs to the strategy and is
        // read-only here. ABSORBED rather than passed on — letting it fall through would write the
        // saved generation instead, moving a number that is not the one on screen. The toolbar
        // disables the same controls from `manual_stop_locked`, so this catches only what arrives by
        // another route: a hotkey, or a popup opened before the mode came on.
        if self.manual_stop_locked(core)
            && matches!(
                edit,
                ClientSettingsEdit::PanicIfPriceDrop(_) | ClientSettingsEdit::StopLossPct(_)
            )
        {
            log::debug!(
                "core {}: the stop belongs to the manual strategy, edit ignored",
                moon_core::feed::core_label(core)
            );
            return true;
        }
        // Seed first: an edit landing before the coordination tick would otherwise create the entry
        // itself, and the freshness guard would then block the strategy's own values from ever being
        // read into it. Idempotent, so this is a no-op once seeded.
        self.seed_exit_from_strategy(core, strategy_id);
        // Where the seed DECLINES — a hook owns the exits, or the schema has not arrived — there is
        // no overlay to edit and none is invented here: the toolbar is showing the saved generation,
        // so the edit belongs to it. Creating one would latch a set whose contents depend on whether
        // the trader clicked before or after the strategy's fields arrived, and the `contains_key`
        // guard would then block the strategy's own values forever.
        let Some(mut current) = self.ms_exit_local.get(&(core, strategy_id)).copied() else {
            return false;
        };
        match edit {
            ClientSettingsEdit::PanicIfPriceDrop(on) => current.stop_on = on,
            ClientSettingsEdit::StopLossPct(pct) => {
                let Some(pct) = GroupExitSettings::canonical_stop_loss_pct(pct) else {
                    return true;
                };
                current.stop_pct = pct;
            }
            ClientSettingsEdit::TakeProfit { pct, .. }
            | ClientSettingsEdit::ScalpTakeProfit(pct) => {
                if !(pct.is_finite() && pct >= 0.0) {
                    return true;
                }
                current.take_profit_pct = Some(pct);
            }
            // A fixed-sell preset is a take profit too, and the engaged one is what the order
            // sells at: keeping it out would leave the readout and the order disagreeing.
            ClientSettingsEdit::SelectFixedSellSlot(slot) if (1..=6).contains(&slot) => {
                current.take_profit_pct =
                    Some(self.write_aligned_group_exit(group).fixed_sell_pcts[slot - 1]);
            }
            _ => return false,
        }
        self.ms_exit_local.insert((core, strategy_id), current);
        true
    }

    /// One strategy's `UseStopLoss`, `StopLoss` and `SellPrice` as the core would apply them.
    ///
    /// `None` only when the answer is not KNOWABLE yet: no such strategy, or no schema. The schema
    /// is what makes an absent field readable at all — the server omits every value equal to its
    /// default and the schema carries the non-zero ones, so without it "absent" cannot be told from
    /// "at its default". WITH it, an unresolved field IS the default: `UseStopLoss=No`,
    /// `StopLoss=0`, `SellPrice=0`. That is why the tuple carries plain values, not options: every
    /// element is a real answer, and a caller that cannot get one gets `None` for the whole thing.
    fn strategy_exit(&self, core: CoreId, strategy_id: u64) -> Option<(bool, f64, f64)> {
        let data = self.session.store().core(core)?;
        let row = data.strategies.iter().find(|s| s.id == strategy_id)?;
        let schema = data.schema.as_ref()?;
        let field = |name: &str| strat_field_value(row, Some(schema), name);
        Some((
            field("UseStopLoss").is_some_and(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "yes" | "true" | "1" | "on"
                )
            }),
            field("StopLoss")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.0),
            field("SellPrice")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.0),
        ))
    }

    /// The MoonHook strategy this one defers its exits to, as the CORE currently holds it.
    ///
    /// Empty means none. Moonbot substitutes the hook at order time — its log says `Manual strategy
    /// X turned into Hook Y` — and the order's exits then come from the hook, not from the selected
    /// strategy (`Using (strategy <Y>) Sell Price`). Only the STOP is readable from here: a hook
    /// carries `UseStopLoss`/`StopLoss` like any strategy, but its sell lives in
    /// `HookSellLevel`/`HookSellFixed` rather than the `SellPrice` every other kind uses — see
    /// [`MsExitOverlay::take_profit_pct`].
    ///
    /// Read from `UseHookStrategy`, whose picklist moonproto builds from the local MoonHook
    /// strategies with an empty first item.
    pub(crate) fn strategy_hook(&self, core: CoreId, strategy_id: u64) -> String {
        let Some(data) = self.session.store().core(core) else {
            return String::new();
        };
        let Some(row) = data.strategies.iter().find(|s| s.id == strategy_id) else {
            return String::new();
        };
        hook_of(row, data.schema.as_ref())
    }

    /// [`Self::strategy_hook`] with this terminal's own unconfirmed edit on top.
    ///
    /// The control that sent the edit has to keep showing what was chosen: a core takes a moment to
    /// echo a strategy back, and a dropdown that snaps to the old value in between reads as a click
    /// that did nothing. The confirmed value returns the moment the echo lands.
    pub(crate) fn strategy_hook_shown(&self, core: CoreId, strategy_id: u64) -> String {
        let open = self
            .session
            .store()
            .core(core)
            .and_then(|data| data.strategy_edit(strategy_id));
        match open {
            // An open edit carries the strategy's DESIRED state whole, and a field equal to its
            // default is omitted from it — so an absent hook there means "no hook", not "this edit
            // says nothing about it". Falling back to the confirmed value instead would leave a
            // just-cleared hook on screen until the core echoed.
            Some(edit) => edit
                .fields
                .iter()
                .find(|(name, _)| name == FIELD_USE_HOOK_STRATEGY)
                .map(|(_, value)| value.trim().to_string())
                .unwrap_or_default(),
            None => self.strategy_hook(core, strategy_id),
        }
    }

    /// The MoonHook strategy carrying this name, when this core has one.
    ///
    /// Through [`exit_source`], the same resolver the exits go through, so a control that reveals
    /// "the hook this strategy uses" and the order that reads its stop can never mean two different
    /// strategies. The `0` is the unused no-hook fallback of that function: with a non-empty name
    /// it only ever answers the hook's own id.
    pub(crate) fn hook_strategy_id(&self, core: CoreId, hook: &str) -> Option<u64> {
        let hook = hook.trim();
        if hook.is_empty() {
            return None;
        }
        exit_source(&self.session.store().core(core)?.strategies, hook, 0)
    }

    /// Names of this core's MoonHook strategies, in snapshot order — the picker's options.
    ///
    /// A strategy the core never named is skipped. `StrategyRow::name` is a DISPLAY name and
    /// substitutes `strat <id>` for those (`feed::strategies::strat_display_name`), which is not a
    /// name the core's own `UseHookStrategy` picklist carries — writing it would set a hook nothing
    /// on the core resolves. Duplicates go too: the field addresses a hook by name, so two rows
    /// sharing one are a choice this picker cannot make on the trader's behalf.
    pub(crate) fn hook_strategy_names(&self, core: CoreId) -> Vec<String> {
        let Some(data) = self.session.store().core(core) else {
            return Vec::new();
        };
        let mut names: Vec<String> = Vec::new();
        for row in data.strategies.iter().filter(|row| is_hook(row)) {
            let name = row.name.trim();
            // The placeholder `strat <id>` spelling `feed::strategies::strat_display_name` puts on
            // an unnamed strategy, tested without allocating one to compare against.
            let placeholder = name
                .strip_prefix("strat ")
                .is_some_and(|rest| rest.parse::<u64>() == Ok(row.id));
            if name.is_empty() || placeholder {
                continue;
            }
            let name = name.to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    /// Point one strategy at a MoonHook strategy, or clear it with an empty `hook`.
    ///
    /// This writes to the CORE's own strategy, exactly as the Strategies panel's field editor does
    /// and through the same command — it is the same setting, reachable from the popup that already
    /// names the strategy. Nothing local is touched: the exit overlay re-seeds itself off the new
    /// source once the core echoes the strategy back ([`MsExitOverlay::source`]), so a write the
    /// core refuses leaves the screen on the values still in force.
    ///
    /// Returns whether the command was queued.
    pub(crate) fn set_strategy_hook(
        &mut self,
        core: CoreId,
        strategy_id: u64,
        hook: String,
    ) -> bool {
        let label = if hook.trim().is_empty() {
            rust_i18n::t!("header.ms_hook_none").to_string()
        } else {
            hook.trim().to_string()
        };
        let edits = vec![(
            strategy_id,
            vec![(FIELD_USE_HOOK_STRATEGY.to_string(), hook)],
        )];
        match self.session.edit_strategies(core, edits) {
            Ok(()) => {
                // The popup this is clicked from has no window of its own, so an outcome other than
                // a clean confirmation can only be reported through the shell's toast queue — the
                // same route the coin menu's field edit uses. Quiet on the way out: the picker
                // already shows what was chosen, and a toast per pick would be noise.
                self.watch_strategy_edit_quiet(core, strategy_id, label);
                true
            }
            Err(error) => {
                log::warn!(
                    "core {} strategy {strategy_id}: setting the hook failed: {error}",
                    moon_core::feed::core_label(core)
                );
                false
            }
        }
    }

    /// Whether this strategy's KIND exposes `field` in the schema the core sent.
    ///
    /// A field outside the kind's schema is dropped by the serializer, so the edit would be a
    /// silent no-op — and with no schema at all the whole batch is refused before it is staged.
    /// Both are "do not offer this control", which is why an absent schema answers `false`. The
    /// coin menu gates its own field edit the same way (`controls::coin_menu`).
    pub(crate) fn strategy_has_field(&self, core: CoreId, strategy_id: u64, field: &str) -> bool {
        let Some(data) = self.session.store().core(core) else {
            return false;
        };
        let Some(row) = data.strategies.iter().find(|s| s.id == strategy_id) else {
            return false;
        };
        let Some(schema) = data.schema.as_ref() else {
            return false;
        };
        schema
            .kinds
            .iter()
            .find(|kind| kind.ordinal == row.kind_ordinal)
            .is_some_and(|kind| {
                kind.sections
                    .iter()
                    .any(|section| section.fields.iter().any(|f| f.name == field))
            })
    }

    /// The strategy whose exits a manual order on `strategy_id` will actually carry.
    ///
    /// `Some(strategy_id)` with no hook set; the hook's own id when one is set and present in the
    /// snapshot; `None` when a hook is NAMED but its row is not here — the one case where this
    /// terminal cannot say what the order's exits will be, and must not guess.
    fn exit_source_strategy(&self, core: CoreId, strategy_id: u64) -> Option<u64> {
        let hook = self.strategy_hook(core, strategy_id);
        let strategies = self
            .session
            .store()
            .core(core)
            .map(|data| data.strategies.as_slice())
            .unwrap_or_default();
        exit_source(strategies, &hook, strategy_id)
    }

    /// Whether this core repeats Moonbot's own stop rule for manual-strategy orders.
    ///
    /// See [`ManualStratState::mb_logic`]. On for a core that has never had this state at all, so
    /// the answer is the same before and after the first selection is stored.
    pub(crate) fn ms_mb_logic(&self, core: CoreId) -> bool {
        self.stored_manual_strat(core)
            .map(|stored| stored.mb_logic)
            .unwrap_or_else(|| ManualStratState::default().mb_logic)
    }

    /// Set this core's Moonbot-stop-rule switch. Local state; nothing is sent.
    pub(crate) fn set_ms_mb_logic(&mut self, core: CoreId, on: bool) {
        if self.ms_mb_logic(core) == on {
            return;
        }
        let mut next = self.stored_manual_strat(core).cloned().unwrap_or_default();
        next.mb_logic = on;
        self.update_server(core, |server| server.manual_strategy = Some(next.clone()));
    }

    /// Who owns this core's stop right now, and what it is.
    ///
    /// Read LIVE from the strategy the core will apply, not from the exit overlay: while the rule
    /// is on nothing here can edit the stop, so there is no local adjustment to preserve, and the
    /// overlay is a snapshot that would keep reporting a number the strategy has since moved away
    /// from. It also settles the case the overlay cannot express — the strategy is in force but its
    /// value is unreadable — which must be STATED rather than filled in from the saved generation,
    /// a different number that no order will carry.
    pub(crate) fn manual_stop(&self, core: CoreId) -> ManualStop {
        if !self.ms_mb_logic(core) {
            return ManualStop::Free;
        }
        let Some(strategy_id) = self.manual_strat_active(core) else {
            return ManualStop::Free;
        };
        let Some(source) = self.exit_source_strategy(core, strategy_id) else {
            return ManualStop::Unknown;
        };
        let Some((on, pct, _)) = self.strategy_exit(core, source) else {
            return ManualStop::Unknown;
        };
        ManualStop::Strategy {
            on,
            pct: GroupExitSettings::canonical_stop_loss_pct(signed_stop_pct(Some(pct)))
                .unwrap_or(0.0),
        }
    }

    /// Whether the stop on screen belongs to the strategy and must not be edited here.
    ///
    /// The one predicate behind both halves of that promise: the toolbar disables its SL control
    /// with it, and `edit_group_exit` swallows a stop edit that reaches it by another route — a
    /// popup left open as the mode came on. Without the second half such an edit would fall through
    /// to the SAVED generation, quietly moving a value the trader cannot see.
    pub(crate) fn manual_stop_locked(&self, core: CoreId) -> bool {
        self.manual_stop(core).locked()
    }

    /// Queue the visible stop for the order about to be placed, when the core would otherwise use
    /// the strategy's own.
    ///
    /// Only with a manual strategy selected: without one the core already takes the stop from the
    /// `ClientSettings` generation this terminal pushes ahead of the order, and a second per-order
    /// write would be the same number twice.
    ///
    /// Args:
    ///     core: Core the order goes to.
    ///     market: Market it is placed on.
    ///     price: Entry price, which the stop's absolute level is computed from.
    ///     short: Position side; a short's stop sits ABOVE its entry.
    ///     exit: Visible exit generation the order was composed under.
    pub(crate) fn queue_visible_stop(
        &mut self,
        core: CoreId,
        market: &str,
        price: f64,
        short: bool,
        exit: GroupExitSettings,
    ) {
        let Some(strategy_id) = self.manual_strat_active(core) else {
            return;
        };
        // Moonbot's own rule, and the default: a manual-strategy order's stop is the strategy's (or
        // its hook's), full stop. There is nothing to override because nothing here was editable —
        // `manual_stop_locked` held the SL control shut — so the whole per-order write is off.
        if self.ms_mb_logic(core) {
            return;
        }
        // What the trader SEES while MS is on, which is the overlay — the saved generation is not
        // on screen in this mode and must not be what the order gets.
        let (stop_on, stop_pct) = self
            .manual_exit_overlay(core)
            .map(|ms| (ms.stop_on, ms.stop_pct))
            .unwrap_or((exit.stop_loss_enabled, exit.stop_loss_pct));
        // The strategy already puts this exact stop on the order, so saying it again costs a round
        // trip and makes the line visibly jump from the strategy's level to an identical one. The
        // per-order write exists to OVERRIDE the strategy; with nothing to override there is
        // nothing to send. Right after a selection this is the common case, because
        // `seed_exit_from_strategy` sets the screen to the strategy's own values.
        // Against the strategy the core will ACTUALLY read the stop from — the hook's when one is
        // set. Comparing with the selected strategy's own number while a hook supplies the real one
        // is how a visible -3% silently became the hook's -4.51% on 2026-09-01. A hook that names no
        // row here answers `None`, and an unknown stop is never equal to the visible one: the write
        // goes out.
        if let Some(source) = self.exit_source_strategy(core, strategy_id)
            && let Some((strat_on, strat_pct, _)) = self.strategy_exit(core, source)
        {
            // Exact equality because both sides come out of `signed_stop_pct`: the overlay compared
            // against was produced from the same strategy field by the same conversion, so an
            // untouched stop matches exactly and only a real edit differs. With both sides disabled
            // there is no percentage on screen to differ.
            if stop_write_is_redundant((strat_on, strat_pct), (stop_on, stop_pct)) {
                log::debug!(
                    "core {} market {market}: the visible stop equals the strategy's, no per-order \
                     write",
                    moon_core::feed::core_label(core)
                );
                return;
            }
        }
        let level = stop_price(price, f64::from(stop_pct), short);
        // A stop that is ON but has no price is not something this write can express: without a
        // fixed level the core resolves it from the wire, the strategy or ClientSettings — the very
        // sources this exists to override — so the order would silently keep the stop the trader is
        // trying to replace. Better to send nothing and leave the strategy's own in place than to
        // send a form that means something else.
        if stop_on && level.is_none() {
            log::warn!(
                "core {} market {market}: visible stop {stop_pct}% yields no price at {price}, no \
                 per-order write",
                moon_core::feed::core_label(core)
            );
            return;
        }
        let form = moon_core::feed::OrderStopsForm {
            sl: Some(moon_core::feed::StopGroupEdit {
                on: stop_on,
                // A FIXED price, not the percentage mode: the percentage mode resolves its level
                // from the wire, the strategy, or ClientSettings — and the strategy is exactly the
                // source this exists to override.
                fixed: stop_on && level.is_some(),
                price: level.unwrap_or(0.0),
            }),
            ..Default::default()
        };
        let before_uids = self
            .session
            .store()
            .core(core)
            .map(|data| {
                data.order_lines
                    .iter_market(market)
                    .map(|order| order.uid)
                    .collect()
            })
            .unwrap_or_default();
        self.pending_stops.insert(
            (core, market.to_string()),
            PendingStop {
                before_uids,
                form,
                at: Instant::now(),
            },
        );
    }

    /// Drop a queued visible stop whose order never went out.
    ///
    /// A pending stop waits for the next order to appear in its market, so an order command that
    /// failed to send must take its stop with it rather than leave it to catch an unrelated one.
    pub(crate) fn cancel_pending_stop(&mut self, core: CoreId, market: &str) {
        if self
            .pending_stops
            .remove(&(core, market.to_string()))
            .is_some()
        {
            log::debug!(
                "core {} market {market}: dropped the queued stop, its order never went out",
                moon_core::feed::core_label(core)
            );
        }
    }

    /// Apply the visible stop to a manual order the moment the core publishes it.
    ///
    /// A manual order placed WITH a strategy takes its stop from that strategy at the fill, so the
    /// generation the terminal pushes ahead of the order never reaches it — which is why an
    /// on-screen stop of -3% ended up as the strategy's own. The order therefore gets its stop
    /// individually, as an absolute price, the same way the Active-order dialog sets one.
    ///
    /// Returns whether anything was applied, so the caller knows to repaint.
    pub(crate) fn tick_pending_stops(&mut self) -> bool {
        let mut applied = Vec::new();
        for ((core, market), pending) in &self.pending_stops {
            if pending.at.elapsed() >= PENDING_STOP_TTL {
                applied.push((*core, market.clone(), None));
                continue;
            }
            let uid = self.session.store().core(*core).and_then(|data| {
                data.order_lines
                    .iter_market(market)
                    .find(|order| {
                        order.closed_ms.is_none() && !pending.before_uids.contains(&order.uid)
                    })
                    .map(|order| order.uid)
            });
            if let Some(uid) = uid {
                applied.push((*core, market.clone(), Some((uid, pending.form))));
            }
        }
        let mut sent = false;
        for (core, market, target) in applied {
            self.pending_stops.remove(&(core, market.clone()));
            let Some((uid, form)) = target else {
                log::warn!("pending stop for core={core} market={market} expired unapplied");
                continue;
            };
            log::info!("core {core} market {market} order {uid}: applying the visible stop");
            if let Err(error) = self.session.update_order_stops(core, uid, form) {
                log::warn!("pending stop failed: core={core} order={uid}: {error:#}");
                continue;
            }
            sent = true;
        }
        sent
    }

    /// Whether `core` keeps its OWN manual-trading generation instead of sharing its group's.
    ///
    /// The flag alone is not enough: a core must also HAVE a generation for the answer to be yes.
    /// `set_core_own_trade` seeds one before it sets the flag and `config::reconcile` seeds any
    /// file that predates the seeding rule, so the two agree in practice — but a hand-edited file
    /// can still carry the flag with no set, and there this must answer the same as the display
    /// resolver ([`Self::core_trade_settings`]), or the toolbar would show the group's numbers with
    /// the switch off while a click silently forked a per-core set.
    pub(crate) fn core_own_trade(&self, core: CoreId) -> bool {
        self.core_trade_settings(core).is_some()
    }

    /// Return `core`'s own manual-trading generation, or `None` while it shares its group's.
    ///
    /// A core with the switch ON but no stored generation cannot happen through
    /// [`Self::set_core_own_trade`], which seeds one before it flips the flag; this still answers
    /// `None` for a hand-edited config so every reader falls back to the group rather than to
    /// invented numbers.
    fn core_trade_settings(&self, core: CoreId) -> Option<&GroupTradeSettings> {
        self.config
            .servers
            .iter()
            .find(|server| server.id == core)
            .filter(|server| server.own_trade_config)
            .and_then(|server| server.trade.as_ref())
    }

    /// Set whether `core` keeps its own manual-trading generation, mirroring the live edit into an
    /// open Settings preview exactly like [`Self::update_group_trade`] (contract:
    /// `docs/ARCHITECTURE.md`'s preview-mirror rule, this never skips it).
    ///
    /// Turning the switch ON seeds the core's generation FROM THE GROUP the first time only: the
    /// numbers on screen must not move at the moment of the flip. A core that already has one
    /// keeps it, so toggling off and on again restores exactly what that core had — the reason the
    /// generation survives an off state at all.
    pub(crate) fn set_core_own_trade(&mut self, core: CoreId, on: bool) {
        let seed = on
            .then(|| {
                self.config
                    .servers
                    .iter()
                    .find(|s| s.id == core)
                    .and_then(|s| {
                        seed_on_enable(s.trade.as_ref(), &self.group_trade_settings(&s.group))
                    })
            })
            .flatten();
        self.update_server(core, |server| {
            server.own_trade_config = on;
            if let Some(seed) = seed.clone() {
                server.trade = Some(seed);
            }
        });
    }

    /// Apply one mutation to `core`'s server row in BOTH the live config and an open Settings
    /// preview — the server-row twin of [`Self::update_group_trade`], and the one place that
    /// mirroring is written for them.
    ///
    /// Without the preview half a per-core edit made while Settings is open is discarded the moment
    /// the user presses Save, which replaces the live config from the preview wholesale.
    fn update_server(
        &mut self,
        core: CoreId,
        update: impl Fn(&mut moon_core::config::ServerConfig),
    ) {
        for servers in [
            Some(&mut self.config.servers),
            self.preview.as_mut().map(|preview| &mut preview.servers),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(server) = servers.iter_mut().find(|s| s.id == core) {
                update(server);
            }
        }
        self.config_dirty = true;
    }

    /// Return one group's complete manual-trading generation, or the neutral standard when the
    /// group row does not exist yet.
    fn group_trade_settings(&self, group: &str) -> GroupTradeSettings {
        self.config
            .group_ref(group)
            .map(|group| group.trade.clone())
            .unwrap_or_default()
    }

    /// Apply one mutation to `core`'s own manual-trading generation, in both the live config and an
    /// open Settings preview — the per-core twin of [`Self::update_group_trade`].
    ///
    /// Seeds from the group when the core somehow has the switch on with no stored generation, so a
    /// hand-edited config cannot make an edit vanish.
    fn update_core_trade(&mut self, core: CoreId, update: impl Fn(&mut GroupTradeSettings)) {
        let group = self
            .config
            .servers
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.group.clone());
        let seed = group.map(|group| self.group_trade_settings(&group));
        self.update_server(core, |server| {
            let trade = server
                .trade
                .get_or_insert_with(|| seed.clone().unwrap_or_default());
            update(trade);
        });
    }

    /// Resolve the core whose OWN manual-trading generation a write must land in, because the
    /// group's active trading core keeps one. `None` means the write proceeds group-local exactly
    /// as before. This is the ONE choke point every group-local writer
    /// (`set_order_size_sel`, `set_order_size_value`, `edit_group_exit`) checks, so a hotkey, a
    /// strip click, a Settings-panel input, and a metric popup cannot reach three different
    /// conclusions about which core (if any) a write must go to.
    ///
    /// Gates on [`Self::active_trade_core`] rather than the hover-aware chart display core the
    /// toolbar renders from: the toolbar's own strips additionally go non-interactive against the
    /// hover-aware core at render time (see [`Self::manual_display_matches_write`]), so the two
    /// together close the gap even in the narrow case where hovering a different chart in the
    /// same group briefly disagrees with this fallback.
    pub(crate) fn manual_write_core(&self, group: &str) -> Option<CoreId> {
        self.active_trade_core(group)
            .filter(|&core| self.core_own_trade(core))
    }

    /// Whether a manual-trading control seeded from `display_core` would write to the source it
    /// just showed.
    ///
    /// The toolbar's displayed core (`toolbar::effective_chart_display_core`) is hover-aware,
    /// while every write targets [`Self::manual_write_core`], which is not: hovering a chart whose
    /// core differs from the group's active trading core — with either or both opted into the
    /// per-core route — can make the two disagree while the strip is still rendered as live. A
    /// control this answers `false` for must go non-interactive rather than mutate a source other
    /// than the one on screen (goal A2 FIX-3): a disabled control with a reason beats a live
    /// control that silently writes elsewhere.
    ///
    /// Args:
    ///     group: Window group whose manual-trading controls are being gated.
    ///     display_core: The hover-aware core the toolbar is currently showing values from.
    ///
    /// Returns:
    ///     `true` when the displayed source and the write target agree — both group-local, or the
    ///     same core.
    pub(crate) fn manual_display_matches_write(
        &self,
        group: &str,
        display_core: Option<CoreId>,
    ) -> bool {
        let display_target = display_core.filter(|&core| self.core_own_trade(core));
        display_target == self.manual_write_core(group)
    }

    /// One resolver for the toolbar, hotkeys, and every metric popup: the effective F1-F6 sizes,
    /// selected slot, and their source, so no caller reaches a different conclusion than another.
    ///
    /// `core` is the chart display core, or `None` with no chart addressed. `GroupLocal` is
    /// returned only when the opt-in is off (or `core` is `None`); once it is on the source is
    /// always `Core`, using a neutral placeholder while genuinely `Awaiting` rather than silently
    /// reverting to group-local numbers that would look like the checkbox is off.
    pub(crate) fn effective_order_size_state(
        &self,
        group: &str,
        core: Option<CoreId>,
    ) -> ([f64; 6], usize, ManualSource) {
        if let Some(trade) = core.and_then(|core| self.core_trade_settings(core)) {
            return (
                trade.order_sizes_usd,
                trade.order_size_sel.min(trade.order_sizes_usd.len() - 1),
                ManualSource::CoreOwn,
            );
        }
        let (sizes, sel) = self.manual_order_size_state(group);
        (sizes, sel, ManualSource::GroupLocal)
    }

    /// Resolve the group's F1-F6 USD-equivalent presets through the SAME source
    /// [`Self::set_order_size_value`] / [`Self::set_order_size_sel`] will write to.
    ///
    /// Every wheel-step or inline-editor seed that feeds one of those writes reads THIS, never
    /// [`Self::manual_order_size_state`] directly: a relative edit (Ctrl+wheel) must be computed
    /// against the value about to be overwritten, not against the group's generation while the
    /// write lands on the core's (goal A2 FIX-1).
    ///
    /// Args:
    ///     group: Window group whose write-target presets are requested.
    ///
    /// Returns:
    ///     The six presets and selected slot from [`Self::manual_write_core`]'s source, or the
    ///     group-local generation while that source is `None`.
    pub(crate) fn write_aligned_order_sizes(&self, group: &str) -> ([f64; 6], usize) {
        let (sizes, sel, _source) =
            self.effective_order_size_state(group, self.manual_write_core(group));
        (sizes, sel)
    }

    /// Exit twin of [`Self::effective_order_size_state`]: the core's retained
    /// `ClientSettings::group_exit_settings()` when the opt-in is on and available, else the
    /// group-local exit generation. Reuses existing machinery entirely — no new projection.
    pub(crate) fn effective_group_exit(
        &self,
        group: &str,
        core: Option<CoreId>,
    ) -> (GroupExitSettings, ManualSource) {
        if let Some(trade) = core.and_then(|core| self.core_trade_settings(core)) {
            return (trade.exit, ManualSource::CoreOwn);
        }
        (self.group_exit_settings(group), ManualSource::GroupLocal)
    }

    /// Resolve the group's complete exit generation through the SAME source
    /// [`Self::edit_group_exit`] will write to.
    ///
    /// Every TP/SL/S-slot reader that feeds a subsequent [`Self::edit_group_exit`] write — popup
    /// seeding, the Extended-TP toggle, the stop-market checkbox, S-slot wheel and inline editing
    /// — reads THIS, never [`Self::group_exit_settings`] directly, so it can never be computed
    /// against a different generation than the one about to be overwritten (goal A2 FIX-2).
    ///
    /// Args:
    ///     group: Window group whose write-target exit generation is requested.
    ///
    /// Returns:
    ///     The exit generation from [`Self::manual_write_core`]'s source, or the group-local
    ///     generation while that source is `None`.
    pub(crate) fn write_aligned_group_exit(&self, group: &str) -> GroupExitSettings {
        self.effective_group_exit(group, self.manual_write_core(group))
            .0
    }

    /// Select an F1-F6 USD-equivalent preset for one group, or for the group's active core when
    /// that core keeps its own generation: the choke point re-targets the write onto the core's
    /// own set rather than the group's.
    pub(crate) fn set_order_size_sel(&mut self, group: &str, ix: usize) {
        if ix >= 6 {
            return;
        }
        if let Some(core) = self.manual_write_core(group) {
            self.update_core_trade(core, |trade| trade.order_size_sel = ix);
            return;
        }
        self.update_group_trade(group, |trade| trade.order_size_sel = ix);
    }

    /// Resolve the USD-equivalent order size for a real order about to reach `core`, refusing
    /// rather than guessing when the core route is on but has not yet reported a real value.
    ///
    /// Unlike [`Self::effective_order_size_state`] (which renders a neutral placeholder while
    /// `Awaiting`, correct for a toolbar that must draw six cells regardless), a placeholder here
    /// would size a real order from `DEFAULT_ORDER_SIZES_USD` while the user believes it is sized
    /// from their configured core. `None` therefore means "do not place this order", not "show 0".
    fn effective_order_size_usd_for_order(&self, group: &str, core: CoreId) -> Option<f64> {
        let (sizes, sel, _source) = self.effective_order_size_state(group, Some(core));
        Some(sizes[sel])
    }

    /// Exit twin of [`Self::effective_order_size_usd_for_order`]: the core's own retained exit
    /// generation when the core route is on, refusing rather than sending a blank/default exit
    /// when the core has never reported `ClientSettings`.
    fn effective_group_exit_for_order(
        &self,
        group: &str,
        core: CoreId,
    ) -> Option<GroupExitSettings> {
        Some(self.effective_group_exit(group, Some(core)).0)
    }

    /// Convert a target core's effective USD amount — group-local, or the core's own when the
    /// per-core opt-in is on (display and order must never be able to disagree) — into the unit
    /// the core places an order in ON THIS MARKET.
    ///
    /// Two units, because a quantity does not mean the same thing on every market and the wire
    /// field is one `size`:
    ///
    /// - **Inverse (coin-margined) markets take a CONTRACT COUNT**, each contract worth a fixed
    ///   amount of quote currency (`BTCUSD_PERP` = $100, other `*USD` = $10), so the size is
    ///   `usd / contract_size` and no price is involved. Sending the USD figure itself put a
    ///   $500 order in as 500 contracts — $50 000 — on the 2026-08-31 QQ run.
    /// - **Linear and spot markets take the account's balance currency**, so the size is
    ///   `usd / rate`, unchanged.
    ///
    /// The unit comes from `MarketDataSource::market_quantity_unit`, the same rule the market's
    /// maximum-order cap uses: a cap stated in USD beside an order sized in contracts is exactly
    /// the disagreement that rule exists to prevent. Its `None` — the market's figures have not
    /// arrived — REFUSES here rather than picking a unit.
    ///
    /// Args:
    ///     core: Core the order will be placed on.
    ///     market: Canonical market name the order is for — the unit depends on it, not just on
    ///         the core.
    ///
    /// Returns:
    ///     The size in the market's own unit and the USD equivalent it came from.
    pub(crate) fn manual_order_size_base(
        &self,
        core: CoreId,
        market: &str,
        price: f64,
    ) -> Option<(f64, f64)> {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server.id == core)?;
        let usd = self.effective_order_size_usd_for_order(&server.group, core)?;
        let source = self.session.market_source();
        // REFUSES on an unknown unit rather than picking one: guessing here is what sends a whole
        // account's worth of coin as a quantity.
        let rules = source.order_size_rules(core, market)?;
        let rate = match rules.unit {
            // Coin-margined: the wire field is the account's balance currency, which on these
            // markets is the COIN — so the amount is `usd / price`, exactly as on a linear market
            // quoted in dollars. `contract_size` does NOT belong here: the core reports positions
            // in contracts but takes orders in coin, measured on the 2026-08-31 QQ run where a
            // sent `50` came back as a $4 990 order (50 SOL), not as 50 contracts ($500).
            MarketQuantityUnit::Contracts(_) => (price.is_finite() && price > 0.0).then_some(price),
            MarketQuantityUnit::Coins => {
                source.currency_usd_rate(core, self.session.core_base(core)?)
            }
        };
        let size = usd_to_base_amount(usd, rate)?;
        // Below what the venue accepts the order cannot be placed at all, so refusing is the honest
        // answer: rounding down reaches zero, rounding up spends more than the trader asked for.
        // On a coin-margined market the venue states its minimum in CONTRACTS while the order is
        // sent in coin, so the floor is converted the same way a position is read back —
        // `min_qty * contract_size / price` — and never drops below one whole contract, which
        // cannot be split.
        let (floor, notional) = match rules.unit {
            MarketQuantityUnit::Contracts(contract_size) => {
                let contracts = rules.min_qty.max(1.0);
                (
                    contracts * contract_size / price,
                    format!(", about ${} of notional", contracts * contract_size),
                )
            }
            MarketQuantityUnit::Coins => (rules.min_qty, String::new()),
        };
        if floor > 0.0 && size < floor {
            log::warn!(
                "manual order refused: core={core} market={market} ${usd} is below this market's \
                 minimum: {size:.8} < {floor:.8} ({:?}){notional}",
                rules.unit
            );
            return None;
        }
        Some((size, usd))
    }

    /// Return the visible USD equivalent for the crosshair label.
    ///
    /// Deliberately does NOT resolve the market's quantity unit, unlike the order path: this runs
    /// on the ChartPanel's per-frame render, and `market_quantity_unit` takes the market-source
    /// lock and reads a snapshot — the class of call `market/source/read.rs` says does not belong
    /// on a frame path. The USD figure is the same either way; only the unit it will be converted
    /// INTO depends on the market, and that conversion happens when an order is actually placed.
    ///
    /// The label can therefore stand while the order path would refuse for a unit it cannot yet
    /// resolve. That is the right way round: a refused order says so in the log, whereas taking a
    /// lock 60 times a second to hide a number would cost every chart frame.
    pub(crate) fn prospective_order_usd(&self, core: CoreId) -> Option<f64> {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server.id == core)?;
        let usd = self.effective_order_size_usd_for_order(&server.group, core)?;
        // Same "can this core price anything at all" gate the order path applies, and cheap: a
        // USD-stable base short-circuits inside `currency_usd_rate` before any lock is taken.
        let base = self.session.core_base(core)?;
        self.session
            .market_source()
            .currency_usd_rate(core, base)
            .filter(|rate| rate.is_finite() && *rate > 0.0)
            .map(|_| usd)
    }

    /// Resolve the effective exit settings and either the visible USD size or a FireTest override.
    ///
    /// Both `exit` and the size (through [`Self::manual_order_size_base`]) come from the SAME
    /// effective resolver the display uses, gated on the per-core flag: a trader sizing from a
    /// number the order does not use is the worst failure this goal can ship.
    pub(crate) fn manual_order_terms(
        &self,
        core: CoreId,
        market: &str,
        price: f64,
        short: bool,
        size_base_override: Option<f64>,
    ) -> Option<super::ManualOrderTerms> {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server.id == core)?;
        let exit = self.effective_group_exit_for_order(&server.group, core)?;
        let (size_base, size_usd) = match size_base_override {
            Some(size) if size.is_finite() && size > 0.0 => (size, None),
            Some(_) => return None,
            None => {
                let (size, usd) = self.manual_order_size_base(core, market, price)?;
                (size, Some(usd))
            }
        };
        // The visible take profit rides ALONG WITH the order, Moonbot-style, and only where it is
        // the thing that applies: with a manual strategy selected, the core sells at that
        // strategy's own price unless its "ignore the strategy's sell price" checkbox is on. Ask
        // for a target in the other case and the core would have two answers for one order.
        let (_, strategy_id) = self.manual_strat_state(core);
        // A selection the retained snapshot cannot resolve is the one case that must not become an
        // order. It happens when the strategy was renamed or deleted on the core, or has simply not
        // arrived yet — and because the order names its strategy explicitly now, letting it through
        // would place a BARE order under the group's TP/SL, which the core is then free to attach
        // its OWN manual strategy to. Refusing is the only reading that cannot lose money quietly.
        if let Some(name) = self.manual_strat_unresolved(core) {
            log::warn!(
                "manual order refused: core={} selects the Manual strategy {name:?}, which is not \
                 in its retained snapshot; nothing sent",
                moon_core::feed::core_label(core)
            );
            return None;
        }
        // The mode with NOTHING selected is not manual trading: there is no strategy to place the
        // order on, so it must behave exactly like the mode being off — the terminal's own exits
        // apply and travel with the order — rather than fall between the two and produce an order
        // with neither a strategy nor a take profit.
        let manual_on = self.manual_strat_active(core).is_some();
        // The core's own switch is no longer written by this terminal, so it can be left on by
        // Moonbot's screen or an older build. A zero StratID then makes the CORE substitute its own
        // manual strategy into an order this terminal priced from the group generation. Nothing on
        // the wire can forbid that — `StratID` has no "deliberately none" value — so the least this
        // can do is say so where the order is logged.
        if !manual_on
            && self
                .session
                .store()
                .core(core)
                .and_then(|data| data.client_settings.as_ref())
                .is_some_and(|settings| settings.use_manual_strategy)
        {
            log::warn!(
                "core {} still has its OWN manual-strategy switch on: it may attach that strategy \
                 to this order, which the terminal is placing without one",
                moon_core::feed::core_label(core)
            );
        }
        let terminal_owns_sell = !manual_on || self.ignore_strat_sell_price(core).unwrap_or(false);
        // The take profit the trader SEES: the manual-strategy overlay while it is in force, the
        // saved generation otherwise.
        let take_profit_pct = self
            .manual_exit_overlay(core)
            .and_then(|ms| ms.take_profit_pct)
            .unwrap_or_else(|| exit.effective_take_profit_pct());
        let planned_sell = terminal_owns_sell
            .then(|| planned_sell_price(price, take_profit_pct, short))
            .flatten();
        Some(ManualOrderTerms {
            size_base,
            size_usd,
            exit,
            planned_sell,
            sync_exit: !manual_on,
            strategy_id: manual_on.then_some(strategy_id),
        })
    }

    /// Write one USD-equivalent F1-F6 preset into the group's set, or into the active core's own
    /// set when it keeps one. The zero/non-finite guard rejects a value the order path could not
    /// size from, identically on both routes.
    pub(crate) fn set_order_size_value(&mut self, group: &str, ix: usize, value: f64) {
        if ix >= 6 || !(value.is_finite() && value > 0.0) {
            return;
        }
        if let Some(core) = self.manual_write_core(group) {
            self.update_core_trade(core, |trade| trade.order_sizes_usd[ix] = value);
            return;
        }
        self.update_group_trade(group, |trade| trade.order_sizes_usd[ix] = value);
    }

    /// Return complete visible group exits, falling back to the neutral standard before repair.
    pub(crate) fn group_exit_settings(&self, group: &str) -> GroupExitSettings {
        self.config
            .group_ref(group)
            .map(|group| group.trade.exit)
            .unwrap_or_default()
    }

    /// Apply a visible TP/SL/S-slot edit to the generation the toolbar is writing to: the group's,
    /// or the active core's own when it keeps one.
    ///
    /// Both routes write LOCAL config only. The core learns the new generation the way it always
    /// has — `sync_manual_settings` pushes it to the cores that use it, and the order path holds
    /// each order behind its own exit generation — so an edit never depends on the core having
    /// answered first.
    pub(crate) fn edit_group_exit(&mut self, group: &str, edit: ClientSettingsEdit) -> bool {
        // With a manual strategy selected the exits on screen belong to that strategy, not to the
        // saved generation — see `manual_exit_overlay`. Absorbed before any config write so the
        // group's (or the core's own) values are left exactly as the trader saved them.
        if self.edit_manual_exit_overlay(group, edit) {
            return true;
        }
        let write_core = self.manual_write_core(group);
        let mut exit = self.effective_group_exit(group, write_core).0;
        if !apply_group_exit_edit(&mut exit, edit) {
            return false;
        }
        match write_core {
            Some(core) => self.update_core_trade(core, |trade| trade.exit = exit),
            None => self.update_group_trade(group, |trade| trade.exit = exit),
        }
        true
    }

    /// Apply one group-trade mutation to both live config and an open Settings preview.
    fn update_group_trade(&mut self, group: &str, update: impl Fn(&mut GroupTradeSettings)) {
        let live = &mut self.config.group_mut(group).trade;
        let preview = self
            .preview
            .as_mut()
            .map(|preview| &mut preview.group_mut(group).trade);
        update_group_trade_pair(live, preview, update);
        self.config_dirty = true;
    }

    /// Drop sync bookkeeping for cores that are no longer live.
    ///
    /// What this function no longer does is the point: it used to PUSH every group's exit
    /// generation into every live core on each coordination tick, which is why a TP changed in
    /// Moonbot itself sprang back within a tick, and why the compact settings channel could stay
    /// permanently busy — starving the safe-share writes queued behind it.
    ///
    /// The terminal's exits are local. They reach a core exactly where they must: the manual-order
    /// path serializes the visible generation ahead of the order it belongs to
    /// (`feed::live::client_settings`), so an order can still never go out under someone else's
    /// TP/SL, while a core left alone stays editable from its own screen.
    pub(crate) fn sync_manual_settings(&mut self) {
        let live_ids: HashSet<CoreId> = self
            .session
            .sessions()
            .iter()
            .map(|session| session.id)
            .collect();
        self.group_exit_sync
            .retain(|core, _| live_ids.contains(core));
        self.manual_strat_checked
            .retain(|core, _| live_ids.contains(core));
        self.manual_exit_checked
            .retain(|core, _| live_ids.contains(core));
        // Keyed by `(core, strategy)`, so it is pruned by the core half. A strategy that goes away
        // on a LIVE core keeps its entry, which is deliberate: the trader may switch back to a
        // rebuilt one and expect the exits they last used with it.
        self.ms_exit_local
            .retain(|(core, _), _| live_ids.contains(core));
    }

    /// Set this core's manual-strategy mode, which is terminal state and stays here.
    ///
    /// Nothing is sent to the core: the strategy travels with the order instead
    /// ([`ManualOrderTerms::strategy_id`]), so Moonbot's own manual-strategy switch is left exactly
    /// where its user put it, and two terminals on one core can sit on different strategies.
    ///
    /// Args:
    ///     core: Core whose mode is being set.
    ///     on: Whether manual-strategy mode is enabled.
    ///     id: Selected strategy, or `0` to keep the stored selection while only `on` changes.
    pub(crate) fn set_manual_strat(&mut self, core: CoreId, on: bool, id: u64) -> u64 {
        // Name resolved here, while the snapshot is at hand, and the id PINNED alongside it: this
        // is the moment the trader actually chose, and every later order must go to that same
        // strategy rather than to whatever the name resolves to at the time.
        //
        // An id that resolves to nothing keeps the stored pair rather than clearing it — that is a
        // strategy list which has not arrived, not a trader deselecting anything.
        let resolved = self.manual_strategy_name(core, id).map(str::to_string);
        let (strategy, id) = match resolved {
            Some(name) => (name, id),
            None => {
                if id != 0 {
                    log::warn!(
                        "core {} manual strategy {id} is not in the retained snapshot; keeping the \
                         previous selection",
                        moon_core::feed::core_label(core)
                    );
                }
                self.stored_manual_strat(core)
                    .map(|stored| (stored.strategy.trim().to_string(), stored.id))
                    .unwrap_or_default()
            }
        };
        // Everything this setter does not own is carried over: `mb_logic` is a separate switch in
        // the same popup, and rebuilding the state around a selection must not reset it.
        let mb_logic = self.ms_mb_logic(core);
        let next = ManualStratState {
            on,
            strategy,
            id,
            mb_logic,
        };
        // Held hotkeys repeat, and a re-selection of the same strategy is the common case; writing
        // unconditionally would raise `config_dirty` and buy a full encrypted save each repeat.
        if self.stored_manual_strat(core) == Some(&next) {
            return id;
        }
        self.update_server(core, |server| server.manual_strategy = Some(next.clone()));
        id
    }

    /// This core's stored manual-strategy mode, or `None` while it has never had one here.
    fn stored_manual_strat(&self, core: CoreId) -> Option<&ManualStratState> {
        self.config
            .servers
            .iter()
            .find(|server| server.id == core)?
            .manual_strategy
            .as_ref()
    }

    /// Name of the manual strategy this core is set to, when one is actually named.
    ///
    /// Answers "did the trader select something" independently of whether it currently resolves,
    /// which is what separates an unconfigured core from one pointing at a missing strategy.
    pub(crate) fn selected_manual_strategy_name(&self, core: CoreId) -> Option<&str> {
        let name = self.stored_manual_strat(core)?.strategy.trim();
        (!name.is_empty()).then_some(name)
    }

    /// Resolve a Manual-kind strategy id to its name in this core's retained snapshot.
    fn manual_strategy_name(&self, core: CoreId, id: u64) -> Option<&str> {
        if id == 0 {
            return None;
        }
        crate::strategies::logic::row(self.session.store(), core, id)
            .filter(|row| is_manual(row))
            .map(|row| row.name.trim())
    }

    /// Seed a core's manual-strategy mode from its own snapshot, once, and only if it has none.
    ///
    /// The upgrade path: before this terminal owned the mode it lived in the core, so a trader who
    /// left Moonbot on a manual strategy must find the terminal on that same strategy after the
    /// first launch instead of silently switched off. Runs on the coordination tick because both
    /// halves it needs — the settings snapshot and a CONFIRMED strategy list to resolve the id
    /// against — arrive asynchronously and at different times.
    ///
    /// Returns whether anything was seeded, so the caller knows to repaint.
    pub(crate) fn tick_manual_strat_seed(&mut self) -> bool {
        // Never while the settings window holds a draft: `update_server` mirrors into that preview,
        // so a tick-driven pin would appear inside a row the user is mid-edit and be committed by
        // their Save. It settles on the next tick after the window closes.
        if self.preview.is_some() {
            return false;
        }
        let mut seeds: Vec<(CoreId, ManualStratState)> = Vec::new();
        let mut answered: Vec<(CoreId, SettleKey)> = Vec::new();
        for server in &self.config.servers {
            let Some(data) = self.session.store().core(server.id) else {
                continue;
            };
            // Nothing this pass can decide differently until one of the inputs it reads has moved.
            // Without this gate every unresolvable core — a deleted strategy, a core with the
            // strategy feed switched off, a dead core — re-ran the whole resolution ten times a
            // second, forever. `client_settings_stale` is part of the key because it clears WITHOUT
            // moving a revision: a reconnect whose settings equal the retained ones bumps nothing,
            // and a core marked while stale would otherwise never be examined again.
            let key = (
                data.strategies_rev,
                data.client_settings_rev,
                data.client_settings_stale,
            );
            if self.manual_strat_checked.get(&server.id) == Some(&key) {
                continue;
            }
            // Marked unconditionally, because the key holds every input this pass reads: the two
            // revisions and the staleness flag. A core it cannot answer for today gets exactly one
            // more attempt per input change, instead of the same two scans ten times a second.
            answered.push((server.id, key));
            match server.manual_strategy.as_ref() {
                // Nothing selected, and nothing adopted into it either. The core's own
                // `manual_strategy_id` is NOT read here, however tempting: Moonbot moves that field
                // by itself, and forwarding it is what put two real orders on a strategy nobody
                // chose on 2026-09-01 (see `manual_strat_state`). Adoption belongs to the `None`
                // arm below — a core this terminal has never known — and a stored state, even one
                // naming nothing, means the trader has already touched this mode here.
                //
                // The cost is narrow and visible: turning MS on before the core has reported skips
                // the one-time carry-over of its selection, and the picker — on screen precisely
                // because the mode is on — is how one gets chosen instead.
                Some(stored) if stored.strategy.trim().is_empty() => {}
                // A stored selection whose pin is missing or has gone stale. Missing is every
                // config written before the id was kept; stale is a strategy deleted and rebuilt,
                // which keeps its name and loses its number. Re-pin from the name either way —
                // leaving it would silently return this core to resolving the name before every
                // order, which is the behaviour the pin exists to end.
                Some(stored) => {
                    if resolve_manual_selection(&data.strategies, stored) == Some(stored.id) {
                        continue;
                    }
                    // Re-pinning writes a per-host id into permanent config, so it waits for the
                    // same freshness signal the first seed demands. PARTIAL cover, knowingly: the
                    // flag tracks the SETTINGS feed while this reads the strategy list, and that
                    // list carries no staleness marker at all. It closes the window around a
                    // disconnect, not the one between a reconnect's settings and its first
                    // strategy publish.
                    if data.client_settings_stale {
                        continue;
                    }
                    if let Some(id) = manual_strategy_id(&data.strategies, &stored.strategy) {
                        seeds.push((
                            server.id,
                            ManualStratState {
                                id,
                                ..stored.clone()
                            },
                        ));
                    }
                }
                None => {
                    let Some(settings) = data.client_settings.as_ref() else {
                        continue;
                    };
                    // Stale settings are a snapshot the store itself will not vouch for — after a
                    // disconnect, or after a key change that may point the feed at a DIFFERENT
                    // Moonbot, since the store keeps the previous host's settings until new ones
                    // arrive. Adopting one into permanent config is the one mistake this seed
                    // cannot undo.
                    //
                    // It covers the settings half only: the retained strategy list carries no
                    // staleness marker at all, so the NAME this resolves can still come from a
                    // previous host's list until that list is replaced. Narrow enough to live with,
                    // wide enough to write down.
                    if data.client_settings_stale {
                        continue;
                    }
                    let Some(state) = manual_strat_seed(
                        settings.use_manual_strategy,
                        settings.manual_strategy_id,
                        &data.strategies,
                    ) else {
                        continue;
                    };
                    seeds.push((server.id, state));
                }
            }
        }
        for (core, key) in answered {
            self.manual_strat_checked.insert(core, key);
        }
        let seeded = !seeds.is_empty();
        for (core, state) in seeds {
            // At info only when there is a selection to report; a fleet settling on the first
            // launch after an upgrade would otherwise put one line per core into the log for
            // having decided that nothing is selected.
            if state.strategy.is_empty() {
                log::debug!(
                    "core {} manual strategy settled: nothing selected",
                    moon_core::feed::core_label(core)
                );
            } else {
                log::info!(
                    "core {} manual strategy settled: on={} strategy={:?} id={}",
                    moon_core::feed::core_label(core),
                    state.on,
                    state.strategy,
                    state.id
                );
            }
            self.update_server(core, |server| server.manual_strategy = Some(state.clone()));
        }
        seeded
    }

    /// Return this core's effective manual-strategy state as `(enabled, id)`.
    ///
    /// The STORED terminal state is the only source. A core the terminal has not adopted yet reads
    /// as off rather than borrowing the core's own `use_manual_strategy`: Moonbot moves that field
    /// itself, and forwarding it put two real orders on a strategy nobody selected. Adoption is one
    /// coordination tick away (`tick_manual_strat_seed`).
    ///
    /// A confirmed snapshot with no Manual-kind strategy makes the state effectively disabled while
    /// preserving the selection; pending strategy data retains the raw state so TP/SL stay
    /// fail-safe. The id comes from `resolve_manual_selection`, so a pinned strategy keeps being
    /// found across a rename and the stored name takes over once the pin names nothing.
    ///
    /// Args:
    ///     core: Core whose effective manual-strategy state is requested.
    ///
    /// Returns:
    ///     Effective enabled state and resolved selected id.
    pub(crate) fn manual_strat_state(&self, core: CoreId) -> (bool, u64) {
        let core_data = self.session.store().core(core);
        // The STORED selection, and nothing else. Reading the core's live `manual_strategy_id` here
        // as a stand-in until the seed runs is what put two real orders on the wrong strategy on
        // 2026-09-01: Moonbot moves that field itself, so the terminal was faithfully forwarding a
        // choice that changed without anybody touching this screen. A core the terminal has not
        // adopted yet simply has the mode off here; `tick_manual_strat_seed` adopts it within a
        // tick of the core reporting enough to adopt.
        let raw = self
            .stored_manual_strat(core)
            .map(|stored| {
                (
                    stored.on,
                    core_data
                        .and_then(|data| resolve_manual_selection(&data.strategies, stored))
                        .unwrap_or(0),
                )
            })
            .unwrap_or((false, 0));
        core_data
            .map(|data| effective_manual_strat_state(raw, &data.strategies))
            .unwrap_or(raw)
    }

    /// Whether this core is configured to receive its strategy list at all.
    ///
    /// `FeedFlags::strategies` is a client-side filter: with it off the terminal never stores the
    /// core's strategies, so nothing that depends on resolving one against them can be answered.
    fn core_receives_strategies(&self, core: CoreId) -> bool {
        self.config
            .servers
            .iter()
            .find(|server| server.id == core)
            .is_some_and(|server| server.feed.strategies)
    }

    /// The manual selection this core carries that currently resolves to NOTHING, described for a
    /// log — a strategy this core cannot provide, and the one state an order is refused on.
    ///
    /// A core with no Manual strategies at all is not counted here to be broken — it reads as
    /// mode-off — and an order on it is an ordinary manual order; `manual_order_terms` warns
    /// separately when such a core still has its own switch on.
    pub(crate) fn manual_strat_unresolved(&self, core: CoreId) -> Option<&str> {
        // A core whose strategy feed is switched off never receives a list, so its selection can
        // never resolve. Refusing every order forever, with the whole MS cluster hidden for the
        // same reason, would leave nothing on screen able to clear the state — the trader would
        // have to edit the config by hand. Their own flag says they accept working without it.
        if !self.core_receives_strategies(core) {
            return None;
        }
        let stored = self.stored_manual_strat(core)?;
        // A core with no store entry at all knows LESS than one with an empty strategy list, which
        // the rule below already refuses on — so it resolves nothing and is judged the same way.
        let strategies = self
            .session
            .store()
            .core(core)
            .map(|data| data.strategies.as_slice())
            .unwrap_or_default();
        manual_selection_is_broken(strategies, stored).then(|| stored.strategy.trim())
    }

    /// The manual strategy the next order would actually be placed on, if any.
    ///
    /// The question every consumer outside the header itself is really asking. `manual_strat_state`
    /// answers what the SWITCH is set to, which stays true with nothing selected and while a stored
    /// selection has not resolved; in both of those the core would receive an ordinary order, so a
    /// toolbar that locked TP and S on the strength of the switch alone would be disabling the very
    /// controls whose values ride along with it.
    pub(crate) fn manual_strat_active(&self, core: CoreId) -> Option<u64> {
        let (on, id) = self.manual_strat_state(core);
        (on && id != 0).then_some(id)
    }

    /// Return whether the retained order-line snapshot shows panic sell armed for `(core, market)`.
    ///
    /// Args:
    ///     core: Core whose retained order lines are queried.
    ///     market: Market whose open order lines are queried.
    ///
    /// Returns:
    ///     `true` when an open retained order line has panic sell armed.
    fn panic_snapshot_armed(&self, core: CoreId, market: &str) -> bool {
        self.session.store().core(core).is_some_and(|data| {
            data.order_lines
                .iter_market(market)
                .any(|order| order.closed_ms.is_none() && order.panic_sell)
        })
    }

    /// Return whether panic sell is armed for `(core, market)` to highlight the Panic Sell button.
    ///
    /// A fresh local override takes precedence over the retained snapshot in both directions. This
    /// stays `&self` and non-mutating because render calls it through `backend.read(cx)`. It scans
    /// `panic_local` instead of probing by an owned `String`: the map is usually empty, so avoiding
    /// that per-render allocation is cheaper.
    ///
    /// Args:
    ///     core: Core that owns the market.
    ///     market: Market whose Panic Sell state is requested.
    ///
    /// Returns:
    ///     Effective armed state, using the snapshot when no fresh override exists.
    pub(crate) fn is_panic_armed(&self, core: CoreId, market: &str) -> bool {
        let local = self
            .panic_local
            .iter()
            .find(|((c, m), _)| *c == core && m.as_str() == market)
            .map(|(_, l)| (l.want, l.at.elapsed()));
        effective_panic_armed(local, || self.panic_snapshot_armed(core, market))
    }

    /// Toggle panic sell for a market, recording a symmetric optimistic override on acceptance.
    ///
    /// Returns whether the command was ACCEPTED, not the resulting armed state. The hotkey reaches
    /// this only through [`Backend::panic_sell_hotkey`], which uses that result after debouncing;
    /// the direct chart-button click is deliberately unguarded and ignores it.
    ///
    /// Args:
    ///     core: Core that receives the command.
    ///     market: Market to arm or disarm.
    pub(crate) fn toggle_panic_sell(&mut self, core: CoreId, market: String) -> bool {
        let key = (core, market.clone());
        let on = !self.is_panic_armed(core, &market);
        if let Err(error) = self.session.panic_sell_market(core, market, on) {
            log::warn!("panic sell market failed: {error:#}");
            return false;
        }
        self.panic_local.insert(
            key,
            PanicLocal {
                want: on,
                at: Instant::now(),
            },
        );
        self.panic_rev = self.panic_rev.wrapping_add(1);
        true
    }

    /// The only debounced Panic Sell entry point. It restarts the hotkey-only debounce window for
    /// absorbed and accepted presses, but leaves no window after a refused command. The direct
    /// chart-button path calls [`Backend::toggle_panic_sell`] instead.
    ///
    /// Args:
    ///     core: Core that receives the command.
    ///     market: Market to arm or disarm.
    ///
    /// Returns:
    ///     `true` when the command was accepted and the caller should repaint.
    pub(crate) fn panic_sell_hotkey(&mut self, core: CoreId, market: String) -> bool {
        let key = (core, market.clone());
        let now = Instant::now();
        if panic_press_absorbed(self.last_panic_press.get(&key).copied(), now) {
            // Every press restarts the window, absorbed or not: the absorbed press is itself the
            // evidence that the user is still inside the burst.
            self.last_panic_press.insert(key, now);
            return false;
        }
        let accepted = self.toggle_panic_sell(core, market);
        if accepted {
            // A refused command starts no window: nothing armed and nothing repainted, so the very
            // next press must be free to retry.
            self.last_panic_press.insert(key, now);
        }
        accepted
    }

    /// Reconcile every `PanicLocal` override against the core snapshot on the coordination tick.
    ///
    /// Buys four things: (1) an entry is dropped the moment the core AGREES, not merely when the
    /// user presses again -- stopping a transient agreement from being forgotten and turning the
    /// next intended re-arm into a disarm; (2) dropping an entry bumps `panic_rev`, so an EXPIRY
    /// repaints too -- without this a stale "Stop Panic" label could survive on a quiet market and a
    /// click on it would arm panic sell; (3) `last_panic_press` is pruned here, so the debounce map
    /// cannot grow for the process lifetime, and pruning only removes entries already outside the
    /// window so it can never change whether a press is absorbed; (4) this reuses the coordination
    /// loop that already runs at a fixed cadence whether or not anything happened, instead of
    /// `stop_overlay`'s per-press one-shot task, so it needs no task, no version stamp and no
    /// render-path work, and it covers the quiet-market case a render-side prune cannot reach.
    ///
    /// Returns whether any entry settled, so the caller knows whether to request a repaint.
    pub(crate) fn tick_panic_local(&mut self) -> bool {
        let settled: Vec<(CoreId, String)> = self
            .panic_local
            .iter()
            .filter(|((core, market), l)| {
                panic_local_settled(
                    l.want,
                    l.at.elapsed(),
                    self.panic_snapshot_armed(*core, market),
                )
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in &settled {
            self.panic_local.remove(key);
        }
        self.last_panic_press
            .retain(|_, at| at.elapsed() < PANIC_TOGGLE_DEBOUNCE);
        if settled.is_empty() {
            return false;
        }
        self.panic_rev = self.panic_rev.wrapping_add(1);
        true
    }

    /// Cancel pending buy orders across all markets for a core for the "cancel all buys" hotkey.
    ///
    /// The retained order snapshot supplies unique markets with a pending, non-short order whose
    /// job is not done. A `cancel_market_buys` request is sent for each market, and the return value
    /// is the number of requests accepted.
    pub(crate) fn cancel_all_buys_for_core(&self, core: CoreId) -> usize {
        let markets: Vec<String> = self
            .session
            .store()
            .core(core)
            .map(|cd| {
                let mut set = std::collections::BTreeSet::new();
                for o in &cd.orders {
                    if !o.is_short && o.pending && !o.job_is_done {
                        set.insert(o.market.clone());
                    }
                }
                set.into_iter().collect()
            })
            .unwrap_or_default();
        let mut n = 0;
        for m in markets {
            n += self.cancel_buy_orders(core, &m);
        }
        n
    }

    /// Return the market position side for `join_sells`, where `true` means short.
    ///
    /// The first matching order in the retained snapshot determines the side; absent a match, the
    /// position defaults to long.
    pub(crate) fn market_position_short(&self, core: CoreId, market: &str) -> bool {
        self.session
            .store()
            .core(core)
            .and_then(|cd| {
                cd.orders
                    .iter()
                    .find(|o| o.market == market)
                    .map(|o| o.is_short)
            })
            .unwrap_or(false)
    }

    /// Send one request to cancel pending market buys and report whether it was accepted.
    pub(crate) fn cancel_buy_orders(&self, core: CoreId, market: &str) -> usize {
        match self.session.cancel_market_buys(core, market.to_string()) {
            Ok(()) => {
                log::info!(
                    "cancel buy: requested market buys for core={} market={market}",
                    moon_core::feed::core_label(core)
                );
                1
            }
            Err(err) => {
                log::warn!(
                    "cancel buy failed: core={} market={market}: {err:#}",
                    moon_core::feed::core_label(core)
                );
                0
            }
        }
    }
}
