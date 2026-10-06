//! Manual order sizing and group trade edits.

use super::terms::{
    ManualOrderTerms, apply_group_exit_edit, planned_sell_price, update_group_trade_pair,
    usd_to_base_amount,
};
use crate::Backend;
use moon_core::config::{GroupExitSettings, GroupTradeSettings};
use moon_core::feed::ClientSettingsEdit;
use moon_core::market::MarketQuantityUnit;
use moon_core::session::CoreId;

impl Backend {
    /// Convert a target core's effective USD amount — group-local, or the core's own when the
    /// per-core opt-in is on (display and order must never be able to disagree) — into the unit
    /// the core places an order in ON THIS MARKET.
    ///
    /// ONE rule with two faces, because the wire field is a single `size`: **the core takes the
    /// account's balance currency for that market**, and which currency that is depends on the
    /// market:
    ///
    /// - **Inverse (coin-margined) markets are margined in the COIN**, so the size is `usd / price`
    ///   and `contract_size` does not enter it — measured on the 2026-08-31 QQ run, where a sent
    ///   `50` came back as a $4 990 order (50 SOL) rather than 50 contracts. The contract count is
    ///   how the venue states its LIMITS and how it reports positions, not how it takes an order.
    /// - **Linear and spot markets are margined in the quote currency**, so the size is
    ///   `usd / rate` — dollars on a USDT account. Confirmed against the core's own log on
    ///   2026-09-06: a sent `60` on `VELVETUSDT` was logged by the core as `OrderSize: 60.00$`.
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
        // On a Contracts market `price` is the divisor below, so a caller that cannot name the FILL
        // price gets a size for the price it did name. The pending-order path is the one such
        // caller: its price is a TRIGGER, and the core moves the real entry off it by its own
        // pending spread (`SharedConfig::pending_orders_spread`, shipped at 0.5%), so an
        // inverse-market pending deploys a notional off by that spread. The trigger is still the
        // closest estimate of the entry that exists on this side — the spread and its high-delta
        // rule live in the core — so this approximates rather than guesses.
        let source = self.session.market_source();
        // REFUSES on an unknown unit rather than picking one: guessing here is what sends a whole
        // account's worth of coin as a quantity.
        let rules = source.order_size_rules(core, market)?;
        let rate = match rules.unit {
            MarketQuantityUnit::Contracts(_) => (price.is_finite() && price > 0.0).then_some(price),
            MarketQuantityUnit::Coins => {
                source.currency_usd_rate(core, self.session.core_base(core)?)
            }
        };
        let size = usd_to_base_amount(usd, rate)?;
        // Below what the venue accepts the order cannot be placed at all, so refusing is the honest
        // answer: rounding down reaches zero, rounding up spends more than the trader asked for.
        //
        // Compared in DOLLARS, which is the one currency both units share — the trader's figure is
        // always dollars, while the wire quantity is coins on one market and contracts on another.
        // Checking the venue's raw QUANTITY here instead is what read `$200 < 1000 STONKS` as one
        // comparison and refused every manual order on Gate's `STONKS_USDT`. An absent floor never
        // blocks; `min_order_floor` argues why.
        if let Some(floor) = rules.min_order_usd.filter(|floor| usd < *floor) {
            log::warn!(
                "manual order refused: core={core} market={market} ${usd:.2} is below this \
                 market's minimum order value ${floor:.2} ({:?}, wire size {size:.8})",
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
    ) -> Option<crate::backend::ManualOrderTerms> {
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
        // With the mode off the order goes out under a zero StratID, which the core reads as
        // "whatever my own manual-strategy switch names" — Moonbot's screen or an older build can
        // have left that switch on. `sync_exit` below is what handles it: the exit barrier the
        // order waits behind switches the core's own mode off in the same packet as the exits
        // (`feed::live::client_settings`, `SettingsMutation::NoManualStrategy`), so the order the
        // terminal priced from the group generation is also the one the core prices that way.
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
    pub(super) fn update_group_trade(
        &mut self,
        group: &str,
        update: impl Fn(&mut GroupTradeSettings),
    ) {
        let live = &mut self.config.group_mut(group).trade;
        let preview = self
            .preview
            .as_mut()
            .map(|preview| &mut preview.group_mut(group).trade);
        update_group_trade_pair(live, preview, update);
        self.config_dirty = true;
    }
}
