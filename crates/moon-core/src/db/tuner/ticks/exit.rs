//! The exit side: where the sell line stood after the fill, and which print crossed it. One
//! model for every strategy kind — after the entry filled, the exit of any strategy is a
//! function of the sell-order rules and the tape.
//!
//! The take-profit is `SellPrice` per cent above the fill for every kind but two: MoonHook,
//! whose take replaces it with `HookSellLevel` per cent of the trade's own detect depth
//! ([`super::hook`]), and Spread, whose take is the edge of the spread it detected — a level,
//! not a rule, taken as the core recorded it ([`take_is_recorded`]). The rules are moved by the
//! Delta-Modifier family (`SellModifier`). It is
//! raised by `MShotSellAtLastPrice` to
//! the pre-spike price less `MShotSellPriceAdjust` (the FAQ: "the 4-second-old ASK, i.e. before
//! the spike"; the model takes the ask the caller recovered from the order archive
//! (`Deal::pre_spike_ask`), else reads the last print at least [`PRE_SPIKE_LOOKBACK_MS`] before
//! the fill, since the tape has no book). From there the line moves under the strategy's sell rules
//! — `PriceDown*`, `SellLevel*`, `SellShot*` — and the stop fires under `StopLoss*`; see
//! [`super::line`]. A position nothing closed inside the tape is [`ExitKind::OpenAtWindowEnd`]:
//! not a trade, whatever the core's exit was.

use super::hook::{KIND_MOONHOOK, hook_take_pct};
use super::line::{LineWalk, walk, walk_held};
use super::mshot::{DEFAULT_LATENCY_MS, Modifiers, PRE_SPIKE_LOOKBACK_MS};
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
    /// `HookSellLevel` — MoonHook's replacement for `SellPrice`: the take in per cent OF THE
    /// TRADE'S DETECT DEPTH (`Deal::hook_depth_pct`). 0 means the level is unknown, and the
    /// verdict then answers nothing rather than judging the line against a guessed level
    /// ([`ExitModel::take_known`]). Ignored by every other kind.
    pub hook_sell_level_pct: f64,
    /// `HookSellFixed` — the core then takes the distance as `HookSellLevel · depth` per cent
    /// "whatever the buy price" (FAQ), which is a different rule from the one below. It is NOT
    /// modelled: no live strategy on this machine sets it, so the branch could not be checked
    /// against anything, and rather than guess, [`ExitModel::take_known`] reports the take as
    /// unknown for such a trade and the verdict answers nothing. Not a grid parameter for the
    /// same reason — a knob that moves no column is worse than no knob.
    pub hook_sell_fixed: bool,
    /// `SellModifier` — the coefficient the summed `Add*` delta modifiers are multiplied by
    /// before they move the sell level (FAQ: a summed delta of 5 % with `SellModifier = 0.2`
    /// places the sell 1 % higher). 0 leaves the level alone.
    ///
    /// Applies to EVERY kind, not only to the hook, because the field is the general one on the
    /// Delta Modifiers tab. It moves what the model computes for any strategy that sets it,
    /// where the field used to be ignored outright. Measured before landing it (118 MoonHook
    /// trades, 2026-09-22): the distance between the modelled take and the fact falls from a
    /// median of 0.763 pp to 0.116 pp, closer on 98 trades of 118, and the ✓ shares of the
    /// kinds that do not set the field did not move.
    pub sell_modifier: f64,
    /// `MaxModifier` — ceiling on the summed modifiers BEFORE the coefficient:
    /// `Min(MaxModifier, Σ Pn · Dn)`. 0 means no ceiling. Live strategies keep it around 70 %
    /// (median of 526 that set it), so it rarely binds.
    pub max_modifier: f64,
    /// `StopLossModifier` — the same summed modifiers, applied to the STOP instead of the sell:
    /// the stop goes DEEPER by `StopLossModifier · Σ`, as the core's FAQ spells it:
    /// `StopLoss adjusted [-1.00% - (10.00*0.98=9.75%) => -10.75%]`. Set on 415 of 1869 live
    /// strategies, median 0.3.
    pub stop_loss_modifier: f64,
    /// The `Add*Delta` family of the Delta Modifiers tab — the same shape as MoonShot's
    /// `MShotAdd*` corridor modifiers, different fields: these move the ORDER PRICE, those the
    /// entry corridor, and one strategy can carry both.
    pub sell_mods: Modifiers,
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
    // PumpMove (PumpsDetection)
    /// `PumpMoveTimer` — seconds after the take before the one pump move; 0 never moves.
    pub pump_move_timer_s: f64,
    /// `PumpMovePersent` (the core's spelling) — per cent of the peak-to-buy distance the move
    /// stops short of the peak.
    pub pump_move_pct: f64,
    // Stops
    /// `StopLoss`, already zeroed by [`super::params::exit_params`] when `UseStopLoss` is off —
    /// the field keeps its value in a strategy whose stop is switched off, and the core then
    /// arms nothing.
    pub stop_loss_pct: f64,
    pub stop_loss_delay_s: f64,
    /// `FastStopLoss` — what the stop watches. YES: the trades ("crosses", FAQ), so the first
    /// print through the level fires it. NO — the core's default: the book's BID (the ASK for a
    /// short), averaged over `StopLossEMA` of its own samples, which the trade tape does not
    /// carry; the walk then reads a sampled proxy of it (see [`super::line`]).
    pub fast_stop_loss: bool,
    /// `StopLossEMA` — how many of the core's samples the non-fast stop averages (FAQ: 0 off,
    /// 3/5/10 "the last 3, 5, 10 ticks", so that a single spike through the line does not start
    /// the panic sell). Ignored by a fast stop — the FAQ's own distinction, and the live
    /// activations agree: 48 fast stops with it at 3 fire as promptly as 81 without it.
    pub stop_loss_ema: f64,
    /// A sell rule the strategy switched on that the model does not have. The walk runs as if
    /// it were off, and the verdict answers nothing for such a trade, which keeps it out of the
    /// search (`record::fit_for_search`): a variant's exit there is whatever the missing rule
    /// would have made of it.
    pub unmodelled: Option<UnmodelledRule>,
    /// Model parameter: how long a replacement of the sell takes to reach the book.
    pub latency_ms: f64,
    /// Verdict-only: start the line at the archived take (`Deal::archived_take`) for a kind
    /// whose take rule the model does not compute itself. Off for every variant, whose take
    /// comes from the RULES — `SellPrice`, or `HookSellLevel · depth` for a MoonHook — so that
    /// turning a knob moves the columns; on when the fact is replayed to be judged, where the
    /// core's own placed level is the truth and nothing the model derives can beat it.
    pub take_from_archive: bool,
}

impl Default for ExitParams {
    /// A plain 1 % take, nothing moving it, no stop.
    fn default() -> Self {
        Self {
            sell_price_pct: 1.0,
            sell_at_last_price: false,
            sell_price_adjust_pct: 0.0,
            sell_delay_ms: 0.0,
            hook_sell_level_pct: 0.0,
            hook_sell_fixed: false,
            sell_modifier: 0.0,
            max_modifier: 0.0,
            stop_loss_modifier: 0.0,
            sell_mods: Modifiers::default(),
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
            pump_move_timer_s: 0.0,
            pump_move_pct: 0.0,
            stop_loss_pct: 0.0,
            stop_loss_delay_s: 0.0,
            // The trigger the tape itself carries; `exit_params` reads the strategy's own, and
            // its absence there is the core's default, NO.
            fast_stop_loss: true,
            stop_loss_ema: 0.0,
            unmodelled: None,
            latency_ms: DEFAULT_LATENCY_MS,
            take_from_archive: false,
        }
    }
}

/// A sell rule the strategy can switch on that the model does not have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnmodelledRule {
    /// `UseTrailing` — the trailing stop.
    Trailing,
    /// `UseSecondStop` / `UseStopLoss3` — the stop ladder.
    StopLadder,
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
        let by_pct = fill.price * pct / 100.0;
        let mut take = if deal.is_long() {
            fill.price + by_pct
        } else {
            fill.price - by_pct
        };
        if self.params.sell_at_last_price {
            let pre = deal
                .pre_spike_ask
                .filter(|p| p.is_finite() && *p > 0.0)
                .or_else(|| pre_spike_price(ticks, fill.t_ms));
            if let Some(pre) = pre {
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

    /// What the delta modifiers add to the sell level, per cent — the capped sum times
    /// `SellModifier`, per the FAQ — as the deltas stood when the sell was placed, at `at_ms`.
    fn modifier_pct(&self, deal: &Deal, at_ms: i64) -> f64 {
        modifier_sum(self.params, deal, at_ms) * self.params.sell_modifier
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
    /// [`super::verify`] then answers the exit group with nothing — which keeps the trade out of
    /// the search (`record::fit_for_search`).
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

    /// The replay with the sell held until `hold_until_ms` — the line's levels through that
    /// moment, whatever print would have sold it earlier (see [`walk_held`]).
    pub fn walk_held(
        &self,
        deal: &Deal,
        ticks: &[Tick],
        fill: Fill,
        hold_until_ms: i64,
    ) -> LineWalk {
        let take = self.take_level(deal, ticks, fill);
        walk_held(deal, ticks, fill, take, self.params, Some(hold_until_ms))
    }
}

/// The summed delta modifiers of a trade, capped: `Min(MaxModifier, Σ Pn · Dn)`.
///
/// One sum, two consumers — the sell level through `SellModifier` and the stop through
/// `StopLossModifier` — because the core computes it once and spends it on both (FAQ).
///
/// The core sums the deltas as they stand when it places the sell: on 121 of its printed sums
/// (2026-09-22) the report's snapshot, stamped at the entry order's placement for every kind but
/// MoonShot, drifted from the core's number the more, the longer the entry order waited. So the
/// sum is read at `at_ms` through the deal's live coin deltas ([`Deal::deltas_at`]); the BTC,
/// market, mark and price-bug terms stay the snapshot, and on the stop the verdict absorbs their
/// residual in its level tolerance (`verify::STOP_PRICE_TOLERANCE`).
///
/// Args:
///     params: The sell parameters, for the coefficients and the ceiling.
///     deal: The trade, for its deltas.
///     at_ms: When the sell was placed — the fill.
pub fn modifier_sum(params: &ExitParams, deal: &Deal, at_ms: i64) -> f64 {
    let sum = params.sell_mods.near_addition(&deal.deltas_at(at_ms));
    if params.max_modifier > 0.0 {
        sum.min(params.max_modifier)
    } else {
        sum
    }
}

/// The stop distance of a trade, per cent: `StopLoss` adjusted by `StopLossModifier · Σ`.
///
/// Normally that deepens the stop (a positive coefficient over a positive delta sum), but
/// neither sign is guaranteed: live strategies carry `StopLossModifier` down to −0.3, and a
/// delta sum can be negative, so the adjustment can also pull the stop TOWARD the entry.
///
/// An adjustment big enough to pull it THROUGH the entry answers `0.0` — no stop on this trade
/// — rather than a level. Clamping it to a hair's breadth from the entry instead would fire on
/// the first print that moves, which is not a stop but a coin flip dressed as one; and placing
/// it beyond the entry would fire on the first print, full stop. What the core does with an
/// adjustment that large is unknown: none of the 1 735 replayed trades reaches this branch, so
/// the model declines to invent an answer. A configured stop on the profit side (`StopLoss` positive —
/// live data has it) is a different thing and is left exactly as configured.
///
/// Args:
///     params: The sell parameters.
///     deal: The trade, for its deltas.
///     at_ms: When the sell was placed — the fill; see [`modifier_sum`].
pub fn stop_pct(params: &ExitParams, deal: &Deal, at_ms: i64) -> f64 {
    if params.stop_loss_pct == 0.0 || params.stop_loss_modifier == 0.0 {
        return params.stop_loss_pct;
    }
    let adjusted =
        params.stop_loss_pct - modifier_sum(params, deal, at_ms) * params.stop_loss_modifier;
    // Same side as configured, or nothing at all.
    if adjusted == 0.0 || adjusted.is_sign_negative() != params.stop_loss_pct.is_sign_negative() {
        return 0.0;
    }
    adjusted
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
    super::entry::entry_model_for(kind)
}

/// The take as an archived Exit line records it: its first point, when it is a price.
pub fn archived_take(exit_points: Option<&[(i64, f64)]>) -> Option<f64> {
    let (_, take) = exit_points?.first().copied()?;
    (take.is_finite() && take > 0.0).then_some(take)
}

/// The pre-spike ask behind an archived Exit line: its first point is the take as the core
/// placed it, `ask · (1 − MShotSellPriceAdjust/100)` when `MShotSellAtLastPrice` lifted it —
/// `ask · (1 + adjust)` for a short, whose take sits below the entry and is adjusted UP toward
/// it — so the ask is that point with the trade's own adjustment divided out. `None` when the
/// rule was off (the take came from `SellPrice`, and the archive says nothing about the ask),
/// when the archive holds no Exit line, or when the first point is not a price.
///
/// When `SellPrice` alone set the take higher than the ask would have, the division reads a
/// slightly high ask back — and the same `max` (a long) or `min` (a short, whose take sits
/// below the entry) puts the take on `SellPrice` again, so the trade's own replay is exact
/// either way; a variant with a smaller adjustment inherits the overread.
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
    let factor = if is_short {
        1.0 + params.sell_price_adjust_pct / 100.0
    } else {
        1.0 - params.sell_price_adjust_pct / 100.0
    };
    if !(factor > 0.0) {
        return None;
    }
    let (_, take) = exit_points?.first().copied()?;
    (take.is_finite() && take > 0.0).then_some(take / factor)
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
