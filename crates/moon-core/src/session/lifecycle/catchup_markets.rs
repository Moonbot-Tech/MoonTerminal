//! The market each listed report row names, looked up once per coin per listing.
//!
//! A listing walks every closed trade of its core, and `MarketDataSource::resolve_market` searches
//! the provider's catalog for every row. On a 70-core station that is 427 thousand searches over
//! 10 thousand distinct coins (#1081): the same answer is computed about forty times. The answer
//! depends only on the row's coin and on inputs the listing reads from its core — the catalog's
//! version in the snapshot that answers, the feed's catalog revision, the account currency, the
//! exchange — so [`CoinMarkets`] keeps every answer, a "not found" included, until one moves.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Answers of one listing, valid for one set of the core's inputs.
///
/// Built per listing and dropped with it: nothing carries over to the next pass.
pub(super) struct CoinMarkets<K> {
    /// The inputs the stored answers were computed under; `None` before the first lookup.
    inputs: Option<K>,
    /// Coin as the row spells it → the market it resolved to, or `None` for "not found".
    answers: HashMap<String, Option<String>>,
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
            answers: HashMap::new(),
            searched: 0,
            spent: Duration::ZERO,
        }
    }
}

impl<K: PartialEq> CoinMarkets<K> {
    /// The market `coin` resolves to, searched only when this coin has no answer under the
    /// current inputs.
    ///
    /// A "not found" is kept like any other answer. That is sound only because the inputs carry
    /// the `markets_version` of the very snapshot the search runs on: a market that appears moves
    /// the version in the same snapshot that shows it, so a refusal cannot outlive the arrival of
    /// the market it missed. A list apply that only changes markets already known may leave the
    /// version where it was; the feed's own catalog revision in the inputs covers that case, a
    /// moment after the snapshot already holds the change.
    ///
    /// Args:
    ///     inputs: Reads everything besides the coin that the search depends on. Read on every
    ///         call; a change discards every stored answer.
    ///     coin: The row's coin token, the cache key.
    ///     search: The uncached lookup, on the snapshot `inputs` read its version from.
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
            self.answers.clear();
            self.inputs = Some(inputs);
        }
        let answer = match self.answers.get(coin) {
            Some(answer) => answer.clone(),
            None => {
                self.searched += 1;
                let answer = search();
                self.answers.insert(coin.to_string(), answer.clone());
                answer
            }
        };
        self.spent += started.elapsed();
        answer
    }
}

#[cfg(test)]
mod tests;
