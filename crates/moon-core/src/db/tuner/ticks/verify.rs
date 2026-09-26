//! Reproducing the fact — the check every search has to pass before it may run.
//!
//! The model is run with the strategy's parameters AS THEY WERE at the trade and its answer is
//! held against the report row, per group: did the entry fill where the core's did, did the
//! exit land where the core's did. The share of ✓ over the sample is the caption's "model ✓
//! K %", and a group below the caller's threshold is not searched at all — a search over a model
//! that cannot reproduce what happened optimizes noise.
//!
//! A group the model does not cover answers `None`, not `false`: a Spread's entry is not wrong,
//! it is not modelled; an exit the core closed by a rule the model does not have is not a miss
//! of the rules it does. An exit the model never reached at all IS a miss.
//!
//! The same holds for the LEVEL the exit is judged against, as a VARIANT would place it: a hook
//! without its detect depth or `HookSellLevel` (or with `HookSellFixed`, whose branch is not
//! modelled), a Spread without the take the core recorded, a MoonShot lifted to a pre-spike ask
//! the record did not keep — the sell line a variant walks stands somewhere the model invented,
//! and everything downstream of it — where PriceDown stepped to, whether a level stood at the
//! close — is invented with it. That answers `None`, whatever the deviation says
//! ([`ExitModel::take_known`]), and a stopped trade is no exception: its stop may be judged
//! right, but a variant of it sells on the invented take first (2026-09-23: 30 of 88 stopped
//! MoonShot trades).
//!
//! The entry is held to the CORRIDOR, not to a price step: a MoonShot order chasing a falling
//! price is re-placed off whichever print left the corridor, and the core's print and the
//! model's differ by a second and a fraction of a per cent on every such chase (GSTOCKBSC
//! 2026-09-21: the core off 0.031130 at −0.55 s, the model off 0.031253 at −2.1 s, levels
//! 0.39 % apart on a 1 % corridor, both filled by the same dump). A fill within the corridor's
//! own width of the fact is the same order in the same corridor; the 0.05 % step is the floor
//! for a corridor narrower than that.
//!
//! The exit is walked from the FACTUAL entry and held against two things: where the
//! modelled line STOOD at the moment the core sold — against the price it sold at — and, when
//! the order archive holds the trade's Exit line, every move the core made with its sell,
//! each of which the model must have made too within the point and price tolerances
//! (`settings::ModelSettings` — [`POINT_TIME_TOLERANCE_MS`] and [`PRICE_TOLERANCE`] by
//! default). A model that lands on the right price by a different path has not
//! reproduced the rule. Which PRINT the model would have sold on is not judged: that is the
//! queue at the level (the spec's §7), which the tape does not carry — a print at the level
//! sold the core's line on ARX and left it standing on COOL the same day. The archive's own
//! record of the fill — its last point, at the sale price — is not a move ([`is_fill_point`]).
//!
//! A stop is judged by its firing, not by a resting line, and not by its sale either — a panic
//! sell or a market order walked through a book the tape does not carry: by the level the core
//! fixed, when the stored reason keeps it, and the moment it activated ([`verify_stop`]). A stop the core fired and the model never did is a miss.

use super::exit::ExitModel;
use super::exit::level_off_buy;
use super::exit::line::LinePoint;
use super::exit::stops::stop_pct;
use super::exit::stops::trailing::trailing_level;
use super::mshot::MshotParams;
use super::settings::ModelSettings;
use super::{
    Deal, EntryParams, Exit, ExitKind, ExitParams, Fill, PRICE_TOLERANCE, reaches, simulate,
};
use crate::feed::types::Tick;

/// How far apart a modelled and an archived replacement may be in time and still be the same
/// move: the archive stamps the core's own moment, the model the print that triggered it.
pub const POINT_TIME_TOLERANCE_MS: i64 = 1_000;

/// How far apart a modelled and a factual book-watching stop (`FastStopLoss` off) may fire and
/// still be the same stop: one gap of the REST ticker the core reads that stop's price off —
/// 2–2.3 s (the core developer, 2026-09-23). The core fires on the first arrival that puts the
/// price past the level, the model on its own sample clock, and the ticker's phase is on no
/// record: a dump through the level fires both at their next arrival, up to a whole gap apart.
/// On the 192 live book-watching stops of 2026-09-23, a second judged 112 on time, the gap 141.
pub const BOOK_STOP_TIME_TOLERANCE_MS: i64 = 2_300;

/// Tolerance on a STOP's level: the modelled level against the one the core fixed carries
/// `StopLossModifier` over deltas the model only partly re-reads live — the coin's and BTC's where
/// the deal has a track, the market, mark and price-bug terms as the report's one snapshot
/// (`exit::delta_mods::modifier_sum`) — and the residual sits right there wherever the record
/// did not keep the core's own sum (`Deal::fact_modifier`). Where it did, the sum is the one
/// that places the recorded level to the price step, and a stop read off the take's band carries
/// that band's width times `StopLossModifier / SellModifier`.
pub const STOP_PRICE_TOLERANCE: f64 = 0.003;

/// How much BETTER than the modelled level the fact's fill may be and still be that level's
/// fill: a limit never fills worse than its price, and on a gap it fills better — GUN
/// 2026-09-21, the line at 0.003042 sold at 0.0030485, 0.21 % above it, the archive showing
/// the same three moves the model made. Bounded, because a fill far beyond the level is
/// another rule's exit, not a lucky fill of this one — and taken only when the archive holds
/// the trade's Exit line and the model re-placed at every move of it: without that
/// corroboration a wrong rule landing within the allowance would pass as a lucky fill.
pub const FILL_IMPROVEMENT_TOLERANCE: f64 = 0.003;

/// One trade's reproduction verdict, per group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verdict {
    /// Entry reproduced: `Some(true)` within tolerance, `Some(false)` off or unfilled, `None`
    /// when the kind has no entry model (the entry was taken from the fact).
    pub entry: Option<bool>,
    /// Modelled fill against the fact, per cent of the fact (`None` when unfilled or unmodelled).
    pub entry_dev_pct: Option<f64>,
    /// Exit reproduced — the line's level at the close against the price the core sold at,
    /// and every archived move re-placed; `None` when the core closed by a rule other than the
    /// one the model's line was under at the close (a take against an "Auto Price Down"
    /// fact) — the two prices are not comparable then. `Some(false)` when no level stood at
    /// the close at all, and when the core's exit was a stop the model — holding a stop of its
    /// own — never fired.
    pub exit: Option<bool>,
    /// Modelled exit against the fact, per cent of the fact — for a book-watching stop, the
    /// modelled stop LEVEL against the level the core printed (see [`verify_stop`]).
    pub exit_dev_pct: Option<f64>,
    /// The modelled fill, for the tooltip.
    pub fill: Option<Fill>,
    /// How the exit walk from the factual entry closed.
    pub exit_kind: Option<ExitKind>,
    /// Archived Exit points matched by the modelled line, as `(matched, archived)`; `None`
    /// when the archive holds no Exit line for the trade.
    pub line_points: Option<(usize, usize)>,
}

/// Relative deviation of `modelled` from `fact`, per cent of the fact.
fn deviation_pct(modelled: f64, fact: f64) -> Option<f64> {
    if !fact.is_finite() || fact <= 0.0 || !modelled.is_finite() {
        return None;
    }
    Some((modelled - fact) / fact * 100.0)
}

/// Run the model on the trade's own parameters and compare with the report row.
///
/// Args:
///     deal: The report row.
///     ticks: Its window's prints, ascending.
///     entry: The entry parameters at the trade — [`EntryParams::Fact`] for a kind without a
///         model, which leaves `Verdict::entry` at `None`.
///     exit: The sell-line parameters at the trade.
///     entry_line: The archived Entry line's `(t_ms, price)` points, when the archive holds
///         them; the entry model starts where they say the order stood.
///     exit_points: The archived Exit line's `(t_ms, price)` points, when the archive holds
///         them; the modelled line must re-place at each.
pub fn verify(
    deal: &Deal,
    ticks: &[Tick],
    entry: &EntryParams,
    exit: &ExitParams,
    entry_line: Option<&[(i64, f64)]>,
    exit_points: Option<&[(i64, f64)]>,
) -> Verdict {
    // The model is what is tested: what the fact proves (the stop's firing, the entry's own
    // fill) is the variants' to lean on, never the verdict's.
    let deal = &super::record::unanchored(deal);
    let outcome = simulate(deal, ticks, entry, exit, entry_line);
    let (entry_ok, entry_dev) = match (entry, outcome.fill) {
        (EntryParams::Fact, _) => (None, None),
        (EntryParams::MoonShot(_), None) => (Some(false), None),
        (EntryParams::MoonShot(params), Some(fill)) => {
            let dev = deviation_pct(fill.price, deal.buy_price);
            let ok = dev.is_some_and(|d| d.abs() <= entry_tolerance_pct(params, deal));
            (Some(ok), dev)
        }
    };
    // The exit answers only where the model's rule and the core's were the same rule: a
    // modelled take against a fact the core closed by its take. A take held against an
    // "Auto Price Down" fact is not a miss of the take model, it is a rule the model does not
    // have yet (phase 2), and it stays unanswered until it does.
    // The exit group is judged from the FACTUAL entry, whatever the entry group modelled: the
    // sell rules measure from the buy, and a fill the model placed a few ticks off would shift
    // every level of a correctly reproduced line — and their timers from the moment the core
    // placed the take (`fact_sell_start`). The entry group has its own verdict above.
    let fact_fill = fact_sell_start(deal, exit, exit_points);
    // The line is walked HELD through the close: its levels are what is judged, and a print
    // that would have sold the model's line earlier is the queue's business, not the rule's.
    // A kind without a take rule of its own starts at the core's archived take.
    let fact_exit = ExitParams {
        take_from_archive: true,
        ..exit.clone()
    };
    let walked = ExitModel::new(&fact_exit).walk_held(deal, ticks, fact_fill, deal.close_ms);
    // What the model is held to: a stop it fired by the close (within the point tolerance —
    // the model stamps a print, the core its own moment) is the stop's own print, and so is
    // a stop it fired later when the core's own exit WAS a stop — a late stop is a timing
    // miss of the stop rule, judged by its price, never an unanswered question. Otherwise
    // the line as it stood when the core sold — the last level the exchange had been given
    // by then, the model's own latency allowed for — and the rule is the take when no move
    // had reached the exchange, the moving line otherwise.
    let fact_stopped = is_stop_reason(&deal.sell_reason);
    // The archive's own record of the sell: its moves, and the fill it filed as a point.
    let archive = exit_points
        .filter(|p| !p.is_empty())
        .map(|archived| ArchivedExit::of(deal, exit, archived));
    // When the fact filled: the archive's own record of the fill when it filed one, else the
    // close. The report books the close when the core does, which can be seconds after the
    // fill — FATCOIN 2026-09-22 filled 15 ms after the line's third move and closed 1.8 s later,
    // long enough for the model's line to take a fourth step the core's never took.
    let filled_at = archive
        .as_ref()
        .and_then(|a| a.fill)
        .map_or(deal.close_ms, |(t, _)| t.min(deal.close_ms));
    let model = &exit.model;
    let closed = match walked.exit.kind {
        ExitKind::Stop
            if walked.exit.t_ms <= deal.close_ms + model.point_time_ms || fact_stopped =>
        {
            walked.exit
        }
        // No level the line reached through the tape's hole is the rules' (`gap`).
        ExitKind::InGap => walked.exit,
        _ => {
            // In time order: the take is stamped when it is armed, after any timer step
            // that fell due inside the sell delay.
            let mut modelled: Vec<&LinePoint> = walked.points.iter().collect();
            modelled.sort_by_key(|p| p.t_ms);
            // A point the model stamps up to its own latency after the fill is a move due
            // before it — the core's stamp is its moment, the model's the print plus latency.
            let horizon = filled_at + model.latency_whole_ms();
            let level = archive
                .as_ref()
                .and_then(|a| level_on_archive_clock(&modelled, a, horizon, model))
                .or_else(|| modelled.iter().rev().find(|p| p.t_ms <= horizon).copied());
            match level {
                Some(level) => Exit {
                    t_ms: deal.close_ms,
                    price: level.price,
                    kind: if modelled
                        .first()
                        .is_some_and(|first| first.t_ms < level.t_ms)
                    {
                        ExitKind::Line
                    } else {
                        ExitKind::Take
                    },
                },
                // No level placed by the close — the sell delay outlived the trade: the
                // walk's own end, `OpenAtWindowEnd` or a stop fired later against a fact
                // that was not a stop, which `exit_rule_matches` leaves unanswered.
                None => walked.exit,
            }
        }
    };
    // Where a variant could not place this trade's take, the line under it is not the model's
    // answer but its guess — see the module doc. Asked with the trade's OWN parameters as a
    // variant runs them (`exit`, not `fact_exit`): the fact's replay starts at the recorded
    // take, a variant does not.
    let take_known = ExitModel::new(exit).take_known(deal);
    // A stop the core fired and the model, holding a stop of its own, never did — the book
    // proxy of a non-fast stop can stay short of the level to the tape's end — is a miss of
    // the stop, whatever the line was doing: not a question about another rule.
    // The trailing answers for it only where the fact's reason can be the trailing's: its own, or
    // the market sale the core rewrites it to — never a stop that printed its fixed level.
    let reason = deal.sell_reason.trim();
    let trailing_can_be_it = reason_starts_with(reason, REASON_TRAILING)
        || reason.eq_ignore_ascii_case(REASON_MARKET_STOP);
    let missed_stop = fact_stopped
        && closed.kind != ExitKind::Stop
        && (stop_pct(&fact_exit, deal, deal.buy_ms) != 0.0
            || (fact_exit.trailing_pct != 0.0 && trailing_can_be_it));
    let (exit_ok, exit_dev, line_points) = if exit.unmodelled.is_some() {
        // A rule the model does not have was on: whatever the walk made of the trade is not
        // an answer about it (see `ExitParams::unmodelled`).
        (None, None, None)
    } else if closed.kind == ExitKind::InGap {
        // A rule of the trade follows the price through its tape's hole, or its stop is not
        // bounded by what the fact proves there: where the line or the stop stood at the close
        // is a function of prints nobody holds (`gap`).
        (None, None, None)
    } else if missed_stop {
        (Some(false), None, None)
    } else if !take_known {
        (None, None, None)
    } else if closed.kind == ExitKind::OpenAtWindowEnd {
        // No line stood at the close: a miss of the exit group, not an unanswered question.
        (Some(false), None, None)
    } else if closed.kind == ExitKind::Stop && exit_rule_matches(closed.kind, &deal.sell_reason) {
        verify_stop(
            deal,
            &fact_exit,
            &walked.points,
            closed,
            walked.stop_level,
            exit_points,
        )
    } else if exit_rule_matches(closed.kind, &deal.sell_reason) {
        let dev = deviation_pct(closed.price, deal.sell_price);
        let tolerance = model.price_pct;
        // Within the tolerance either way, or a limit's fill on the better side of its level:
        // `dev` is the model against the fact, so a fact above the modelled sell (a long) or
        // below the modelled buy-back (a short) reads as a negative deviation of the model.
        let better_by = |d: f64| if deal.is_long() { -d } else { d };
        let improved = |d: f64| {
            let better = better_by(d);
            better > 0.0 && better <= model.fill_improvement_pct
        };
        let archived_fill = archive.as_ref().and_then(|a| a.fill);
        let archived_level = archive.as_ref().and_then(|a| a.moves.last().copied());
        let points = archive.as_ref().map(|a| {
            (
                matched_points(&walked.points, &a.moves, model),
                a.moves.len(),
            )
        });
        let line_ok = points.is_none_or(|(matched, total)| matched == total);
        let corroborated = points.is_some_and(|(matched, total)| matched == total);
        // A level placed THROUGH the market: the archive filed the fill as a point of its own
        // within a moment of the last move, the model re-placed at every move before it, and
        // the line stood where that last move put it. The rule is reproduced, and how far past
        // the level the fill landed is the book's — a marketable limit takes the best bid:
        // INDEX 2026-09-22, the line at 0.03460 bought back 16 ms later at 0.0343988, 0.6 %
        // better, the model on every one of the nine moves before it; live fills of this shape
        // follow their move by a median 31 ms. A level that RESTED before its fill keeps the
        // `FILL_IMPROVEMENT_TOLERANCE` bound — a fill far past a resting level is another exit.
        // Held only on the better side: a limit never fills worse than its price.
        let level_reproduced = corroborated
            && archived_fill
                .zip(archived_level)
                .is_some_and(|(fill, level)| {
                    fill.0 - level.0 <= model.point_time_ms
                        && deviation_pct(closed.price, level.1)
                            .is_some_and(|d| d.abs() <= model.price_pct)
                });
        let price_ok = dev.is_some_and(|d| {
            d.abs() <= tolerance
                || (corroborated && improved(d))
                || (level_reproduced && better_by(d) >= -tolerance)
        });
        (Some(price_ok && line_ok), dev, points)
    } else {
        (None, None, None)
    };
    Verdict {
        entry: entry_ok,
        entry_dev_pct: entry_dev,
        exit: exit_ok,
        exit_dev_pct: exit_dev,
        fill: outcome.fill,
        exit_kind: Some(closed.kind),
        line_points,
    }
}

/// The entry the fact's sell rules count from: the buy price, at the moment the core placed
/// its take — the archived Exit line's first point, less `SellDelay` — when that is later than
/// `buydatems`, else at `buydatems`.
///
/// The report stamps the buy at its first fill; the core starts the sell, and every timer of
/// it, when it books the buy done. On the live sample (2026-09-22) 32 archived lines placed the
/// take more than 0.3 s after `buydatems` — up to 32 s, a limit buy filling in parts — and on
/// every one from 0.5 s up the first PriceDown step came `PriceDownTimer` after the TAKE, not
/// after the buy (MORPHO: take +2 135 ms, first step +32 155 ms on a 30 s timer). Below half a
/// second both happen — a Spread's take stamped 317 ms late still stepped off the buy — and
/// holding the take-anchored reading back under a threshold of 0.5 s cost two verdicts more than
/// it saved on the same sample.
///
/// `SellDelay` keeps the relation the walk already has: the timers run from the booked buy and
/// the take goes up `SellDelay` after it. Every one of the 1 613 live deals ran it at 0, so which
/// moment the core's timers count from when it is not is unchecked.
///
/// Args:
///     deal: The report row.
///     exit: The parameters, for `SellDelay`.
///     exit_points: The archived Exit line, when the archive holds it.
pub fn fact_sell_start(deal: &Deal, exit: &ExitParams, exit_points: Option<&[(i64, f64)]>) -> Fill {
    let booked = exit_points
        .and_then(|points| points.first())
        .map(|&(t, _)| t - exit.sell_delay_ms.max(0.0) as i64);
    Fill {
        t_ms: booked.map_or(deal.buy_ms, |t| t.max(deal.buy_ms)),
        price: deal.buy_price,
    }
}

/// An archived Exit line as the verdict reads it: the moves the core made with its sell, and
/// the fill when the archive filed it as a point of its own ([`is_fill_point`]).
pub(super) struct ArchivedExit {
    pub(super) moves: Vec<(i64, f64)>,
    pub(super) fill: Option<(i64, f64)>,
}

impl ArchivedExit {
    pub(super) fn of(deal: &Deal, exit: &ExitParams, archived: &[(i64, f64)]) -> Self {
        let mut moves = archived_replacements(archived);
        let fill = match moves.as_slice() {
            [.., prev, last] if is_fill_point(deal, exit, *last, *prev) => moves.pop(),
            _ => None,
        };
        Self { moves, fill }
    }
}

/// The modelled level at the fill read on the ARCHIVE's clock: when the model re-placed at
/// every archived move, the level is the model's own point for the core's last move before the
/// fill — unless the model moved again, with no archived move to match, more than the point
/// tolerance (`ModelSettings::point_time_ms`) before `horizon`: that is a step the core never took, and its
/// level is what the model is held to. `None` — read the model's own clock instead — when a
/// move went unmatched.
///
/// The fill point counts as the core's last move when the model made that move too: a line
/// that stepped onto the level it then filled at is filed as ONE point — the step and the fill
/// at the same price, which [`is_fill_point`] reads as the fill because it came within the
/// latency of the close (AKE 2026-09-22: the fourth step 94 ms before the close, sold at it).
///
/// The model's timing is good to the point tolerance and no better, and the level at the fill
/// is the one place the verdict read it to the millisecond: a fill 15 ms after the core's move
/// (a level placed through the market) failed whenever the model stamped that same move a
/// little later, and a step the model took a few hundred ms before a fill the core took before
/// ITS step failed the other way. On the live sample (2026-09-22) timing the steps by each
/// core's own replace lag won 48 verdicts and lost 35 of exactly these two shapes.
///
/// Args:
///     modelled: The model's points, in time order.
///     archive: The archived line — its moves and its fill point.
///     horizon: The fill, plus the model's own latency.
///     model: The model's settings, for the tolerances.
fn level_on_archive_clock<'a>(
    modelled: &[&'a LinePoint],
    archive: &ArchivedExit,
    horizon: i64,
    model: &ModelSettings,
) -> Option<&'a LinePoint> {
    let same = |m: &LinePoint, point: (i64, f64)| same_move(m, point, model);
    // A line the archive holds as its take alone says nothing about when the core stepped
    // (MUSEBOOK 2026-09-22 — "Auto Price Down", the archive one point long, sold 1.2 s after
    // the take); a take and a fill point is a line, the step filed as the fill.
    let filed = archive.moves.len() + usize::from(archive.fill.is_some());
    if filed < 2 || matched_points_of(modelled, &archive.moves, model) != archive.moves.len() {
        return None;
    }
    let own_of = |mv: (i64, f64)| {
        modelled
            .iter()
            .filter(|m| same(m, mv))
            .min_by_key(|m| (m.t_ms - mv.0).abs())
            .copied()
    };
    // The latest archived point before the fill the model re-placed at: the fill point when
    // the model made that move too, the last move otherwise (matched, as every move is here).
    let own = archive
        .moves
        .iter()
        .chain(archive.fill.iter())
        .rev()
        .filter(|(t, _)| *t <= horizon)
        .find_map(|&mv| own_of(mv))?;
    let stray = modelled
        .iter()
        .rev()
        .find(|m| {
            m.t_ms > own.t_ms
                && m.t_ms <= horizon - model.point_time_ms
                && !archive
                    .moves
                    .iter()
                    .chain(archive.fill.iter())
                    .any(|&mv| same(m, mv))
        })
        .copied();
    Some(stray.unwrap_or(own))
}

/// Whether the archive's last move is the FILL filed as a point rather than a move of the
/// line: at the price the core sold at, and either within the model's latency of the close
/// (GUN 2026-09-21: 31 ms before it) or on the fill side of the level before it — a limit
/// fills at its price or better, and no rule moves a sell line the instant after placing it.
///
/// The close stamp alone missed most of them. On the live sample (2026-09-22) the fill point
/// sat a median 250 ms before `closedatems` and up to a second — the report stamps the close
/// when the core books it — so 145 "Auto Price Down" trades failed on that one point alone,
/// the model having re-placed at every move before it. 447 archived lines end on the better
/// side of their last level, at the sale price, most within 100 ms of it.
///
/// Args:
///     deal: The report row — its sale price, close and side.
///     exit: The parameters, for the model's latency.
///     last: The archive's last move.
///     prev: The move before it.
fn is_fill_point(deal: &Deal, exit: &ExitParams, last: (i64, f64), prev: (i64, f64)) -> bool {
    let (t, p) = last;
    let at_sale =
        deviation_pct(p, deal.sell_price).is_some_and(|d| d.abs() <= exit.model.price_pct);
    if !at_sale {
        return false;
    }
    let at_close = (t - deal.close_ms).abs() <= exit.model.latency_whole_ms();
    // Not worse than the level it was filed against: at or above a long's sell, at or below a
    // short's buy-back — `reaches` with the long's side reads "at or above".
    let fill_side = reaches(p, prev.1, !deal.is_long());
    at_close || fill_side
}

/// The stop's verdict: by what it DECIDED, never by what its sale fetched.
///
/// Every stop's sale is walked through a book the tape does not carry. Without `UseMarketOrder`
/// the core runs a panic sell — a limit through the book stepped by `StopLossSpread` down to
/// `AllowedDrop` (FAQ) — whose fills sit 1–3 % past the level (0 of 173 passed on the sale
/// price, 2026-09-22); with it (`StopLoss Market Sell`, 149 of 149 such trades) a market order
/// sweeps our size into the bids.
///
/// So the rule is judged by the modelled stop LEVEL against the one the core fixed — the stored
/// reason's `StopLoss fixed: X`, which a panic sell carries and which is cut off one time in
/// seven — and by the moment it fired against the activation — the archive's jump past the
/// level, else the close. The core's `X` agreed with the model's level within 0.3 % on 144 of
/// 183 (2026-09-22). With no level on record, the moment and the line are what is left to judge.
///
/// The level tolerance is [`STOP_PRICE_TOLERANCE`] rather than the line's: the modelled level
/// carries `StopLossModifier` over deltas the model only partly re-reads live (see
/// `exit::delta_mods::modifier_sum`), and the residual sits right there where the record kept
/// no sum of the core's own.
///
/// Archived moves from the activation on — the first move past the level [`stop_jump_level`]
/// gives: the stop's, or for a trailing stop's reason its own line under the printed peak — are
/// the panic sell, not the line the rules moved, and are not held against the model. A fact whose
/// level is on no record (a trailing stop rewritten to `StopLoss Market Sell`) keeps them.
///
/// A trailing stop's reason is timed with the book stop's tolerance whatever `FastStopLoss` says:
/// the trailing fires on the ticker's arrivals, like the book stop.
///
/// Args:
///     deal: The report row.
///     exit: The parameters the fact is replayed with.
///     modelled: Every level the modelled line stood at.
///     closed: The modelled stop.
///     stop_level: Where the walk's stop stood when it fired — a ladder step's level once one was
///         taken; `None` falls back to the first stop's level.
///     exit_points: The archived Exit line, when the archive holds it.
fn verify_stop(
    deal: &Deal,
    exit: &ExitParams,
    modelled: &[LinePoint],
    closed: Exit,
    stop_level: Option<f64>,
    exit_points: Option<&[(i64, f64)]>,
) -> (Option<bool>, Option<f64>, Option<(usize, usize)>) {
    let level = stop_level.unwrap_or_else(|| {
        level_off_buy(
            deal.buy_price,
            stop_pct(exit, deal, deal.buy_ms),
            deal.is_long(),
        )
    });
    let stated = stated_stop_level(&deal.sell_reason);
    let panic_at = stop_jump_level(deal, exit);
    let mut activation: Option<i64> = None;
    let points = exit_points.filter(|p| !p.is_empty()).map(|archived| {
        let mut moves = archived_replacements(archived);
        if let Some(i) = panic_at.and_then(|level| stop_jump(deal, &moves, level)) {
            activation = Some(moves[i].0);
            moves.truncate(i);
        }
        (matched_points(modelled, &moves, &exit.model), moves.len())
    });
    let line_ok = points.is_none_or(|(matched, total)| matched == total);
    let trailing_fact = reason_starts_with(deal.sell_reason.trim(), REASON_TRAILING);
    let tolerance_ms = if exit.fast_stop_loss && !trailing_fact {
        exit.model.point_time_ms
    } else {
        exit.model.book_stop_time_ms
    };
    let on_time = (closed.t_ms - activation.unwrap_or(deal.close_ms)).abs() <= tolerance_ms;
    match stated {
        Some(stated) => {
            let dev = deviation_pct(level, stated);
            let level_ok = dev.is_some_and(|d| d.abs() <= exit.model.stop_price_pct);
            (Some(level_ok && on_time && line_ok), dev, points)
        }
        // No level on record — a book-watching stop whose stored reason cut it off (28 of 206
        // live), or a market stop, whose reason never carries one: the sale is a panic sell or a
        // market order swept through a book the tape does not carry, so what is left to judge
        // is the moment and the line, never the price. On the live sample (2026-09-23) 18 market
        // stops fired on time and failed on the sweep alone; a variant keeping the stop sells at
        // the fact's own price (`record::StopAnchor`).
        None => (Some(on_time && line_ok), None, points),
    }
}

/// The level an archived line's jump into the panic sell is read against:
///
/// - a stop's reason (`REASON_STOP`): the level the core printed into it when it kept one, else
///   the stop's own — `None` for a strategy without a stop;
/// - a trailing stop's reason (`REASON_TRAILING`): the fact's own line at the activation, under
///   the `PeakPrice` the core printed ([`trailing_level`]) — `None` without a trailing setting or
///   a readable peak.
///
/// `None` leaves the moment to the close. A `StopLoss Market Sell` can still be a trailing stop the
/// core rewrote (the core's answer of 2026-09-24): read against the stop's level, its market sale
/// stands short of it, no jump is found, and the moment is the close's as well.
pub(super) fn stop_jump_level(deal: &Deal, exit: &ExitParams) -> Option<f64> {
    let reason = deal.sell_reason.trim();
    if reason_starts_with(reason, REASON_TRAILING) {
        return (exit.trailing_pct != 0.0)
            .then(|| stated_peak(reason))
            .flatten()
            .map(|peak| trailing_level(peak, deal.buy_price, exit, deal.is_long()));
    }
    let pct = stop_pct(exit, deal, deal.buy_ms);
    (reason_starts_with(reason, REASON_STOP) && pct != 0.0).then(|| {
        stated_stop_level(reason)
            .unwrap_or_else(|| level_off_buy(deal.buy_price, pct, deal.is_long()))
    })
}

/// The peak a trailing stop's reason prints — `PeakPrice = X;` — when it is a usable price.
pub fn stated_peak(reason: &str) -> Option<f64> {
    const MARKER: &str = "PeakPrice =";
    let at = reason.find(MARKER)? + MARKER.len();
    let rest = reason[at..].trim_start();
    let len = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter(|&len| len > 0)?;
    let value: f64 = rest[..len].parse().ok()?;
    (value.is_finite() && value > 0.0).then_some(value)
}

/// Where the stop took over an archived line: the first move at or after the buy that stands at
/// or past the stop level — the panic sell's first price, or the market order's.
fn stop_jump(deal: &Deal, moves: &[(i64, f64)], level: f64) -> Option<usize> {
    moves
        .iter()
        .position(|&(t, p)| t >= deal.buy_ms && reaches(p, level, deal.is_long()))
}

/// The moment the core's own Exit line jumped past the stop level — the stop's activation as the
/// line records it — or `None` when the line holds no such move.
///
/// Args:
///     deal: The trade.
///     level: The stop level: the one the core printed when it did, else the model's.
///     exit_points: The core's own Exit line.
pub(super) fn archived_stop_jump(
    deal: &Deal,
    level: f64,
    exit_points: Option<&[(i64, f64)]>,
) -> Option<i64> {
    let moves = archived_replacements(exit_points?);
    stop_jump(deal, &moves, level).map(|i| moves[i].0)
}

/// The stop level the core printed into a book-watching stop's reason — `StopLoss fixed: X` —
/// when it is a usable price: positive, not cut off by the column's length, and printed finely
/// enough that its rounding sits inside [`PRICE_TOLERANCE`]. The reason is stored truncated,
/// and the level sits near its end: 28 of the 206 live reasons that carry it end inside the
/// number (`StopLoss fixed: 0.`), which would read as a stop at zero — so a number running
/// into the end of the text answers `None`, and [`verify_stop`] judges that stop by its moment
/// and its line alone.
pub fn stated_stop_level(reason: &str) -> Option<f64> {
    const MARKER: &str = "StopLoss fixed:";
    let at = reason.find(MARKER)? + MARKER.len();
    let rest = reason[at..].trim_start();
    let len = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter(|&len| len > 0)?;
    let token = &rest[..len];
    let value: f64 = token.parse().ok()?;
    let decimals = token.split_once('.').map_or(0, |(_, frac)| frac.len());
    let half_unit = 0.5 * 10f64.powi(-(decimals as i32));
    (value.is_finite() && value > 0.0 && half_unit / value <= PRICE_TOLERANCE).then_some(value)
}

/// How far a modelled entry may sit from the fact and still be the same order, per cent: the
/// corridor's own width (`MShotPrice − MShotPriceMin` with the trade's modifiers, as they stood
/// at the fill), floored at the price tolerance (`ModelSettings::price_pct`) — see the module
/// doc.
pub fn entry_tolerance_pct(params: &MshotParams, deal: &Deal) -> f64 {
    let (near, far) = params.bounds_pct(&deal.deltas_at(deal.buy_ms));
    (far - near).max(params.model.price_pct)
}

/// The replacements an archived line records: its first point and every point whose price
/// differs from the one before it. The core files a line as a polyline — each move as the
/// old level's end and the new level's start at the same instant — so the moves are the
/// price changes, not the points.
///
/// Args:
///     points: The archived `(t_ms, price)` points, in the archive's order.
pub fn archived_replacements(points: &[(i64, f64)]) -> Vec<(i64, f64)> {
    let mut out: Vec<(i64, f64)> = Vec::new();
    for &(t, p) in points {
        match out.last() {
            Some(&(_, last)) if deviation_pct(p, last).is_some_and(|d| d.abs() <= 1e-9) => {}
            _ => out.push((t, p)),
        }
    }
    out
}

/// Whether a modelled point is the archived move `(t, p)`: within the point tolerance in time
/// and the price tolerance in price.
fn same_move(m: &LinePoint, (t, p): (i64, f64), model: &ModelSettings) -> bool {
    (m.t_ms - t).abs() <= model.point_time_ms
        && deviation_pct(m.price, p).is_some_and(|d| d.abs() <= model.price_pct)
}

/// How many archived moves the modelled line re-placed at, within the tolerances.
fn matched_points(modelled: &[LinePoint], archived: &[(i64, f64)], model: &ModelSettings) -> usize {
    let modelled: Vec<&LinePoint> = modelled.iter().collect();
    matched_points_of(&modelled, archived, model)
}

/// [`matched_points`] over borrowed points.
fn matched_points_of(
    modelled: &[&LinePoint],
    archived: &[(i64, f64)],
    model: &ModelSettings,
) -> usize {
    archived
        .iter()
        .filter(|&&point| modelled.iter().any(|m| same_move(m, point, model)))
        .count()
}

/// The core's `sellreason` for a position its take closed.
pub const REASON_TAKE: &str = "Sell Price";

/// The core's `sellreason` prefixes for a position its moving line closed.
pub const REASONS_LINE: [&str; 2] = ["Auto Price Down", "Sell Level"];

/// The core's `sellreason` prefix for a position its stop closed.
pub const REASON_STOP: &str = "StopLoss";

/// The core's `sellreason` of a stop — or a trailing stop — sold at market (`UseMarketOrder`).
pub const REASON_MARKET_STOP: &str = "StopLoss Market Sell";

/// The core's `sellreason` prefix for a position its trailing stop closed with a limit panic
/// sell; with `UseMarketOrder` the core rewrites it to `StopLoss Market Sell`, like a stop's
/// (the core's answer of 2026-09-24).
pub const REASON_TRAILING: &str = "TrailingStop";

/// Whether the core closed the position by a stop or by its trailing stop — the exits the model
/// fires as [`ExitKind::Stop`] and judges by the decision, not the sale.
pub(super) fn is_stop_reason(sell_reason: &str) -> bool {
    let reason = sell_reason.trim();
    reason_starts_with(reason, REASON_STOP) || reason_starts_with(reason, REASON_TRAILING)
}

/// Whether the model's exit rule is the one the core's `sellreason` names, so the two prices
/// are comparable: the take against "Sell Price", the moving line against the PriceDown /
/// SellLevel reasons, the stop against "StopLoss …". A SellShot close has no counterpart: the
/// model has no SellShot, and a strategy under it is not judged (`exit::UnmodelledRule`).
fn exit_rule_matches(kind: ExitKind, sell_reason: &str) -> bool {
    let reason = sell_reason.trim();
    let starts = |prefix: &str| reason_starts_with(reason, prefix);
    match kind {
        ExitKind::Take => reason.eq_ignore_ascii_case(REASON_TAKE),
        ExitKind::Line => REASONS_LINE.iter().any(|r| starts(r)),
        ExitKind::Stop => is_stop_reason(reason),
        ExitKind::OpenAtWindowEnd | ExitKind::InGap => false,
    }
}

/// Whether a `sellreason` starts with an ASCII prefix, case-insensitively — on characters,
/// never bytes: the reason is database text, and a slice at a byte inside a multi-byte
/// character would panic.
pub(super) fn reason_starts_with(reason: &str, prefix: &str) -> bool {
    reason
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Share of ✓ over verdicts that answered, as `(hits, answered)`; the caption prints it and
/// the gate compares it with the threshold. Unanswered verdicts are out of both counts.
pub fn share(verdicts: impl IntoIterator<Item = Option<bool>>) -> (usize, usize) {
    verdicts
        .into_iter()
        .flatten()
        .fold((0, 0), |(hits, n), ok| (hits + usize::from(ok), n + 1))
}
