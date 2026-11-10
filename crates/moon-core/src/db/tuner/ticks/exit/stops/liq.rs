//! The liquidation guards of the strategy window's "Stops" section: `StopAboveLiq` and
//! `DontSellBelowLiq`. Both keep a leveraged position's exit away from its liquidation price.
//!
//! - **StopAboveLiq** — "the percentage above the liquidation level at which to place the stop",
//!   0 off. A stop standing deeper than `liq · (1 + StopAboveLiq / 100)` is moved up to it (a
//!   short: down to `liq · (1 − StopAboveLiq / 100)` — a product, not a division off the buy),
//!   and never nearer the buy than [`STOP_HOLD_SHARE`]. The move is one-way: a stop already
//!   nearer the buy stays, and the ladder's rungs are held the same way.
//! - **DontSellBelowLiq** — "do not sell by stop below the liquidation price", isolated margin
//!   only: the panic sell and a PriceDown step are not priced past the liquidation level. A stop
//!   sold by a market order (`UseMarketOrder`) has no price to hold and is not bounded.
//!
//! Source: https://moonbot.pro/documentation/general-parameters-for-all-strategies/stops-tab
//!
//! The report keeps no liquidation price, only the leverage the trade ran at (`lev`,
//! [`Deal::leverage`]), so both levels are read off it:
//!
//! - `StopAboveLiq` follows the exchange's own liquidation price, estimated here for an isolated
//!   position without maintenance margin: `buy · (1 − 1 / lev)`, a short's `buy · (1 + 1 / lev)`.
//!   Where the hold at the buy decides the level, the estimate does not enter it; where the
//!   liquidation term decides, the level is as rough as the estimate — a cross-margin or merged
//!   position liquidates elsewhere.
//! - `DontSellBelowLiq` reads the core's own estimate, `buy / (1 + 0.95 / lev)` (a short's
//!   `buy · (1 + 0.95 / lev)`), above a leverage of 1. The margin mode is not on the record: the
//!   level is applied as if isolated, which can only leave a deeper sale unjudged, never judge it
//!   wrongly.
//!
//! Checked on the report (2026-11-10), 355 stops of strategies running `StopAboveLiq`: at 50 with
//! a leverage of 5–10 the core printed `StopLoss fixed` at 0.2 % off the buy on 41 of 41, longs
//! and shorts, against a `StopLoss` of −2.6 to −6.5; at 0.5–2 with a leverage of 4–5 the level
//! stayed the strategy's, as the estimate reads it, on all but 4 — positions liquidating nearer
//! than their leverage tells.

use super::super::ExitParams;
use crate::db::tuner::ticks::Deal;

/// How near the buy `StopAboveLiq` may bring the stop, as a share of the buy: 0.2 % on the
/// losing side.
const STOP_HOLD_SHARE: f64 = 0.002;

/// The share of `1 / leverage` the core's own liquidation estimate keeps for `DontSellBelowLiq`.
const LIQ_ESTIMATE_SHARE: f64 = 0.95;

/// A trade's liquidation guards, resolved to prices off its buy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiqGuard {
    long: bool,
    /// Where `StopAboveLiq` holds the stop; `None` when it is off or the leverage is unknown.
    stop_bound: Option<f64>,
    /// The level `DontSellBelowLiq` keeps a sale from passing; `None` when it does not apply.
    sale_floor: Option<f64>,
}

impl LiqGuard {
    /// The guards of a trade bought at `buy` under `params`.
    ///
    /// `StopAboveLiq` hangs on `UseStopLoss`, as every stop field does; `DontSellBelowLiq` is
    /// read on its own — it bounds a PriceDown step whether or not a stop is armed, which
    /// matches the core's rule: the strategy window greys the field out with the stop off, and
    /// the stored value keeps acting.
    ///
    /// Args:
    ///     params: The sell parameters.
    ///     deal: The trade, for its side and leverage.
    ///     buy: The buy every level counts from — a variant's own fill, not the fact's.
    pub fn new(params: &ExitParams, deal: &Deal, buy: f64) -> Self {
        let long = deal.is_long();
        let leverage = deal
            .leverage
            .filter(|lev| lev.is_finite() && *lev >= 1.0)
            .filter(|_| buy.is_finite() && buy > 0.0);
        Self {
            long,
            stop_bound: leverage
                .filter(|_| params.use_stop_loss)
                .and_then(|lev| stop_bound(buy, lev, params.stop_above_liq_pct, long)),
            sale_floor: leverage
                .filter(|_| params.dont_sell_below_liq)
                .and_then(|lev| sale_floor(buy, lev, long)),
        }
    }

    /// A stop level as `StopAboveLiq` leaves it: moved to the bound when it stands deeper,
    /// untouched otherwise.
    pub fn stop(&self, level: f64) -> f64 {
        match self.stop_bound {
            Some(bound) if self.long => level.max(bound),
            Some(bound) => level.min(bound),
            None => level,
        }
    }

    /// Where `StopAboveLiq` holds the stop, when it does.
    pub fn stop_bound(&self) -> Option<f64> {
        self.stop_bound
    }

    /// The level `DontSellBelowLiq` keeps a sale from passing, when it applies.
    pub fn sale_floor(&self) -> Option<f64> {
        self.sale_floor
    }
}

/// `StopAboveLiq`'s level off an isolated position's liquidation estimate, held at
/// [`STOP_HOLD_SHARE`] from the buy. `None` when the rule is off or the position has no
/// liquidation price to stand above — a long at a leverage of 1.
fn stop_bound(buy: f64, leverage: f64, above_pct: f64, long: bool) -> Option<f64> {
    if !(above_pct > 0.0) {
        return None;
    }
    let liq = if long {
        buy * (1.0 - 1.0 / leverage)
    } else {
        buy * (1.0 + 1.0 / leverage)
    };
    if !(liq.is_finite() && liq > 0.0) {
        return None;
    }
    Some(if long {
        (buy * (1.0 - STOP_HOLD_SHARE)).min(liq * (1.0 + above_pct / 100.0))
    } else {
        (buy * (1.0 + STOP_HOLD_SHARE)).max(liq * (1.0 - above_pct / 100.0))
    })
}

/// The core's own liquidation estimate, the level `DontSellBelowLiq` holds a sale at; `None` at
/// a leverage of 1 or under, where the rule is off.
fn sale_floor(buy: f64, leverage: f64, long: bool) -> Option<f64> {
    if leverage <= 1.0 {
        return None;
    }
    let reach = 1.0 + LIQ_ESTIMATE_SHARE / leverage;
    Some(if long { buy / reach } else { buy * reach })
}

#[cfg(test)]
mod tests;
