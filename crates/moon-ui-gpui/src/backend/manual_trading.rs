//! Manual-trading settings, order terms, and core command helpers for [`Backend`].

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use moon_core::config::{
    DEFAULT_ORDER_SIZES_USD, GroupExitSettings, GroupTradeSettings, TakeProfitMode,
};
use moon_core::feed::{ClientSettingsEdit, StrategyRow};
use moon_core::session::CoreId;

use crate::Backend;

/// Ordinal of the Manual kind in the Moonbot strategy schema; see `strat_kind_name`.
pub(crate) const MANUAL_STRATEGY_KIND: u8 = 12;

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

    /// Select an F1-F6 USD-equivalent preset for one group.
    pub(crate) fn set_order_size_sel(&mut self, group: &str, ix: usize) {
        if ix >= 6 {
            return;
        }
        self.update_group_trade(group, |trade| trade.order_size_sel = ix);
    }

    /// Return the selected USD-equivalent order amount for one group.
    pub(crate) fn manual_order_size_usd(&self, group: &str) -> f64 {
        let (sizes, sel) = self.manual_order_size_state(group);
        sizes[sel]
    }

    /// Convert a target core's group-local USD amount into that core's base currency.
    pub(crate) fn manual_order_size_base(&self, core: CoreId) -> Option<(f64, f64)> {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server.id == core)?;
        let usd = self.manual_order_size_usd(&server.group);
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

    /// Resolve group-owned exit settings and either the visible USD size or a FireTest override.
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
        let exit = self.group_exit_settings(&server.group);
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

    /// Return one group-local USD-equivalent F1-F6 preset.
    pub(crate) fn order_size_value(&self, group: &str, ix: usize) -> f64 {
        let (sizes, _) = self.manual_order_size_state(group);
        sizes[ix.min(sizes.len().saturating_sub(1))]
    }

    /// Write one group-local USD-equivalent F1-F6 preset.
    pub(crate) fn set_order_size_value(&mut self, group: &str, ix: usize, value: f64) {
        if ix >= 6 || !(value.is_finite() && value > 0.0) {
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

    /// Return one visible S1-S6 percentage for a group.
    pub(crate) fn fixed_sell_pct(&self, group: &str, ix: usize) -> f64 {
        self.group_exit_settings(group).fixed_sell_pcts[ix.min(5)]
    }

    /// Apply a visible ClientSettings edit to the group's local source of truth.
    pub(crate) fn edit_group_exit(&mut self, group: &str, edit: ClientSettingsEdit) -> bool {
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

    /// Return whether panic sell is armed for `(core, market)` to highlight the Panic Sell button.
    ///
    /// The state is the union of the retained order-line snapshot and the process-local armed set.
    pub(crate) fn is_panic_armed(&self, core: CoreId, market: &str) -> bool {
        let snapshot_armed = self.session.store().core(core).is_some_and(|data| {
            data.order_lines
                .iter_market(market)
                .any(|order| order.closed_ms.is_none() && order.panic_sell)
        });
        if snapshot_armed {
            return true;
        }
        self.panic_armed.contains(&(core, market.to_string()))
    }

    /// Toggle panic sell for a market using the union of the order-line snapshot and local armed set.
    ///
    /// A successfully enabled local entry survives core updates until a later toggle removes it.
    pub(crate) fn toggle_panic_sell(&mut self, core: CoreId, market: String) -> bool {
        let key = (core, market.clone());
        let on = !self.is_panic_armed(core, &market);
        if let Err(error) = self.session.panic_sell_market(core, market, on) {
            log::warn!("panic sell market failed: {error:#}");
            return !on;
        }
        if on {
            self.panic_armed.insert(key);
        } else {
            self.panic_armed.remove(&key);
        }
        on
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
