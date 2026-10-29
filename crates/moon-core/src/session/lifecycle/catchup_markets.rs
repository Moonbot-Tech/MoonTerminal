//! The market each listed report row names, looked up once per coin per listing.
//!
//! A listing walks every closed trade of its core, and `MarketDataSource::resolve_market` searches
//! the provider's catalog for every row. On a 70-core station that is 427 thousand searches over
//! 10 thousand distinct coins (#1081): the same answer is computed about forty times. The answer
//! depends only on the row's coin and on inputs the listing reads from its core — the catalog
//! revision, the account currency, the exchange — so [`CoinMarkets`] keeps every market found
//! until one of those inputs moves.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Markets found by one listing, valid for one set of the core's inputs.
///
/// Built per listing and dropped with it: nothing carries over to the next pass.
pub(super) struct CoinMarkets<K> {
    /// The inputs the stored markets were found under; `None` before the first lookup.
    inputs: Option<K>,
    /// Coin as the row spells it → the market it resolved to.
    found: HashMap<String, String>,
    /// Lookups that actually ran the search.
    pub(super) searched: usize,
    /// Time spent answering, cached answers and input reads included.
    pub(super) spent: Duration,
}

impl<K> Default for CoinMarkets<K> {
    /// An empty set with no inputs, so the first lookup always searches.
    fn default() -> Self {
        Self {
            inputs: None,
            found: HashMap::new(),
            searched: 0,
            spent: Duration::ZERO,
        }
    }
}

impl<K: PartialEq> CoinMarkets<K> {
    /// The market `coin` resolves to, searched only when this coin has no market under the
    /// current inputs.
    ///
    /// A "not found" is never kept. The catalog revision moves when the feed thread drains the
    /// market-list event, a moment after the client applied the list, so for that moment a kept
    /// answer can lag the catalog. A lagging market lasts one listing; a lagging "not found"
    /// would latch the trade as unknown until the process restarts. Searching refused coins
    /// again costs little: on our cores they were 441 coins over 11 thousand of 210 thousand rows.
    ///
    /// Args:
    ///     inputs: Reads everything besides the coin that the search depends on. Read on every
    ///         call; a change discards every stored market.
    ///     coin: The row's coin token, the cache key.
    ///     search: The uncached lookup.
    ///
    /// Returns:
    ///     The same value `search` returns for this coin under these inputs.
    pub(super) fn resolve(
        &mut self,
        inputs: impl FnOnce() -> K,
        coin: &str,
        search: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        let started = Instant::now();
        let inputs = inputs();
        if self.inputs.as_ref() != Some(&inputs) {
            self.found.clear();
            self.inputs = Some(inputs);
        }
        let answer = match self.found.get(coin) {
            Some(market) => Some(market.clone()),
            None => {
                self.searched += 1;
                let answer = search();
                if let Some(market) = &answer {
                    self.found.insert(coin.to_string(), market.clone());
                }
                answer
            }
        };
        self.spent += started.elapsed();
        answer
    }
}

#[cfg(test)]
mod tests;
