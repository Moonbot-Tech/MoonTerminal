//! The strategy window's "Delta Modifiers" section: the `Add*Delta` family, `SellModifier` and
//! `MaxModifier` — one capped sum of the trade's deltas, spent on the take through
//! `SellModifier` and on the stop through `StopLossModifier` ([`super::stops::stop_pct`]).
//!
//! **The core's own sum, off its record** ([`FactModifier`]). The core sums its LIVE deltas at
//! the moment it places the sell, and the report keeps one snapshot of them, stamped when the
//! entry order was placed — for a MoonHook a median 77 s before the fill, across the very dump
//! the hook buys. The model's deltas miss the core's sum by as much as the price moved in
//! between (2026-09-25, 723 archived hook takes: the snapshot's sum placed 329 of them within
//! 0.05 %, the live track 334). The record keeps the sum itself, spent: the take the core placed
//! and the stop level it printed are both `f(Σ)`, and read back they agree with each other (96
//! trades holding both: a median gap of 0.021 in Σ, 92 within the price step) and not with the
//! snapshot (0.105).

use super::sell_order::{archived_take, take_is_recorded};
use super::{ExitModel, ExitParams, level_off_buy};
use crate::db::tuner::ticks::mshot::Modifiers;
use crate::db::tuner::ticks::verify::{
    REASON_STOP, REASON_TAKE, reason_starts_with, stated_stop_level,
};
use crate::db::tuner::ticks::{Deal, PRICE_TOLERANCE};

impl ExitModel<'_> {
    /// What the delta modifiers add to the sell level, per cent — the capped sum times
    /// `SellModifier`, per the FAQ — as the deltas stood when the sell was placed, at `at_ms`.
    pub(super) fn modifier_pct(&self, deal: &Deal, at_ms: i64) -> f64 {
        modifier_sum(self.params, deal, at_ms) * self.params.sell_modifier
    }
}

/// The summed delta modifiers of a trade, capped: `Min(MaxModifier, |Σ Pn · Dn|)` — the core
/// takes the sum's magnitude and caps it when `MaxModifier` is above zero (the core developer via
/// LinKvo, 2026-09-24), so the sum is never negative: only a negative coefficient moves a level
/// toward the entry.
///
/// One sum, two consumers — the sell level through `SellModifier` and the stop through
/// `StopLossModifier` — because the core computes it once and spends it on both (FAQ).
///
/// The core sums the deltas as they stand when it places the sell: on 121 of its printed sums
/// (2026-09-22) the report's snapshot, stamped at the entry order's placement for every kind but
/// MoonShot, drifted from the core's number the more, the longer the entry order waited. So the
/// sum is read at `at_ms` through the deal's live coin deltas ([`Deal::deltas_at`]); the BTC,
/// market, mark and price-bug terms stay the snapshot. Where the record kept the core's own sum
/// ([`Deal::fact_modifier`]), what the deltas miss of it is added back — scaled to these
/// coefficients ([`FactModifier::residual_for`]), so the fact's own parameters read a sum that
/// places the core's level to the price step, and a variant's the model's sum moved by the same
/// miss.
///
/// Args:
///     params: The sell parameters, for the coefficients and the ceiling.
///     deal: The trade, for its deltas and the core's own sum.
///     at_ms: When the sell was placed — the fill.
pub fn modifier_sum(params: &ExitParams, deal: &Deal, at_ms: i64) -> f64 {
    let model = model_sum(&params.sell_mods, deal, at_ms);
    let sum = match deal.fact_modifier {
        Some(fact) => (model + fact.residual_for(&params.sell_mods)).max(0.0),
        None => model,
    };
    if params.max_modifier > 0.0 {
        sum.min(params.max_modifier)
    } else {
        sum
    }
}

/// The sum as the deltas give it, uncapped and without the record's correction.
fn model_sum(mods: &Modifiers, deal: &Deal, at_ms: i64) -> f64 {
    mods.near_addition(&deal.deltas_at(at_ms)).abs()
}

/// The weights the record's miss is spread over, one per `Add*` term: the coefficient's
/// magnitude. The price-bug term is left out — its contribution has a ceiling of its own
/// (`Modifiers::pricebug_term`) and it never moves off the snapshot, so no part of the miss is
/// its.
fn weights(mods: &Modifiers) -> [f64; 15] {
    [
        mods.add_5s,
        mods.add_1m,
        mods.add_5m,
        mods.add_15m,
        mods.add_1h,
        mods.add_3h,
        mods.add_24h,
        mods.add_mark,
        mods.add_btc_1h,
        mods.add_btc_5m,
        mods.add_btc_1m,
        mods.add_market_1h,
        mods.add_market_24h,
        mods.add_pump_1h,
        mods.add_dump_1h,
    ]
    .map(f64::abs)
}

/// The total weight of a family's terms ([`weights`]).
fn weight(mods: &Modifiers) -> f64 {
    weights(mods).iter().sum()
}

/// The core's own delta-modifier sum on one trade, read back off its record, kept as what the
/// model's deltas miss of it — see the module doc.
///
/// The miss is scaled for a variant by the total weight of its terms against the fact's
/// ([`weight`]). That reads the miss as the same number of per cent points on every term — the
/// move after the snapshot widens every range delta alike — so a variant that doubles every
/// coefficient doubles it, and one that zeroes them all leaves no sum at all, as the core would.
/// Magnitudes, so coefficients of both signs never cancel into a scale of nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FactModifier {
    /// The core's sum less the model's own at the fill, both under the fact's coefficients.
    residual: f64,
    /// [`weight`] of the fact's terms; above zero by construction ([`FactModifier::of`]).
    weight: f64,
}

impl FactModifier {
    /// The core's sum on a trade that ran `params`, off its record: the take the core placed —
    /// the archived Exit line's first point, or the sale itself on a trade its untouched take
    /// closed — and the stop level its reason printed (`StopLoss fixed: X`). A trade holding both
    /// reads the sums both allow; where they do not meet, the take's, as `SellModifier` is the
    /// larger coefficient on every live strategy that sets both, so one price step costs the
    /// take's reading less.
    ///
    /// A level on the record is a price on the market's grid, so it gives not one sum but the
    /// band of sums that round to it ([`Reading`]); the sum kept is the model's own where it lies
    /// inside that band, else the band's nearer edge. A coarse step or a small coefficient widens
    /// the band and leaves the model's sum alone — never amplifies the grid's rounding into a sum.
    ///
    /// A take-closed sale reads a limit's fill, and a fill a hair past its level reads a sum a
    /// hair high; the verdict's own fill tolerance is the same size.
    ///
    /// Returns:
    ///     `None` when the trade runs no `Add*` term the miss can sit on ([`weights`] — a sum of
    ///     the price-bug term alone stays the model's), or the record holds neither reading.
    ///
    /// Args:
    ///     deal: The trade, with its live deltas and its hook depth already on it
    ///         (`Deal::delta_track`, `record::placed_hook_depth`).
    ///     params: The sell parameters as of the buy.
    ///     exit_points: The archived Exit line, when the archive holds it.
    pub fn of(
        deal: &Deal,
        params: &ExitParams,
        exit_points: Option<&[(i64, f64)]>,
    ) -> Option<Self> {
        let weight = weight(&params.sell_mods);
        if weight <= 0.0 {
            return None;
        }
        // Both readings are the one sum: where both exist, the sums both allow; where they do
        // not overlap, the take's.
        let reading = match (
            take_reading(deal, params, exit_points),
            stop_reading(deal, params),
        ) {
            (Some(take), Some(stop)) => take.overlap(stop).unwrap_or(take),
            (take, stop) => take.or(stop)?,
        };
        let raw = model_sum(&params.sell_mods, deal, deal.buy_ms);
        let cap = params.max_modifier;
        let capped = if cap > 0.0 { raw.min(cap) } else { raw };
        let kept = reading.nearest(capped);
        // A sum read at the cap says only that the core's reached it: the miss is at least what
        // lifts the model's sum to the cap, and no more is known.
        let residual = if cap > 0.0 && kept >= cap {
            (cap - raw).max(0.0)
        } else {
            kept - raw
        };
        Some(Self { residual, weight })
    }

    /// What the model's sum misses under `mods`: the fact's miss, scaled by the terms' weight.
    pub fn residual_for(&self, mods: &Modifiers) -> f64 {
        self.residual * weight(mods) / self.weight
    }
}

/// The sums a level on the record is consistent with: every sum whose level rounds to it on the
/// market's grid, `[low, high]`, never below zero (the core takes the magnitude).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reading {
    low: f64,
    high: f64,
}

impl Reading {
    /// The band `sum_at` maps a level's rounding interval onto: the level give or take just
    /// under half a price step — the core placed it on the grid, the formula lands off it — or
    /// give or take the price tolerance where the grid is unknown.
    ///
    /// Returns:
    ///     `None` for a level the formula cannot explain: its whole band below zero by more than
    ///     the price tolerance's worth of sum.
    ///
    /// Args:
    ///     deal: The trade, for its price step.
    ///     level: The level on the record.
    ///     coefficient: What the sum is multiplied by in the level's formula.
    ///     sum_at: The sum a level stands for.
    fn of(deal: &Deal, level: f64, coefficient: f64, sum_at: impl Fn(f64) -> f64) -> Option<Self> {
        if !(level.is_finite() && level > 0.0 && coefficient.is_finite()) || coefficient == 0.0 {
            return None;
        }
        let half = match deal.tick.filter(|t| t.is_finite() && *t > 0.0) {
            Some(step) => HALF_STEP_SHARE * step,
            None => level * PRICE_TOLERANCE,
        };
        let (a, b) = (sum_at(level - half), sum_at(level + half));
        let (low, high) = (a.min(b), a.max(b));
        let slack = PRICE_TOLERANCE * 100.0 / coefficient.abs();
        if !(low.is_finite() && high.is_finite()) || high < -slack {
            return None;
        }
        Some(Self {
            low: low.max(0.0),
            high: high.max(0.0),
        })
    }

    /// The sums both bands allow, or `None` where they do not meet.
    fn overlap(self, other: Self) -> Option<Self> {
        let (low, high) = (self.low.max(other.low), self.high.min(other.high));
        (low <= high).then_some(Self { low, high })
    }

    /// The sum of the band nearest to `sum`.
    fn nearest(self, sum: f64) -> f64 {
        sum.clamp(self.low, self.high)
    }
}

/// Just under half a price step: a level this far from a grid price still rounds to it, with
/// room for the float arithmetic the replay places it by.
const HALF_STEP_SHARE: f64 = 0.49;

/// The sums the core's take carries: the take as placed against the same take before the
/// modifiers, `(take / base − 1) / SellModifier` for a long, `(base / take − 1) / SellModifier`
/// for a short, as `take_level` applies it. The base is the rule's at the fact's parameters — a
/// MoonHook's off the depth its take was placed at (`record::placed_hook_depth`).
///
/// `None` without `SellModifier`, for a take the rule does not place (MoonShot's lift to the
/// ask carries no modifier; Spread's level is recorded, not computed; a hook without its detect
/// depth), and without a reading.
fn take_reading(
    deal: &Deal,
    params: &ExitParams,
    exit_points: Option<&[(i64, f64)]>,
) -> Option<Reading> {
    let model = ExitModel::new(params);
    if params.sell_modifier == 0.0
        || params.sell_at_last_price
        || take_is_recorded(&deal.kind)
        || !model.take_known(deal)
    {
        return None;
    }
    let take_closed = deal.sell_reason.trim().eq_ignore_ascii_case(REASON_TAKE);
    let level = archived_take(exit_points).or_else(|| take_closed.then_some(deal.sell_price))?;
    let long = deal.is_long();
    let base = level_off_buy(deal.buy_price, model.base_take_pct(deal).max(0.0), long);
    if !(base.is_finite() && base > 0.0) {
        return None;
    }
    Reading::of(deal, level, params.sell_modifier, |take| {
        let shift_pct = if long {
            take / base - 1.0
        } else {
            base / take - 1.0
        } * 100.0;
        shift_pct / params.sell_modifier
    })
}

/// The sums the core's stop carries: `StopLoss − StopLossModifier · Σ` is the distance the
/// printed level stands at. `None` without `StopLossModifier`, without a stop, with a ladder
/// configured (its later steps print their own levels), and on a reason that is no stop or
/// prints no usable level.
fn stop_reading(deal: &Deal, params: &ExitParams) -> Option<Reading> {
    if params.stop_loss_modifier == 0.0
        || params.stop_loss_pct == 0.0
        || params.second_stop.is_some()
        || params.third_stop.is_some()
        || !reason_starts_with(deal.sell_reason.trim(), REASON_STOP)
    {
        return None;
    }
    let level = stated_stop_level(&deal.sell_reason)?;
    let buy = deal.buy_price;
    let long = deal.is_long();
    Reading::of(deal, level, params.stop_loss_modifier, |stop| {
        // The stop's per cent off the buy, negative on the losing side; a short's divides.
        let stop_pct = if long {
            stop / buy - 1.0
        } else {
            buy / stop - 1.0
        } * 100.0;
        (params.stop_loss_pct - stop_pct) / params.stop_loss_modifier
    })
}

#[cfg(test)]
mod tests;
