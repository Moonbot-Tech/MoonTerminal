//! The KPI columns of the axis out of deals: the same `VarStats` shape every axis' matrix
//! draws, so the "Fact" of this axis and the "Fact" of the others are one number when the scope
//! is one scope.

use super::Deal;
use crate::db::metrics::Tally;
use crate::db::tuner::{VarStats, stats_from_tally};

/// The KPI of `deals` as the report has them — the "Fact" column.
///
/// Fed in the order given, which the reader keeps chronological (`read_deals`), because the
/// drawdown is a property of that order.
///
/// Args:
///     deals: The rows, chronological by close.
pub fn fact_stats<'a>(deals: impl IntoIterator<Item = &'a Deal>) -> VarStats {
    let (tally, spent) = fact_tally_of(deals);
    stats_from_tally(tally, spent)
}

/// The fact of `deals` as a tally — every deal's reported result, in the order given — and
/// the sum of their spend: what [`fact_stats`] draws, for a caller that compares tallies.
///
/// Args:
///     deals: The rows, chronological by close.
pub fn fact_tally_of<'a>(deals: impl IntoIterator<Item = &'a Deal>) -> (Tally, f64) {
    let mut tally = Tally::default();
    let mut spent = 0.0;
    for deal in deals {
        tally.push(deal.fact_pnl);
        spent += deal.spent;
    }
    (tally, spent)
}

/// The KPI of one variant out of its tally and the spend of the deals it traded — the shape
/// the matrix draws, from what the search and the variant columns compute.
///
/// Args:
///     tally: The variant's results, chronological.
///     spent: Sum of the entry sizes of the deals the variant traded.
pub fn stats_of(tally: Tally, spent: f64) -> VarStats {
    stats_from_tally(tally, spent)
}

#[cfg(test)]
mod tests;
