//! The exit side: where the sell line stood after the fill, and which print crossed it. One
//! model for every strategy kind — after the entry filled, the exit of any strategy is a
//! function of the sell-order rules and the tape.
//!
//! One file per section of the strategy window, in the window's order: [`stops`],
//! [`sell_order`] (the take, `SellDelay`, `PriceDown*`, `SellLevel*`), [`delta_mods`] — and
//! PumpsDetection's own [`pump_move`]. The step they share — the walk over the tape, the price
//! grid, the latency, the recorded replacements — is [`line`].
//!
//! The "Sell order / SellShot" and "Sell order / SellSpread" sections are not modelled at all (the
//! developer's call, 2026-09-24): SellShot follows the book's side of the market and SellSpread the
//! spread, neither of which the trade tape carries, and 2 live strategies of 1 422 switch either
//! on. A trade under one of them is not judged ([`UnmodelledRule`]).
//!
//! A position nothing closed inside the tape is [`ExitKind::OpenAtWindowEnd`]: not a trade,
//! whatever the core's exit was.
//!
//! [`ExitKind::OpenAtWindowEnd`]: super::ExitKind::OpenAtWindowEnd

pub mod delta_mods;
pub mod line;
pub mod pump_move;
pub mod sell_order;
pub mod stops;

pub use self::stops::ladder::StopStep;

use self::line::{LineWalk, walk, walk_held};
use super::mshot::Modifiers;
use super::settings::ModelSettings;
use super::{Deal, Exit, Fill};
use crate::feed::types::Tick;

/// A level `pct` per cent off the buy, positive in the PROFIT direction: `buy·(1 + pct/100)` for a
/// long, `buy/(1 + pct/100)` for a short — the short's per cents are counted from the level back
/// to the buy, not the long's product mirrored. The one formula for every level a strategy states
/// in per cent of the buy: the take of every kind, the stop, the trailing's take profit and the
/// `*AllowedDrop` floors.
///
/// The core developer (2026-09-24, answer 9): every per cent of a short off the buy divides; only
/// the trailing distance, a PriceDown step without `Relative` and `StopAboveLiq` multiply. The
/// site's page on the short says the same ("a take of +50 % at 100 stands at 66.6"). Checked on
/// the data: the stop's `StopLoss fixed: X` lands on the division for 11 099 short stops against
/// 25 for the mirror, and the `PriceDownAllowedDrop` floor of the archived short Exit lines for 67
/// against 1 (2026-09-24; 215 more sit where the two readings round to one step).
///
/// A loss of 100 % or more leaves no price: 0 for a long and `f64::INFINITY` for a short, levels
/// no print reaches.
///
/// Args:
///     buy: The buy the level counts from.
///     pct: The distance, negative on the losing side.
///     long: The trade's side.
pub fn level_off_buy(buy: f64, pct: f64, long: bool) -> f64 {
    let keep = 1.0 + pct / 100.0;
    match (long, keep <= 0.0) {
        (true, true) => 0.0,
        (true, false) => buy * keep,
        (false, true) => f64::INFINITY,
        (false, false) => buy / keep,
    }
}

/// Which way the position profits, folding every "above/below the buy" into one sign. Every
/// section's rule is written for a long and mirrored for a short through it.
#[derive(Clone, Copy)]
struct Side {
    long: bool,
}

impl Side {
    /// `pct` per cent off the buy in the profit direction — [`level_off_buy`].
    fn off_buy(self, buy: f64, pct: f64) -> f64 {
        level_off_buy(buy, pct, self.long)
    }

    /// `pct` per cent of `base` in the PROFIT direction, the long's product mirrored: above for
    /// a long, below for a short. For a distance off a price that is NOT the buy — the high a
    /// `SellLevelAdjust` counts from — and for a step that is a share of the price rather than a
    /// level. A level off the buy is [`Self::off_buy`].
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

    /// The extreme print in the profit direction over a run.
    fn extreme(self, prices: impl Iterator<Item = f64>) -> Option<f64> {
        if self.long {
            prices.reduce(f64::max)
        } else {
            prices.reduce(f64::min)
        }
    }

    /// The extreme print in the profit direction among `seen` stamped from `from` to `to`, both
    /// included.
    fn extreme_between(self, seen: &[Tick], from: i64, to: i64) -> Option<f64> {
        self.extreme(
            seen.iter()
                .filter(|t| {
                    let tt = t.time_ms as i64;
                    tt >= from && tt <= to && t.price > 0.0
                })
                .map(|t| f64::from(t.price)),
        )
    }
}

/// A timer rule's next moment, when it is due by the print at `t_ms`.
fn due_by(next: Option<i64>, t_ms: i64) -> Option<i64> {
    next.filter(|due| t_ms >= *due)
}

/// Sell-line parameters, in the strategy's own units (per cent, seconds; `SellDelay` is ms).
/// Every rule's fields are documented in its section's module.
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
    /// `Min(MaxModifier, |Σ Pn · Dn|)` — the core caps the sum's magnitude, so the sum is never
    /// negative (`exit::delta_mods::modifier_sum`). 0 means no ceiling. Set on 127 of 1 423 live
    /// strategies (2026-09-25), 10…1000, so it rarely binds. The same field caps the
    /// MoonShot corridor's `MShotAdd*` sum (`MshotParams::max_modifier`).
    pub max_modifier: f64,
    /// `StopLossModifier` — the same summed modifiers, applied to the STOP instead of the sell:
    /// the stop goes DEEPER by `StopLossModifier · Σ`, as the core's FAQ spells it:
    /// `StopLoss adjusted [-1.00% - (10.00*0.98=9.75%) => -10.75%]`. Set on 150 of 1 423 live
    /// strategies (2026-09-25), 0.2 on 139 of them.
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
    /// print through the level fires it. NO — the core's default: the REST ticker's BID (the
    /// ASK for a short), a long's averaged per `StopLossEMA`, which the trade tape does not
    /// carry; the walk then reads a sampled proxy of it (see [`stops`]).
    pub fast_stop_loss: bool,
    /// `StopLossEMA` — the non-fast stop's average of the ticker's BID, `(avg·(N − 1) + bid)/N`
    /// per arrival, kept for a LONG at 3, 5 or 10 only; any other value and every short watch the
    /// bare price, and at 0 the core's price series fires it too (the core developer,
    /// 2026-09-23; see `stops::stop_average_weight`). Ignored by a fast stop — the FAQ's own
    /// distinction, and the live activations agree: 48 fast stops with it at 3 fire as promptly
    /// as 81 without it.
    pub stop_loss_ema: f64,
    /// `TrailingPercent` when `UseTrailing` is on, 0 when it is off: how far under the peak of the
    /// spread's middle the trailing line stands, per cent (negative). See [`stops`].
    pub trailing_pct: f64,
    /// `TrailingEMA` — a step of the trailing peak moves `1/(N + 1)` of the way to the middle.
    pub trailing_ema: f64,
    /// `TakeProfit` when `UseTakeProfit` is on — the trailing's own take profit, per cent off the
    /// buy, NOT the order's `SellPrice`: no line until the middle passed it by `|TrailingPercent|`,
    /// and no sale below it. `None` when it is off.
    pub trailing_take_profit_pct: Option<f64>,
    /// The second stop (`UseSecondStop` and its three fields), `None` when it is off or there is
    /// no stop to move (`exit::stops::ladder`).
    pub second_stop: Option<StopStep>,
    /// The third stop (`UseStopLoss3` and its three fields), likewise.
    pub third_stop: Option<StopStep>,
    /// A sell rule the strategy switched on that the model does not have. The walk runs as if
    /// it were off, and the verdict answers nothing for such a trade, which keeps it out of the
    /// search (`record::fit_for_search`): a variant's exit there is whatever the missing rule
    /// would have made of it.
    pub unmodelled: Option<UnmodelledRule>,
    /// The model's own settings — the sell's replacement latency, the stop's clocks, the
    /// verdict's tolerances; not strategy fields.
    pub model: ModelSettings,
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
            pump_move_timer_s: 0.0,
            pump_move_pct: 0.0,
            stop_loss_pct: 0.0,
            stop_loss_delay_s: 0.0,
            // The trigger the tape itself carries; `exit_params` reads the strategy's own, and
            // its absence there is the core's default, NO.
            fast_stop_loss: true,
            stop_loss_ema: 0.0,
            trailing_pct: 0.0,
            trailing_ema: 0.0,
            trailing_take_profit_pct: None,
            second_stop: None,
            third_stop: None,
            unmodelled: None,
            model: ModelSettings::default(),
            take_from_archive: false,
        }
    }
}

/// A sell rule the strategy can switch on that the model does not have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnmodelledRule {
    /// `IgnoreSellShot` off with a `SellShotDistance` — the sell kept at a distance from the
    /// market's high.
    SellShot,
    /// `IgnoreSellSpread` off — the sell placed under the spread.
    SellSpread,
    /// `AutoSell` off — the core places no sell order at all, so there is no line to replay.
    NoAutoSell,
}

/// The exit model over one parameter set.
pub struct ExitModel<'a> {
    params: &'a ExitParams,
}

impl<'a> ExitModel<'a> {
    pub fn new(params: &'a ExitParams) -> Self {
        Self { params }
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

#[cfg(test)]
mod tests;
