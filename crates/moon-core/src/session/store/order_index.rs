//! Per-core indexes derived once from each published order-table batch.

use std::collections::{BTreeSet, HashMap};

use crate::feed::OrderRow;

/// Active rows by strategy and every distinct quote/market pair used by Mini App cache keys.
#[derive(Default)]
pub struct OrderIndex {
    by_strategy: HashMap<u64, Vec<usize>>,
    quote_markets: BTreeSet<(String, String)>,
}

impl OrderIndex {
    /// Index one complete order-table snapshot, retaining original row order within each strategy.
    pub(crate) fn new(orders: &[OrderRow]) -> Self {
        let mut index = Self::default();
        for (position, order) in orders.iter().enumerate() {
            index
                .quote_markets
                .insert((order.quote.clone(), order.market.clone()));
            if !order.job_is_done {
                index
                    .by_strategy
                    .entry(order.strat_id)
                    .or_default()
                    .push(position);
            }
        }
        index
    }

    /// Iterate active-order positions for each strategy in the order the core published them.
    pub fn strategies(&self) -> impl Iterator<Item = (u64, &[usize])> {
        self.by_strategy
            .iter()
            .map(|(strategy, rows)| (*strategy, rows.as_slice()))
    }

    /// Iterate every distinct raw quote and market pair, including terminal rows in Orders.
    pub fn quote_markets(&self) -> impl Iterator<Item = (&str, &str)> {
        self.quote_markets
            .iter()
            .map(|(quote, market)| (quote.as_str(), market.as_str()))
    }
}

#[cfg(test)]
mod tests;
