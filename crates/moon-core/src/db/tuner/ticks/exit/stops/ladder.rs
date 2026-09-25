//! The stop ladder of the strategy window's "Stops" section: the second stop (`UseSecondStop`,
//! `TimeToSwitch2Stop`, `PriceToSwitch2Stop`, `SecondStopLoss`) and the third (`UseStopLoss3`,
//! `TimeToSwitchStop3`, `PriceToSwitchStop3`, `StopLoss3`) — one rule, two steps.
//!
//! From the FAQ (1228–1235) and the Stops tab of moonbot.pro: "if after `TimeToSwitch2Stop`
//! seconds or more the price is above `PriceToSwitch2Stop`, the stop line moves to the second
//! stop's line" — the prices per cent off the buy. From the core's answers of 2026-09-24
//! (`docs-internal/STRATEGY_FORMULAS/sell-common.md` §7 and `STOPS_QUESTIONS_2.md`):
//!
//! - the condition is read off the BID of the REST ticker, a short's as well, and a short's per
//!   cents off the buy divide;
//! - the time counts from the SELL's placement (a MoonShot's after the full fill), its seconds
//!   rounded as usual — `TimeToSwitchStop3 = 60` holds from 60.5 s;
//! - `StopLossDelay` holds the ladder as it holds the stop: nothing is read before the delay's
//!   end, and the first read is the first check after it — a bid back under the switch price by
//!   then takes nothing;
//! - both steps are checked on every cycle, the second first; each is taken once and sets its own
//!   level as it is, not against the one in force, so the step taken LAST decides — the third on
//!   a cycle both are taken, and a second taken after the third overwrites it;
//! - the moved stop fires by the same check as the first one (`StopLossEMA` / `FastStopLoss`).
//!
//! From the report (6 364 ladder stops, 2026-09-24): the level a step moves to is its field off
//! the buy, `StopLossModifier` left out — 939 of 939 stops fired at a second stop's level print
//! exactly that; and every one fired with the price back under the switch price, so a step once
//! taken stays.
//!
//! The tape carries no book, so the bid is read the way the book stop reads it: the last taker
//! sell stands for it, sampled on the ticker's clock ([`super::TICKER_PERIOD_MS`]).

use super::super::level_off_buy;
use crate::feed::types::{Side as TickSide, Tick};

/// One step of the ladder, in the strategy's own units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StopStep {
    /// `TimeToSwitch2Stop` / `TimeToSwitchStop3` — whole seconds after the sell's placement; the
    /// core rounds the elapsed seconds, so the step may be taken from half a second past it.
    pub after_s: f64,
    /// `PriceToSwitch2Stop` / `PriceToSwitchStop3` — per cent off the buy the bid must be past.
    pub switch_pct: f64,
    /// `SecondStopLoss` / `StopLoss3` — per cent off the buy the stop moves to.
    pub level_pct: f64,
}

/// A step as the walk runs it: its moment, prices and whether it was taken.
struct Rung {
    from_ms: i64,
    switch_at: f64,
    level: f64,
    taken: bool,
}

/// The ladder as the walk runs it.
pub(super) struct Ladder {
    long: bool,
    rungs: Vec<Rung>,
    /// The bid as the prints so far leave it: the last taker sell.
    bid: Option<f64>,
    next_sample: i64,
    period_ms: i64,
    fill_ms: i64,
}

impl Ladder {
    /// The trade's ladder, `None` without a step.
    ///
    /// Args:
    ///     long: The trade's side.
    ///     fill_price: The buy every level counts from.
    ///     fill_ms: The buy's moment: nothing is read up to it.
    ///     placed_ms: The sell's placement, every step's time counts from.
    ///     armed_ms: The end of `StopLossDelay`: nothing is read before it.
    ///     first_ms: The first print the walk feeds; the ticker's clock is run back to it, as the
    ///         book stop's is, so both read the same arrivals.
    ///     period_ms: The ticker's period.
    ///     steps: The second stop and the third, those switched on.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        long: bool,
        fill_price: f64,
        fill_ms: i64,
        placed_ms: i64,
        armed_ms: i64,
        first_ms: i64,
        period_ms: i64,
        steps: &[Option<StopStep>],
    ) -> Option<Self> {
        let rungs: Vec<Rung> = steps
            .iter()
            .flatten()
            .map(|step| Rung {
                // Whole seconds (the site), the elapsed ones rounded (the core): "more than 60"
                // holds from 60.5 s — and never inside `StopLossDelay`.
                from_ms: (placed_ms + ((step.after_s.max(0.0).trunc() + 0.5) * 1000.0) as i64)
                    .max(armed_ms),
                switch_at: level_off_buy(fill_price, step.switch_pct, long),
                level: level_off_buy(fill_price, step.level_pct, long),
                taken: false,
            })
            .collect();
        if rungs.is_empty() {
            return None;
        }
        let period_ms = period_ms.max(1);
        let anchor = fill_ms + period_ms;
        let back = (anchor - first_ms).max(0) / period_ms;
        Some(Self {
            long,
            rungs,
            bid: None,
            next_sample: anchor - back * period_ms,
            period_ms,
            fill_ms,
        })
    }

    /// The ticker's arrivals strictly before `until`: every step whose time has come and whose
    /// switch price the bid is past is taken, and the level of the last one taken is where the
    /// stop now stands — `None` when no step was taken by `until`.
    pub(super) fn before(&mut self, until: i64) -> Option<f64> {
        let mut moved = None;
        while self.next_sample < until {
            let at = self.next_sample;
            self.next_sample += self.period_ms;
            if at <= self.fill_ms {
                continue;
            }
            let Some(bid) = self.bid else {
                continue;
            };
            let long = self.long;
            for rung in self.rungs.iter_mut().filter(|r| !r.taken) {
                let past = if long {
                    bid > rung.switch_at
                } else {
                    bid < rung.switch_at
                };
                if at >= rung.from_ms && past {
                    rung.taken = true;
                    moved = Some(rung.level);
                }
            }
        }
        moved
    }

    /// Whether a step is still to be taken.
    pub(super) fn pending(&self) -> bool {
        self.rungs.iter().any(|r| !r.taken)
    }

    /// Read a print: a taker sell stands for the bid.
    pub(super) fn see(&mut self, tick: &Tick) {
        if tick.side == TickSide::Sell {
            self.bid = Some(f64::from(tick.price));
        }
    }
}

#[cfg(test)]
mod tests;
