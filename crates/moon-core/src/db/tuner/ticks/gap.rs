//! The hole in a long position's tape, and what the fact proves about it.
//!
//! A position held past `[trade_replay] long_position_min` keeps its prints around the entry and
//! around the exit only (`ReplayWindow::focus_spans`); between them the tape holds nothing. A walk
//! that treats the first print after the hole as the next print of the market meets it with a sell
//! line that took hours of timer steps down in between, and "sells" there — an exit invented at the
//! seam (the spec, `ENTRY_EXIT_TUNER.md` §3.1).
//!
//! What the fact does prove: the core's own sell line was NOT reached inside the hole, and the
//! core's own stop did not fire there — or the trade would have closed inside it. So a variant
//! whose sell stands no nearer the price than the fact's line at every moment of the hole, and
//! whose stop stands no nearer than the fact's, was not closed inside it either, and the part of
//! the tape around the close judges it. A variant that stood nearer at some moment could have been
//! crossed then, at an unknown moment and price: it is not judged on the trade
//! ([`crate::db::tuner::ticks::ExitKind::InGap`]). So is one whose rule follows the price through
//! the hole — SellLevel, the pump move, the trailing stop, a stop ladder rung still to take — since
//! where such a rule stood is a function of prints nobody holds.

use std::sync::Arc;

use super::exit::ExitParams;
use super::exit::level_off_buy;
use super::exit::stops::stop_pct;
use super::verify::POINT_TIME_TOLERANCE_MS;
use super::{Deal, PRICE_EPS, PRICE_TOLERANCE};
use crate::market::trade_replay::Coverage;

/// How far apart in time a modelled level and the core's archived one may stand and still be one
/// line to the hole's comparison: the verdict's default for one move ([`POINT_TIME_TOLERANCE_MS`]).
/// The constant, not the verdict's setting: widening how strictly the model is JUDGED must not
/// widen what a variant may do unseen for hours.
pub const HOLE_TIME_SLACK_MS: i64 = POINT_TIME_TOLERANCE_MS;

/// How far a modelled level may sit on the price's side of the core's archived one and still be
/// that level to the hole's comparison, relative: the verdict's default price tolerance
/// ([`PRICE_TOLERANCE`]), a constant for the same reason as [`HOLE_TIME_SLACK_MS`].
pub const HOLE_PRICE_TOLERANCE: f64 = PRICE_TOLERANCE;

/// The stretch between a long position's two held ends, and the fact's own record of it.
#[derive(Clone, Debug, PartialEq)]
pub struct TapeGap {
    /// The last held millisecond before the hole: a print at or before it is the entry end's.
    pub from_ms: i64,
    /// The first held millisecond after the hole: a print at or after it is the exit end's.
    pub to_ms: i64,
    /// The core's sell line through the hole — the archived Exit line's `(t_ms, price)` points —
    /// the level no print reached while it stood; `None` without the archive, and then no variant
    /// with a sell standing in the hole can be told unreached.
    pub fact_line: Option<Arc<[(i64, f64)]>>,
    /// The core's stop, which the fact proves quiet through the hole; `None` when the fact ran no
    /// stop, or fired it before the hole ended — then nothing is proven about the prices there.
    pub fact_stop: Option<GapStop>,
}

/// The fact's stop as the hole's proof reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GapStop {
    /// The deepest level the fact's stop could stand at inside the hole: the first stop's, or a
    /// ladder rung's deeper still — every price of the hole stayed on the position's side of it.
    pub level: f64,
    /// `FastStopLoss` of the fact: its stop fired on any print through the level.
    pub fast: bool,
    /// `StopLossEMA` of the fact: the average a book-watching stop compares.
    pub ema: f64,
}

impl TapeGap {
    /// The hole of a covered long position's window, when the held tape has one between the buy
    /// and the close; `None` for a window held whole.
    ///
    /// Several holes (a middle partly held, a chart fetched a stretch of it) read as one: from the
    /// end of the held run at the buy to the start of the held run at the close. What lies between
    /// them is then taken as unknown, which can only leave a variant unjudged, never invent one.
    ///
    /// Args:
    ///     deal: The trade — its buy, close, side and deltas.
    ///     covered: What the tape store holds of the window.
    ///     exit: The sell parameters the trade ran, for the fact's stop.
    ///     exit_points: The archived Exit line, when the archive holds it.
    pub fn of(
        deal: &Deal,
        covered: &Coverage,
        exit: &ExitParams,
        exit_points: Option<&[(i64, f64)]>,
    ) -> Option<Self> {
        let inside = covered.clip(&Coverage::one((deal.buy_ms, deal.close_ms)));
        let spans = inside.spans();
        let (&(_, from_ms), &(to_ms, _)) = (spans.first()?, spans.last()?);
        // One held run from the buy to the close: no hole. A covered row holds both ends; a row
        // that does not is no trade of the model's and has no hole to speak of.
        if spans.len() < 2 || !inside.contains_ms(deal.buy_ms) || !inside.contains_ms(deal.close_ms)
        {
            return None;
        }
        Some(Self {
            from_ms,
            to_ms,
            fact_line: exit_points.filter(|p| !p.is_empty()).map(Arc::from),
            fact_stop: fact_stop(deal, exit, to_ms),
        })
    }

    /// The core's sell level at `t_ms`: the archived point in force then, `None` before the line's
    /// first point or without the archive.
    pub fn fact_level_at(&self, t_ms: i64) -> Option<f64> {
        level_at(self.fact_line.as_deref()?, t_ms)
    }
}

/// The fact's stop as it bounds the hole's prices, `None` when it bounds nothing: no stop, or one
/// the fact proves quiet only up to a moment before the hole ends (`StopAnchor::quiet_until_ms`,
/// its activation on a stopped trade) — a stop that may have fired inside the hole proves nothing
/// about the prices there.
fn fact_stop(deal: &Deal, exit: &ExitParams, hole_end_ms: i64) -> Option<GapStop> {
    let pct = stop_pct(exit, deal, deal.buy_ms);
    if pct == 0.0 {
        return None;
    }
    let quiet_until = deal
        .stop_anchor
        .map_or(deal.close_ms, |anchor| anchor.quiet_until_ms);
    if quiet_until < hole_end_ms {
        return None;
    }
    let long = deal.is_long();
    let deepest = [
        Some(pct),
        exit.second_stop.map(|s| s.level_pct),
        exit.third_stop.map(|s| s.level_pct),
    ]
    .into_iter()
    .flatten()
    .map(|pct| level_off_buy(deal.buy_price, pct, long))
    .reduce(|a, b| if long { a.min(b) } else { a.max(b) })?;
    Some(GapStop {
        level: deepest,
        fast: exit.fast_stop_loss,
        ema: exit.stop_loss_ema,
    })
}

/// The level of a stepped line at `t_ms`: the last point stamped at or before it; `None` before
/// the first.
pub(super) fn level_at(points: &[(i64, f64)], t_ms: i64) -> Option<f64> {
    points
        .iter()
        .filter(|(t, _)| *t <= t_ms)
        .max_by_key(|(t, _)| *t)
        .map(|&(_, price)| price)
}

/// The core's sell level at `t_ms` as the hole's comparison reads it: of the levels its archived
/// line stood at within `slack_ms` either side, the one nearest the price — the lowest for a long,
/// whose sell the price comes up to, the highest for a short — so the loosest bound. A modelled
/// move and an archived one within [`POINT_TIME_TOLERANCE_MS`] are one move to the verdict; a
/// comparison with no slack read the trade's OWN settings as
/// nearer the price than the core's line whenever a modelled step fell a fraction of a second
/// before the archived one — 22 of the 1 703 fit trades of the live bench (2026-09-25), 18 of them
/// PumpsDetection's long PriceDown chains. `None` where the line is not on record at `t_ms`.
pub(super) fn fact_level_near(
    points: &[(i64, f64)],
    t_ms: i64,
    slack_ms: i64,
    long: bool,
) -> Option<f64> {
    let from = t_ms.saturating_sub(slack_ms.max(0));
    let to = t_ms.saturating_add(slack_ms.max(0));
    let at_start = level_at(points, from);
    let inside = points
        .iter()
        .filter(|(t, _)| *t > from && *t <= to)
        .map(|&(_, p)| p);
    at_start
        .into_iter()
        .chain(inside)
        .reduce(|a, b| if long { a.min(b) } else { a.max(b) })
}

/// Whether a variant's sell at `variant` stands no nearer the price than the fact's at `fact`: at
/// or above it for a long (the price comes UP to a long's sell), at or below it for a short —
/// within `tolerance`, relative — [`HOLE_PRICE_TOLERANCE`] from the walk — and never under
/// [`PRICE_EPS`], the `f32` of the archive.
pub(super) fn sell_not_nearer(variant: f64, fact: f64, long: bool, tolerance: f64) -> bool {
    let tolerance = tolerance.max(PRICE_EPS);
    if long {
        variant >= fact * (1.0 - tolerance)
    } else {
        variant <= fact * (1.0 + tolerance)
    }
}

/// Whether a variant's stop at `variant` stands no nearer the price than the fact's bound: at or
/// below it for a long (the price comes DOWN to a long's stop), at or above it for a short.
pub(super) fn stop_not_nearer(variant: f64, bound: f64, long: bool) -> bool {
    if long {
        variant <= bound * (1.0 + PRICE_EPS)
    } else {
        variant >= bound * (1.0 - PRICE_EPS)
    }
}

/// Whether a variant's stop trigger is no quicker than the fact's: a stop firing on any print
/// (`FastStopLoss`) is the quickest, so any trigger is no quicker than it; otherwise the same
/// trigger — a book-watching stop averaged differently fires at other moments, and neither is
/// bounded by the other.
pub(super) fn trigger_not_quicker(variant_fast: bool, variant_ema: f64, fact: &GapStop) -> bool {
    fact.fast || (!variant_fast && variant_ema == fact.ema)
}

#[cfg(test)]
mod tests;
