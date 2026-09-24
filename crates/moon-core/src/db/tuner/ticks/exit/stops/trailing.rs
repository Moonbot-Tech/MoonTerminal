//! The trailing stop (`UseTrailing`) of the strategy window's "Stops" section.
//!
//! From the Moonbot documentation (moonbot.pro, Stops tab and "Trailing Stop") and the core's
//! answers of 2026-09-24 (`docs-internal/STRATEGY_FORMULAS/sell-common.md` §7):
//!
//! - The line follows the middle of the ticker's spread, `(bid + ask)/2`, and only rises: it
//!   stands `TrailingPercent` (negative) under the peak of that middle — a multiplication, the one
//!   distance of a short that is not a division off the buy.
//! - The peak steps at most once a second, and only when the middle is past it; with
//!   `TrailingEMA` N a step moves it `1/(N + 1)` of the way to the middle, so a single spike does
//!   not drag the line after it.
//! - Inside `StopLossDelay` the peak is followed, and at the delay's end it restarts at the
//!   middle of that moment.
//! - With `UseTakeProfit`, there is no line until the middle has gone `TakeProfit +
//!   |TrailingPercent|` past the buy; from then on the line stands no lower than the `TakeProfit`
//!   level, and it sells only while the middle is still beyond that level — a middle that fell
//!   through it in one tick sells nothing. A short's levels off the buy are divisions.
//! - It fires on the middle crossing the line, at the ticker's arrival; a stop past its own level
//!   at the same moment goes first ([`super::Stops`]).
//!
//! The tape carries no book, so the middle is read off the prints the way the book stop reads its
//! BID: the last taker sell stands for the bid, the last taker buy for the ask, sampled on the
//! ticker's clock ([`super::TICKER_PERIOD_MS`]).

use super::super::ExitParams;
use super::stop_level;
use crate::db::tuner::ticks::{Exit, ExitKind};
use crate::feed::types::{Side as TickSide, Tick};

/// The shortest spacing of two steps of the peak (the core's answer of 2026-09-24).
pub const PEAK_STEP_MS: i64 = 1_000;

/// The trailing line under `peak`: `TrailingPercent` off it — a multiplication for both sides,
/// mirrored for a short — and no lower than the take profit's level off the buy when
/// `UseTakeProfit` is on. What the walk sells on and what the verdict reads an archived line's
/// jump against.
///
/// Args:
///     peak: The peak of the spread's middle.
///     buy: The buy the take profit's level counts from.
///     params: The sell parameters: `trailing_pct`, `trailing_take_profit_pct`.
///     long: The trade's side.
pub fn trailing_level(peak: f64, buy: f64, params: &ExitParams, long: bool) -> f64 {
    line_of(
        peak,
        -params.trailing_pct.abs(),
        params
            .trailing_take_profit_pct
            .map(|tp| stop_level(buy, tp, long)),
        long,
    )
}

/// [`trailing_level`] over its parts: `pct` negative, `take` the take profit's level.
fn line_of(peak: f64, pct: f64, take: Option<f64>, long: bool) -> f64 {
    let line = if long {
        peak * (1.0 + pct / 100.0)
    } else {
        peak * (1.0 - pct / 100.0)
    };
    match take {
        Some(take) if (long && take > line) || (!long && take < line) => take,
        _ => line,
    }
}

/// The trailing stop as the walk runs it.
pub(super) struct Trailing {
    long: bool,
    /// `TrailingPercent`, negative: the line's distance under the peak.
    pct: f64,
    /// A step's share of the way from the peak to the middle, `1/(TrailingEMA + 1)`.
    weight: f64,
    /// The `TakeProfit` level off the buy, when `UseTakeProfit` is on.
    take: Option<f64>,
    /// How far past the buy the middle must go before the line appears, when `UseTakeProfit` is
    /// on: `TakeProfit + |TrailingPercent|`.
    activation: Option<f64>,
    /// Whether the line stands — always, without a take profit.
    active: bool,
    /// The end of `StopLossDelay`: the peak restarts there, and nothing fires before it.
    armed_at: i64,
    /// The fill: the peak is not followed before it.
    fill_ms: i64,
    /// Up to when the fact proves the position was not sold (`record::StopAnchor`): the peak is
    /// followed, nothing fires. `i64::MIN` when the walk is not the trade's own.
    quiet_until: i64,
    /// Whether the peak has restarted at the delay's end.
    restarted: bool,
    peak: Option<f64>,
    last_step: i64,
    bid: Option<f64>,
    ask: Option<f64>,
    next_sample: i64,
    period_ms: i64,
}

impl Trailing {
    /// The trade's trailing stop under `params`, `None` when `UseTrailing` is off.
    ///
    /// Args:
    ///     long: The trade's side.
    ///     buy: The fill price every level off the buy counts from.
    ///     armed_at: The end of `StopLossDelay`.
    ///     fill_ms: The fill.
    ///     first_ms: The first print the walk will feed; the ticker's clock is run back to it,
    ///         as the book stop's is.
    ///     params: The sell parameters.
    ///     quiet_until: See the field.
    pub(super) fn new(
        long: bool,
        buy: f64,
        armed_at: i64,
        fill_ms: i64,
        first_ms: i64,
        params: &ExitParams,
        quiet_until: i64,
    ) -> Option<Self> {
        if params.trailing_pct == 0.0 {
            return None;
        }
        let pct = -params.trailing_pct.abs();
        // A level off the buy: a long's product, a short's division (the core's answer 9) — the
        // stop's own conversion, boundary included.
        let take = params
            .trailing_take_profit_pct
            .map(|tp| stop_level(buy, tp, long));
        let activation = params
            .trailing_take_profit_pct
            .map(|tp| stop_level(buy, tp + pct.abs(), long));
        let period_ms = params.model.ticker_period_ms.max(1);
        let anchor = fill_ms + period_ms;
        let back = (anchor - first_ms).max(0) / period_ms;
        Some(Self {
            long,
            pct,
            weight: 1.0 / (params.trailing_ema.max(0.0).round() + 1.0),
            take,
            activation,
            active: take.is_none(),
            armed_at,
            fill_ms,
            quiet_until,
            restarted: false,
            peak: None,
            last_step: i64::MIN,
            bid: None,
            ask: None,
            next_sample: anchor - back * period_ms,
            period_ms,
        })
    }

    /// Whether `a` is past `b` in the profit direction: above for a long.
    fn beyond(&self, a: f64, b: f64) -> bool {
        if self.long { a > b } else { a < b }
    }

    /// The middle of the spread as the prints so far leave it.
    fn middle(&self) -> Option<f64> {
        match (self.bid, self.ask) {
            (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
            (one, other) => one.or(other),
        }
    }

    /// The line under the peak, never lower than the take profit's level ([`trailing_level`]).
    fn line(&self, peak: f64) -> f64 {
        line_of(peak, self.pct, self.take, self.long)
    }

    /// The ticker's arrivals strictly before `until`, and the first that crossed the line: the
    /// trailing stop, at that moment and the middle's price.
    pub(super) fn before(&mut self, until: i64) -> Option<Exit> {
        while self.next_sample < until {
            let at = self.next_sample;
            self.next_sample += self.period_ms;
            if at <= self.fill_ms {
                continue;
            }
            let Some(mid) = self.middle() else {
                continue;
            };
            if at >= self.armed_at && !self.restarted {
                self.restarted = true;
                self.peak = Some(mid);
                self.last_step = at;
            } else {
                match self.peak {
                    None => {
                        self.peak = Some(mid);
                        self.last_step = at;
                    }
                    Some(peak) if self.beyond(mid, peak) && at - self.last_step >= PEAK_STEP_MS => {
                        self.peak = Some(peak + (mid - peak) * self.weight);
                        self.last_step = at;
                    }
                    Some(_) => {}
                }
            }
            if at < self.armed_at {
                continue;
            }
            if !self.active {
                match self.activation {
                    Some(level) if !self.beyond(level, mid) => self.active = true,
                    _ => continue,
                }
            }
            let Some(peak) = self.peak else {
                continue;
            };
            let crossed = !self.beyond(mid, self.line(peak));
            let above_take = self.take.is_none_or(|take| self.beyond(mid, take));
            if crossed && above_take && at > self.quiet_until {
                return Some(Exit {
                    t_ms: at,
                    price: mid,
                    kind: ExitKind::Stop,
                });
            }
        }
        None
    }

    /// Read a print into the spread proxy: a taker sell prints at the bid, a taker buy at the ask.
    pub(super) fn see(&mut self, tick: &Tick) {
        let price = f64::from(tick.price);
        match tick.side {
            TickSide::Sell => self.bid = Some(price),
            TickSide::Buy => self.ask = Some(price),
        }
    }
}

#[cfg(test)]
mod tests;
