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

use super::super::custom_ema::{EmaHistoryPath, cached_series};
use super::custom_ema_exit::{CustomEmaClock, first_check_ms, sell_delay_ms};
use super::pump_move::PumpMove;
use super::sell_order::{PriceDown, SellLevel, armed_at, take_model_for};
use super::stops::{StopTrigger, Stops, stop_side};
use super::{ExitParams, Side};
use crate::db::tuner::ticks::ModelSettings;
use crate::db::tuner::ticks::gap::{
    HOLE_PRICE_TOLERANCE, HOLE_TIME_SLACK_MS, TapeGap, fact_level_near, sell_not_nearer,
    stop_not_nearer, trigger_not_quicker,
};
use crate::db::tuner::ticks::mshot::MSHOT_TAKE_LATENCY_MS;
use crate::db::tuner::ticks::settings::StopFill;
use crate::db::tuner::ticks::{Deal, Exit, ExitKind, Fill, reaches, round_to_step};
use crate::feed::types::Tick;
use std::collections::VecDeque;

/// MoonShot's first take reaches the book `MSHOT_TAKE_LATENCY_MS` after arming, never sooner
/// than a re-place (the model latency floor). Other kinds keep the replacement latency.
/// First placement only: applying 220 ms to every exit move worsened MoonStrike/MoonHook takes
/// on the bench, while MoonShot take exits improved from 2114 to 3249 and stops stayed at 1994.
pub(crate) fn take_latency_ms(deal: &Deal, model: &ModelSettings) -> i64 {
    let latency_ms = model.latency_whole_ms();
    if take_model_for(&deal.kind) {
        MSHOT_TAKE_LATENCY_MS.max(latency_ms)
    } else {
        latency_ms
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
    /// Where the stop stood when the walk ended — the ladder's last step, else the first stop's
    /// level; `None` without a stop. What the verdict holds against the level the core printed.
    pub stop_level: Option<f64>,
    /// Which rule fired the stop the walk closed on; `None` when it closed otherwise — or on a
    /// stop judged across a hole of the tape, which no single rule fired.
    pub stop_trigger: Option<StopTrigger>,
    /// The trigger's fire moment, before the panic sell's optional order latency.
    pub stop_fired_ms: Option<i64>,
    /// Formula trigger moment, separate from its market fill; absent if the take filled first.
    pub custom_ema_fired_ms: Option<i64>,
    /// The first formula check lacked the required warm-up tape.
    pub custom_ema_short: bool,
    /// First SellLevel move whose look-back the admissible tape and bars do not cover.
    pub sell_level_short_at: Option<i64>,
    /// Source that warmed the first evaluable formula check.
    pub custom_ema_path: Option<EmaHistoryPath>,
    /// The first print once the take was live, when it was already through the untouched take:
    /// the take fills at the market there, not at its level. What the verdict prices the take at,
    /// while no move had landed by the moment it judges ([`Self::moved_at`]).
    pub marketable_take: Option<f64>,
    /// When the first move of the line reached the exchange; `None` when none did.
    pub moved_at: Option<i64>,
    /// The stop sale is priced strictly past the panic sell's floor (`Stops::sale_floor`), or
    /// the immediate stop has no print after the fill to price it: no sale the core could have
    /// made, so the outcome is left open and the verdict unjudged.
    pub past_floor: bool,
}

/// Whether no move of the line had landed at or before `horizon`: the take still untouched.
fn untouched_until(moved_at: Option<i64>, horizon: i64) -> bool {
    moved_at.is_none_or(|t| t > horizon)
}

impl LineWalk {
    /// The market fill of a take placed through the market, while the take stood untouched at
    /// `horizon` — no move landed at or before it.
    pub fn marketable_until(&self, horizon: i64) -> Option<f64> {
        self.marketable_take
            .filter(|_| untouched_until(self.moved_at, horizon))
    }
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
    /// When the take is on the book, after its initial placement latency from `armed_at`.
    live_at: i64,
    /// The take as placed, before any move landed: what "through the take" is judged against.
    take: f64,
    /// The exchange's level (what fills, on the grid).
    exch: f64,
    /// Whether a move has reached the exchange: what tells a fill at the take from a fill at
    /// a level a rule moved the line to — not the price, which a moved line can round back onto.
    exch_moved: bool,
    /// When the first move reached the exchange.
    moved_at: Option<i64>,
    /// The core's sell price: the ORDER's price once a move reached the book, the computed value
    /// while rounding kept the order where it was — see [`advance`]. The take is an order too.
    core: f64,
    /// The last level sent to the book, pending or not: what a new level must differ from to
    /// be a move at all.
    last_sent: f64,
    /// Moves the exchange has not seen yet, in arrival order: when each lands and its level.
    pending: VecDeque<(i64, f64)>,
    /// The last observed print and position direction, before the next replacement lands.
    last_print: Option<(f64, bool)>,
    /// Whether the market was already through the exchange's current replacement on arrival.
    marketable_move: bool,
    /// The first print once the take was live: `None` until one came, `Some(None)` when it did
    /// not reach the untouched take, `Some(Some((t_ms, price)))` when it was through it — a take
    /// placed through the market fills at that print, as the core's does. Only a take the
    /// market was ALREADY through when it went live (`prevailing`) counts: one resting short of
    /// the market and crossed later by a jump is a resting limit and fills at its level.
    first_live: Option<Option<(i64, f64)>>,
    /// The last print at or before `live_at`, the buy's own print included: the price the take
    /// met when it reached the book. On a sparse tape that may be the buy's own print or an
    /// earlier one — still the market the take meets.
    prevailing: Option<f64>,
    points: Vec<LinePoint>,
    /// Once a SellLevel move lacks history, later moves cannot restore a priced verdict.
    pub(super) sell_level_short_at: Option<i64>,
}

impl Line {
    /// The take on `tick`'s grid, live after `take_latency_ms`; moves use `latency_ms`.
    fn new(
        tick: Option<f64>,
        take: f64,
        armed_at: i64,
        take_latency_ms: i64,
        latency_ms: i64,
    ) -> Self {
        let take_placed = placed(tick, take);
        Self {
            tick,
            latency_ms,
            armed_at,
            live_at: armed_at + take_latency_ms,
            take: take_placed,
            exch: take_placed,
            exch_moved: false,
            moved_at: None,
            core: take_placed,
            last_sent: take_placed,
            pending: VecDeque::new(),
            last_print: None,
            marketable_move: false,
            first_live: None,
            prevailing: None,
            points: vec![LinePoint {
                t_ms: armed_at,
                price: take_placed,
            }],
            sell_level_short_at: None,
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
        self.pending.push_back((t_ms + self.latency_ms, order));
        self.points.push(LinePoint {
            t_ms: t_ms + self.latency_ms,
            price: order,
        });
        true
    }

    /// Apply every replacement due by `t_ms` in arrival order, retaining the first move's time.
    fn land(&mut self, t_ms: i64) {
        while let Some((apply_at, level)) = self.pending.pop_front_if(|(at, _)| *at <= t_ms) {
            self.exch = level;
            self.marketable_move = self
                .last_print
                .is_some_and(|(price, long)| reaches(price, level, !long));
            self.exch_moved = true;
            self.moved_at.get_or_insert(apply_at);
        }
    }

    /// Remember any print up to the moment the take goes live: the last one is the market the
    /// take meets on the book.
    fn see_before_live(&mut self, t_ms: i64, price: f64) {
        if t_ms <= self.live_at {
            self.prevailing = Some(price);
        }
    }

    /// Remember the latest print, and the first live print's marketability:
    /// the market ALREADY through the untouched take when it went live (`prevailing`), and
    /// that print through it too. A take crossed only later rests and fills at its level.
    /// Invariant: "through" is judged against the untouched take (`take`), never `exch` — a move
    /// landed by then already rules the marketable fill out (see [`Self::marketable_until`]).
    fn see_live(&mut self, t_ms: i64, price: f64, side: Side) {
        self.last_print = Some((price, side.long));
        // A print at exactly `armed_at` is prevailing, never the first live one (as `fill_on`).
        if self.first_live.is_none() && t_ms > self.armed_at && t_ms >= self.live_at {
            let through = |p: f64| reaches(p, self.take, !side.long);
            self.first_live = Some(
                (self.prevailing.is_some_and(through) && through(price)).then_some((t_ms, price)),
            );
        }
    }

    /// The market fill of a take placed through the market, while no move landed at or before
    /// `horizon` — the same test as [`LineWalk::marketable_until`].
    fn marketable_until(&self, horizon: i64) -> Option<(i64, f64)> {
        self.first_live
            .flatten()
            .filter(|_| untouched_until(self.moved_at, horizon))
    }

    /// Fill at `t_ms`, `price` once either the take or an earlier replacement is on the book.
    /// A replacement already through the prevailing market fills at the print; a resting
    /// replacement fills at its limit. The take's own arrival governs marketable-take tracking.
    fn fill_on(&self, t_ms: i64, price: f64, side: Side) -> Option<Exit> {
        let book_live_at = self
            .moved_at
            .map_or(self.live_at, |at| at.min(self.live_at));
        (t_ms > self.armed_at && t_ms >= book_live_at && reaches(price, self.exch, !side.long))
            .then_some(Exit {
                t_ms,
                // A take already through the market at its first live print fills at that print.
                price: if self.marketable_move {
                    price
                } else {
                    self.marketable_until(t_ms)
                        .filter(|(at, _)| *at == t_ms)
                        .map_or(self.exch, |(_, p)| p)
                },
                // What the print met: the take as placed, or a level a rule moved it to.
                kind: if !self.exch_moved {
                    ExitKind::Take
                } else {
                    ExitKind::Line
                },
            })
    }

    /// The level the exchange held at `t_ms` — the last level sent that reached it by then;
    /// `None` before the take was placed.
    fn level_at(&self, t_ms: i64) -> Option<f64> {
        self.points
            .iter()
            .rev()
            .find(|p| p.t_ms <= t_ms)
            .map(|p| p.price)
    }

    /// Close without trigger metadata; `finish` attaches the winning market rule's clock.
    fn close(self, exit: Exit) -> LineWalk {
        LineWalk {
            // The first live print alone; the verdict weighs it against `moved_at`.
            marketable_take: self.first_live.flatten().map(|(_, p)| p),
            moved_at: self.moved_at,
            exit,
            points: self.points,
            stop_level: None,
            stop_trigger: None,
            stop_fired_ms: None,
            past_floor: false,
            custom_ema_fired_ms: None,
            custom_ema_short: false,
            sell_level_short_at: self.sell_level_short_at,
            custom_ema_path: None,
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
/// verdict on a fact reads the line this way, at the close. Custom-EMA triggers are not held.
/// Initial placement uses [`take_latency_ms`]; subsequent moves use the model's latency.
///
/// Args:
///     hold_until_ms: `None` walks as [`walk`] does.
/// EMA history starts at the first check's warm-up while retaining the full variant tail.
pub fn walk_held(
    deal: &Deal,
    ticks: &[Tick],
    fill: Fill,
    take: f64,
    params: &ExitParams,
    hold_until_ms: Option<i64>,
) -> LineWalk {
    // Core logs place the sale in the condition's second, so the delay is read as time after
    // purchase before checks start: https://moonbot.pro/documentation/sell-order-tab/sell-order-tab-parameters
    let check_start_ms = fill
        .t_ms
        .saturating_add(sell_delay_ms(params.custom_ema_delay_s));
    let series = params.custom_ema.as_ref().map(|formula| {
        let second = |tick: &Tick| (tick.time_ms / 1000.0).floor() as i64;
        let first = first_check_ms(check_start_ms);
        let warm_from_s = first
            .div_euclid(1000)
            .saturating_sub(1)
            .saturating_sub(i64::from(formula.warm_up_s()));
        cached_series(
            ticks,
            ticks.first().map_or(0, second).max(warm_from_s),
            ticks.last().map_or(0, second),
        )
    });
    let mut clock = params
        .custom_ema
        .as_ref()
        .zip(series.as_deref())
        .map(|(formula, series)| {
            CustomEmaClock::new(
                formula,
                check_start_ms,
                deal.bars.as_deref().unwrap_or_default(),
                series,
            )
        });
    let mut walked = walk_with_clock(deal, ticks, fill, take, params, hold_until_ms, &mut clock);
    if let Some(clock) = clock {
        walked.custom_ema_short = clock.short_history;
        walked.custom_ema_path = clock.history_path;
        if walked.exit.kind == ExitKind::CustomEma {
            walked.custom_ema_fired_ms = clock.fired_ms;
        }
    }
    walked
}

/// Merge formula checks with the existing sell-line and stop clocks.
fn walk_with_clock(
    deal: &Deal,
    ticks: &[Tick],
    fill: Fill,
    take: f64,
    params: &ExitParams,
    hold_until_ms: Option<i64>,
    clock: &mut Option<CustomEmaClock<'_>>,
) -> LineWalk {
    let side = Side {
        long: deal.is_long(),
    };
    let armed_at = armed_at(fill, params);
    let mut line = Line::new(
        deal.tick,
        take,
        armed_at,
        take_latency_ms(deal, &params.model),
        params.model.latency_whole_ms(),
    );
    let mut price_down = PriceDown::new(params, deal, fill, side);
    let mut pump_move = PumpMove::new(params, fill, side, armed_at);
    let mut price_down_deferred = false;
    let mut sell_level = SellLevel::new(params, deal, fill, side);
    let mut stops = Stops::new(deal, ticks, fill, params, side);

    // A long position's hole, while the walk has yet to cross it.
    let mut hole = deal.gap.as_ref().filter(|gap| gap.to_ms > fill.t_ms);
    let mut last_t = fill.t_ms;
    let post_fill = ticks.partition_point(|tick| tick.time_ms as i64 <= fill.t_ms);
    for (index, tick) in ticks.iter().enumerate() {
        let t_ms = tick.time_ms as i64;
        let price = f64::from(tick.price);
        if !price.is_finite() || price <= 0.0 {
            continue;
        }
        line.see_before_live(t_ms, price);
        if t_ms <= fill.t_ms {
            line.last_print = Some((price, side.long));
            continue;
        }
        last_t = t_ms;
        let seen = &ticks[..=index];
        // Never evaluate through a known hole, even when the next print lies beyond it.
        let ema_until = hole
            .filter(|gap| t_ms > gap.from_ms.max(fill.t_ms))
            .map_or(t_ms, |gap| gap.from_ms.max(fill.t_ms));
        if let Some(clock) = clock.as_mut() {
            clock.next_fire(ema_until, seen);
        }
        let ema_at = clock.as_ref().and_then(|clock| clock.fired_ms);
        // The fact's own stop fired between the last print and this one: ahead of anything
        // this print does, and after everything the prints before it did.
        if let Some(exit) = stops.fired_by(t_ms) {
            if ema_at.is_none_or(|at| exit.t_ms <= at) {
                return finish(line, exit, &stops);
            }
        }
        // The first print past a hole is not the market's next print: the rules ran through
        // hours nobody holds. Everything due before the hole ran on the prints before it; what
        // the hole itself did is judged against the fact (`cross_hole`), never sold on here.
        if let Some(gap) = hole.filter(|gap| t_ms > gap.from_ms.max(fill.t_ms)) {
            if ema_at.is_none()
                && clock
                    .as_ref()
                    .is_some_and(|clock| clock.armed_by(gap.to_ms))
            {
                return finish(
                    line,
                    Exit {
                        t_ms: gap.from_ms.max(fill.t_ms),
                        price: f64::NAN,
                        kind: ExitKind::InGap,
                    },
                    &stops,
                );
            }
            // A formula already fired before the hole: its market-fill path judges the gap.
            if let Some(at) = ema_at {
                return finish_stop(
                    line,
                    Exit {
                        t_ms: at,
                        price,
                        kind: ExitKind::CustomEma,
                    },
                    &stops,
                    ticks,
                    post_fill,
                    index,
                    StopFillContext {
                        deal,
                        params,
                        hold_until_ms,
                    },
                );
            }
            hole = None;
            let from_ms = gap.from_ms.max(fill.t_ms);
            let rules = Rules {
                price_down_deferred: &mut price_down_deferred,
                pump_move: &mut pump_move,
                price_down: &mut price_down,
                sell_level: &mut sell_level,
            };
            rules.step_through(from_ms, &ticks[..index], &mut line);
            line.land(from_ms);
            let held_through = hold_until_ms.is_some_and(|until| until >= gap.to_ms);
            let rules = Rules {
                price_down_deferred: &mut price_down_deferred,
                pump_move: &mut pump_move,
                price_down: &mut price_down,
                sell_level: &mut sell_level,
            };
            if let Some(exit) = cross_hole(
                gap,
                from_ms,
                rules,
                &mut line,
                &stops,
                params,
                side,
                held_through,
            ) {
                return finish(line, exit, &stops);
            }
        }
        // The timer-driven rules moved the line at their own moments, between prints; every
        // step due by this print happened BEFORE it, and a step that also reached the book
        // before it is what this print meets.
        let rules = Rules {
            price_down_deferred: &mut price_down_deferred,
            pump_move: &mut pump_move,
            price_down: &mut price_down,
            sell_level: &mut sell_level,
        };
        rules.step_through(t_ms, seen, &mut line);
        line.land(t_ms);
        // The stops come before the print-driven rule below moves anything.
        let stop_exit = stops.on_print(tick, t_ms, price);
        let fire = match (stop_exit, ema_at) {
            (Some(stop), Some(at)) if stop.t_ms <= at => Some(stop),
            (_, Some(at)) => Some(Exit {
                t_ms: at,
                price,
                kind: ExitKind::CustomEma,
            }),
            (stop, None) => stop,
        };
        if let Some(exit) = fire {
            return finish_stop(
                line,
                exit,
                &stops,
                ticks,
                post_fill,
                index,
                StopFillContext {
                    deal,
                    params,
                    hold_until_ms,
                },
            );
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
        line.see_live(t_ms, price, side);
        let held = hold_until_ms.is_some_and(|until| t_ms <= until);
        if !held {
            if let Some(exit) = line.fill_on(t_ms, price, side) {
                return finish(line, exit, &stops);
            }
        }
    }
    let tail = ticks.last().map(|t| t.time_ms as i64).unwrap_or(last_t);
    // The fact's own stop past the last print, or the book stop's samples up to the tape's end.
    if let Some(exit) = stops.after_tape(tail) {
        return finish_stop(
            line,
            exit,
            &stops,
            ticks,
            post_fill,
            ticks.len(),
            StopFillContext {
                deal,
                params,
                hold_until_ms,
            },
        );
    }
    // Nothing closed it inside the tape. Not the report's own exit: a variant that never
    // closes is not a trade, whatever the core's rules did, and the caption counts it.
    finish(
        line,
        Exit {
            t_ms: tail,
            price: f64::NAN,
            kind: ExitKind::OpenAtWindowEnd,
        },
        &stops,
    )
}

/// The rules that move the sell line on their own clocks.
struct Rules<'r, 'a> {
    /// A PumpMove pass left PriceDown waiting for the next print's placement clock.
    price_down_deferred: &'r mut bool,
    pump_move: &'r mut PumpMove<'a>,
    price_down: &'r mut PriceDown<'a>,
    sell_level: &'r mut SellLevel<'a>,
}

impl Rules<'_, '_> {
    /// Drain steps in due-time order. At one due moment the pass matches the core's rule:
    /// SellLevel first, then PumpMove or PriceDown, chaining off the preceding level. A fired
    /// PumpMove suppresses a simultaneous PriceDown even without a price change; that step waits
    /// for the next print and is placed at its time. Other overdue steps keep their due moments.
    ///
    /// Args:
    ///     seen: The prints up to `t_ms` — what the price-following rules read.
    fn step_through(self, t_ms: i64, seen: &[Tick], line: &mut Line) {
        let mut suppress_price_down = false;
        loop {
            let price_down_due = self
                .price_down
                .due(t_ms)
                .filter(|_| !suppress_price_down)
                .map(|due| if *self.price_down_deferred { t_ms } else { due });
            let due = [
                self.sell_level.due(t_ms),
                self.pump_move.due(t_ms),
                price_down_due,
            ]
            .into_iter()
            .enumerate()
            .filter_map(|(rule, due)| due.map(|due| (due, rule)))
            .min();
            match due {
                None => break,
                Some((due, 0)) => self.sell_level.step(due, seen, line),
                Some((due, 1)) => {
                    self.pump_move.step(due, seen, line);
                    if price_down_due == Some(due) {
                        *self.price_down_deferred = true;
                        suppress_price_down = true;
                    }
                }
                Some((due, _)) => {
                    *self.price_down_deferred = false;
                    self.price_down.step(due, line);
                }
            }
        }
    }
}

/// Carry the walk across a long position's hole ([`TapeGap`]), from `from_ms` — its start, or the
/// fill when the take came after it — to its end, the PriceDown steps it holds taken on their
/// timer, which needs no print. `None` when the variant cannot have closed inside the hole; the
/// exit [`ExitKind::InGap`] when it may have.
///
/// It may have when a rule of it follows the price through the hole (SellLevel or the pump move
/// still to come, the trailing stop, a ladder rung still to take — their levels are functions of
/// prints nobody holds); when its stop stands nearer the price than the fact's, or fires on a
/// quicker trigger, and the fact does not prove the variant's own stop quiet; and when its sell
/// stood nearer the price than the core's line at some moment of the hole, or the core's line is
/// not on record for that moment. The core's own line and stop were not reached there, or the
/// trade would have closed inside it.
///
/// Args:
///     held_through: The walk holds the sell past the hole — the verdict judging where the line
///         stood at the close: no print fills the sell there, so its comparison proves nothing
///         the verdict needs.
#[allow(clippy::too_many_arguments)]
fn cross_hole(
    gap: &TapeGap,
    from_ms: i64,
    rules: Rules<'_, '_>,
    line: &mut Line,
    stops: &Stops,
    params: &ExitParams,
    side: Side,
    held_through: bool,
) -> Option<Exit> {
    let in_gap = Some(Exit {
        t_ms: from_ms,
        price: f64::NAN,
        kind: ExitKind::InGap,
    });
    if *rules.price_down_deferred
        || rules.pump_move.due(i64::MAX).is_some()
        || rules.sell_level.due(i64::MAX).is_some()
        || stops.follows_price()
    {
        return in_gap;
    }
    if let Some(level) = stops.level().filter(|_| stops.quiet_until() < gap.to_ms) {
        let bounded = gap.fact_stop.is_some_and(|fact| {
            stop_not_nearer(level, fact.level, side.long)
                && trigger_not_quicker(params.fast_stop_loss, params.stop_loss_ema, &fact)
        });
        if !bounded {
            return in_gap;
        }
    }
    while let Some(due) = rules.price_down.due(gap.to_ms) {
        rules.price_down.step(due, line);
    }
    line.land(gap.to_ms);
    if held_through {
        return None;
    }
    // Every moment either line moved — the core's within the slack either side — and the hole's
    // start: both are steps, so the comparison at each is the comparison throughout. Within the
    // verdict's default tolerances the two lines are one line (`gap::fact_level_near`).
    let fact_points = gap.fact_line.as_deref().unwrap_or_default();
    let slack_ms = HOLE_TIME_SLACK_MS;
    let price_tolerance = HOLE_PRICE_TOLERANCE;
    let mut moments: Vec<i64> = std::iter::once(from_ms)
        .chain(line.points.iter().map(|p| p.t_ms))
        .chain(
            fact_points
                .iter()
                .flat_map(|(t, _)| [t.saturating_sub(slack_ms), *t, t.saturating_add(slack_ms)]),
        )
        .filter(|t| (from_ms..=gap.to_ms).contains(t))
        .collect();
    moments.sort_unstable();
    moments.dedup();
    let reached = moments.into_iter().any(|t| match line.level_at(t) {
        // No sell stood yet (`SellDelay`): nothing to reach.
        None => false,
        Some(variant) => fact_level_near(fact_points, t, slack_ms, side.long)
            .is_none_or(|fact| !sell_not_nearer(variant, fact, side.long, price_tolerance)),
    });
    if reached { in_gap } else { None }
}

/// Close on `exit`, retaining the stop level and only the winning market rule's trigger clock.
fn finish(line: Line, exit: Exit, stops: &Stops) -> LineWalk {
    let mut walked = line.close(exit);
    walked.stop_level = stops.level();
    walked.stop_trigger = stops
        .fired_trigger()
        .filter(|_| exit.kind == ExitKind::Stop);
    walked.stop_fired_ms = (exit.kind == ExitKind::Stop).then_some(exit.t_ms);
    walked.custom_ema_fired_ms = (exit.kind == ExitKind::CustomEma).then_some(exit.t_ms);
    walked
}

/// Inputs shared by the resting sell and its panic replacement.
struct StopFillContext<'a> {
    deal: &'a Deal,
    params: &'a ExitParams,
    hold_until_ms: Option<i64>,
}

/// Resolve a stop or custom-EMA market sell while retaining its kind and original fire time.
/// Without an arrival print, carry the last consumed post-fill stop-side price, else fire price.
/// Only prints at or past `post_fill` can price the replacement.
fn finish_stop(
    mut line: Line,
    fire: Exit,
    stops: &Stops,
    ticks: &[Tick],
    post_fill: usize,
    index: usize,
    context: StopFillContext<'_>,
) -> LineWalk {
    let StopFillContext {
        deal,
        params,
        hold_until_ms,
    } = context;
    let model = &params.model;
    let index = index.max(post_fill);
    if model.stop_fill == StopFill::MarketAfterLatency
        && (fire.kind == ExitKind::CustomEma || stops.fired_trigger() != Some(StopTrigger::Fact))
    {
        let arrival = fire.t_ms.saturating_add(model.latency_whole_ms());
        let side = Side {
            long: deal.is_long(),
        };
        for tick in &ticks[index..] {
            let t_ms = tick.time_ms as i64;
            if t_ms >= arrival {
                break;
            }
            if let Some(gap) = deal
                .gap
                .as_ref()
                .filter(|gap| fire.t_ms < gap.to_ms && t_ms > gap.from_ms)
            {
                return finish(
                    line,
                    Exit {
                        t_ms: fire.t_ms.max(gap.from_ms),
                        price: f64::NAN,
                        kind: ExitKind::InGap,
                    },
                    stops,
                );
            }
            let price = f64::from(tick.price);
            if !price.is_finite() || price <= 0.0 {
                continue;
            }
            line.see_live(t_ms, price, side);
            if !hold_until_ms.is_some_and(|until| t_ms <= until) {
                if let Some(exit) = line.fill_on(t_ms, price, side) {
                    return finish(line, exit, stops);
                }
            }
        }
    }
    let mut walked = finish(line, fire, stops);
    let stop_side = stop_side(deal.is_long());
    let valid = |tick: &&Tick| tick.side == stop_side && tick.price.is_finite() && tick.price > 0.0;
    if model.stop_fill == StopFill::MarketAfterLatency
        && walked.stop_trigger != Some(StopTrigger::Fact)
    {
        let arrival = fire.t_ms.saturating_add(model.latency_whole_ms());
        let next_print = ticks[index..]
            .iter()
            .filter(valid)
            .find(|tick| tick.time_ms as i64 >= arrival);
        // A fallback searched to the tape end, even when the carried quote precedes a hole.
        let searched_until = next_print.map_or_else(
            || {
                ticks
                    .last()
                    .map_or(arrival, |tick| arrival.max(tick.time_ms as i64))
            },
            |tick| tick.time_ms as i64,
        );
        let print = next_print.or_else(|| ticks[post_fill..index].iter().rfind(valid));
        // No print after the fill shows where the immediate stop's order filled.
        walked.past_floor = print.is_none() && walked.stop_trigger == Some(StopTrigger::Immediate);
        // A short tape still dates the order at arrival; the last bid/ask supplies its price.
        walked.exit.t_ms = arrival;
        if let Some(print) = print {
            walked.exit.t_ms = arrival.max(print.time_ms as i64);
            walked.exit.price = f64::from(print.price);
        }
        if let Some(gap) = deal
            .gap
            .as_ref()
            .filter(|gap| fire.t_ms < gap.to_ms && searched_until > gap.from_ms)
        {
            walked.exit = Exit {
                t_ms: fire.t_ms.max(gap.from_ms),
                price: f64::NAN,
                kind: ExitKind::InGap,
            };
            walked.stop_trigger = None;
            walked.stop_fired_ms = None;
            walked.past_floor = false;
            walked.custom_ema_fired_ms = None;
        }
    } else if walked.stop_trigger == Some(StopTrigger::Immediate) {
        // No print after the fill shows where the immediate stop's order filled.
        walked.past_floor = !ticks[post_fill..].iter().any(|tick| valid(&tick));
    }
    // A sale past the panic sell's floor is none the core could have made; the fact's own
    // stop keeps the fact's sale.
    if walked.exit.kind == ExitKind::Stop && walked.stop_trigger != Some(StopTrigger::Fact) {
        let long = deal.is_long();
        let price = walked.exit.price;
        if let Some(floor) = stops.sale_floor(params, price, long) {
            // Past the floor only by more than the price tolerance.
            let tol = params.model.price_pct / 100.0;
            walked.past_floor |= if long {
                price < floor * (1.0 - tol)
            } else {
                price > floor * (1.0 + tol)
            };
        }
    }
    walked
}

#[cfg(test)]
mod tests;
