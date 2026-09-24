//! The strategy window's "Sell order / SellShot" section.
//!
//! **SellShot** (`IgnoreSellShot` off, `SellShotDistance` non-zero) — after `SellShotDelay` the
//! sell keeps `SellShotDistance` per cent off the highest print of the last
//! `SellShotCalcInterval` seconds, re-placed when its distance leaves the corridor
//! `Distance · (1 ± Corridor/100)`: after `SellShotRaiseWait` when moving away from the buy,
//! after `SellShotReplaceDelay` when moving toward it; `SellShotPriceDown` narrows the distance
//! by that much per second past `SellShotPriceDownDelay`; the line stays between
//! `SellShotAllowedDown` and `SellShotAllowedUp` per cent over the buy.
//!
//! From the Moonbot FAQ (`SellShot*` answers). Not checked on the tape: on 2026-09-20
//! `IgnoreSellShot` was on in all live strategies but two.

use super::line::Line;
use super::{ExitParams, Side};
use crate::db::tuner::ticks::Fill;
use crate::db::tuner::ticks::mshot::FAST_ALGO_WINDOW_MS;
use crate::feed::types::Tick;

/// SellShot's window, bounds and the corridor breach it is waiting out.
pub(super) struct SellShot<'a> {
    params: &'a ExitParams,
    fill: Fill,
    side: Side,
    /// Whether the strategy switched the rule on.
    on: bool,
    /// The end of `SellShotDelay`.
    from: i64,
    /// The calculation window.
    calc_ms: i64,
    /// `SellShotAllowedDown` over the buy.
    low: f64,
    /// `SellShotAllowedUp` over the buy.
    high: f64,
    /// `(kind, since)`: which way the line is out of the corridor and since when.
    breach: Option<(bool, i64)>,
}

impl<'a> SellShot<'a> {
    pub(super) fn new(params: &'a ExitParams, fill: Fill, side: Side) -> Self {
        Self {
            params,
            fill,
            side,
            on: !params.ignore_sell_shot && params.sell_shot_distance_pct != 0.0,
            from: fill.t_ms + (params.sell_shot_delay_s.max(0.0) * 1000.0) as i64,
            // The core's own floor on the SellShot calculation window — the same 100 ms its fast
            // algorithm reads, but a rule of the sell, not the entry's re-place window
            // (`ModelSettings::replace_window_ms`), and not a setting of the model.
            calc_ms: ((params.sell_shot_calc_interval_s.max(0.0) * 1000.0) as i64)
                .max(FAST_ALGO_WINDOW_MS),
            low: side.over(fill.price, params.sell_shot_allowed_down_pct),
            high: side.over(fill.price, params.sell_shot_allowed_up_pct),
            breach: None,
        }
    }

    /// The line follows the market inside its corridor — driven by the print at `t_ms`.
    ///
    /// Args:
    ///     seen: The prints up to and including the one at `t_ms`.
    pub(super) fn on_print(&mut self, t_ms: i64, seen: &[Tick], line: &mut Line) {
        if !self.on || t_ms < self.from {
            return;
        }
        let (params, fill, side) = (self.params, self.fill, self.side);
        let from = t_ms - self.calc_ms;
        let reference = side.extreme(
            seen.iter()
                .filter(|t| (t.time_ms as i64) >= from && t.price > 0.0)
                .map(|t| f64::from(t.price)),
        );
        let Some(reference) = reference else {
            return;
        };
        let elapsed_s = (t_ms - fill.t_ms) as f64 / 1000.0;
        let mut distance = params.sell_shot_distance_pct;
        if params.sell_shot_price_down < 0.0 {
            let past = (elapsed_s - params.sell_shot_price_down_delay_s).max(0.0);
            distance -= params.sell_shot_price_down.abs() * past;
        }
        let corridor = distance.abs() * params.sell_shot_corridor_pct / 100.0;
        let d = side.distance_pct(reference, line.core());
        let out = if d > distance + corridor {
            Some(false) // too far from the market: move toward the buy
        } else if d < distance - corridor {
            Some(true) // too close: move away from the buy
        } else {
            None
        };
        match out {
            None => self.breach = None,
            Some(away) => {
                let since = match self.breach {
                    Some((seen, since)) if seen == away => since,
                    _ => {
                        self.breach = Some((away, t_ms));
                        t_ms
                    }
                };
                let wait_ms = if away {
                    (params.sell_shot_raise_wait_s * 1000.0) as i64
                } else {
                    (params.sell_shot_replace_delay_s * 1000.0) as i64
                };
                if t_ms - since >= wait_ms {
                    let next = side.over(reference, distance);
                    let next = side.nearer(side.farther(next, self.low), self.high);
                    line.place(t_ms, next);
                    self.breach = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
