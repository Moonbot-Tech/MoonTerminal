//! The protection set: every resolved closed trade the recorder knows, by id, with an index by
//! close time.
//!
//! The recorder expires this set against the retention edge on every 250 ms tick. A station with
//! tens of cores holds millions of trades here, and walking all of them four times a second cost
//! most of a CPU while nothing expired (#1081). The index lets the expiry visit only the trades
//! that closed before the edge, which is exactly the set the full walk used to filter for.

use std::collections::{BTreeSet, HashMap};

use super::{CatchupTrade, TradeId};

/// Resolved closed trades keyed by id, plus `(close_ms, core, rec_id)` of each, ordered.
#[derive(Default)]
pub(crate) struct Known {
    by_id: HashMap<TradeId, CatchupTrade>,
    /// One entry per `by_id` entry, holding that entry's current `close_ms`; every mutation
    /// below keeps the two in step.
    by_close: BTreeSet<(i64, u64, i64)>,
}

/// The index key of `trade`.
fn close_key(trade: &CatchupTrade) -> (i64, u64, i64) {
    (trade.close_ms, trade.id.core, trade.id.rec_id)
}

impl Known {
    /// Store `trade` under `id`, replacing an earlier entry and its index key.
    ///
    /// Args:
    ///     id: The trade's identity; always `trade.id`.
    ///     trade: The resolved trade.
    ///
    /// Returns:
    ///     The entry this one replaced.
    pub(crate) fn insert(&mut self, id: TradeId, trade: CatchupTrade) -> Option<CatchupTrade> {
        debug_assert_eq!(id, trade.id);
        let key = close_key(&trade);
        let old = self.by_id.insert(id, trade);
        if let Some(old) = &old {
            self.by_close.remove(&close_key(old));
        }
        self.by_close.insert(key);
        old
    }

    /// Forget `id` and its index key.
    pub(crate) fn remove(&mut self, id: &TradeId) -> Option<CatchupTrade> {
        let old = self.by_id.remove(id)?;
        self.by_close.remove(&close_key(&old));
        Some(old)
    }

    /// The trade stored under `id`.
    pub(crate) fn get(&self, id: &TradeId) -> Option<&CatchupTrade> {
        self.by_id.get(id)
    }

    /// Whether a trade is stored under `id`.
    #[cfg(test)]
    pub(crate) fn contains_key(&self, id: &TradeId) -> bool {
        self.by_id.contains_key(id)
    }

    /// How many trades are stored.
    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Every stored trade, in no particular order.
    pub(crate) fn values(&self) -> impl Iterator<Item = &CatchupTrade> {
        self.by_id.values()
    }

    /// Every stored id and trade, in no particular order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&TradeId, &CatchupTrade)> {
        self.by_id.iter()
    }

    /// Every stored id, in no particular order.
    pub(crate) fn keys(&self) -> impl Iterator<Item = &TradeId> {
        self.by_id.keys()
    }

    /// Every stored trade whose `close_ms` is strictly below `cutoff`, oldest first: the same
    /// trades a full walk filtered by `close_ms < cutoff` finds, without visiting the rest.
    pub(crate) fn closed_before(&self, cutoff: i64) -> impl Iterator<Item = &CatchupTrade> {
        self.by_close
            .range(..(cutoff, u64::MIN, i64::MIN))
            .filter_map(|&(_, core, rec_id)| self.by_id.get(&TradeId { core, rec_id }))
    }
}

#[cfg(test)]
mod tests;
