//! Pure strategy resolution and manual-order term conversions.

use super::types::{
    FIELD_USE_HOOK_STRATEGY, HOOK_STRATEGY_KIND, IGNORE_SELL_LOCAL_TTL, MANUAL_STRATEGY_KIND,
};
use moon_core::config::{GroupExitSettings, GroupTradeSettings, ManualStratState, TakeProfitMode};
use moon_core::feed::{ClientSettingsEdit, StrategyRow, percentage_take_price};
use std::time::Duration;

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
/// the core's own screen shows. A short sells BELOW its entry, at the core's division
/// ([`percentage_take_price`]).
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
pub(super) fn planned_sell_price(entry: f64, pct: f64, short: bool) -> Option<f64> {
    percentage_take_price(entry, pct, short)
}

/// Resolve the sell-price flag to show: a fresh request the core has not answered yet, or the
/// core's own value.
///
/// Settles the moment the core AGREES rather than only on the TTL, exactly like
/// `panic_override::panic_local_settled`: an override still asserting a value the core already holds would make
/// the next click — which asks for the opposite — look like the no-op this override exists to
/// prevent.
///
/// Args:
///     local: The requested value and how long ago it was queued, when one is in flight.
///     core_value: What the core's own configuration currently says.
///
/// Returns:
///     The value the checkbox must render.
pub(super) fn effective_ignore_sell(local: Option<(bool, Duration)>, core_value: bool) -> bool {
    match local {
        Some((want, age)) if want != core_value && age < IGNORE_SELL_LOCAL_TTL => want,
        _ => core_value,
    }
}

/// Apply one visible toolbar edit with the same wire quantization used by MoonProto.
pub(super) fn apply_group_exit_edit(
    exit: &mut GroupExitSettings,
    edit: ClientSettingsEdit,
) -> bool {
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
pub(super) fn update_group_trade_pair(
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
pub(super) fn usd_to_base_amount(usd: f64, rate: Option<f64>) -> Option<f64> {
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
    /// Quantity sent to the target core, in the ACCOUNT's balance currency for that market — which
    /// is the coin on a coin-margined market and the quote currency, so dollars, on a linear or
    /// spot one. See `manual_order_size_base`.
    ///
    /// NOT a coin amount on a linear market — reading it as one is what compared a dollar figure
    /// against a coin minimum. `manual_order_size_base` carries the measurement that settles it.
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
    /// what it is, and leaves Moonbot's own screen alone. `None` is the mode being off, and then
    /// the exit barrier switches the core's own mode off ahead of the order so it stays bare.
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
pub(super) fn stop_write_is_redundant(strategy: (bool, f64), visible: (bool, f32)) -> bool {
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
pub(super) fn exit_source(strategies: &[StrategyRow], hook: &str, strategy_id: u64) -> Option<u64> {
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
pub(super) fn is_manual(row: &StrategyRow) -> bool {
    row.kind_ordinal == MANUAL_STRATEGY_KIND
}

/// Whether a retained strategy row is a MoonHook — the only kind `UseHookStrategy` can name.
pub(super) fn is_hook(row: &StrategyRow) -> bool {
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
pub(super) fn signed_stop_pct(strategy_pct: Option<f64>) -> f32 {
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
pub(super) fn manual_selection_is_broken(
    strategies: &[StrategyRow],
    stored: &ManualStratState,
) -> bool {
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
pub(super) fn resolve_manual_selection(
    strategies: &[StrategyRow],
    stored: &ManualStratState,
) -> Option<u64> {
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
pub(super) fn manual_strat_seed(
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
pub(super) fn effective_manual_strat_state(
    raw: (bool, u64),
    strategies: &[StrategyRow],
) -> (bool, u64) {
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
