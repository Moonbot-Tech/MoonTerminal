//! The strategy window's "Sell order" section: the take, `SellDelay`, `PriceDown*` and
//! `SellLevel*`.
//!
//! - **The take** is `SellPrice` per cent above the fill for every kind but two: MoonHook, whose
//!   take replaces it with `HookSellLevel` per cent of the trade's own detect depth
//!   ([`crate::db::tuner::ticks::hook`]), and Spread, whose take is the edge of the spread it
//!   detected — a level, not a rule, taken as the core recorded it ([`take_is_recorded`]). The
//!   Delta-Modifier family moves it ([`super::delta_mods`]). It is raised by
//!   `MShotSellAtLastPrice` to the pre-spike price less `MShotSellPriceAdjust` (the FAQ: "the
//!   4-second-old ASK, i.e. before the spike"; the model takes the ask the caller recovered from
//!   the order archive (`Deal::pre_spike_ask`), else reads the last print at least
//!   `ModelSettings::pre_spike_lookback_ms` ([`crate::db::tuner::ticks::mshot::PRE_SPIKE_LOOKBACK_MS`]
//!   by default) before the fill, since the tape has no book).
//! - **SellDelay** — milliseconds after the fill before the take is placed ([`armed_at`]).
//!
//! From the Moonbot FAQ (`PriceDown*`, `SellLevel*` answers) and the live strategies
//! (2026-09-20: 862 of 1 331 run `PriceDownTimer` 1 s with `PriceDownPercent` 50 relative,
//! `SellLevelDelay` is absent everywhere):
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

use super::line::Line;
use super::{ExitModel, ExitParams, Side, due_by};
use crate::db::tuner::ticks::hook::{KIND_MOONHOOK, hook_take_pct};
use crate::db::tuner::ticks::{Deal, Fill};
use crate::feed::types::Tick;

impl ExitModel<'_> {
    /// The take-profit level for a fill: `SellPrice` off the fill, lifted to the pre-spike
    /// ask (the archive's, else the tape's last print) less the adjustment when
    /// `MShotSellAtLastPrice` is on. Long above, short below.
    pub fn take_level(&self, deal: &Deal, ticks: &[Tick], fill: Fill) -> f64 {
        // A kind whose take rule the model does not have starts where the core's line did —
        // when the fact is being judged; a variant computes the take from the rules for every
        // kind (see `ExitParams::take_from_archive`) but the one whose take is not a rule at
        // all ([`take_is_recorded`]). The recorded level is the core's own, modifiers and all,
        // so nothing below is added to it.
        if (self.params.take_from_archive && !take_model_for(&deal.kind))
            || take_is_recorded(&deal.kind)
        {
            if let Some(take) = deal.archived_take.filter(|t| t.is_finite() && *t > 0.0) {
                return take;
            }
        }
        // Floored at zero: a modifier deep enough to drive the distance negative would put the
        // TAKE on the losing side of the entry and turn every level the line steps down from
        // inside out. The rules that legitimately sell below the entry are the moving ones
        // (`PriceDownAllowedDrop`, a negative `SellShotDistance`), and they get there by
        // stepping down from the take, not by starting underneath it.
        let pct = (self.base_take_pct(deal) + self.modifier_pct(deal, fill.t_ms)).max(0.0);
        let mshot = take_model_for(&deal.kind);
        let mut take = if deal.is_long() {
            fill.price * (1.0 + pct / 100.0)
        } else if mshot {
            // The core divides a short MoonShot's take off the fill (the core developer,
            // 2026-09-23): `fill / (1 + SellPrice/100)`. The two archived short takes that
            // SellPrice placed and whose price step tells the formulas apart (ONE, BCH_RP) sit
            // on it; MoonHook's stored take is rounded too coarsely to tell, and keeps the
            // product.
            fill.price / (1.0 + pct / 100.0)
        } else {
            fill.price * (1.0 - pct / 100.0)
        };
        if self.params.sell_at_last_price {
            let pre = deal
                .pre_spike_ask
                .filter(|p| p.is_finite() && *p > 0.0)
                .or_else(|| {
                    pre_spike_price(ticks, fill.t_ms, self.params.model.pre_spike_lookback_ms)
                });
            if let (Some(pre), Some(factor)) = (pre, ask_take_factor(self.params, deal.is_short)) {
                take = if deal.is_long() {
                    take.max(pre * factor)
                } else {
                    take.min(pre * factor)
                };
            }
        }
        take
    }

    /// The take distance before the modifiers, per cent of the fill: `HookSellLevel` of the
    /// trade's own detect depth for a MoonHook, `SellPrice` for every other kind.
    ///
    /// A hook whose depth or level is unknown falls back to `SellPrice` so the line still has
    /// somewhere to step down from — and [`Self::take_known`] answers `false` for it, which is
    /// what keeps that fallback out of the verdict.
    fn base_take_pct(&self, deal: &Deal) -> f64 {
        if deal.kind == KIND_MOONHOOK && self.params.hook_sell_level_pct > 0.0 {
            if let Some(depth) = deal.hook_depth_pct.filter(|d| d.is_finite() && *d > 0.0) {
                return hook_take_pct(depth, self.params.hook_sell_level_pct);
            }
        }
        self.params.sell_price_pct
    }

    /// Whether a walk under these parameters knows where the trade's take stands — the level
    /// every PriceDown step and every fill of the line is counted from.
    ///
    /// Asked with a VARIANT's parameters, so it answers for the variants: a trade whose take a
    /// variant cannot place is one the search cannot run, whatever the fact's own replay did
    /// with the level the core recorded. Per kind:
    ///
    /// - a MoonHook needs its rule's inputs — the detect depth and `HookSellLevel`, with
    ///   `HookSellFixed` off (that branch computes the distance differently and is not modelled:
    ///   no live strategy sets it, so it could not be checked against anything);
    /// - a Spread needs the take the core recorded ([`take_is_recorded`]);
    /// - a MoonShot lifted to the pre-spike ask (`MShotSellAtLastPrice`) needs that ask off the
    ///   core's record (`Deal::pre_spike_ask`) — the tape's print before the spike sits 0.1–0.5 %
    ///   under the book's ask on a dump, and on 88 stopped MoonShot trades (2026-09-23) a take
    ///   placed off it was touched before the core's stop on 30, turning a loss into a win;
    /// - every other kind takes `SellPrice`, known by construction.
    ///
    /// `false` is not "the model was wrong": it is "this trade's take is not modelled here", and
    /// [`crate::db::tuner::ticks::verify`] then answers the exit group with nothing — which keeps
    /// the trade out of the search (`record::fit_for_search`).
    pub fn take_known(&self, deal: &Deal) -> bool {
        let positive = |v: Option<f64>| v.is_some_and(|x| x.is_finite() && x > 0.0);
        if deal.kind == KIND_MOONHOOK {
            return !self.params.hook_sell_fixed
                && self.params.hook_sell_level_pct > 0.0
                && positive(deal.hook_depth_pct);
        }
        if take_is_recorded(&deal.kind) {
            return positive(deal.archived_take);
        }
        if take_model_for(&deal.kind) && self.params.sell_at_last_price {
            return positive(deal.pre_spike_ask);
        }
        true
    }
}

/// The kind name of Spread as the strategy list spells it.
pub const KIND_SPREAD: &str = "Spread";

/// Whether the kind's take is not a rule of its parameters but a level the core took off the
/// detect — the spread it detected. `SellPrice` places it on 5 of 157 archived Spread takes
/// (2026-09-23) — the coincidences it takes for the width of a spread to land on the field. The
/// report's `comment` keeps only the width, rounded to 0.1 %, so the level is the take the order
/// archive recorded or nothing, for the fact and for every variant alike; and `SellPrice` is no
/// knob of the kind (`params::TICK_PARAMS`).
pub fn take_is_recorded(kind: &str) -> bool {
    kind == KIND_SPREAD
}

/// Whether the take the model computes for the kind is the kind's OWN rule, rather than the
/// general `SellPrice` — true for MoonShot, whose `MShotSellAtLastPrice` lift belongs to it
/// alone. For the rest the verdict prefers the archived level when the trade has one
/// (`Deal::archived_take`, `ExitParams::take_from_archive`), because the core's placed level
/// carries the delta modifiers exactly as the core applied them.
///
/// It is NOT the test for "is the take known at all" — that is [`ExitModel::take_known`], and
/// reading this one in its place silenced five legitimate verdicts on the live sample
/// (2026-09-22), four of them hits: `SellPrice` is the take of every kind but MoonHook, and a
/// Spread trade without an archived line is judged by it perfectly well.
pub fn take_model_for(kind: &str) -> bool {
    crate::db::tuner::ticks::entry::entry_model_for(kind)
}

/// The take as an archived Exit line records it: its first point, when it is a price.
pub fn archived_take(exit_points: Option<&[(i64, f64)]>) -> Option<f64> {
    let (_, take) = exit_points?.first().copied()?;
    (take.is_finite() && take > 0.0).then_some(take)
}

/// What `MShotSellAtLastPrice` multiplies the ask by to place the take: `1 − adjust/100` for a
/// long, `1/(1 − adjust/100)` for a short, whose take sits below the entry and is adjusted UP
/// toward it (the core developer, 2026-09-23: `max(Y·(1 − adj/100), …)` and
/// `min(Y/(1 − adj/100), …)`). `None` for an adjustment of 100 % or more, which leaves no price.
pub fn ask_take_factor(params: &ExitParams, is_short: bool) -> Option<f64> {
    let keep = 1.0 - params.sell_price_adjust_pct / 100.0;
    // NaN included: an adjustment the strategy did not spell as a number leaves no price.
    if keep.is_nan() || keep <= 0.0 {
        return None;
    }
    Some(if is_short { 1.0 / keep } else { keep })
}

/// The pre-spike ask behind an archived Exit line: its first point is the take as the core
/// placed it, the ask times [`ask_take_factor`] when `MShotSellAtLastPrice` placed it, so the
/// ask is that point with the factor divided out. The ask's branch carries no delta modifier
/// (the core developer, 2026-09-23), so the ask read back is the core's own, to the price step.
/// `None` when the rule was off (the take came from `SellPrice`, and the archive says nothing
/// about the ask), when the archive holds no Exit line, or when the first point is not a price.
///
/// When `SellPrice` alone set the take farther than the ask would have, the division reads a
/// slightly high ask back — and the same `max` (a long) or `min` (a short, whose take sits
/// below the entry) puts the take on `SellPrice` again, so the trade's own replay is exact
/// either way; a variant with a smaller adjustment inherits the overread. On the live sample
/// (2026-09-23) the ask placed 776 of 785 archived MoonShot takes.
///
/// Args:
///     exit_points: The archived Exit line's `(t_ms, price)` points, in the archive's order.
///     params: The sell-line parameters as of the trade.
///     is_short: The trade's side — which way the adjustment went.
pub fn archived_pre_spike_ask(
    exit_points: Option<&[(i64, f64)]>,
    params: &ExitParams,
    is_short: bool,
) -> Option<f64> {
    if !params.sell_at_last_price {
        return None;
    }
    let factor = ask_take_factor(params, is_short)?;
    let (_, take) = exit_points?.first().copied()?;
    (take.is_finite() && take > 0.0).then_some(take / factor)
}

/// The last print at least `lookback_ms` ([`crate::db::tuner::ticks::mshot::PRE_SPIKE_LOOKBACK_MS`]
/// by default) before `at_ms` — the FAQ's "price before the spike".
pub fn pre_spike_price(ticks: &[Tick], at_ms: i64, lookback_ms: i64) -> Option<f64> {
    let cutoff = at_ms - lookback_ms;
    ticks
        .iter()
        .rev()
        .find(|t| (t.time_ms as i64) <= cutoff && t.price > 0.0)
        .map(|t| f64::from(t.price))
}

/// The terminal's own floor on a step delay of zero: the FAQ's "0.33 s internal minimum".
pub const STEP_FLOOR_MS: i64 = 330;

/// Seconds to milliseconds, with the terminal's floor (`ModelSettings::step_floor_ms`,
/// [`STEP_FLOOR_MS`] by default) for a zero delay.
pub(in crate::db::tuner::ticks) fn step_ms(seconds: f64, floor_ms: i64) -> i64 {
    let ms = (seconds * 1000.0) as i64;
    if ms <= 0 { floor_ms } else { ms }
}

/// When the take is placed: `SellDelay` after the fill. Prints inside the delay cannot fill it,
/// and the take-timed rules (the pump move) count from here.
pub(super) fn armed_at(fill: Fill, params: &ExitParams) -> i64 {
    fill.t_ms + params.sell_delay_ms.max(0.0) as i64
}

/// PriceDown's clock and floor.
pub(super) struct PriceDown<'a> {
    params: &'a ExitParams,
    fill: Fill,
    side: Side,
    /// When the next step is due; `None` when the rule is off or reached its floor.
    next: Option<i64>,
    /// `PriceDownAllowedDrop` over the buy.
    floor: f64,
    /// `PriceDownDelay`, with the terminal's floor.
    delay_ms: i64,
    /// The core's own lag between a step going through and the next one's timer
    /// (`Deal::step_lag_ms`).
    lag_ms: i64,
}

impl<'a> PriceDown<'a> {
    pub(super) fn new(params: &'a ExitParams, deal: &Deal, fill: Fill, side: Side) -> Self {
        let pd_on = params.price_down_timer_s > 0.0 && params.price_down_pct > 0.0;
        Self {
            params,
            fill,
            side,
            next: pd_on.then(|| fill.t_ms + (params.price_down_timer_s * 1000.0) as i64),
            floor: side.over(fill.price, params.price_down_allowed_drop_pct),
            delay_ms: step_ms(params.price_down_delay_s, params.model.step_floor_ms),
            lag_ms: deal.step_lag_ms.max(0.0) as i64,
        }
    }

    /// The step due by the print at `t_ms`, if any.
    pub(super) fn due(&self, t_ms: i64) -> Option<i64> {
        due_by(self.next, t_ms)
    }

    /// The step due at `due`: the line lowered by `PriceDownPercent` from where it stands,
    /// never below the floor; at the floor the rule stops.
    pub(super) fn step(&mut self, due: i64, line: &mut Line) {
        let (params, fill, side) = (self.params, self.fill, self.side);
        let core = line.core();
        let next = if params.price_down_relative {
            core - (core - fill.price) * params.price_down_pct / 100.0
        } else {
            core - side.over(fill.price, params.price_down_pct) + fill.price
        };
        let next = side.farther(next, self.floor);
        if (next - core).abs() <= f64::EPSILON * core.abs() {
            self.next = None;
            return;
        }
        // The core times the next step from this one's going through (`Deal::step_lag_ms`);
        // a step rounding kept in place sent nothing and waits for nothing — over the
        // archived GateF lines two delays with such a step between them run 32 ms over,
        // against 47 ms for one real step.
        let moved = line.place(due, next);
        let lag_ms = if moved { self.lag_ms } else { 0 };
        self.next = Some(due + self.delay_ms + lag_ms);
    }
}

/// SellLevel's clock, count and floor.
pub(super) struct SellLevel<'a> {
    params: &'a ExitParams,
    fill: Fill,
    side: Side,
    /// When the next move is due; `None` when the rule is off or done.
    next: Option<i64>,
    /// The spacing of the moves after the first.
    every_ms: i64,
    /// Moves left of `SellLevelCount`.
    left: u32,
    /// The end of `SellLevelWorkTime`, `None` without one.
    until: Option<i64>,
    /// `SellLevelAllowedDrop` over the buy.
    floor: f64,
}

impl<'a> SellLevel<'a> {
    pub(super) fn new(params: &'a ExitParams, fill: Fill, side: Side) -> Self {
        let floor_ms = params.model.step_floor_ms;
        let sl_on = params.sell_level_delay_s != 0.0
            && params.sell_level_time_s > 0.0
            && params.sell_level_count > 0;
        let sl_first_ms = if params.sell_level_delay_s < 0.0 {
            floor_ms
        } else {
            (params.sell_level_delay_s * 1000.0) as i64
        };
        let every_ms = if params.sell_level_delay_next_s > 0.0 {
            (params.sell_level_delay_next_s * 1000.0) as i64
        } else if params.sell_level_delay_next_s < 0.0 {
            floor_ms
        } else {
            sl_first_ms.max(floor_ms)
        };
        Self {
            params,
            fill,
            side,
            next: sl_on.then(|| fill.t_ms + sl_first_ms),
            every_ms,
            left: params.sell_level_count,
            until: (params.sell_level_work_time_s > 0.0)
                .then(|| fill.t_ms + (params.sell_level_work_time_s * 1000.0) as i64),
            floor: side.over(fill.price, params.sell_level_allowed_drop_pct),
        }
    }

    /// Every move due by the print at `t_ms`: to the high of the look-back, adjusted.
    ///
    /// Args:
    ///     seen: The prints up to and including the one at `t_ms`.
    pub(super) fn catch_up(&mut self, t_ms: i64, seen: &[Tick], line: &mut Line) {
        let (params, fill, side) = (self.params, self.fill, self.side);
        while let Some(due) = due_by(self.next, t_ms) {
            if self.left == 0 || self.until.is_some_and(|until| due > until) {
                self.next = None;
                break;
            }
            let from = due - (params.sell_level_time_s * 1000.0) as i64;
            if let Some(high) = side.extreme_between(seen, from, due) {
                let next = if params.sell_level_relative {
                    fill.price + (high - fill.price) * params.sell_level_adjust_pct / 100.0
                } else {
                    side.over(high, params.sell_level_adjust_pct)
                };
                let next = side.farther(next, self.floor);
                line.place(due, next);
            }
            self.left -= 1;
            self.next = Some(due + self.every_ms);
        }
    }
}

#[cfg(test)]
mod tests;
