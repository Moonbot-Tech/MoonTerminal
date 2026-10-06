//! Panic-sell commands, reconciliation, and buy cancellation.

use super::selectors::{cancel_all_buys_markets, ready_cores};
use super::types::{PANIC_TOGGLE_DEBOUNCE, panic_press_absorbed};
use crate::Backend;
use moon_core::session::CoreId;
use moon_core::session::panic_override::{
    PanicLocal, effective_panic_armed, panic_local_settled, panic_snapshot_armed,
};
use std::time::Instant;

impl Backend {
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
        effective_panic_armed(local, || {
            panic_snapshot_armed(self.session.store(), core, market)
        })
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
                    panic_snapshot_armed(self.session.store(), *core, market),
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

    /// Cancel every buy order across all markets of a core for the "cancel all buys" hotkey.
    ///
    /// The chart's Cancel Buy button, pressed once per market of the core: the retained order
    /// snapshot names the markets ([`cancel_all_buys_markets`]) and one `cancel_market_buys` request
    /// goes to each. The return value is the number of requests accepted.
    pub(crate) fn cancel_all_buys_for_core(&self, core: CoreId) -> usize {
        let markets = self
            .session
            .store()
            .core(core)
            .map(|cd| cancel_all_buys_markets(&cd.orders))
            .unwrap_or_default();
        let mut n = 0;
        for m in markets {
            n += self.cancel_buy_orders(core, &m);
        }
        n
    }

    /// Moonbot's "Cancel buys in all bots": [`Self::cancel_all_buys_for_core`] on every core that
    /// is online.
    ///
    /// Online means `ConnStatus::Ready` ([`ready_cores`]); a core still connecting, failed or
    /// disconnected is skipped rather than handed a command its feed cannot act on. No window
    /// group or active core is read, so the key does the same thing from every window that routes
    /// trading keys, Auto Overview included. The return value is the number of requests accepted.
    pub(crate) fn cancel_all_buys_all_cores(&self) -> usize {
        let (ready, offline) = ready_cores(self.session.store().statuses());
        let requests: usize = ready
            .iter()
            .map(|core| self.cancel_all_buys_for_core(*core))
            .sum();
        log::info!(
            "cancel buys on all cores: {requests} market request(s) to {} online core(s), {offline} offline skipped",
            ready.len()
        );
        requests
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
