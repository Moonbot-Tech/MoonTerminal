//! The core's marked markets: what the chart's star shows, and what pressing it writes.
//!
//! The list is the CORE's (`trading.fav_markets` in its safe-share configuration — the same field
//! MoonBot's own star writes), not a preference of this terminal. That decides everything here: the
//! star reads what the core reported, a press is a DELTA the core's queue resolves against its own
//! snapshot at send time, and a core that has not reported its configuration yet has no star state
//! at all rather than a hollow one — see [`Backend::fav_market`].
//!
//! The write travels on the SLOW channel: a whole safe-share packet, behind the compact-settings
//! gate, with an echo and three attempts. A star drawn from the core's value alone therefore would
//! not move when pressed and would read as broken, so a press keeps a time-boxed local override,
//! reconciled on the coordination tick exactly as `panic_local` is.

use std::time::Instant;

use moon_core::feed::{fav_markets_has, fav_markets_list};
use moon_core::session::CoreId;

use super::Backend;

/// How long a queued mark asserts itself over the core's own value.
///
/// The same window `ignore_strat_sell_price` uses, and for the same reason — it covers the whole
/// slow-channel budget of three attempts at a ten-second echo — so the two overrides cannot
/// disagree about when a write has provably failed.
pub(crate) const FAV_LOCAL_TTL: std::time::Duration = super::manual_trading::IGNORE_SELL_LOCAL_TTL;

/// One in-flight mark or unmark of a market.
#[derive(Clone, Debug)]
pub(crate) struct FavLocal {
    /// Whether the trader asked for it to be marked.
    pub want: bool,
    /// When it was queued, for the TTL above.
    pub at: Instant,
}

/// Whether an override has done its work and may be dropped: the core agrees, or the window ran
/// out.
///
/// Pure so the rule can be tested without a backend, and shaped like `panic_local_settled` beside
/// it: settling on AGREEMENT is what stops a transient agreement being forgotten and turning the
/// next intended press into a repeat of the last one.
///
/// Args:
///     want: The state the press asked for.
///     age: How long ago it was queued.
///     listed: What the core's own list says now.
///
/// Returns:
///     Whether the override is finished with.
pub(crate) fn fav_local_settled(want: bool, age: std::time::Duration, listed: bool) -> bool {
    listed == want || age >= FAV_LOCAL_TTL
}

impl Backend {
    /// Whether this market is marked, as the star must SHOW it: a fresh local request while one is
    /// in flight, the core's own list otherwise.
    ///
    /// `None` before the core has reported its configuration at all — and that is not a hollow
    /// star, it is a star with nothing to say. Its button is disabled on that answer: a press would
    /// queue a state for a list this terminal has never seen.
    ///
    /// Args:
    ///     core: Core whose list is being read.
    ///     coin: The core's own `market_currency` for the coin.
    ///
    /// Returns:
    ///     Whether it is marked, or `None` while the core's configuration has not arrived.
    pub(crate) fn fav_market(&self, core: CoreId, coin: &str) -> Option<bool> {
        let listed = fav_markets_has(self.core_fav_markets(core)?, coin);
        let local = self
            .fav_local
            .get(&fav_key(core, coin))
            .map(|local| (local.want, local.at.elapsed()));
        Some(match local {
            Some((want, age)) if !fav_local_settled(want, age, listed) => want,
            _ => listed,
        })
    }

    /// Mark this market on that core, or unmark it when it is already marked.
    ///
    /// The direction is decided against what the star SHOWS ([`Self::fav_market`]), not against the
    /// core's not-yet-updated list: a second press inside the override window must undo the first,
    /// and a toggle resolved from the raw list would repeat it instead.
    ///
    /// Args:
    ///     core: Core that holds the list.
    ///     coin: The core's own `market_currency` for the coin.
    pub(crate) fn toggle_fav_market(&mut self, core: CoreId, coin: &str) {
        let Some(shown) = self.fav_market(core, coin) else {
            log::warn!(
                "favorite {coin}: core {} has not reported its configuration",
                moon_core::feed::core_label(core)
            );
            return;
        };
        self.set_fav_market(core, coin, !shown);
    }

    /// Put this market in the core's favourites, or take it out — an absolute state.
    ///
    /// What travels is that state, not a rebuilt list: see
    /// `moon_core::session::Session::set_fav_market` on why this one field is a delta.
    ///
    /// Args:
    ///     core: Core that holds the list.
    ///     coin: The core's own `market_currency` for the coin.
    ///     on: Whether it must be listed afterwards.
    pub(crate) fn set_fav_market(&mut self, core: CoreId, coin: &str, on: bool) {
        // Trimmed ONCE, here, and that spelling is what the wire, the override key and the queue's
        // own "already in that state" test all see: the writer trims too, so a market carrying
        // whitespace would otherwise file its override under a key nothing reads.
        let coin = coin.trim();
        if coin.is_empty() {
            return;
        }
        if let Err(error) = self.session.set_fav_market(core, coin.to_string(), on) {
            log::warn!(
                "favorite {coin} on core {} failed: {error:#}",
                moon_core::feed::core_label(core)
            );
            return;
        }
        self.fav_local.insert(
            fav_key(core, coin),
            FavLocal {
                want: on,
                at: Instant::now(),
            },
        );
        self.fav_rev = self.fav_rev.wrapping_add(1);
    }

    /// Reconcile every override against the core's own list on the coordination tick.
    ///
    /// The three things it buys are `tick_panic_local`'s three: an entry is dropped the moment the
    /// core AGREES rather than when the user presses again; dropping one bumps [`Self::fav_rev`],
    /// so an EXPIRY repaints too — without it a star left on a quiet market would keep asserting a
    /// write that never landed; and the map cannot grow for the process lifetime.
    ///
    /// Returns:
    ///     Whether anything settled, so the caller knows whether to request a repaint.
    pub(crate) fn tick_fav_local(&mut self) -> bool {
        let settled: Vec<(CoreId, String)> = self
            .fav_local
            .iter()
            .filter(|((core, coin), local)| {
                let listed = self
                    .core_fav_markets(*core)
                    .is_some_and(|text| fav_markets_has(text, coin));
                fav_local_settled(local.want, local.at.elapsed(), listed)
            })
            .map(|(key, _)| key.clone())
            .collect();
        if settled.is_empty() {
            return false;
        }
        for key in &settled {
            self.fav_local.remove(key);
        }
        self.fav_rev = self.fav_rev.wrapping_add(1);
        true
    }

    /// Every marked market this core reported, in its own order.
    ///
    /// The core's list verbatim, without the overrides [`Self::fav_market`] applies: a row in a
    /// list is a fact about the core, while a star is a control that has to answer its own press.
    ///
    /// Args:
    ///     core: Core whose list is being read.
    ///
    /// Returns:
    ///     The markets, or `None` while the core has reported no configuration — which is not the
    ///     same answer as an empty list, and the two must not be drawn alike.
    pub(crate) fn fav_markets_of(&self, core: CoreId) -> Option<Vec<String>> {
        Some(
            fav_markets_list(self.core_fav_markets(core)?)
                .into_iter()
                .map(str::to_string)
                .collect(),
        )
    }

    /// The core's raw list field, or `None` while it has reported no configuration.
    ///
    /// Borrowed, not cloned: this is read once per chart pane per frame, and the sibling override
    /// (`is_panic_armed`) documents avoiding exactly that per-render allocation.
    fn core_fav_markets(&self, core: CoreId) -> Option<&str> {
        Some(
            self.session
                .store()
                .core(core)?
                .core_config
                .as_ref()?
                .fav_markets
                .as_str(),
        )
    }
}

/// The key one market's override is held under.
///
/// Folded to upper case because MEMBERSHIP is decided case-insensitively: the dropdown's row
/// carries the core's spelling and the chart's star the market source's, and two spellings of one
/// coin must not hold two overrides whose winner is hash order.
fn fav_key(core: CoreId, coin: &str) -> (CoreId, String) {
    (core, coin.to_ascii_uppercase())
}

#[cfg(test)]
mod tests;
