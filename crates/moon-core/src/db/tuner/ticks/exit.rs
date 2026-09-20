//! The exit side: where the sell line stood after the fill, and which print crossed it. One
//! model for every strategy kind — after the entry filled, the exit of any strategy is a
//! function of the sell-order rules and the tape.
//!
//! The take-profit is `SellPrice` per cent above the fill, raised by `MShotSellAtLastPrice` to
//! the pre-spike price less `MShotSellPriceAdjust` (the FAQ: "the 4-second-old ASK, i.e. before
//! the spike"; the model reads the last print at least [`PRE_SPIKE_LOOKBACK_MS`] before the
//! fill, since the tape has no book). From there the line moves under the strategy's sell rules
//! — `PriceDown*`, `SellLevel*`, `SellShot*` — and the stop fires under `StopLoss*`; see
//! [`super::line`]. A position nothing closed inside the tape is [`ExitKind::OpenAtWindowEnd`]:
//! not a trade, whatever the core's exit was.

use super::line::{LineWalk, walk};
use super::mshot::{DEFAULT_LATENCY_MS, PRE_SPIKE_LOOKBACK_MS};
use super::{Deal, Exit, Fill};
use crate::feed::types::Tick;

/// Sell-line parameters, in the strategy's own units (per cent, seconds; `SellDelay` is ms).
/// Every rule's fields are documented in [`super::line`].
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
    // PriceDown
    pub price_down_timer_s: f64,
    pub price_down_pct: f64,
    pub price_down_delay_s: f64,
    pub price_down_relative: bool,
    pub price_down_allowed_drop_pct: f64,
    // SellLevel
    pub sell_level_delay_s: f64,
    pub sell_level_delay_next_s: f64,
    pub sell_level_time_s: f64,
    pub sell_level_count: u32,
    pub sell_level_adjust_pct: f64,
    pub sell_level_relative: bool,
    pub sell_level_allowed_drop_pct: f64,
    pub sell_level_work_time_s: f64,
    // SellShot
    pub ignore_sell_shot: bool,
    pub sell_shot_distance_pct: f64,
    pub sell_shot_corridor_pct: f64,
    pub sell_shot_calc_interval_s: f64,
    pub sell_shot_raise_wait_s: f64,
    pub sell_shot_replace_delay_s: f64,
    pub sell_shot_price_down: f64,
    pub sell_shot_price_down_delay_s: f64,
    pub sell_shot_allowed_up_pct: f64,
    pub sell_shot_allowed_down_pct: f64,
    pub sell_shot_delay_s: f64,
    // Stops
    pub stop_loss_pct: f64,
    pub stop_loss_delay_s: f64,
    /// Model parameter: how long a replacement of the sell takes to reach the book.
    pub latency_ms: f64,
}

impl Default for ExitParams {
    /// A plain 1 % take, nothing moving it, no stop.
    fn default() -> Self {
        Self {
            sell_price_pct: 1.0,
            sell_at_last_price: false,
            sell_price_adjust_pct: 0.0,
            sell_delay_ms: 0.0,
            price_down_timer_s: 0.0,
            price_down_pct: 0.0,
            price_down_delay_s: 0.0,
            price_down_relative: true,
            price_down_allowed_drop_pct: 0.0,
            sell_level_delay_s: 0.0,
            sell_level_delay_next_s: 0.0,
            sell_level_time_s: 0.0,
            sell_level_count: 0,
            sell_level_adjust_pct: 0.0,
            sell_level_relative: false,
            sell_level_allowed_drop_pct: 0.0,
            sell_level_work_time_s: 0.0,
            ignore_sell_shot: true,
            sell_shot_distance_pct: 0.0,
            sell_shot_corridor_pct: 50.0,
            sell_shot_calc_interval_s: 0.6,
            sell_shot_raise_wait_s: 0.0,
            sell_shot_replace_delay_s: 0.0,
            sell_shot_price_down: 0.0,
            sell_shot_price_down_delay_s: 0.0,
            sell_shot_allowed_up_pct: 10.0,
            sell_shot_allowed_down_pct: -100.0,
            sell_shot_delay_s: 0.0,
            stop_loss_pct: 0.0,
            stop_loss_delay_s: 0.0,
            latency_ms: DEFAULT_LATENCY_MS,
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

    /// Replay the tape after the fill: the take, the moving line, the stop.
    ///
    /// Args:
    ///     deal: The report row — its side, and its own exit for the fallback.
    ///     ticks: The window's prints, ascending.
    ///     fill: The modelled (or factual) entry.
    pub fn exit(&self, deal: &Deal, ticks: &[Tick], fill: Fill) -> Exit {
        self.walk(deal, ticks, fill).exit
    }

    /// The same replay with every level the line stood at, for the archive comparison.
    pub fn walk(&self, deal: &Deal, ticks: &[Tick], fill: Fill) -> LineWalk {
        let take = self.take_level(deal, ticks, fill);
        walk(deal, ticks, fill, take, self.params)
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
