//! Opening a coin's chart from a surface that knows only its ticker.
//!
//! Three surfaces name a bare coin and have to turn it into a chart: the crowd's minute and coin
//! boards, and the card its rule fires. None of them has a core — the crowd trades on exchanges
//! this terminal may not even be connected to — so the resolution is the same question every time,
//! and it is asked here once rather than copied per caller.

use std::rc::Rc;

use gpui::{App, Entity, Pixels, Point, Window};
use moon_core::market::MarketLabel;
use moon_core::session::CoreId;
use moon_ui::{MoonContextMenuWindowExt as _, MoonWindowExt as _};

use crate::Backend;
use crate::controls::coin_search;

/// How wide the cores picker is, in pixels — the same width the news feed's picker uses.
const PICKER_WIDTH: f32 = 240.0;

/// One core, its name, and every market its catalog offered for the ticker.
///
/// Gathered before anything is chosen, because the identity rule picks among a core's markets
/// rather than taking the first the search ranked: a Bybit core lists BTC under ten expiries beside
/// the perpetual, and opening one of those would draw a chart whose price is not the one clicked.
type CoreMarkets = (CoreId, String, Vec<(String, MarketLabel)>);

/// One core of this group that trades exactly this coin.
pub(crate) struct CoinCore {
    /// The core.
    pub(crate) core: CoreId,
    /// The market it trades the coin as, which is what a chart is opened on.
    pub(crate) market: String,
    /// The core's own name, which is how a picker tells two of them apart.
    pub(crate) server: String,
}

/// Every core in this group that trades this coin, one market each.
///
/// Two hops, because a bare ticker is all a caller here has: the crowd publishes `BONK` and no
/// catalog, and an identity cannot be computed without one.
///
/// 1. **Seed by NAME.** The search is a substring match over each core's catalog, so `BONK` finds
///    `1kBONKPERP` and `BONK3L` alike; the first hit whose own coin folds to the ticker is the one
///    that anchors the answer, and its catalog gives us `MarketLabel::identity` — the core's own
///    `market_currency_canonic`.
/// 2. **Decide by IDENTITY.** Every hit is then judged against that identity rather than against
///    the name. This is what makes "whatever exchange it is on" true: Bybit spells the coin
///    `1kBONKPERP` and Binance spells it otherwise, no rule over the two names connects them, and
///    `match_key` deliberately keeps them apart because a coin list and a report must. It also
///    decides WHICH of a core's markets opens — the perpetual over ten expiries, a USD quote over
///    an exotic one — instead of whichever the search happened to rank first.
///
/// Both rules are the arbitrage column's (`panels::chart::arb_open`), and deliberately so: "the
/// same coin somewhere else" must not have two answers in one terminal.
///
/// A ticker no core spells the same way anywhere resolves to nothing, which is the honest answer
/// for a coin this terminal does not follow.
///
/// The order is the search's, so "the first core that trades it" means the same thing to every
/// caller — which is what a card with no core of its own draws its chart from.
///
/// Args:
///     backend: The terminal.
///     group: The window group that scopes the search.
///     coin: Ticker, as the crowd spells it.
pub(crate) fn cores_for(backend: &Backend, group: &str, coin: &str) -> Vec<CoinCore> {
    let wanted = moon_core::symbol::coin_match_key(coin);
    if wanted.is_empty() {
        return Vec::new();
    }
    // The WIDE cap, the one every "find me this exact instrument" caller uses.
    // `coin_search::search` keeps eight markets per core because a popup renders what it returns;
    // this caller filters by identity and shows one, so a core whose catalog holds nine
    // contains-matches for the ticker would have the exact one cut off the end — the core would
    // silently vanish from the picker and the card would lose its chart.
    let hits =
        coin_search::search_limited(backend, group, None, coin, coin_search::COIN_MATCH_LIMIT);
    // The anchor: a market this ticker names OUTRIGHT. A hit that merely contains the letters
    // (`BONK3L` for `BONK`) must not be allowed to define what the coin is.
    let identity = hits
        .iter()
        .find(|hit| hit.label.match_key() == wanted || hit.label.identity() == wanted)
        .map(|hit| hit.label.identity())
        .unwrap_or(wanted);
    let mut per_core: Vec<CoreMarkets> = Vec::new();
    for hit in hits {
        match per_core.iter_mut().find(|(core, _, _)| *core == hit.core) {
            Some((_, _, markets)) => markets.push((hit.market, hit.label)),
            None => per_core.push((hit.core, hit.server, vec![(hit.market, hit.label)])),
        }
    }
    per_core
        .into_iter()
        .filter_map(|(core, server, markets)| {
            // No quote preference: a coin clicked here came from a board that has no quote of its
            // own, so the rule falls through to "a USD stablecoin, then whatever is left".
            let market = moon_core::market::pick_market_for_identity(&markets, &identity, "")?;
            Some(CoinCore {
                core,
                market: market.to_string(),
                server,
            })
        })
        .collect()
}

/// Open this coin's chart on Main, the way every other coin in the terminal opens.
///
/// One core opens it outright; several offer a picker naming the core and its exchange; none does
/// nothing, which is the honest answer for a coin this terminal does not follow.
///
/// Args:
///     backend: The terminal.
///     group: The window group the chart opens in.
///     coin: Ticker clicked.
///     pos: Where, so a picker can be anchored to it.
///     window: Owning window, used to host that picker.
///     app: Application context.
///     opened: Run once the open has been AUTHORISED and requested, and not before. A picker is a
///         question, not an answer: a caller that dismisses its own row on the click would take
///         away the thing the menu is about while the menu is still open, and would dismiss it for
///         a coin no core trades — where the click does nothing at all.
///
///         "Authorised and requested" is the strongest thing this can promise. What it reports is
///         `Backend::open_on_main_if_authorized`, which queues the request; a core that has no live
///         session yet queues nothing further, and no chart appears. A caller must therefore treat
///         this as "the click was accepted", not as "a chart is on screen".
///
///         It is run DEFERRED, at the end of the current effect cycle. This function is called
///         from inside a click listener, which means the caller's own entity is leased for the
///         duration; a callback invoked synchronously could not touch it, and reaching for it
///         panics with "cannot update … while it is already being updated" rather than failing
///         quietly. Deferring returns the entity to the app first, so a caller may do the obvious
///         thing — dismiss its own row — without knowing any of this.
pub(crate) fn open(
    backend: &Entity<Backend>,
    group: &str,
    coin: &str,
    pos: Point<Pixels>,
    window: &mut Window,
    app: &mut App,
    opened: impl Fn(&mut App) + 'static,
) {
    let (rows, exchanges) = {
        let state = backend.read(app);
        let rows = cores_for(state, group, coin);
        // Exchange labels are only needed to tell two cores apart in the picker.
        let exchanges = if rows.len() > 1 {
            state.session.core_venues().clone()
        } else {
            std::collections::HashMap::new()
        };
        (rows, exchanges)
    };
    let opened = Rc::new(opened);
    match rows.len() {
        0 => {}
        1 => {
            let hit = rows.into_iter().next().expect("one row");
            let group = group.to_string();
            let done = backend.update(app, |backend, backend_cx| {
                // `false`: open without stealing focus, matching every other coin-nav site.
                let done =
                    backend.open_on_main_if_authorized(Some(&group), (hit.core, hit.market), false);
                if done {
                    backend_cx.notify();
                }
                done
            });
            if done {
                app.defer(move |app| opened(app));
            }
        }
        _ => {
            let items: Vec<moon_ui::MoonMenuItem> = rows
                .into_iter()
                .map(|hit| {
                    // Only a NAMEABLE venue earns a suffix: the row is there to tell two cores
                    // apart, and " · not identified" tells them apart from nothing.
                    let label = match exchanges.get(&hit.core).filter(|venue| venue.is_nameable()) {
                        Some(venue) => {
                            format!("{} · {}", hit.server, crate::controls::venue_label(venue))
                        }
                        None => hit.server,
                    };
                    let backend = backend.clone();
                    let group = group.to_string();
                    let core = hit.core;
                    let market = hit.market;
                    let opened = Rc::clone(&opened);
                    moon_ui::MoonMenuItem::with_key(format!("coin-open-core-{core}"), label)
                        .on_click(move |_, window, app| {
                            window.close_context_menu(app);
                            let done = backend.update(app, |backend, backend_cx| {
                                let done = backend.open_on_main_if_authorized(
                                    Some(&group),
                                    (core, market.clone()),
                                    false,
                                );
                                if done {
                                    backend_cx.notify();
                                }
                                done
                            });
                            if done {
                                let opened = Rc::clone(&opened);
                                app.defer(move |app| opened(app));
                            }
                        })
                })
                .collect();
            window.open_moon_context_menu(app, "coin-open-cores", pos, items, PICKER_WIDTH);
        }
    }
}

#[cfg(test)]
mod tests;
