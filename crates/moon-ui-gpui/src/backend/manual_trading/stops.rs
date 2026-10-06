//! Manual stop overlays and pending stop reconciliation.

use super::terms::{signed_stop_pct, stop_write_is_redundant};
use super::types::{ManualStop, PENDING_STOP_TTL, PendingStop, stop_price};
use crate::Backend;
use moon_core::config::{GroupExitSettings, ManualStratState};
use moon_core::session::CoreId;
use std::time::Instant;

impl Backend {
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
                short,
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
                        order.closed_ms.is_none()
                            && !pending.before_uids.contains(&order.uid)
                            // A stop is queued only by an IMMEDIATE order, so a row still waiting on
                            // its trigger is never that order. The core publishes a pending in the
                            // ordinary order stream the moment it is created, so without this a
                            // pending gesture fired within the TTL of an immediate click would take
                            // that order's absolute stop — computed for an entry the pending has not
                            // reached — and leave the order it was meant for unstopped.
                            //
                            // `pending` clears when the trigger FIRES, so this flag alone leaves a
                            // window; the side below narrows what is left, and it is the same
                            // pre-existing race any other new row in the market has always been in.
                            // The full answer is to pick by lowest `seq` among side-matching
                            // candidates — see the plan's debt list.
                            && !order.pending
                            && order.is_short == pending.short
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
}
