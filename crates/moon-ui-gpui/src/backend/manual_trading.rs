//! Manual-trading settings, order terms, and core command helpers for [`Backend`].

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use moon_core::config::{
    DEFAULT_ORDER_SIZES_USD, GroupExitSettings, GroupTradeSettings, TakeProfitMode,
};
use moon_core::feed::{ClientSettingsEdit, CoreConfigState, FieldMask, StrategyRow};
use moon_core::session::CoreId;
use moon_core::session::store::CoreData;

use crate::Backend;

/// Where the toolbar's visible manual-trading block (sizes, TP/SL, sell presets) is sourced from.
///
/// `GroupLocal` is reached only when the per-core opt-in is off or no chart core resolved — the
/// exact, unconditional path every group-window toolbar has always used. `Core` is reached
/// whenever the opt-in is on, regardless of whether the core has reported real values yet: an
/// enabled-but-`Awaiting` core never silently falls back to group-local numbers, which would look
/// exactly like the checkbox being off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ManualSource {
    GroupLocal,
    Core(CoreConfigState),
}

/// Combine two independent core-config arrivals (sizes from `core_config.manual`, exits from
/// `client_settings`) into the ONE freshness state the toolbar shows for the whole manual-trading
/// block. The weaker of the two always wins: a `Live` size beside an `Awaiting` exit is shown as
/// `Awaiting`, never as a live number sitting beside a stale one with no marker.
pub(crate) fn weaker_config_state(a: CoreConfigState, b: CoreConfigState) -> CoreConfigState {
    use CoreConfigState::{Awaiting, Live, Stale};
    match (a, b) {
        (Awaiting, _) | (_, Awaiting) => Awaiting,
        (Stale, _) | (_, Stale) => Stale,
        (Live, Live) => Live,
    }
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
    /// Base-currency quantity sent to the target core.
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

    /// Whether `core` reads its manual-trading order sizes, strategies, and exits from its own
    /// shared core config instead of the group-local settings.
    pub(crate) fn core_manual_enabled(&self, core: CoreId) -> bool {
        self.config
            .servers
            .iter()
            .find(|server| server.id == core)
            .is_some_and(|server| server.use_core_manual_config)
    }

    /// Set the per-core manual-config opt-in, mirroring the live edit into an open Settings preview
    /// exactly like [`Self::update_group_trade`] (contract: `docs/ARCHITECTURE.md`'s preview-mirror
    /// rule, `set_core_manual_enabled` never skips it).
    pub(crate) fn set_core_manual_enabled(&mut self, core: CoreId, on: bool) {
        if let Some(server) = self.config.servers.iter_mut().find(|s| s.id == core) {
            server.use_core_manual_config = on;
        }
        if let Some(preview_server) = self
            .preview
            .as_mut()
            .and_then(|preview| preview.servers.iter_mut().find(|s| s.id == core))
        {
            preview_server.use_core_manual_config = on;
        }
        self.config_dirty = true;
    }

    /// Resolve the core a group-local manual-trading write must instead reach, because the
    /// group's active trading core has opted into the core route. `None` means the write proceeds
    /// group-local exactly as before. This is the ONE choke point every group-local writer
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
            .filter(|&core| self.core_manual_enabled(core))
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
        let display_target = display_core.filter(|&core| self.core_manual_enabled(core));
        display_target == self.manual_write_core(group)
    }

    /// Return the core's retained manual order-size preset block and its freshness, tolerant of a
    /// stale-but-retained snapshot. `None` only while the core has never reported one at all.
    pub(crate) fn core_manual_sizes(
        &self,
        core: CoreId,
    ) -> Option<([f64; 6], usize, CoreConfigState)> {
        let data = self.session.store().core(core)?;
        let cfg = data.core_config.as_ref()?;
        Some((
            cfg.manual.order_sizes,
            cfg.manual.order_size_sel.min(5),
            data.core_config_state(),
        ))
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
        if let Some(core) = core.filter(|&core| self.core_manual_enabled(core)) {
            return match self.core_manual_sizes(core) {
                Some((sizes, sel, state)) => (sizes, sel, ManualSource::Core(state)),
                None => (
                    DEFAULT_ORDER_SIZES_USD,
                    2,
                    ManualSource::Core(CoreConfigState::Awaiting),
                ),
            };
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
        if let Some(core) = core.filter(|&core| self.core_manual_enabled(core)) {
            let data = self.session.store().core(core);
            return match data.and_then(|data| data.client_settings.as_ref()) {
                Some(settings) => (
                    settings.group_exit_settings(),
                    // The exit half's freshness comes from `CoreData` itself, which owns the
                    // latched `client_settings_stale` marker. A local approximation here would
                    // miss that latch and read `Live` for a reconnect that reached `Ready` again
                    // before a fresh snapshot arrived.
                    ManualSource::Core(
                        data.map_or(CoreConfigState::Awaiting, CoreData::client_settings_state),
                    ),
                ),
                None => (
                    GroupExitSettings::default(),
                    ManualSource::Core(CoreConfigState::Awaiting),
                ),
            };
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
        self.effective_group_exit(group, self.manual_write_core(group)).0
    }

    /// Select an F1-F6 USD-equivalent preset for one group, or the group's active core when its
    /// per-core opt-in is on: the choke point re-targets the write onto the core's own shared
    /// config rather than the group-local settings.
    pub(crate) fn set_order_size_sel(&mut self, group: &str, ix: usize) {
        if ix >= 6 {
            return;
        }
        if let Some(core) = self.manual_write_core(group) {
            let Some(data) = self.session.store().core(core) else {
                return;
            };
            // A `None` `core_config` REFUSES rather than inventing defaults for the other ~90
            // fields a whole-projection write would otherwise have to guess.
            let Some(mut cfg) = data.core_config.clone() else {
                return;
            };
            cfg.manual.order_size_sel = ix;
            if let Err(error) =
                self.session
                    .edit_core_config(core, cfg, FieldMask::EMPTY.with_order_size_sel())
            {
                log::warn!("set order size sel failed: core={core}: {error:#}");
            }
            return;
        }
        self.update_group_trade(group, |trade| trade.order_size_sel = ix);
    }

    /// Return the selected USD-equivalent order amount for one group.
    pub(crate) fn manual_order_size_usd(&self, group: &str) -> f64 {
        let (sizes, sel) = self.manual_order_size_state(group);
        sizes[sel]
    }

    /// Resolve the USD-equivalent order size for a real order about to reach `core`, refusing
    /// rather than guessing when the core route is on but has not yet reported a real value.
    ///
    /// Unlike [`Self::effective_order_size_state`] (which renders a neutral placeholder while
    /// `Awaiting`, correct for a toolbar that must draw six cells regardless), a placeholder here
    /// would size a real order from `DEFAULT_ORDER_SIZES_USD` while the user believes it is sized
    /// from their configured core. `None` therefore means "do not place this order", not "show 0".
    fn effective_order_size_usd_for_order(&self, group: &str, core: CoreId) -> Option<f64> {
        if self.core_manual_enabled(core) {
            let (sizes, sel, _state) = self.core_manual_sizes(core)?;
            Some(sizes[sel])
        } else {
            Some(self.manual_order_size_usd(group))
        }
    }

    /// Exit twin of [`Self::effective_order_size_usd_for_order`]: the core's own retained exit
    /// generation when the core route is on, refusing rather than sending a blank/default exit
    /// when the core has never reported `ClientSettings`.
    fn effective_group_exit_for_order(
        &self,
        group: &str,
        core: CoreId,
    ) -> Option<GroupExitSettings> {
        if self.core_manual_enabled(core) {
            let settings = self
                .session
                .store()
                .core(core)
                .and_then(|data| data.client_settings.as_ref())?;
            Some(settings.group_exit_settings())
        } else {
            Some(self.group_exit_settings(group))
        }
    }

    /// Convert a target core's effective USD amount — group-local, or the core's own when the
    /// per-core opt-in is on (display and order must never be able to disagree) — into that
    /// core's base currency.
    pub(crate) fn manual_order_size_base(&self, core: CoreId) -> Option<(f64, f64)> {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server.id == core)?;
        let usd = self.effective_order_size_usd_for_order(&server.group, core)?;
        let base = self.session.core_base(core)?;
        let size = usd_to_base_amount(
            usd,
            self.session.market_source().currency_usd_rate(core, base),
        )?;
        Some((size, usd))
    }

    /// Return the visible USD equivalent only when the target core can currently convert it.
    pub(crate) fn prospective_order_usd(&self, core: CoreId) -> Option<f64> {
        self.manual_order_size_base(core).map(|(_, usd)| usd)
    }

    /// Resolve the effective exit settings and either the visible USD size or a FireTest override.
    ///
    /// Both `exit` and the size (through [`Self::manual_order_size_base`]) come from the SAME
    /// effective resolver the display uses, gated on the per-core flag: a trader sizing from a
    /// number the order does not use is the worst failure this goal can ship.
    pub(crate) fn manual_order_terms(
        &self,
        core: CoreId,
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
                let (size, usd) = self.manual_order_size_base(core)?;
                (size, Some(usd))
            }
        };
        Some(ManualOrderTerms {
            size_base,
            size_usd,
            exit,
        })
    }

    /// Write one USD-equivalent F1-F6 preset, group-local or (per-core opt-in on) through the
    /// core's own shared config. The zero/non-finite guard above is client-side and applies
    /// identically to both routes, so a core-side `NotApplied` stays reserved for a genuine core
    /// refusal rather than a request already known to be bad.
    pub(crate) fn set_order_size_value(&mut self, group: &str, ix: usize, value: f64) {
        if ix >= 6 || !(value.is_finite() && value > 0.0) {
            return;
        }
        if let Some(core) = self.manual_write_core(group) {
            let Some(data) = self.session.store().core(core) else {
                return;
            };
            // A `None` `core_config` REFUSES — the same principle as the money path, and the only
            // way to avoid inventing defaults for the other ~90 fields.
            let Some(mut cfg) = data.core_config.clone() else {
                return;
            };
            cfg.manual.order_sizes[ix] = value;
            if let Err(error) =
                self.session
                    .edit_core_config(core, cfg, FieldMask::EMPTY.with_order_size_slot(ix))
            {
                log::warn!("set order size value failed: core={core}: {error:#}");
            }
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

    /// Apply a visible ClientSettings edit to the group's local source of truth, or (per-core
    /// opt-in on) to the group's active core's own exit generation through the EXISTING
    /// `sync_group_exit` command path — TP/SL/S1-S6 stay on the `ClientSettingsEdit` channel,
    /// unchanged. Refuses rather than guessing while the core has not reported `ClientSettings`
    /// yet: editing a synthesized default exit and pushing it would overwrite whatever the core
    /// actually has.
    pub(crate) fn edit_group_exit(&mut self, group: &str, edit: ClientSettingsEdit) -> bool {
        if let Some(core) = self.manual_write_core(group) {
            let Some(settings) = self
                .session
                .store()
                .core(core)
                .and_then(|data| data.client_settings.as_ref())
            else {
                return false;
            };
            let mut exit = settings.group_exit_settings();
            if !apply_group_exit_edit(&mut exit, edit) {
                return false;
            }
            return match self.session.sync_group_exit(core, exit) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("edit group exit failed: core={core}: {error:#}");
                    false
                }
            };
        }
        let mut exit = self.group_exit_settings(group);
        if !apply_group_exit_edit(&mut exit, edit) {
            return false;
        }
        self.update_group_trade(group, |trade| trade.exit = exit);
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

    /// Synchronize each group's complete local exit generation to every live core in that group.
    pub(crate) fn sync_group_manual_settings(&mut self) {
        let live_ids: HashSet<CoreId> = self
            .session
            .sessions()
            .iter()
            .map(|session| session.id)
            .collect();
        self.group_exit_sync
            .retain(|core, _| live_ids.contains(core));

        for server in self
            .config
            .servers
            .iter()
            .filter(|server| server.active && live_ids.contains(&server.id))
        {
            // A core reading its own manual config keeps its own exit generation. Pushing the
            // group's local exit here would overwrite the very core values the checkbox exists to
            // respect, within one tick, regardless of what the order path does.
            if server.use_core_manual_config {
                self.group_exit_sync.remove(&server.id);
                continue;
            }
            let exit = self.group_exit_settings(&server.group);
            let (revision, ready, matches) = self
                .session
                .store()
                .core(server.id)
                .map(|data| {
                    (
                        data.client_settings_rev,
                        data.status == moon_core::feed::ConnStatus::Ready,
                        data.client_settings
                            .as_ref()
                            .is_some_and(|settings| settings.group_exit_settings() == exit),
                    )
                })
                .unwrap_or((0, false, false));
            let generation = (exit, revision, ready);
            if matches {
                self.group_exit_sync.insert(server.id, generation);
                continue;
            }
            if self.group_exit_sync.get(&server.id) == Some(&generation) {
                continue;
            }
            if let Err(error) = self.session.sync_group_exit(server.id, exit) {
                log::warn!(
                    "group manual settings sync failed: core={} group={}: {error:#}",
                    server.id,
                    server.group
                );
                continue;
            }
            self.group_exit_sync.insert(server.id, generation);
        }
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
