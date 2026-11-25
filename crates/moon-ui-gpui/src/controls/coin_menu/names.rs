//! What each connected core calls one coin.
//!
//! The menu is opened on ONE core's row, and its multi-core actions used to send that row's own
//! spelling to every target. Exchanges do not agree on spellings: Bybit's `1kBONK` is Binance's
//! `1000BONK` and Gate's `BONK`. A core matches its coin lists by exact text, so a foreign spelling
//! is accepted, stored, and never matches anything — the ban is on screen and not in effect.
//!
//! The one thing every core does agree on is the identity its own catalog resolves
//! ([`MarketLabel::identity`], the core's `market_currency_canonic`). [`CoinNames`] looks the
//! clicked coin up by that identity on every connected core once, when the menu is built, and the
//! permanent rows, the temporary rows and the "where is it listed" count all read the same answer.
//!
//! One name per core, by the rule the arbitrage column and the comparison tab already follow
//! ([`pick_market_for_identity`]): an undated contract over an expiry, the quote the core trades in
//! first. A core whose catalog does not carry the coin has no entry and is left out of every
//! multi-core action rather than sent a token it cannot match.

use std::collections::{BTreeMap, HashMap};

use moon_core::market::{MarketLabel, pick_market_for_coin, pick_market_for_identity};
use moon_core::session::CoreId;

use super::CoinMenuCtx;
use crate::Backend;

/// Markets one catalog is searched for under each query — the coin's identity, and the clicked
/// token where that is searched as well.
///
/// The search ranks exact before prefix before contains and the pick wants one specific market
/// among the hits: the undated contract in the quote the core trades in. A major coin is listed
/// against dozens of quotes and expiries on one exchange, and a hit cut off here reads as "this
/// core does not carry the coin". The depth is the one the chart uses to carry a market across
/// cores; the search runs once per catalog per menu, so the depth costs labels, not searches.
const LOOKUP_LIMIT: usize = 256;

/// One core's own spelling of the coin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CoreName {
    /// Market the core lists the coin under, the key of its TEMPORARY blacklist. Empty only for
    /// the clicked core when the click named no market (a wallet row).
    pub(crate) market: String,
    /// Token the core's PERMANENT coin lists are matched against — its `market_currency`.
    pub(crate) coin: String,
}

/// The clicked coin as every connected core spells it.
#[derive(Clone, Debug, Default)]
pub(crate) struct CoinNames {
    /// The clicked row's own token, which is what captions print.
    clicked: String,
    /// Cores that carry the coin. The clicked core is always present, verbatim.
    by_core: BTreeMap<CoreId, CoreName>,
}

impl CoinNames {
    /// Resolve the clicked coin on every connected core.
    ///
    /// The clicked core keeps exactly what was clicked. Every other core is searched by the coin's
    /// identity, once per market catalog: cores of one exchange share a catalog, so a fleet of two
    /// hundred cores on five exchanges costs five searches, not two hundred. A click the catalog
    /// gives no identity for is searched by its own token too, which doubles that at most.
    ///
    /// Args:
    ///     b: Live sessions and the market catalogs.
    ///     ctx: The clicked core, market and coin token.
    ///
    /// Returns:
    ///     One entry per core that carries the coin.
    pub(super) fn resolve(b: &Backend, ctx: &CoinMenuCtx) -> Self {
        Self::resolve_for(b, ctx, None)
    }

    /// Resolve only cores the menu will offer, while retaining the clicked core's own spelling.
    pub(super) fn resolve_for(b: &Backend, ctx: &CoinMenuCtx, targets: Option<&[CoreId]>) -> Self {
        let source = b.session.market_source();
        let clicked = match ctx.market.is_empty() {
            true => MarketLabel::default(),
            false => source.market_label(ctx.core, &ctx.market),
        };
        // The identity is the catalog's where the catalog holds the clicked market. A wallet row
        // names no market and a delisted one has left the catalog: there the clicked token itself
        // is all there is. It still serves as an identity for a plain coin (`BONK` is what every
        // catalog folds `1000BONK` to), and it is kept as a TOKEN as well, so a core that spells
        // the coin the same way — a second core of the same exchange — is found by that spelling
        // even when the token is one no identity equals (`1kBONK`).
        let catalog_identity = clicked.canonic.trim().to_ascii_uppercase();
        let (identity, token) = match catalog_identity.is_empty() {
            false => (catalog_identity, None),
            true => (
                moon_core::symbol::coin_match_key(&ctx.coin)
                    .trim()
                    .to_ascii_uppercase(),
                Some(ctx.coin.as_str()),
            ),
        };
        // A catalog search matches the literal text, so it is asked for everything the pick below
        // compares. The identity always: `BTC` is what reaches another exchange's `BTCUSDT` from a
        // clicked `BTC_0925`. The token as well where it reads differently from the identity, or
        // the same-spelling fallback would have nothing to find. Where the two differ only in
        // case the token alone is asked, in the case the catalogs themselves use.
        let queries: Vec<&str> = match token {
            None => vec![identity.as_str()],
            Some(token) if token.eq_ignore_ascii_case(&identity) => vec![token],
            Some(token) => vec![identity.as_str(), token],
        };

        let mut by_core = BTreeMap::new();
        by_core.insert(
            ctx.core,
            CoreName {
                market: ctx.market.clone(),
                coin: ctx.coin.clone(),
            },
        );
        if !identity.is_empty() {
            let mut hits: HashMap<CoreId, Vec<(String, MarketLabel)>> = HashMap::new();
            for session in b.session.sessions() {
                let core = session.id;
                if core == ctx.core || targets.is_some_and(|targets| !targets.contains(&core)) {
                    continue;
                }
                let catalog = source.provider_of(core).unwrap_or(core);
                let labelled = hits.entry(catalog).or_insert_with(|| {
                    let mut found: Vec<(String, MarketLabel)> = Vec::new();
                    for query in &queries {
                        for hit in source.labelled_search(core, query, LOOKUP_LIMIT) {
                            // The two queries overlap; one market must stay one candidate.
                            if !found.iter().any(|(name, _)| *name == hit.0) {
                                found.push(hit);
                            }
                        }
                    }
                    found
                });
                let traded = source.traded_quote(core).unwrap_or_default();
                if let Some(name) = pick_core_name(
                    labelled,
                    &identity,
                    &ctx.market,
                    &traded,
                    &clicked.quote,
                    token,
                ) {
                    by_core.insert(core, name);
                }
            }
        }
        Self {
            clicked: ctx.coin.clone(),
            by_core,
        }
    }

    /// The clicked row's own token.
    pub(crate) fn clicked(&self) -> &str {
        &self.clicked
    }

    /// `core`'s own spelling, or `None` when its catalog does not carry the coin.
    pub(crate) fn on(&self, core: CoreId) -> Option<&CoreName> {
        self.by_core.get(&core)
    }

    /// The token to look for in `core`'s lists: its own spelling, else the clicked one.
    ///
    /// Reading is more forgiving than writing. A core whose catalog has not arrived still holds
    /// lists, and the clicked spelling is the only candidate there is for them — the answer this
    /// gave before spellings were resolved. Writing never uses this: see [`Self::on`].
    pub(crate) fn coin_or_clicked(&self, core: CoreId) -> &str {
        self.on(core).map_or(&self.clicked, |name| &name.coin)
    }
}

/// Choose one core's market for the coin among that core's catalog hits.
///
/// The clicked market itself wins where the core lists it under the same identity: two cores of one
/// exchange spell it alike, and a ban placed on `BTCUSDC` should not land on `BTCUSDT` next door.
/// Otherwise the shared identity rule decides, preferring the quote this core trades in and, when
/// the core states none, the clicked market's.
///
/// `token` is the last resort for a click whose identity did not come from a catalog: a core that
/// spells the coin exactly as the clicked row does carries the same coin, whatever the identity
/// comparison could not establish. It is never a guess across spellings — `1kBONK` does not reach
/// a core that lists `1000BONK`.
///
/// Args:
///     labelled: `(market, label)` pairs from this core's catalog.
///     identity: Uppercase identity of the clicked coin.
///     clicked_market: Market the click named; empty for a wallet row.
///     traded_quote: Quote currency this core trades in; empty when it stated none.
///     clicked_quote: Quote currency of the clicked market.
///     token: The clicked token, when `identity` is only that token folded; `None` when the
///         identity is the catalog's own.
///
/// Returns:
///     The core's market and token, or `None` when its catalog does not carry the coin.
fn pick_core_name(
    labelled: &[(String, MarketLabel)],
    identity: &str,
    clicked_market: &str,
    traded_quote: &str,
    clicked_quote: &str,
    token: Option<&str>,
) -> Option<CoreName> {
    let same_market = labelled.iter().find(|(name, label)| {
        !clicked_market.is_empty()
            && name.eq_ignore_ascii_case(clicked_market)
            && label.identity().eq_ignore_ascii_case(identity)
    });
    let (market, label) = match same_market {
        Some(hit) => hit,
        None => {
            let quote = match traded_quote.is_empty() {
                true => clicked_quote,
                false => traded_quote,
            };
            let market = pick_market_for_identity(labelled, identity, quote)
                .or_else(|| token.and_then(|token| pick_market_for_coin(labelled, token, quote)))?;
            labelled.iter().find(|(name, _)| name == market)?
        }
    };
    // A token is what the permanent list is written with; a label without one names nothing.
    (!label.coin.trim().is_empty()).then(|| CoreName {
        market: market.clone(),
        coin: label.coin.clone(),
    })
}

#[cfg(test)]
mod tests;
