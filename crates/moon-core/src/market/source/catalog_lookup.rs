//! One provider snapshot answering a run of market lookups.
//!
//! [`MarketDataSource::resolve_market`] read the catalog twice — the name search and the labels of
//! what it found — each from whatever snapshot was current at that moment. A [`CatalogLookup`]
//! holds one snapshot for both and carries that snapshot's `MarketsState::markets_version`, so a
//! cache keyed on the version can never pair an answer from a new list with an old version.

use moonproto::state::MarketsState;

use super::MarketDataSource;
use crate::feed::SharedMoonClient;
use crate::market::MarketLabel;
use crate::session::CoreId;
use crate::symbol::Exchange;

/// The catalog of one core's market-data provider, frozen at one snapshot.
pub struct CatalogLookup {
    snapshot: moonproto::MoonClientSnapshot,
    exchange: Exchange,
}

impl MarketDataSource {
    /// Freeze the catalog `core` reads its markets from, for lookups that must agree.
    ///
    /// Returns:
    ///     The lookup, or `None` while the core has no provider, client or snapshot yet.
    pub fn catalog_lookup(&self, core: CoreId) -> Option<CatalogLookup> {
        let (client, exchange) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            let client = inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)?;
            (client, inner.exchange_of_provider(provider))
        };
        Some(CatalogLookup {
            snapshot: client.snapshot_versioned()?,
            exchange,
        })
    }
}

impl CatalogLookup {
    /// MoonProto's version of the market-name universe in this snapshot.
    ///
    /// It moves when new names become visible and may move on a full-list apply; it does not
    /// move on prices, tags or trading status. Comparable only within one provider client.
    pub fn markets_version(&self) -> u64 {
        self.snapshot.markets().markets_version()
    }

    /// The catalog-verified market a report row's `coin` names, from this snapshot alone.
    ///
    /// The rules are [`MarketDataSource::resolve_market`]'s, which calls this.
    ///
    /// Args:
    ///     quote: The core's quote setting (`ServerConfig::market`).
    ///     traded_quote: The core's account currency, when it has stated one.
    ///     coin: Historical base coin or full market value from the row.
    ///
    /// Returns:
    ///     Catalog-verified market, or `None` when this catalog cannot resolve it.
    pub fn resolve_market(
        &self,
        quote: &str,
        traded_quote: Option<String>,
        coin: &str,
    ) -> Option<String> {
        let exchange = self.exchange;
        let quote = crate::symbol::resolve_quote_on(quote, exchange);
        // A complete market is one carrying a HIP-3 DEX prefix or THIS CORE'S quote. Deliberately
        // not "carries any recognized quote": a coin whose own name ends in one — `WBTC`,
        // `STETH`, `PYUSD` — would then be taken for a finished market and never get its quote
        // appended.
        let parsed = crate::symbol::parse::split_market(coin, exchange);
        let already_full = parsed.dex.is_some()
            || (!quote.is_empty() && parsed.quote.eq_ignore_ascii_case(&quote));
        let candidate = if already_full || quote.is_empty() {
            coin.to_string()
        } else {
            crate::symbol::parse::market_names_for(coin, &quote, exchange)
                .next()
                .unwrap_or_else(|| coin.to_string())
        };
        let catalog = self.snapshot.markets();
        let universe: Vec<String> = catalog
            .search(coin, 32)
            .into_iter()
            .map(|handle| handle.name().to_string())
            .collect();
        if universe.is_empty() {
            return None;
        }
        let labelled: Vec<(String, MarketLabel)> = universe
            .iter()
            .map(|market| (market.clone(), label_in(Some(catalog), market, exchange)))
            .collect();
        let traded_quote = traded_quote.unwrap_or_else(|| quote.clone());
        if let Some(name) = crate::market::pick_market_for_coin(&labelled, coin, &traded_quote) {
            return Some(name.to_string());
        }
        // The historical format where the stored value is already a full market name.
        universe
            .iter()
            .find(|market| market.eq_ignore_ascii_case(&candidate))
            .cloned()
    }

    /// The names a report row's `coin` would carry on this exchange, for a coin this catalog no
    /// longer lists — a market delisted after the trade, whose prints the terminal may still
    /// hold under the name it was filed with.
    ///
    /// These are UNVERIFIED spellings: the caller must confirm one against something that only
    /// a once-listed market can have (the local tape store), never use one as is. An empty
    /// catalog is catalog-not-ready, not "everything delisted", and answers nothing.
    ///
    /// Args:
    ///     quote: The core's quote setting (`ServerConfig::market`).
    ///     traded_quote: The core's account currency, when it has stated one.
    ///     coin: Historical base coin or full market value from the row.
    ///
    /// Returns:
    ///     The candidate market names, most specific first; empty while the catalog is not in.
    pub fn unlisted_spellings(
        &self,
        quote: &str,
        traded_quote: Option<String>,
        coin: &str,
    ) -> Vec<String> {
        if self.snapshot.markets().market_count() == 0 {
            return Vec::new();
        }
        spellings_on(self.exchange, quote, traded_quote, coin)
    }
}

/// How `coin` is spelled into a market on `exchange`, by the exchange's naming rules alone.
///
/// The account currency wins over the quote setting, as in [`CatalogLookup::resolve_market`]:
/// the setting defaults to `BTCUSDT` and is not what the core trades in. A value that already
/// is a market of that quote (or carries a HIP-3 DEX prefix) is its own only spelling.
pub(super) fn spellings_on(
    exchange: Exchange,
    quote: &str,
    traded_quote: Option<String>,
    coin: &str,
) -> Vec<String> {
    let quote = traded_quote
        .filter(|traded| !traded.is_empty())
        .unwrap_or_else(|| crate::symbol::resolve_quote_on(quote, exchange));
    let parsed = crate::symbol::parse::split_market(coin, exchange);
    if quote.is_empty() || parsed.dex.is_some() || parsed.quote.eq_ignore_ascii_case(&quote) {
        return vec![coin.to_string()];
    }
    crate::symbol::parse::market_names_for(coin, &quote, exchange).collect()
}

/// How `market` is named to the user, from `catalog` first and from its name otherwise.
///
/// The one copy of [`MarketDataSource::market_labels`]'s per-market rule, shared with
/// [`CatalogLookup::resolve_market`].
pub(super) fn label_in(
    catalog: Option<&MarketsState>,
    market: &str,
    exchange: Exchange,
) -> MarketLabel {
    catalog
        .and_then(|catalog| catalog.get(market))
        .and_then(|handle| {
            handle.with(|m| {
                let coin = m.market_currency.trim();
                (!coin.is_empty()).then(|| {
                    let quote = m.base_currency.trim().to_ascii_uppercase();
                    // What the NAME alone says, read ONCE: both fields below fall back to it, and
                    // this runs per market of a search result.
                    let from_name = MarketLabel::from_name(market, exchange);
                    MarketLabel {
                        coin: coin.to_string(),
                        // The core's own cross-exchange identity, carried whole. See
                        // `MarketLabel::canonic` for why it is not derived here.
                        canonic: m.market_currency_canonic.trim().to_string(),
                        // A COIN-M contract reports no base currency at all, so its quote comes
                        // from the name (`BTCUSD_PERP` → `USD`); without this the pair collapses
                        // to a bare coin.
                        quote: match quote.is_empty() {
                            true => from_name.quote,
                            false => quote,
                        },
                        // The expiry the NAME carries, which the token often does not: Bybit's
                        // dated contract is the market `BTCUSDT-25SEP26` whose token is a flat
                        // `BTCUSDT25SEP`, and reading only the token made every one of its ten
                        // expiries look like a perpetual. COIN-M spells it inside the token
                        // (`BTC_0925`) and answers from there, so this fills a gap rather than
                        // competing.
                        contract: from_name.contract,
                    }
                })
            })
        })
        // A market the catalog does not hold — delisted under an open order, or a core that has
        // not sent its list yet.
        .unwrap_or_else(|| MarketLabel::from_name(market, exchange))
}

#[cfg(test)]
mod tests;
