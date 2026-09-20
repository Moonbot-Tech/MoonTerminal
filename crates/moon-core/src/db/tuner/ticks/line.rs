//! The moving sell line: where the sell order stood at every moment after the fill, under the
//! rules of the strategy's "Sell order" and "Stops" tabs, and which print crossed it.
//!
//! From the Moonbot FAQ (`PriceDown*`, `SellLevel*`, `SellShot*`, `StopLoss*` answers) and the
//! live strategies (2026-09-20: 862 of 1 331 run `PriceDownTimer` 1 s with `PriceDownPercent`
//! 50 relative, `SellLevelDelay` is absent everywhere, `IgnoreSellShot` is on all but two):
//!
//! - **PriceDown** — `PriceDownTimer` seconds after the buy the sell is lowered by
//!   `PriceDownPercent`: of the distance to the buy when `PriceDownRelative` (`sell −= (sell −
//!   buy) · pct`), of the price otherwise (`sell −= buy · pct`); then again every
//!   `PriceDownDelay` seconds (0 reads as the terminal's own 0.33 s floor), never below
//!   `PriceDownAllowedDrop` per cent over the buy. A zero timer never starts.
//! - **SellLevel** — `SellLevelDelay` seconds after the buy (a negative value is the 0.33 s
//!   floor, zero never), the sell moves to the highest print of the last `SellLevelTime`
//!   seconds adjusted by `SellLevelAdjust` per cent — of that high, or of its distance to the
//!   buy when `SellLevelRelative` — at most `SellLevelCount` times, every `SellLevelDelayNext`
//!   (or `SellLevelDelay`) seconds, inside `SellLevelWorkTime`, never below
//!   `SellLevelAllowedDrop` per cent over the buy.
//! - **SellShot** (`IgnoreSellShot` off, `SellShotDistance` non-zero) — after `SellShotDelay`
//!   the sell keeps `SellShotDistance` per cent off the highest print of the last
//!   `SellShotCalcInterval` seconds, re-placed when its distance leaves the corridor
//!   `Distance · (1 ± Corridor/100)`: after `SellShotRaiseWait` when moving away from the buy,
//!   after `SellShotReplaceDelay` when moving toward it; `SellShotPriceDown` narrows the
//!   distance by that much per second past `SellShotPriceDownDelay`; the line stays between
//!   `SellShotAllowedDown` and `SellShotAllowedUp` per cent over the buy.
//! - **StopLoss** — `StopLoss` per cent from the buy (negative: a loss), armed
//!   `StopLossDelay` seconds after the buy; the first print through it is a market exit at
//!   the print's own price.
//!
//! Every rule is written for a long and mirrored for a short by [`Side`]. A replacement reaches
//! the exchange `latency_ms` later, as the entry's does: a spike through the OLD level in that
//! gap fills there. The line's replacements are recorded so the model can be held against the
//! archived Exit line of the trade.

use super::exit::ExitParams;
use super::mshot::FAST_ALGO_WINDOW_MS;
use super::{Deal, Exit, ExitKind, Fill, reaches};
use crate::feed::types::Tick;

/// The terminal's own floor on a step delay of zero: the FAQ's "0.33 s internal minimum".
pub const STEP_FLOOR_MS: i64 = 330;

/// Which way the position profits, folding every "above/below the buy" into one sign.
#[derive(Clone, Copy)]
struct Side {
    long: bool,
}

impl Side {
    /// `pct` per cent over the buy in the PROFIT direction: above for a long, below for a
    /// short.
    fn over(self, base: f64, pct: f64) -> f64 {
        if self.long {
            base * (1.0 + pct / 100.0)
        } else {
            base * (1.0 - pct / 100.0)
        }
    }

    /// The take side of two levels — the higher for a long — i.e. farther in profit.
    fn farther(self, a: f64, b: f64) -> f64 {
        if self.long { a.max(b) } else { a.min(b) }
    }

    /// The nearer of two levels in profit terms.
    fn nearer(self, a: f64, b: f64) -> f64 {
        if self.long { a.min(b) } else { a.max(b) }
    }

    /// The extreme print in the profit direction over a run.
    fn extreme(self, prices: impl Iterator<Item = f64>) -> Option<f64> {
        if self.long {
            prices.reduce(f64::max)
        } else {
            prices.reduce(f64::min)
        }
    }

    /// Distance of `level` from `reference`, per cent, positive in the profit direction.
    fn distance_pct(self, reference: f64, level: f64) -> f64 {
        if reference <= 0.0 {
            return 0.0;
        }
        let signed = if self.long {
            level - reference
        } else {
            reference - level
        };
        signed / reference * 100.0
    }
}

/// One replacement of the line, for the comparison with the archived Exit points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinePoint {
    pub t_ms: i64,
    pub price: f64,
}

/// The walk's answer: the exit and every level the line stood at, placement first.
#[derive(Clone, Debug, PartialEq)]
pub struct LineWalk {
    pub exit: Exit,
    pub points: Vec<LinePoint>,
}

/// Seconds to milliseconds, with the terminal's floor for a zero delay.
fn step_ms(seconds: f64) -> i64 {
    let ms = (seconds * 1000.0) as i64;
    if ms <= 0 { STEP_FLOOR_MS } else { ms }
}

/// Walk the tape after the fill under `params`, starting from the take `take`.
///
/// Args:
///     deal: The row — its side.
///     ticks: The window's prints, ascending.
///     fill: The entry.
///     take: The take level the line starts at (see `ExitModel::take_level`).
///     params: The sell-line rules.
pub fn walk(deal: &Deal, ticks: &[Tick], fill: Fill, take: f64, params: &ExitParams) -> LineWalk {
    let side = Side {
        long: deal.is_long(),
    };
    let latency_ms = params.latency_ms.max(0.0) as i64;
    let armed_at = fill.t_ms + params.sell_delay_ms.max(0.0) as i64;
    let mut points = vec![LinePoint {
        t_ms: armed_at,
        price: take,
    }];
    // The exchange's level (what fills) and the core's (what the rules move); a move the
    // exchange has not seen yet is `pending`.
    let mut exch_line = take;
    let mut core_line = take;
    let mut pending: Option<(i64, f64)> = None;
    let mut place = |t_ms: i64, level: f64, core: &mut f64, pending: &mut Option<(i64, f64)>| {
        if (level - *core).abs() <= f64::EPSILON * core.abs() {
            return;
        }
        *core = level;
        *pending = Some((t_ms + latency_ms, level));
        points.push(LinePoint {
            t_ms: t_ms + latency_ms,
            price: level,
        });
    };

    // --- PriceDown ---
    let pd_on = params.price_down_timer_s > 0.0 && params.price_down_pct > 0.0;
    let mut pd_next = if pd_on {
        Some(fill.t_ms + (params.price_down_timer_s * 1000.0) as i64)
    } else {
        None
    };
    let pd_floor = side.over(fill.price, params.price_down_allowed_drop_pct);

    // --- SellLevel ---
    let sl_on = params.sell_level_delay_s != 0.0
        && params.sell_level_time_s > 0.0
        && params.sell_level_count > 0;
    let sl_first_ms = if params.sell_level_delay_s < 0.0 {
        STEP_FLOOR_MS
    } else {
        (params.sell_level_delay_s * 1000.0) as i64
    };
    let mut sl_next = if sl_on {
        Some(fill.t_ms + sl_first_ms)
    } else {
        None
    };
    let sl_step_ms = if params.sell_level_delay_next_s > 0.0 {
        (params.sell_level_delay_next_s * 1000.0) as i64
    } else if params.sell_level_delay_next_s < 0.0 {
        STEP_FLOOR_MS
    } else {
        sl_first_ms.max(STEP_FLOOR_MS)
    };
    let mut sl_left = params.sell_level_count;
    let sl_until = if params.sell_level_work_time_s > 0.0 {
        Some(fill.t_ms + (params.sell_level_work_time_s * 1000.0) as i64)
    } else {
        None
    };
    let sl_floor = side.over(fill.price, params.sell_level_allowed_drop_pct);

    // --- SellShot ---
    let ss_on = !params.ignore_sell_shot && params.sell_shot_distance_pct != 0.0;
    let ss_from = fill.t_ms + (params.sell_shot_delay_s.max(0.0) * 1000.0) as i64;
    let ss_calc_ms =
        ((params.sell_shot_calc_interval_s.max(0.0) * 1000.0) as i64).max(FAST_ALGO_WINDOW_MS);
    let ss_low = side.over(fill.price, params.sell_shot_allowed_down_pct);
    let ss_high = side.over(fill.price, params.sell_shot_allowed_up_pct);
    // `(kind, since)`: which way the line is out of the corridor and since when.
    let mut ss_breach: Option<(bool, i64)> = None;

    // --- StopLoss ---
    let stop_on = params.stop_loss_pct != 0.0;
    let stop_level = side.over(fill.price, params.stop_loss_pct);
    let stop_from = fill.t_ms + (params.stop_loss_delay_s.max(0.0) * 1000.0) as i64;

    let mut last_t = fill.t_ms;
    for (index, tick) in ticks.iter().enumerate() {
        let t_ms = tick.time_ms as i64;
        let price = f64::from(tick.price);
        if t_ms <= fill.t_ms || !price.is_finite() || price <= 0.0 {
            continue;
        }
        last_t = t_ms;
        // The timer-driven rules moved the line at their own moments, between prints; every
        // step due by this print happened BEFORE it, and a step that also reached the book
        // before it is what this print meets.
        //
        // PriceDown steps, one per due moment.
        while let Some(due) = pd_next.filter(|due| t_ms >= *due) {
            let next = if params.price_down_relative {
                core_line - (core_line - fill.price) * params.price_down_pct / 100.0
            } else {
                core_line - side.over(fill.price, params.price_down_pct) + fill.price
            };
            let next = side.farther(next, pd_floor);
            if (next - core_line).abs() <= f64::EPSILON * core_line.abs() {
                pd_next = None;
                break;
            }
            place(due, next, &mut core_line, &mut pending);
            pd_next = Some(due + step_ms(params.price_down_delay_s));
        }
        // SellLevel: to the high of the look-back, adjusted.
        while let Some(due) = sl_next.filter(|due| t_ms >= *due) {
            if sl_left == 0 || sl_until.is_some_and(|until| due > until) {
                sl_next = None;
                break;
            }
            let from = due - (params.sell_level_time_s * 1000.0) as i64;
            let high = side.extreme(
                ticks[..=index]
                    .iter()
                    .filter(|t| {
                        let tt = t.time_ms as i64;
                        tt >= from && tt <= due && t.price > 0.0
                    })
                    .map(|t| f64::from(t.price)),
            );
            if let Some(high) = high {
                let next = if params.sell_level_relative {
                    fill.price + (high - fill.price) * params.sell_level_adjust_pct / 100.0
                } else {
                    side.over(high, params.sell_level_adjust_pct)
                };
                let next = side.farther(next, sl_floor);
                place(due, next, &mut core_line, &mut pending);
            }
            sl_left -= 1;
            sl_next = Some(due + sl_step_ms);
        }
        if let Some((_, level)) = pending.filter(|(apply_at, _)| t_ms >= *apply_at) {
            exch_line = level;
            pending = None;
        }
        // The stop is a market order the core fires on the print; the sell is a limit the
        // print reaches. Both come before the print-driven rule below moves anything.
        if stop_on && t_ms >= stop_from && reaches(price, stop_level, side.long) {
            return LineWalk {
                exit: Exit {
                    t_ms,
                    price,
                    kind: ExitKind::Stop,
                },
                points,
            };
        }
        if t_ms > armed_at && reaches(price, exch_line, !side.long) {
            return LineWalk {
                exit: Exit {
                    t_ms,
                    price: exch_line,
                    // What the print met: the take as placed, or a level a rule moved it to.
                    kind: if exch_line == take {
                        ExitKind::Take
                    } else {
                        ExitKind::Line
                    },
                },
                points,
            };
        }
        // SellShot: the line follows the market inside its corridor — driven by this print.
        if ss_on && t_ms >= ss_from {
            let from = t_ms - ss_calc_ms;
            let reference = side.extreme(
                ticks[..=index]
                    .iter()
                    .filter(|t| (t.time_ms as i64) >= from && t.price > 0.0)
                    .map(|t| f64::from(t.price)),
            );
            if let Some(reference) = reference {
                let elapsed_s = (t_ms - fill.t_ms) as f64 / 1000.0;
                let mut distance = params.sell_shot_distance_pct;
                if params.sell_shot_price_down < 0.0 {
                    let past = (elapsed_s - params.sell_shot_price_down_delay_s).max(0.0);
                    distance -= params.sell_shot_price_down.abs() * past;
                }
                let corridor = distance.abs() * params.sell_shot_corridor_pct / 100.0;
                let d = side.distance_pct(reference, core_line);
                let out = if d > distance + corridor {
                    Some(false) // too far from the market: move toward the buy
                } else if d < distance - corridor {
                    Some(true) // too close: move away from the buy
                } else {
                    None
                };
                match out {
                    None => ss_breach = None,
                    Some(away) => {
                        let since = match ss_breach {
                            Some((seen, since)) if seen == away => since,
                            _ => {
                                ss_breach = Some((away, t_ms));
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
                            let next = side.nearer(side.farther(next, ss_low), ss_high);
                            place(t_ms, next, &mut core_line, &mut pending);
                            ss_breach = None;
                        }
                    }
                }
            }
        }
    }
    // Nothing closed it inside the tape. Not the report's own exit: a variant that never
    // closes is not a trade, whatever the core's rules did, and the caption counts it.
    let tail = ticks.last().map(|t| t.time_ms as i64).unwrap_or(last_t);
    LineWalk {
        exit: Exit {
            t_ms: tail,
            price: f64::NAN,
            kind: ExitKind::OpenAtWindowEnd,
        },
        points,
    }
}

#[cfg(test)]
mod tests;
