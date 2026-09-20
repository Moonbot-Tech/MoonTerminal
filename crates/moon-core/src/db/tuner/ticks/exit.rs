//! The exit side: where the sell line stood after the fill, and which print crossed it. One
//! model for every strategy kind — after the entry filled, the exit of any strategy is a
//! function of the sell-order rules and the tape.
//!
//! Phase 1 carries the take-profit alone: `SellPrice` per cent above the fill, raised by
//! `MShotSellAtLastPrice` to the pre-spike price less `MShotSellPriceAdjust` (the FAQ: "the
//! 4-second-old ASK, i.e. before the spike"; the model reads the last print at least
//! [`PRE_SPIKE_LOOKBACK_MS`] before the fill, since the tape has no book). A position the take
//! never closed exits AS THE REPORT SAYS IT DID — [`ExitKind::Fact`] — which the caller shows as
//! "exit not modelled" rather than as a reproduction. The moving line (`PriceDown*`,
//! `SellLevel*`, `SellShot*`, `StopLoss`) is phase 2, checked against the archived Exit lines
//! before it is trusted.

use super::mshot::PRE_SPIKE_LOOKBACK_MS;
use super::{Deal, Exit, ExitKind, Fill, reaches};
use crate::feed::types::Tick;

/// Sell-line parameters, in the strategy's own units.
#[derive(Clone, Debug, PartialEq)]
pub struct ExitParams {
    /// `SellPrice` — take-profit distance from the fill, per cent.
    pub sell_price_pct: f64,
    /// `MShotSellAtLastPrice` — lift the take to the pre-spike price less the adjustment.
    pub sell_at_last_price: bool,
    /// `MShotSellPriceAdjust` — per cent SUBTRACTED from the pre-spike price.
    pub sell_price_adjust_pct: f64,
    /// `SellDelay` — milliseconds the core waits before placing the sell; prints inside the
    /// delay cannot fill it.
    pub sell_delay_ms: f64,
}

impl Default for ExitParams {
    fn default() -> Self {
        Self {
            sell_price_pct: 1.0,
            sell_at_last_price: false,
            sell_price_adjust_pct: 0.0,
            sell_delay_ms: 0.0,
        }
    }
}

/// The exit model over one parameter set.
pub struct ExitModel<'a> {
    params: &'a ExitParams,
}

impl<'a> ExitModel<'a> {
    pub fn new(params: &'a ExitParams) -> Self {
        Self { params }
    }

    /// The take-profit level for a fill: `SellPrice` off the fill, lifted to the pre-spike
    /// print less the adjustment when `MShotSellAtLastPrice` is on. Long above, short below.
    pub fn take_level(&self, deal: &Deal, ticks: &[Tick], fill: Fill) -> f64 {
        let by_pct = fill.price * self.params.sell_price_pct / 100.0;
        let mut take = if deal.is_long() {
            fill.price + by_pct
        } else {
            fill.price - by_pct
        };
        if self.params.sell_at_last_price {
            if let Some(pre) = pre_spike_price(ticks, fill.t_ms) {
                let adjust = pre * self.params.sell_price_adjust_pct / 100.0;
                take = if deal.is_long() {
                    take.max(pre - adjust)
                } else {
                    take.min(pre + adjust)
                };
            }
        }
        take
    }

    /// Replay the tape after the fill.
    ///
    /// Args:
    ///     deal: The report row — its side, and its own exit for the fallback.
    ///     ticks: The window's prints, ascending.
    ///     fill: The modelled (or factual) entry.
    pub fn exit(&self, deal: &Deal, ticks: &[Tick], fill: Fill) -> Exit {
        let take = self.take_level(deal, ticks, fill);
        let armed_at = fill.t_ms + self.params.sell_delay_ms.max(0.0) as i64;
        for tick in ticks {
            let t_ms = tick.time_ms as i64;
            // A print at the fill's own millisecond is the fill itself, not the exit.
            if t_ms <= armed_at || t_ms <= fill.t_ms {
                continue;
            }
            let price = f64::from(tick.price);
            // A long's take is reached from below by a print coming UP, a short's from above.
            if price > 0.0 && reaches(price, take, deal.is_short) {
                return Exit {
                    t_ms,
                    price: take,
                    kind: ExitKind::Take,
                };
            }
        }
        // No rule of this phase closed it. The report's exit is the honest stand-in while the
        // fill is the factual one; a modelled fill that differs from the fact makes the fact's
        // exit a guess — the caller keeps that distinction (`Verdict::exit` is `None` here).
        let tail = ticks.last().map(|t| t.time_ms as i64).unwrap_or(fill.t_ms);
        if deal.close_ms > fill.t_ms && deal.close_ms <= tail && deal.sell_price > 0.0 {
            return Exit {
                t_ms: deal.close_ms,
                price: deal.sell_price,
                kind: ExitKind::Fact,
            };
        }
        Exit {
            t_ms: tail,
            price: f64::NAN,
            kind: ExitKind::OpenAtWindowEnd,
        }
    }
}

/// The last print at least [`PRE_SPIKE_LOOKBACK_MS`] before `at_ms` — the FAQ's "price before
/// the spike".
pub fn pre_spike_price(ticks: &[Tick], at_ms: i64) -> Option<f64> {
    let cutoff = at_ms - PRE_SPIKE_LOOKBACK_MS;
    ticks
        .iter()
        .rev()
        .find(|t| (t.time_ms as i64) <= cutoff && t.price > 0.0)
        .map(|t| f64::from(t.price))
}
