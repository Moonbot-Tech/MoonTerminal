//! The strategy window's "Sell order" section: the take, `SellDelay`, `PriceDown*` and
//! `SellLevel*`.
//!
//! - **The take** is `SellPrice` per cent above the fill for every kind but three: MoonHook, whose
//!   take uses a fixed depth share or preserves its stated factual take
//!   ([`crate::db::tuner::ticks::hook`]), MoonStrike, whose take is `MStrikeSellLevel` per cent
//!   of the strike's span off its detect price ([`crate::db::tuner::ticks::strike`]), and
//!   Spread, whose take is the edge of the spread it
//!   detected — a level, not a rule, taken as the core recorded it ([`take_is_recorded`]). The
//!   Delta-Modifier family moves it ([`super::delta_mods`]). A MoonShot's take is raised by
//!   `MShotSellAtLastPrice` — read for MoonShot alone ([`lifts_take_to_ask`]) — to the
//!   pre-spike price less `MShotSellPriceAdjust` (the FAQ: "the
//!   4-second-old ASK, i.e. before the spike"; the model takes the ask the caller recovered from
//!   the order archive (`Deal::pre_spike_ask`), else reads the last taker buy at least
//!   `ModelSettings::pre_spike_lookback_ms` ([`crate::db::tuner::ticks::mshot::PRE_SPIKE_LOOKBACK_MS`]
//!   by default) before the fill, since the tape has no book — [`pre_spike_price`]).
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
//!   `SellLevelAllowedDrop` per cent over the buy. The high is read over the tape AND the
//!   market's minute bars (`Deal::bars`): the tape reaches minutes before the order, the look-back
//!   an hour — read off the tape alone, the "hour's high" was the high of the run-up, and a
//!   variant turning SellLevel on sold at a level the core would never have placed.

use super::line::Line;
use super::{ExitModel, ExitParams, Side, due_by, level_off_buy};
use crate::db::tuner::ticks::deltas::Bar;
use crate::db::tuner::ticks::hook::{KIND_MOONHOOK, hook_take_pct};
use crate::db::tuner::ticks::strike::{KIND_MOONSTRIKE, strike_take_level};
use crate::db::tuner::ticks::{Deal, Fill};
use crate::feed::types::Tick;

impl ExitModel<'_> {
    /// The take-profit level for a fill: the kind's take off the fill, a MoonShot's lifted to the
    /// pre-spike ask (the archive's, else the tape's last taker buy, [`pre_spike_price`]) less the
    /// adjustment when `MShotSellAtLastPrice` is on ([`lifts_take_to_ask`]). Long above, short
    /// below. An archived ask belongs only to fills inside the fact's shift window.
    pub fn take_level(&self, deal: &Deal, ticks: &[Tick], fill: Fill) -> f64 {
        let mut spread_base = None;
        // A kind whose take rule the model does not have starts where the core's line did —
        // when the fact is being judged; a variant computes the take from the rules for every
        // kind (see `ExitParams::take_from_archive`) but the one whose take is not a rule at
        // all ([`take_is_recorded`]). The recorded level is the core's own, modifiers and all,
        // so nothing below is added to it.
        if (self.params.take_from_archive && !take_model_for(&deal.kind))
            || take_is_recorded(&deal.kind)
        {
            if let Some(take) = deal.archived_take.filter(|t| t.is_finite() && *t > 0.0) {
                if take_is_recorded(&deal.kind) && self.spread_shift_changed(deal) {
                    // Source: https://moonbot.pro/documentation/advanced-strategy-parameters/spread-strategy-and-its-parameters
                    // Divide out the fact's shift before the normal variant shift below.
                    spread_base = self.spread_base(deal);
                } else {
                    return take;
                }
            }
        }
        // A short's take divides off the fill for every kind (the core developer, 2026-09-23 for
        // MoonShot, 2026-09-24 for every per cent of a short off the buy). A MoonStrike with its
        // level and detect places a PRICE off the strike's span instead ([`Self::strike_take`]);
        // it never stands on the losing side of the fill, and the modifier shift below moves it
        // as any other take. A strike's take
        // is no model of its own ([`take_model_for`] is false for it), so when the fact is judged
        // the archived level above still comes first.
        let mut take = spread_base.unwrap_or_else(|| self.unmodified_take(deal, fill.price));
        if lifts_take_to_ask(self.params, &deal.kind) {
            let pre = deal
                .pre_spike_ask
                .filter(|_| {
                    fill.t_ms.abs_diff(deal.buy_ms)
                        <= self.params.model.shift_window_ms.max(0) as u64
                })
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
        // Then the delta modifiers move the sell so placed — by `SellModifier · Σ` per cent of
        // its own price, a short's by a division as every short per cent — after the floor of
        // `SellPrice` over the buy and after the lift to the ask, so a negative coefficient can
        // take it under that floor (the core developer via LinKvo, 2026-09-24; the core's own
        // "SellPrice adjusted by +0.30 * +2.30% = +0.69% (0.00056430 => 0.00056044)" is a short's
        // 0.00056430 / 1.0069).
        // Under the floor, but not through the fill: a shift deep enough to carry the sell past
        // the entry — toward zero, a short's toward infinity at −100 % — is held AT the entry.
        // What the core does there is not known (no trade of the live sample reaches it), and a
        // take on the losing side would turn every level the line steps from inside out.
        let shift_ms = spread_base
            .and(deal.stop_anchor.map(|a| a.entry_ms))
            .unwrap_or(fill.t_ms);
        let shift = self.modifier_pct(deal, shift_ms);
        if shift.is_finite() && shift != 0.0 {
            let shifted = level_off_buy(take, shift.max(-99.0), deal.is_long());
            take = if deal.is_long() {
                shifted.max(fill.price.min(take))
            } else {
                shifted.min(fill.price.max(take))
            };
        }
        take
    }

    /// Whether a Spread variant changes the fact's sell shift at its booking clock.
    fn spread_shift_changed(&self, deal: &Deal) -> bool {
        deal.stop_anchor.is_some_and(|a| {
            let shift = self.modifier_pct(deal, a.entry_ms);
            !(shift.is_finite()
                && a.take_shift_pct.is_finite()
                && (shift - a.take_shift_pct).abs() <= 1e-9)
        })
    }

    /// The archived Spread take with the fact's modifier divided out, if invertible.
    fn spread_base(&self, deal: &Deal) -> Option<f64> {
        let shift = deal.stop_anchor?.take_shift_pct;
        if !shift.is_finite() || shift <= -99.0 {
            return None;
        }
        let archived = deal.archived_take?;
        // A negative shift clamped at the fill has lost its original base.
        if shift < 0.0
            && if deal.is_long() {
                archived <= deal.buy_price
            } else {
                archived >= deal.buy_price
            }
        {
            return None;
        }
        let take = level_off_buy(archived, shift, !deal.is_long());
        (take.is_finite() && take > 0.0).then_some(take)
    }

    /// The take price before the lift and delta modifiers: a hook's stated factual take or
    /// a strike's span rule where their inputs are known, else the kind's
    /// per cent ([`Self::base_take_pct`]) off the fill. Shared by [`Self::take_level`] and the
    /// fact's modifier reading (`delta_mods::take_reading`), so the two never disagree on the base.
    pub(in crate::db::tuner::ticks) fn unmodified_take(&self, deal: &Deal, fill_price: f64) -> f64 {
        self.strike_take(deal, fill_price).unwrap_or_else(|| {
            level_off_buy(
                fill_price,
                self.base_take_pct(deal).max(0.0),
                deal.is_long(),
            )
        })
    }

    /// The fact's calibrated hook level, when its stated take defines one.
    fn hook_fact_level(&self, deal: &Deal) -> Option<f64> {
        let depth = deal.hook_depth_pct.filter(|v| v.is_finite() && *v > 0.0)?;
        let stated = deal
            .hook_stated_take_pct
            .filter(|v| v.is_finite() && *v > 0.0)?;
        let level = 100.0 * stated / depth;
        level.is_finite().then_some(level)
    }

    /// A MoonStrike's take price before the modifiers — `MStrikeSellLevel` of the strike's span
    /// ([`strike_take_level`]) — or `None` for every other kind and for a strike whose level or
    /// detect is unknown. Non-positive levels have no defined strike take.
    fn strike_take(&self, deal: &Deal, fill_price: f64) -> Option<f64> {
        if deal.kind != KIND_MOONSTRIKE || !(self.params.strike_sell_level_pct > 0.0) {
            return None;
        }
        let d = deal.strike_detect?;
        let adjust = if self.params.strike_sell_adjust {
            Some(self.strike_buy(deal)?)
        } else {
            None
        };
        let take = strike_take_level(
            d,
            fill_price,
            self.params.strike_sell_level_pct,
            deal.is_long(),
            adjust,
        );
        // A fill past the strike's top would put the span's take on the losing side; it is held
        // at the fill instead, as the modifier shift is.
        Some(if deal.is_long() {
            take.max(fill_price)
        } else {
            take.min(fill_price)
        })
    }

    /// The strike's calculated buy; missing or non-finite inputs leave it unknown.
    /// Source: https://moonbot.pro/documentation/advanced-strategy-parameters/moonstrike-strategy-and-its-parameters
    fn strike_buy(&self, deal: &Deal) -> Option<f64> {
        let d = deal.strike_detect?;
        let level = self.params.strike_buy_level_pct.filter(|v| v.is_finite())?;
        let sign = if deal.is_long() { 1.0 } else { -1.0 };
        let top = d.trade_price * (1.0 + sign * d.depth_pct / 100.0);
        let buy = if self.params.strike_buy_relative {
            d.trade_price + level / 100.0 * (top - d.trade_price)
        } else {
            level_off_buy(top, level, deal.is_long())
        };
        (buy.is_finite() && buy > 0.0).then_some(buy)
    }

    /// The percent-path take distance: fixed `HookSellLevel` of the detect depth for MoonHook,
    /// or the fallback for an unknown non-fixed take; `SellPrice` for other kinds. A MoonStrike
    /// with its level and detect never reaches it ([`Self::strike_take`] places a price); one
    /// without falls back to `SellPrice` like a hook below.
    ///
    /// A hook whose depth or level is unknown falls back to `SellPrice` so the line still has
    /// somewhere to step down from — and [`Self::take_known`] answers `false` for it, which is
    /// what keeps that fallback out of the verdict.
    pub(super) fn base_take_pct(&self, deal: &Deal) -> f64 {
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
    /// - a MoonHook needs positive depth and level; Fixed=NO also needs a stated take and
    ///   the fact's calibrated level;
    /// - a MoonStrike needs `MStrikeSellLevel` and its detect — the depth and `TradePrice` the
    ///   comment wrote ([`Deal::strike_detect`]), and its calculated buy when adjusted;
    /// - a Spread needs the recorded take and an invertible fact shift when modifiers change;
    /// - a MoonShot lifted to the pre-spike ask (`MShotSellAtLastPrice`) needs that ask off the
    ///   core's record (`Deal::pre_spike_ask`) — the tape's print before the spike sits 0.1–0.5 %
    ///   under the book's ask on a dump, and on 88 stopped MoonShot trades (2026-09-23) a take
    ///   placed off it was touched before the core's stop on 30, turning a loss into a win; the
    ///   taker buy it reads now lands within 0.05 % of the archived ask on under half the trades
    ///   ([`pre_spike_price`]), still no ground for a verdict;
    /// - every other kind takes `SellPrice`, known by construction.
    ///
    /// `false` is not "the model was wrong": it is "this trade's take is not modelled here", and
    /// [`crate::db::tuner::ticks::verify`] then answers the exit group with nothing — which keeps
    /// the trade out of the search (`record::fit_for_search`).
    pub fn take_known(&self, deal: &Deal) -> bool {
        let positive = |v: Option<f64>| v.is_some_and(|x| x.is_finite() && x > 0.0);
        if deal.kind == KIND_MOONHOOK {
            // Fixed=NO: the documented spike rule does not reproduce the core's stated takes;
            // only the stated take at the fact's own level is kept, other levels are refused.
            // Fixed=YES keeps the percent-of-depth rule. Source:
            // https://moonbot.pro/documentation/advanced-strategy-parameters/moonhook-strategy-and-its-parameters
            return positive(Some(self.params.hook_sell_level_pct))
                && positive(deal.hook_depth_pct)
                && (self.params.hook_sell_fixed
                    || self
                        .hook_fact_level(deal)
                        .is_some_and(|fact| same_level(self.params.hook_sell_level_pct, fact)));
        }
        if deal.kind == KIND_MOONSTRIKE {
            // Re-checked here though the scan parses only positive numbers: a Deal is also
            // built by hand (tests, callers), and the verdict must not trust it.
            return self.params.strike_sell_level_pct > 0.0
                && (!self.params.strike_sell_adjust || self.strike_buy(deal).is_some())
                && deal
                    .strike_detect
                    .is_some_and(|d| positive(Some(d.depth_pct)) && positive(Some(d.trade_price)));
        }
        if take_is_recorded(&deal.kind) {
            return positive(deal.archived_take)
                && (!self.spread_shift_changed(deal) || self.spread_base(deal).is_some());
        }
        if lifts_take_to_ask(self.params, &deal.kind) {
            return positive(deal.pre_spike_ask);
        }
        true
    }
}

/// Relative equality preserves the fact's exact price without hiding a changed level.
fn same_level(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-9 * a.abs().max(b.abs())
}

/// The kind name of Spread as the strategy list spells it.
pub const KIND_SPREAD: &str = "Spread";

/// Whether the kind's take is not a rule of its parameters but a level the core took off the
/// detect — the spread it detected. `SellPrice` places it on 5 of 157 archived Spread takes
/// (2026-09-23) — the coincidences it takes for the width of a spread to land on the field. The
/// report's `comment` keeps only the rounded width, so the archive supplies the fact's take.
/// A variant changing modifiers divides out the anchored fact shift before applying its own.
/// `SellPrice` is no knob of the kind (`params::TICK_PARAMS`).
/// Source: https://moonbot.pro/documentation/advanced-strategy-parameters/spread-strategy-and-its-parameters
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
/// (2026-09-22), four of them hits: `SellPrice` is the take of every kind but MoonHook and
/// MoonStrike (whose own rules [`ExitModel::take_known`] checks), and a
/// Spread trade without an archived line is judged by it perfectly well.
pub fn take_model_for(kind: &str) -> bool {
    kind == crate::db::tuner::ticks::entry::KIND_MOONSHOT
}

/// Whether `MShotSellAtLastPrice` lifts this trade's take to the pre-spike ask: the field is
/// MoonShot's, and only a kind whose take is MoonShot's rule ([`take_model_for`]) reads it.
///
/// The core's schema defaults the field on, so a strategy of every kind carries it, and the
/// terminal reads that default. Lifting a MoonHook's take by it put the take 2–3 % past the
/// core's: on the `real_data` bench run with the schema's default (2026-10-06), take-closed
/// hooks on Gate Futures reproduced 3 of 61 and 2 of 67 with the lift, 53 of 61 and 58 of 68
/// without, the sale on `HookSellLevel` of the depth to a median −0.004 %.
pub fn lifts_take_to_ask(params: &ExitParams, kind: &str) -> bool {
    params.sell_at_last_price && take_model_for(kind)
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
/// placed it — the ask times [`ask_take_factor`] when `MShotSellAtLastPrice` placed it, then
/// shifted by the delta modifiers (`SellModifier · Σ`, the same shift [`ExitModel::take_level`]
/// applies after the lift) — so the ask is that point with the shift and the factor divided out,
/// on the clock the take rule shifts it on (`verify::fact_sell_start`).
///
/// The shift is the core's: measured 2026-10-06 against the tape's own pre-spike print, the
/// ask read back without it sat a median +8.56 % above the tape on the 96 MoonShot longs that
/// run `SellModifier` (the model's own shift there: +8.42 %), and +0.006 % on the 1 240 that do
/// not. Read back without it, the take rule shifted it a second time and missed the archived take
/// on 95 of the 96 — an earlier answer of the core developer (2026-09-23, "the ask's branch
/// carries no modifier") the data does not bear out.
///
/// The take rule holds a shift that would carry the take through the fill at the fill; that hold
/// is not inverted here. A take the core held there reads back an ask the shift distorts — above
/// the real one for a long, below it for a short: the
/// fact's own replay still lands on the archived take (the rule holds it at the fill again), a
/// variant with another `SellModifier` starts from the inflated ask. No live trade has reached
/// the hold (`take_level`).
/// `None` when the rule was off or is not the kind's ([`lifts_take_to_ask`] — the take came from
/// the kind's own rule, and the archive says nothing about the ask), when the archive holds no
/// Exit line, or when the first point is not a price.
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
///     deal: The trade — its kind (whether the rule is its at all), its side (which way the
///         adjustment and the shift went) and the deltas the shift is summed over, with the
///         core's own sum (`Deal::fact_modifier`) already read.
pub fn archived_pre_spike_ask(
    exit_points: Option<&[(i64, f64)]>,
    params: &ExitParams,
    deal: &Deal,
) -> Option<f64> {
    if !lifts_take_to_ask(params, &deal.kind) {
        return None;
    }
    let factor = ask_take_factor(params, deal.is_short)?;
    let (_, take) = exit_points?.first().copied()?;
    if !(take.is_finite() && take > 0.0) {
        return None;
    }
    let at_ms = crate::db::tuner::ticks::verify::fact_sell_start(deal, params, exit_points).t_ms;
    let shift = ExitModel::new(params).modifier_pct(deal, at_ms);
    // The inverse of the shift `take_level` applies: a long's multiplied, a short's divided.
    let unshifted = if shift.is_finite() && shift != 0.0 {
        level_off_buy(take, shift.max(-99.0), !deal.is_long())
    } else {
        take
    };
    Some(unshifted / factor)
}

/// How far before the cutoff [`pre_spike_price`] still takes a taker buy for the ask. On a tape of
/// sells alone the last buy can lie minutes back (the longest over the sample of 2026-09-26: 177 s,
/// 1 % past 49 s) — another market, not the ask before the spike. A minute costs nothing on that
/// sample; ten seconds already cost 1–3 points.
pub const PRE_SPIKE_BUY_WINDOW_MS: i64 = 60_000;

/// The tape's reading of the FAQ's "4-second-old ASK": the last taker BUY at least `lookback_ms`
/// ([`crate::db::tuner::ticks::mshot::PRE_SPIKE_LOOKBACK_MS`] by default) before `at_ms` and at
/// most [`PRE_SPIKE_BUY_WINDOW_MS`] before that — a taker buy prints at the ask — else the last
/// print of either side by then. The ask for a short too: the core lifts a short's take off the
/// ASK as well.
///
/// Measured against the ask the archive gives back (`archived_pre_spike_ask`) on 1 099 MoonShot
/// trades (2026-09-26), at 4 s: within 0.05 % on 46 % of the longs and 48 % of the shorts, where
/// the last print of either side — the reading before — landed on 38 % and 36 %, half a spread
/// under the ask (median −0.04 % long, −0.07 % short). On Binance's cores 56 %; on Gate's no
/// reading of the prints reaches 20 % — too thin a tape under too wide a spread; the side is not
/// what fails there (Gate futures infer it from the sign of `size`): the last taker sell lands
/// farther from the ask than the last buy on Gate too. Hence a take lifted off this reading is
/// never judged ([`ExitModel::take_known`]).
pub fn pre_spike_price(ticks: &[Tick], at_ms: i64, lookback_ms: i64) -> Option<f64> {
    let cutoff = at_ms - lookback_ms;
    let before = || {
        ticks
            .iter()
            .rev()
            .filter(move |t| (t.time_ms as i64) <= cutoff && t.price > 0.0)
    };
    before()
        .take_while(|t| cutoff - (t.time_ms as i64) <= PRE_SPIKE_BUY_WINDOW_MS)
        .find(|t| t.side == crate::feed::types::Side::Buy)
        .or_else(|| before().next())
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
        let pd_on = params.price_down_on();
        Self {
            params,
            fill,
            side,
            next: pd_on.then(|| fill.t_ms + (params.price_down_timer_s * 1000.0) as i64),
            floor: side.off_buy(fill.price, params.price_down_allowed_drop_pct),
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
            // A share of the price, not a level off the buy: the step multiplies for a short too
            // (the core developer, 2026-09-24, answer 9).
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
    /// The market's bars (`Deal::bars`), for the part of the look-back the tape does not reach.
    bars: &'a [Bar],
}

impl<'a> SellLevel<'a> {
    pub(super) fn new(params: &'a ExitParams, deal: &'a Deal, fill: Fill, side: Side) -> Self {
        let floor_ms = params.model.step_floor_ms;
        let sl_on = params.sell_level_on();
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
            floor: side.off_buy(fill.price, params.sell_level_allowed_drop_pct),
            bars: deal.bars.as_deref().unwrap_or_default(),
        }
    }

    /// The move due by the print at `t_ms`, if any.
    pub(super) fn due(&self, t_ms: i64) -> Option<i64> {
        due_by(self.next, t_ms)
    }

    /// The extreme of `[from, due]` in the profit direction: the tape's prints, and every bar that
    /// lies inside the look-back and closed by `due` — a bar still open at `due` holds prints
    /// from after it, and one that began before `from` holds prints from before the look-back.
    fn high(&self, seen: &[Tick], from: i64, due: i64) -> Option<f64> {
        let side = self.side;
        let bars = self
            .bars
            .iter()
            .filter(|b| b.from_ms >= from && b.to_ms <= due)
            .map(|b| if side.long { b.high } else { b.low })
            .filter(|p| p.is_finite() && *p > 0.0);
        let prints = side.extreme_between(seen, from, due);
        side.extreme(prints.into_iter().chain(bars))
    }

    /// The move due at `due`: to the high of the look-back, adjusted.
    ///
    /// Args:
    ///     seen: The prints up to and including the one the move is due by.
    pub(super) fn step(&mut self, due: i64, seen: &[Tick], line: &mut Line) {
        let (params, fill, side) = (self.params, self.fill, self.side);
        if self.left == 0 || self.until.is_some_and(|until| due > until) {
            self.next = None;
            return;
        }
        let from = due - (params.sell_level_time_s * 1000.0) as i64;
        if let Some(high) = self.high(seen, from, due) {
            let next = if params.sell_level_relative {
                fill.price + (high - fill.price) * params.sell_level_adjust_pct / 100.0
            } else {
                // Off the high, not off the buy — the product, like the trailing distance off its
                // peak. No live strategy runs SellLevel (`SellLevelDelay` is absent from all
                // 1 422, 2026-09-24), so no archive tells the readings apart.
                side.over(high, params.sell_level_adjust_pct)
            };
            let next = side.farther(next, self.floor);
            line.place(due, next);
        }
        self.left -= 1;
        self.next = Some(due + self.every_ms);
    }
}

#[cfg(test)]
mod tests;
