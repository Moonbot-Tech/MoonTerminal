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
//! - **PumpMove** (PumpsDetection's own tab; `PumpMoveTimer` non-zero) — once, `PumpMoveTimer`
//!   seconds after the take is placed, the sell moves to `PumpMovePersent` per cent of the way
//!   from the pump's peak back to the buy (FAQ: "учитывается процент между пиковой ценой и ценой
//!   покупки"), the peak read over [`PUMP_PEAK_LOOKBACK_MS`] before the take up to the move.
//! - **StopLoss** — `StopLoss` per cent from the buy (negative: a loss), armed
//!   `StopLossDelay` seconds after the buy. With `FastStopLoss` the first print through it is
//!   a market exit at the print's own price. Without it — the core's default — the core
//!   watches the book's BID (a short's ASK) averaged over `StopLossEMA` samples, and the walk
//!   reads a proxy of that: the last print on that side of the book (a taker sell prints at the
//!   BID), sampled every [`STOP_SAMPLE_MS`], averaged the same way; the exit is at the sample,
//!   at the proxy's price. Where the core's panic sell then fills is the book's business — see
//!   [`super::verify`] for how the fact is judged.
//!
//! Every rule is written for a long and mirrored for a short by [`Side`]. A replacement reaches
//! the exchange `latency_ms` later, as the entry's does: a spike through the OLD level in that
//! gap fills there. The line's replacements are recorded so the model can be held against the
//! archived Exit line of the trade.
//!
//! What goes to the exchange is rounded to the nearest step of the market's price grid (a sell
//! limit is placed on the grid), and the rules carry on from the ORDER's price once a move
//! reached the book — from the computed value while rounding kept the order where it was
//! ([`advance`]). The rounding is what decides a print AT the level: on ARX (2026-09-21) the
//! `PriceDownAllowedDrop` floor computed to 0.196445, the core's order stood at 0.1964, and
//! the tape's high was exactly 0.1964 — the unrounded line was never reached.

use super::exit::{ExitParams, stop_pct};
use super::mshot::FAST_ALGO_WINDOW_MS;
use super::{Deal, Exit, ExitKind, Fill, reaches, round_to_step};
use crate::feed::types::Tick;

/// The terminal's own floor on a step delay of zero: the FAQ's "0.33 s internal minimum".
pub const STEP_FLOOR_MS: i64 = 330;

/// How often the non-fast stop's BID proxy is sampled. The core's own cadence is not in the
/// FAQ and the tape has no book, so this is a CALIBRATION, not the core's constant: against
/// the activation the order archive records (the sell line's jump past the stop), on 199 live
/// book-watching stops (2026-09-22), the first print through the level fired a median 3.9 s
/// early with `StopLossEMA` at 3 and 0.6 s with it off; sampling the proxy every 2 s and
/// averaging the samples brings both medians within 0.4 s and puts 64 of 98 (EMA off) and 35
/// of 101 (EMA 3) within a second, against 53 and 25. Faster sampling left the EMA-3 stops
/// seconds early, 3 s left the rest a second late. What the proxy still cannot see is the book
/// itself, and the EMA-3 stops are where that shows.
pub const STOP_SAMPLE_MS: i64 = 2_000;

/// How far past `PumpMoveTimer` the core's pump move lands, less the model's own placement
/// latency: over 32 archived PumpsDetection lines (2026-09-22, every live Pump strategy runs
/// `PumpMoveTimer` 2 with `PumpMovePersent` 1) the move came 575–704 ms past the timer, a
/// median of 610 ms — counted from the take, not from the buy's report stamp: one take placed
/// 32 s after `buydatems` still moved 2.6 s after itself.
pub const PUMP_MOVE_LAG_MS: i64 = 500;

/// How far before the take the pump's peak is looked for. The FAQ counts the peak from the
/// detect, which the report does not stamp on older rows, and the peak itself is the print that
/// triggered it — a median 70 ms before the buy, up to 6 s when the buy order waited for the
/// retrace. Over the same 32 lines a window from 10 s before the take reproduces the moved level
/// on 31; from the buy it reproduces 1, from 4 s before the take 27.
pub const PUMP_PEAK_LOOKBACK_MS: i64 = 10_000;

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

/// The book-watching stop's state: a BID proxy — the last print on the stop's side of the
/// book — sampled every [`STOP_SAMPLE_MS`] and averaged over `StopLossEMA` samples.
struct BookStop {
    long: bool,
    level: f64,
    /// The end of `StopLossDelay`: a sample before it is averaged but cannot fire.
    armed_at: i64,
    /// The EMA weight, `2 / (StopLossEMA + 1)`; 1 without averaging.
    alpha: f64,
    proxy: Option<f64>,
    avg: Option<f64>,
    next_sample: i64,
    /// Up to when the fact proves this stop did not fire (`record::StopAnchor`): a sample by
    /// then is averaged but cannot fire. `i64::MIN` when the walk is not the trade's own stop.
    quiet_until: i64,
}

impl BookStop {
    /// Take every sample due strictly before `until` — the prints before it are all the
    /// proxy has seen — and answer the first one whose average is past the level: the stop,
    /// at the sample's moment and the proxy's price.
    fn sample_before(&mut self, until: i64) -> Option<Exit> {
        while self.next_sample < until {
            let at = self.next_sample;
            self.next_sample += STOP_SAMPLE_MS;
            let Some(bid) = self.proxy else {
                continue;
            };
            let avg = self
                .avg
                .map_or(bid, |a| self.alpha * bid + (1.0 - self.alpha) * a);
            self.avg = Some(avg);
            if at >= self.armed_at && at > self.quiet_until && reaches(avg, self.level, self.long) {
                return Some(Exit {
                    t_ms: at,
                    price: bid,
                    kind: ExitKind::Stop,
                });
            }
        }
        None
    }

    /// Read a print into the proxy: a taker sell prints at the BID — a long's stop side; a
    /// short's stop watches the ASK, where a taker buy prints.
    fn see(&mut self, tick: &Tick) {
        let stop_side = if self.long {
            crate::feed::types::Side::Sell
        } else {
            crate::feed::types::Side::Buy
        };
        if tick.side == stop_side {
            self.proxy = Some(f64::from(tick.price));
        }
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

/// One step of the core's sell price: `level` is what a rule computed, `order` that level on
/// the price grid. When the order lands on a new price, the move goes to the book and the core
/// carries on from the ORDER's price; when rounding keeps it where it was, nothing is sent and
/// the core carries on from the computed value, so the next step can still cross a grid line.
/// Answers whether the order moved.
///
/// Read off the archived Exit lines (2026-09-22): every step of INDEX's twelve (Gate, relative
/// PriceDown 10 %) lands only when chained off the placed price — off the exact value the second
/// already rounds a step short — and FATCOIN's one-step-per-tick lines climb past a step that
/// rounds back onto the order only when that step's exact value carries into the next. Over
/// 1 421 archived PriceDown lines the rule reproduces every level of 997, against 647 for the
/// exact chain and 949 for the placed price alone.
///
/// Args:
///     core: The core's sell price, advanced in place.
///     last_sent: The order's price as last sent, advanced when the order moves.
///     level: The level a rule computed.
///     order: `level` on the price grid.
fn advance(core: &mut f64, last_sent: &mut f64, level: f64, order: f64) -> bool {
    if (order - *last_sent).abs() <= f64::EPSILON * last_sent.abs() {
        *core = level;
        return false;
    }
    *core = order;
    *last_sent = order;
    true
}

/// Seconds to milliseconds, with the terminal's floor for a zero delay.
pub(super) fn step_ms(seconds: f64) -> i64 {
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
    walk_held(deal, ticks, fill, take, params, None)
}

/// [`walk`] with the sell HELD — no print fills it — until `hold_until_ms`: the rules keep
/// moving the line, so its level at that moment is known whatever print the model would have
/// sold on before it. The stop is not held: it is a market order and fires as it does. The
/// verdict on a fact reads the line this way, at the close.
///
/// Args:
///     hold_until_ms: `None` walks as [`walk`] does.
pub fn walk_held(
    deal: &Deal,
    ticks: &[Tick],
    fill: Fill,
    take: f64,
    params: &ExitParams,
    hold_until_ms: Option<i64>,
) -> LineWalk {
    let side = Side {
        long: deal.is_long(),
    };
    let latency_ms = params.latency_ms.max(0.0) as i64;
    let armed_at = fill.t_ms + params.sell_delay_ms.max(0.0) as i64;
    // When the take is on the book: placed at `armed_at`, there after the same latency as any
    // move of the line.
    let take_live_at = armed_at + latency_ms;
    // What the exchange is given: the level on the price grid.
    let placed = |level: f64| match deal.tick {
        Some(tick) => round_to_step(level, tick),
        None => level,
    };
    let take_placed = placed(take);
    let mut points = vec![LinePoint {
        t_ms: armed_at,
        price: take_placed,
    }];
    // The exchange's level (what fills, on the grid) and the core's (what the rules move from);
    // a move the exchange has not seen yet is `pending`.
    let mut exch_line = take_placed;
    // Whether a move has reached the exchange: what tells a fill at the take from a fill at
    // a level a rule moved the line to — not the price, which a moved line can round back onto.
    let mut exch_moved = false;
    // The core's sell price: the ORDER's price once a move reached the book, the computed value
    // while rounding kept the order where it was — see [`advance`]. The take is an order too.
    let mut core_line = take_placed;
    // The last level sent to the book, pending or not: what a new level must differ from to
    // be a move at all.
    let mut last_sent = take_placed;
    let mut pending: Option<(i64, f64)> = None;
    // Answers whether the order moved — a replace went to the book.
    let mut place = |t_ms: i64, level: f64, core: &mut f64, pending: &mut Option<(i64, f64)>| {
        if (level - *core).abs() <= f64::EPSILON * core.abs() {
            return false;
        }
        let order = placed(level);
        if !advance(core, &mut last_sent, level, order) {
            return false;
        }
        *pending = Some((t_ms + latency_ms, order));
        points.push(LinePoint {
            t_ms: t_ms + latency_ms,
            price: order,
        });
        true
    };

    // --- PriceDown ---
    let pd_on = params.price_down_timer_s > 0.0 && params.price_down_pct > 0.0;
    let mut pd_next = if pd_on {
        Some(fill.t_ms + (params.price_down_timer_s * 1000.0) as i64)
    } else {
        None
    };
    let pd_floor = side.over(fill.price, params.price_down_allowed_drop_pct);

    // --- PumpMove --- one move, timed off the take (see `PUMP_MOVE_LAG_MS`).
    let mut pm_next = (params.pump_move_timer_s > 0.0)
        .then(|| armed_at + (params.pump_move_timer_s * 1000.0) as i64 + PUMP_MOVE_LAG_MS);

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
    // The ADJUSTED distance decides both whether there is a stop and where it stands —
    // reading the raw `stop_loss_pct` for the first and the adjusted one for the second would
    // arm a stop the adjustment had cancelled, at the fill price itself, where the next print
    // fires it. `over` mirrors the sign for a short, so only the distance is adjusted here; the
    // level is NOT snapped to the price grid, unlike every level that reaches the exchange —
    // a stop is the core's own trigger for a market sell, and nothing about it is ever placed.
    let stop = stop_pct(params, deal);
    let stop_on = stop != 0.0;
    let stop_level = side.over(fill.price, stop);
    let stop_from = fill.t_ms + (params.stop_loss_delay_s.max(0.0) * 1000.0) as i64;
    // What the fact proves about the stop when this walk runs the trade's own
    // (`record::StopAnchor`): it fired when the core's did, at the price the core sold at, and
    // not a moment before — nor before the close, on a trade it never stopped. The book the
    // stop watches is not on the tape; the fact is the book's own answer.
    let anchor = deal.stop_anchor.filter(|a| a.holds(deal, fill, params));
    let quiet_until = anchor.map_or(i64::MIN, |a| a.quiet_until_ms);
    let fired = anchor.and_then(|a| a.fired);
    // The non-fast stop's BID proxy: the last print on the stop's side of the book, sampled on
    // its own clock and averaged over `StopLossEMA` samples (see `STOP_SAMPLE_MS`).
    let book_stop = stop_on && !params.fast_stop_loss;
    let mut book = book_stop.then(|| BookStop {
        long: side.long,
        level: stop_level,
        armed_at: stop_from,
        alpha: 2.0 / (params.stop_loss_ema.max(1.0) + 1.0),
        proxy: None,
        avg: None,
        next_sample: fill.t_ms + STOP_SAMPLE_MS,
        quiet_until,
    });
    let anchored_stop = |at: i64, price: f64, points: Vec<LinePoint>| LineWalk {
        exit: Exit {
            t_ms: at,
            price,
            kind: ExitKind::Stop,
        },
        points,
    };

    let mut last_t = fill.t_ms;
    for (index, tick) in ticks.iter().enumerate() {
        let t_ms = tick.time_ms as i64;
        let price = f64::from(tick.price);
        if t_ms <= fill.t_ms || !price.is_finite() || price <= 0.0 {
            continue;
        }
        last_t = t_ms;
        // The fact's own stop fired between the last print and this one: ahead of anything
        // this print does, and after everything the prints before it did.
        if let Some((at, sold)) = fired.filter(|(at, _)| t_ms >= *at) {
            return anchored_stop(at, sold, points);
        }
        // The timer-driven rules moved the line at their own moments, between prints; every
        // step due by this print happened BEFORE it, and a step that also reached the book
        // before it is what this print meets.
        //
        // PriceDown steps, one per due moment, and the pump move, in the order they fell due:
        // each step chains off where the one before it left the line.
        loop {
            let pd_due = pd_next.filter(|due| t_ms >= *due);
            let pm_due = pm_next.filter(|due| t_ms >= *due);
            if let Some(due) = pm_due.filter(|pm| pd_due.is_none_or(|pd| *pm <= pd)) {
                pm_next = None;
                let from = armed_at - PUMP_PEAK_LOOKBACK_MS;
                let peak = side.extreme(
                    ticks[..=index]
                        .iter()
                        .filter(|t| {
                            let tt = t.time_ms as i64;
                            tt >= from && tt <= due && t.price > 0.0
                        })
                        .map(|t| f64::from(t.price)),
                );
                if let Some(peak) = peak {
                    let next = peak + (fill.price - peak) * params.pump_move_pct / 100.0;
                    place(due, next, &mut core_line, &mut pending);
                }
                continue;
            }
            let Some(due) = pd_due else {
                break;
            };
            let next = if params.price_down_relative {
                core_line - (core_line - fill.price) * params.price_down_pct / 100.0
            } else {
                core_line - side.over(fill.price, params.price_down_pct) + fill.price
            };
            let next = side.farther(next, pd_floor);
            if (next - core_line).abs() <= f64::EPSILON * core_line.abs() {
                pd_next = None;
                continue;
            }
            // The core times the next step from this one's going through (`Deal::step_lag_ms`);
            // a step rounding kept in place sent nothing and waits for nothing — over the
            // archived GateF lines two delays with such a step between them run 32 ms over,
            // against 47 ms for one real step.
            let moved = place(due, next, &mut core_line, &mut pending);
            let lag_ms = if moved {
                deal.step_lag_ms.max(0.0) as i64
            } else {
                0
            };
            pd_next = Some(due + step_ms(params.price_down_delay_s) + lag_ms);
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
            exch_moved = true;
            pending = None;
        }
        // The book-watching stop samples between prints: every sample due BEFORE this print
        // reads the proxy the earlier prints left, and one past the level fires at its own
        // moment, ahead of anything this print does.
        if let Some(book) = book.as_mut() {
            if let Some(exit) = book.sample_before(t_ms) {
                return LineWalk { exit, points };
            }
            book.see(tick);
        }
        // The fast stop is a market order the core fires on the print; the sell is a limit the
        // print reaches. Both come before the print-driven rule below moves anything.
        if stop_on
            && !book_stop
            && t_ms >= stop_from
            && t_ms > quiet_until
            && reaches(price, stop_level, side.long)
        {
            return LineWalk {
                exit: Exit {
                    t_ms,
                    price,
                    kind: ExitKind::Stop,
                },
                points,
            };
        }
        // A print AT the level fills the sell — the optimistic reading the spec states (§7:
        // the queue standing at the level is not modelled; COOL 2026-09-21 printed 31
        // contracts at the level against a sell of 18 000 and the core's line stood). The
        // verdict on the fact does not lean on this: `verify` judges the line by where it
        // STOOD at the close, not by which print the model sold on.
        //
        // The take is an order like every move, and reaches the book `latency_ms` after the
        // core placed it: the spike's own tail, printed in the milliseconds after the fill, is
        // not a print the take was there for. Filling on it turned 30 of 88 stopped MoonShot
        // trades into wins on the live sample (2026-09-23), the take "touched" 9 ms after the
        // buy by the pump it was bought on.
        let held = hold_until_ms.is_some_and(|until| t_ms <= until);
        if t_ms > armed_at && t_ms >= take_live_at && !held && reaches(price, exch_line, !side.long)
        {
            return LineWalk {
                exit: Exit {
                    t_ms,
                    price: exch_line,
                    // What the print met: the take as placed, or a level a rule moved it to.
                    kind: if !exch_moved {
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
    let tail = ticks.last().map(|t| t.time_ms as i64).unwrap_or(last_t);
    // The fact's own stop, past the last print: the tape went quiet, the core did not.
    if let Some((at, sold)) = fired {
        return anchored_stop(at, sold, points);
    }
    // The book stop's samples up to the tape's end — the one AT the last print included — read
    // the proxy the last prints left; the loop only ever reaches the samples before a print.
    if let Some(exit) = book.as_mut().and_then(|book| book.sample_before(tail + 1)) {
        return LineWalk { exit, points };
    }
    // Nothing closed it inside the tape. Not the report's own exit: a variant that never
    // closes is not a trade, whatever the core's rules did, and the caption counts it.
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
