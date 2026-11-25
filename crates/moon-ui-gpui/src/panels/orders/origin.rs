//! Order-origin classification behind the Orders panel's All / Auto / Manual filter.
//!
//! An order counts as MANUAL when the trader placed it by hand. That is either an order with no
//! strategy at all (`strat_id == 0`), or one driven by a Manual-kind strategy: with the core's
//! "manual strategy" mode on, a hand-placed order carries that strategy's id, so `strat_id`
//! alone would file it under Auto.
//!
//! The kind is read from the row itself (`OrderRow::strat`), which moon-core fills at row-build
//! time from the protocol's own strategy snapshot. That snapshot exists whether or not the core
//! publishes its strategy LIST to the terminal (`feed.strategies`), so this never depends on the
//! Strategies window's data; and it is the very value the Strat column shows, so the filter and
//! the column cannot disagree about a row.

use moon_core::feed::strat_kind_name;

use crate::backend::MANUAL_STRATEGY_KIND;

/// Whether an order with this strategy id and strategy-kind name was placed by hand.
///
/// `strat_kind` is `OrderRow::strat`. While the strategy snapshot is unknown moon-core writes the
/// numeric id there instead of a kind name, which reads as Auto — the Strat column shows that same
/// number, and both settle on the core's next order batch.
pub(super) fn is_manual_order(strat_id: u64, strat_kind: &str) -> bool {
    strat_id == 0 || strat_kind == strat_kind_name(MANUAL_STRATEGY_KIND)
}

#[cfg(test)]
mod tests;
