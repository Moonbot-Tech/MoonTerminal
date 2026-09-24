//! The moving sell line: where the sell order stood at every moment after the fill, and which
//! print crossed it — the step every section's rules share. The rules themselves live with their
//! section of the strategy window: [`super::stops`], [`super::sell_order`],
//! [`super::delta_mods`], and PumpsDetection's own [`super::pump_move`].
//!
//! Every rule is written for a long and mirrored for a short by `Side`. A replacement reaches
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

use super::pump_move::PumpMove;
use super::sell_order::{PriceDown, SellLevel, armed_at};
use super::stops::Stops;
use super::{ExitParams, Side};
use crate::db::tuner::ticks::{Deal, Exit, ExitKind, Fill, reaches, round_to_step};
use crate::feed::types::Tick;

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

/// What the exchange is given: the level on the price grid of `tick`, the exact level when the
/// grid is unknown.
fn placed(tick: Option<f64>, level: f64) -> f64 {
    match tick {
        Some(tick) => round_to_step(level, tick),
        None => level,
    }
}

/// The sell order as the rules move it: the core's price, what the exchange holds, the move on
/// its way there, and every level it was sent to.
pub(super) struct Line {
    /// The market's price step, `None` when the market's grid is unknown.
    tick: Option<f64>,
    latency_ms: i64,
    /// When the take was placed (`SellDelay` after the fill).
    armed_at: i64,
    /// When the take is on the book: placed at `armed_at`, there after the same latency as any
    /// move of the line.
    live_at: i64,
    /// The exchange's level (what fills, on the grid).
    exch: f64,
    /// Whether a move has reached the exchange: what tells a fill at the take from a fill at
    /// a level a rule moved the line to — not the price, which a moved line can round back onto.
    exch_moved: bool,
    /// The core's sell price: the ORDER's price once a move reached the book, the computed value
    /// while rounding kept the order where it was — see [`advance`]. The take is an order too.
    core: f64,
    /// The last level sent to the book, pending or not: what a new level must differ from to
    /// be a move at all.
    last_sent: f64,
    /// A move the exchange has not seen yet: when it lands, and at what level.
    pending: Option<(i64, f64)>,
    points: Vec<LinePoint>,
}

impl Line {
    /// The take, placed at `armed_at` on the grid of `tick`.
    fn new(tick: Option<f64>, take: f64, armed_at: i64, latency_ms: i64) -> Self {
        let take_placed = placed(tick, take);
        Self {
            tick,
            latency_ms,
            armed_at,
            live_at: armed_at + latency_ms,
            exch: take_placed,
            exch_moved: false,
            core: take_placed,
            last_sent: take_placed,
            pending: None,
            points: vec![LinePoint {
                t_ms: armed_at,
                price: take_placed,
            }],
        }
    }

    /// The core's sell price, which every rule moves from.
    pub(super) fn core(&self) -> f64 {
        self.core
    }

    /// Send the line to `level` at `t_ms`. Answers whether the order moved — a replace went to
    /// the book.
    pub(super) fn place(&mut self, t_ms: i64, level: f64) -> bool {
        if (level - self.core).abs() <= f64::EPSILON * self.core.abs() {
            return false;
        }
        let order = placed(self.tick, level);
        if !advance(&mut self.core, &mut self.last_sent, level, order) {
            return false;
        }
        self.pending = Some((t_ms + self.latency_ms, order));
        self.points.push(LinePoint {
            t_ms: t_ms + self.latency_ms,
            price: order,
        });
        true
    }

    /// The move on its way lands on the exchange, when it is due by `t_ms`.
    fn land(&mut self, t_ms: i64) {
        if let Some((_, level)) = self.pending.filter(|(apply_at, _)| t_ms >= *apply_at) {
            self.exch = level;
            self.exch_moved = true;
            self.pending = None;
        }
    }

    /// The print at `t_ms`, `price` filling the order the exchange holds: once the take is on
    /// the book, a print AT the level or through it.
    fn fill_on(&self, t_ms: i64, price: f64, side: Side) -> Option<Exit> {
        (t_ms > self.armed_at && t_ms >= self.live_at && reaches(price, self.exch, !side.long))
            .then_some(Exit {
                t_ms,
                price: self.exch,
                // What the print met: the take as placed, or a level a rule moved it to.
                kind: if !self.exch_moved {
                    ExitKind::Take
                } else {
                    ExitKind::Line
                },
            })
    }

    fn close(self, exit: Exit) -> LineWalk {
        LineWalk {
            exit,
            points: self.points,
        }
    }
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
    let armed_at = armed_at(fill, params);
    let mut line = Line::new(deal.tick, take, armed_at, params.model.latency_whole_ms());
    let mut price_down = PriceDown::new(params, deal, fill, side);
    let mut pump_move = PumpMove::new(params, fill, side, armed_at);
    let mut sell_level = SellLevel::new(params, deal, fill, side);
    let mut stops = Stops::new(deal, ticks, fill, params, side);

    let mut last_t = fill.t_ms;
    for (index, tick) in ticks.iter().enumerate() {
        let t_ms = tick.time_ms as i64;
        let price = f64::from(tick.price);
        if t_ms <= fill.t_ms || !price.is_finite() || price <= 0.0 {
            continue;
        }
        last_t = t_ms;
        let seen = &ticks[..=index];
        // The fact's own stop fired between the last print and this one: ahead of anything
        // this print does, and after everything the prints before it did.
        if let Some(exit) = stops.fired_by(t_ms) {
            return line.close(exit);
        }
        // The timer-driven rules moved the line at their own moments, between prints; every
        // step due by this print happened BEFORE it, and a step that also reached the book
        // before it is what this print meets.
        //
        // PriceDown steps, the pump move and SellLevel's moves, one per due moment, in the
        // order they fell due — on a tie the pump move, then PriceDown, then SellLevel: each step
        // chains off where the one before it left the line. (SellLevel ran as a pass of its own
        // after the others until 2026-09-24, so a PriceDown step due after a SellLevel move went
        // first, off the level the move then replaced.)
        loop {
            let due = [
                pump_move.due(t_ms),
                price_down.due(t_ms),
                sell_level.due(t_ms),
            ]
            .into_iter()
            .enumerate()
            .filter_map(|(rule, due)| due.map(|due| (due, rule)))
            .min();
            match due {
                None => break,
                Some((due, 0)) => pump_move.step(due, seen, &mut line),
                Some((due, 1)) => price_down.step(due, &mut line),
                Some((due, _)) => sell_level.step(due, seen, &mut line),
            }
        }
        line.land(t_ms);
        // The stops come before the print-driven rule below moves anything.
        if let Some(exit) = stops.on_print(tick, t_ms, price) {
            return line.close(exit);
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
        if !held {
            if let Some(exit) = line.fill_on(t_ms, price, side) {
                return line.close(exit);
            }
        }
    }
    let tail = ticks.last().map(|t| t.time_ms as i64).unwrap_or(last_t);
    // The fact's own stop past the last print, or the book stop's samples up to the tape's end.
    if let Some(exit) = stops.after_tape(tail) {
        return line.close(exit);
    }
    // Nothing closed it inside the tape. Not the report's own exit: a variant that never
    // closes is not a trade, whatever the core's rules did, and the caption counts it.
    line.close(Exit {
        t_ms: tail,
        price: f64::NAN,
        kind: ExitKind::OpenAtWindowEnd,
    })
}

#[cfg(test)]
mod tests;
