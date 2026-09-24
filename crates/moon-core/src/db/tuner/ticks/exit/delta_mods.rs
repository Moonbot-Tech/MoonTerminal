//! The strategy window's "Delta Modifiers" section: the `Add*Delta` family, `SellModifier` and
//! `MaxModifier` — one capped sum of the trade's deltas, spent on the take through
//! `SellModifier` and on the stop through `StopLossModifier` ([`super::stops::stop_pct`]).

use super::{ExitModel, ExitParams};
use crate::db::tuner::ticks::Deal;

impl ExitModel<'_> {
    /// What the delta modifiers add to the sell level, per cent — the capped sum times
    /// `SellModifier`, per the FAQ — as the deltas stood when the sell was placed, at `at_ms`.
    pub(super) fn modifier_pct(&self, deal: &Deal, at_ms: i64) -> f64 {
        modifier_sum(self.params, deal, at_ms) * self.params.sell_modifier
    }
}

/// The summed delta modifiers of a trade, capped: `Min(MaxModifier, Σ Pn · Dn)`.
///
/// One sum, two consumers — the sell level through `SellModifier` and the stop through
/// `StopLossModifier` — because the core computes it once and spends it on both (FAQ).
///
/// The core sums the deltas as they stand when it places the sell: on 121 of its printed sums
/// (2026-09-22) the report's snapshot, stamped at the entry order's placement for every kind but
/// MoonShot, drifted from the core's number the more, the longer the entry order waited. So the
/// sum is read at `at_ms` through the deal's live coin deltas ([`Deal::deltas_at`]); the BTC,
/// market, mark and price-bug terms stay the snapshot, and on the stop the verdict absorbs their
/// residual in its level tolerance (`verify::STOP_PRICE_TOLERANCE`).
///
/// Args:
///     params: The sell parameters, for the coefficients and the ceiling.
///     deal: The trade, for its deltas.
///     at_ms: When the sell was placed — the fill.
pub fn modifier_sum(params: &ExitParams, deal: &Deal, at_ms: i64) -> f64 {
    let sum = params.sell_mods.near_addition(&deal.deltas_at(at_ms));
    if params.max_modifier > 0.0 {
        sum.min(params.max_modifier)
    } else {
        sum
    }
}
