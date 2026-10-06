//! What the diagnostic report reads about a stop the core fired, beside the verdict: which rule of
//! the model fired its own, the moment against the core's archived activation on every judged stop
//! that has one (not only the misses), where the quote the core printed into its reason stood
//! against the level, and how stale the core's ticker was against the tape.
//!
//! Each answers a question the developer otherwise settles with a bench run on the trades
//! themselves, which a user's report cannot carry (2026-10-06: "shorts fire early, longs late"
//! took two replica runs to read as a median over ten misses, the ticker proxy's and not the
//! series', with the core watching the ASK of a short).

use super::{REASON_STOP, archived_stop_jump, reason_starts_with, stop_jump_level};
use crate::db::tuner::ticks::exit::stops::StopTrigger;
#[cfg(test)]
use crate::db::tuner::ticks::gap::TapeGap;
use crate::db::tuner::ticks::{Deal, ExitParams, reaches};
use crate::feed::types::{Side, Tick};

/// How far a print may sit from the reason's quote and still be the print that quote showed:
/// the window of the 2026-09-26 measurement that put GateF's ticker ~4 s behind the tape.
const QUOTE_MATCH: f64 = 0.001;

/// How far back from the activation a print matching the quote is looked for. GateF's p75 was
/// 7.6 s; a quote no print matched within a minute says nothing about the ticker's age.
const QUOTE_LOOKBACK_MS: i64 = 60_000;

/// Where the BID and ASK the core printed into a stop's reason stood against the stop level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuoteSide {
    /// Both quotes past the level.
    Both,
    /// Only the quote of the stop's own side — a long's BID, a short's ASK — past it: the core
    /// watched that side, since the other could not have fired it.
    StopSideOnly,
    /// Only the other side past it — a stop that fired on a quote it does not watch, or a
    /// snapshot taken off the moment it fired.
    OtherSideOnly,
    /// Neither: the quote was taken after the price came back, or the series fired the stop.
    Neither,
}

/// The report's facts about one stop. Every field is `None` where it does not apply.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StopFacts {
    /// Which rule fired the stop of the walk the verdict judged, when that walk closed on one.
    pub trigger: Option<StopTrigger>,
    /// The model's stop minus the core's archived activation, ms, on every stop judged by its
    /// moment — hit or miss — so a lean to one side shows over the whole segment. `None` without
    /// an archived jump: the close the verdict then falls back to trails the activation.
    pub moment_ms: Option<i64>,
    /// Where the reason's quote stood, for a book-watching stop whose reason printed one.
    pub quote: Option<QuoteSide>,
    /// How long before the archived activation the tape last printed the reason's stop-side
    /// quote, ms: how far the core's REST ticker ran behind the trades it summarises. `None`
    /// without an archived jump (the close trails the activation by the sale) and when a hole of
    /// the tape cuts into the lookback (the print the quote showed may be in it).
    pub ticker_age_ms: Option<i64>,
}

/// The reason's quote and the ticker's age for a fact the core closed by a stop (`StopLoss …`);
/// `trigger` and `moment_ms` are the verdict's to fill.
///
/// Args:
///     deal: The trade, its reason and side.
///     ticks: Its window's prints, ascending.
///     exit: The sell parameters, for the level when the reason printed none.
///     exit_points: The archived Exit line, for the activation.
pub(super) fn quote_facts(
    deal: &Deal,
    ticks: &[Tick],
    exit: &ExitParams,
    exit_points: Option<&[(i64, f64)]>,
) -> StopFacts {
    let reason = deal.sell_reason.trim();
    if !reason_starts_with(reason, REASON_STOP) {
        return StopFacts::default();
    }
    let (Some(level), Some((bid, ask))) = (stop_jump_level(deal, exit), reason_quote(reason))
    else {
        return StopFacts::default();
    };
    let long = deal.is_long();
    let (own, other) = if long { (bid, ask) } else { (ask, bid) };
    let quote = Some(
        match (reaches(own, level, long), reaches(other, level, long)) {
            (true, true) => QuoteSide::Both,
            (true, false) => QuoteSide::StopSideOnly,
            (false, true) => QuoteSide::OtherSideOnly,
            (false, false) => QuoteSide::Neither,
        },
    );
    let held = |at: i64| {
        deal.gap
            .as_ref()
            .is_none_or(|gap| gap.to_ms <= at - QUOTE_LOOKBACK_MS || gap.from_ms >= at)
    };
    let ticker_age_ms = archived_stop_jump(deal, level, exit_points)
        .filter(|&at| held(at))
        .and_then(|at| ticker_age_ms(ticks, at, own, long));
    StopFacts {
        quote,
        ticker_age_ms,
        ..StopFacts::default()
    }
}

/// How long before `at` the tape last printed `quote` on the stop's side — a taker sell at a
/// long's BID, a taker buy at a short's ASK — within [`QUOTE_MATCH`]; `None` when no such print
/// fell in the [`QUOTE_LOOKBACK_MS`] before it.
fn ticker_age_ms(ticks: &[Tick], at: i64, quote: f64, long: bool) -> Option<i64> {
    let side = if long { Side::Sell } else { Side::Buy };
    let end = ticks.partition_point(|t| (t.time_ms as i64) <= at);
    ticks[..end]
        .iter()
        .rev()
        .take_while(|t| at - (t.time_ms as i64) <= QUOTE_LOOKBACK_MS)
        .find(|t| t.side == side && (f64::from(t.price) - quote).abs() <= quote * QUOTE_MATCH)
        .map(|t| at - t.time_ms as i64)
}

/// The BID and ASK a book-watching stop's reason prints — `BID = X ASK: Y` — when both are usable
/// numbers; a number the stored reason cut off at its end answers `None`.
fn reason_quote(reason: &str) -> Option<(f64, f64)> {
    Some((
        number_after(reason, "BID =")?,
        number_after(reason, "ASK:")?,
    ))
}

/// The positive number after `marker`, when it ends before the text does — the stored reason is
/// truncated, and a number running into its end may be missing digits.
fn number_after(text: &str, marker: &str) -> Option<f64> {
    let at = text.find(marker)? + marker.len();
    let rest = text[at..].trim_start();
    let len = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter(|&len| len > 0)?;
    let value: f64 = rest[..len].parse().ok()?;
    (value.is_finite() && value > 0.0).then_some(value)
}

#[cfg(test)]
mod tests;
