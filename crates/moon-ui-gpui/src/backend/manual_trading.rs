//! Manual-trading settings, order terms, and core command helpers for [`Backend`].

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use moon_core::config::{
    DEFAULT_ORDER_SIZES_USD, GroupExitSettings, GroupTradeSettings, MANUAL_STRAT_SLOTS, StratSlot,
    TakeProfitMode,
};
use moon_core::feed::{ClientSettingsEdit, FieldMask, StrategyRow};
use moon_core::market::MarketQuantityUnit;
use moon_core::session::CoreId;

use crate::Backend;

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

/// Exit values shown while a manual strategy owns them, seeded from that strategy.
///
/// Deliberately NOT written to the saved generation: switching charts, switching strategies, or
/// turning MS off must all show what the trader saved for that core or group, not what a strategy
/// left behind.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MsExitOverlay {
    /// Whether the stop is enabled (`UseStopLoss` at seed time).
    pub stop_on: bool,
    /// Stop loss as the toolbar states it: signed percent.
    pub stop_pct: f32,
    /// Take profit in percent (`SellPrice` at seed time).
    pub take_profit_pct: f64,
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
fn strat_field_value(
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
}

/// Resolve whether a raw manual-strategy state is usable with the retained strategy snapshot.
///
/// Args:
///     raw: State from the process-local override or retained ClientSettings.
///     strategies_rev: Retained snapshot revision; zero means no snapshot has arrived yet.
///     strategies: Rows in the retained strategy snapshot.
///
/// Returns:
///     Raw state while the snapshot is pending or contains a Manual-kind row; otherwise an
///     effective disabled state that preserves the selected id.
fn effective_manual_strat_state(
    raw: (bool, u64),
    strategies_rev: u64,
    strategies: &[StrategyRow],
) -> (bool, u64) {
    let confirmed_without_manual = strategies_rev != 0
        && !strategies
            .iter()
            .any(|strategy| strategy.kind_ordinal == MANUAL_STRATEGY_KIND);
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
            label: String::new(),
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

    /// Rename one slot's button. An empty label falls back to the strategy's own name.
    pub(crate) fn set_strat_slot_label(&mut self, core: CoreId, ix: usize, label: String) {
        self.update_strat_slot(core, ix, |slot| slot.label = label.clone());
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

    /// Seed the manual-strategy exit OVERLAY from the strategy just selected.
    ///
    /// An overlay rather than a write: while MS is on, the take profit and stop on screen belong to
    /// the SELECTED STRATEGY, and the group's (or the core's own) saved values must survive
    /// untouched underneath. Switch to another chart and those saved values are what show; switch
    /// MS off and they come back here too.
    ///
    /// Keyed by `(core, strategy)`, so returning to a strategy restores what was last used with it
    /// rather than re-reading the strategy over the trader's own adjustment.
    pub(crate) fn seed_exit_from_strategy(&mut self, core: CoreId, strategy_id: u64) {
        if strategy_id == 0 || self.ms_exit_local.contains_key(&(core, strategy_id)) {
            return;
        }
        let Some((stop_on, stop_pct, sell_pct)) = self.strategy_exit(core, strategy_id) else {
            return;
        };
        self.ms_exit_local.insert(
            (core, strategy_id),
            MsExitOverlay {
                stop_on,
                // The strategy stores a positive distance; the toolbar's stop is signed.
                stop_pct: stop_pct.map(|pct| -(pct.abs() as f32)).unwrap_or(0.0),
                take_profit_pct: sell_pct.unwrap_or(0.0),
            },
        );
    }

    /// The exit the toolbar must show and the order must use, while MS owns it.
    ///
    /// `None` whenever the saved generation is the one in force: MS off, no strategy selected, or
    /// no overlay seeded for it yet.
    pub(crate) fn manual_exit_overlay(&self, core: CoreId) -> Option<MsExitOverlay> {
        let (on, strategy_id) = self.manual_strat_state(core);
        if !on || strategy_id == 0 {
            return None;
        }
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
        let (manual_on, strategy_id) = self.manual_strat_state(core);
        if !manual_on || strategy_id == 0 {
            return false;
        }
        let mut current = self
            .ms_exit_local
            .get(&(core, strategy_id))
            .copied()
            .unwrap_or_default();
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
                current.take_profit_pct = pct;
            }
            // A fixed-sell preset is a take profit too, and the engaged one is what the order
            // sells at: keeping it out would leave the readout and the order disagreeing.
            ClientSettingsEdit::SelectFixedSellSlot(slot) if (1..=6).contains(&slot) => {
                current.take_profit_pct =
                    self.write_aligned_group_exit(group).fixed_sell_pcts[slot - 1];
            }
            _ => return false,
        }
        self.ms_exit_local.insert((core, strategy_id), current);
        true
    }

    /// One strategy's `UseStopLoss`, `StopLoss` and `SellPrice`, resolved through the schema like
    /// the header's own parameter summary — a field left at its default is absent from the
    /// snapshot.
    fn strategy_exit(
        &self,
        core: CoreId,
        strategy_id: u64,
    ) -> Option<(bool, Option<f64>, Option<f64>)> {
        let data = self.session.store().core(core)?;
        let row = data.strategies.iter().find(|s| s.id == strategy_id)?;
        let schema = data.schema.as_ref();
        let field = |name: &str| strat_field_value(row, schema, name);
        Some((
            field("UseStopLoss")
                .map(|v| {
                    matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "yes" | "true" | "1" | "on"
                    )
                })
                .unwrap_or(false),
            field("StopLoss").and_then(|v| v.trim().parse::<f64>().ok()),
            field("SellPrice").and_then(|v| v.trim().parse::<f64>().ok()),
        ))
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
        let (manual_on, strategy_id) = self.manual_strat_state(core);
        if !manual_on || strategy_id == 0 {
            return;
        }
        // What the trader SEES while MS is on, which is the overlay — the saved generation is not
        // on screen in this mode and must not be what the order gets.
        let (stop_on, stop_pct) = self
            .manual_exit_overlay(core)
            .map(|ms| (ms.stop_on, ms.stop_pct))
            .unwrap_or((exit.stop_loss_enabled, exit.stop_loss_pct));
        let level = stop_price(price, f64::from(stop_pct), short);
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
        let (manual_on, _) = self.manual_strat_state(core);
        let terminal_owns_sell = !manual_on || self.ignore_strat_sell_price(core).unwrap_or(false);
        // The take profit the trader SEES: the manual-strategy overlay while it is in force, the
        // saved generation otherwise.
        let take_profit_pct = self
            .manual_exit_overlay(core)
            .map(|ms| ms.take_profit_pct)
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
    }

    /// Store a process-lifetime local manual-strategy override for immediate feedback.
    pub(crate) fn set_manual_strat_local(&mut self, core: CoreId, on: bool, id: u64) {
        self.manual_strat_local.insert(core, (on, id));
    }

    /// Return the core's effective manual-strategy state as `(enabled, id)`.
    ///
    /// A local override takes precedence over the `ClientSettings` snapshot and remains until
    /// replaced or process exit; core echoes and command failures do not reconcile it. A confirmed
    /// snapshot with no Manual-kind strategy makes the state effectively disabled while preserving
    /// the selected id. Pending strategy data retains the raw state so TP/SL stay fail-safe. If
    /// neither state source exists, this returns `(false, 0)`.
    ///
    /// Args:
    ///     core: Core whose effective manual-strategy state is requested.
    ///
    /// Returns:
    ///     Effective enabled state and retained selected id.
    pub(crate) fn manual_strat_state(&self, core: CoreId) -> (bool, u64) {
        let core_data = self.session.store().core(core);
        let raw = self
            .manual_strat_local
            .get(&core)
            .copied()
            .or_else(|| {
                core_data
                    .and_then(|data| data.client_settings.as_ref())
                    .map(|settings| (settings.use_manual_strategy, settings.manual_strategy_id))
            })
            .unwrap_or((false, 0));
        core_data
            .map(|data| effective_manual_strat_state(raw, data.strategies_rev, &data.strategies))
            .unwrap_or(raw)
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
