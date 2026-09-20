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
//! The exit is walked from the FACTUAL entry and held against two things: the price the core
//! sold at, and — when the order archive holds the trade's Exit line — every move the core
//! made with its sell, each of which the model must have made too within
//! [`POINT_TIME_TOLERANCE_MS`] and [`PRICE_TOLERANCE`]. A model that lands on the right price
//! by a different path has not reproduced the rule.

use super::exit::ExitModel;
use super::line::LinePoint;
use super::{Deal, EntryParams, ExitKind, ExitParams, Fill, PRICE_TOLERANCE, simulate};
use crate::feed::types::Tick;

/// How far apart a modelled and an archived replacement may be in time and still be the same
/// move: the archive stamps the core's own moment, the model the print that triggered it.
pub const POINT_TIME_TOLERANCE_MS: i64 = 1_000;

/// Tolerance on a STOP's price: the core's stop is a market order, and the report's
/// `sellprice` is what the book gave for it, while the model knows only the print that
/// fired it. Measured on the live tape (2026-09-20): 0.16–0.28 % between the two on a spike.
pub const STOP_PRICE_TOLERANCE: f64 = 0.003;

/// One trade's reproduction verdict, per group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verdict {
    /// Entry reproduced: `Some(true)` within tolerance, `Some(false)` off or unfilled, `None`
    /// when the kind has no entry model (the entry was taken from the fact).
    pub entry: Option<bool>,
    /// Modelled fill against the fact, per cent of the fact (`None` when unfilled or unmodelled).
    pub entry_dev_pct: Option<f64>,
    /// Exit reproduced; `None` when no exit rule decided (the exit was taken from the fact),
    /// when there was no fill to exit from, or when the core closed by a rule the model does
    /// not have yet (`sellreason` is not the take's) — the two prices are not comparable then.
    pub exit: Option<bool>,
    /// Modelled exit against the fact, per cent of the fact.
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
///     entry_start: The archived first point of the entry line, when known.
///     exit_points: The archived Exit line's `(t_ms, price)` points, when the archive holds
///         them; the modelled line must re-place at each.
pub fn verify(
    deal: &Deal,
    ticks: &[Tick],
    entry: &EntryParams,
    exit: &ExitParams,
    entry_start: Option<(i64, f64)>,
    exit_points: Option<&[(i64, f64)]>,
) -> Verdict {
    let outcome = simulate(deal, ticks, entry, exit, entry_start);
    let entry_modelled = !matches!(entry, EntryParams::Fact);
    let (entry_ok, entry_dev) = match (entry_modelled, outcome.fill) {
        (false, _) => (None, None),
        (true, None) => (Some(false), None),
        (true, Some(fill)) => {
            let dev = deviation_pct(fill.price, deal.buy_price);
            let ok = dev.is_some_and(|d| d.abs() <= PRICE_TOLERANCE * 100.0);
            (Some(ok), dev)
        }
    };
    // The exit answers only where the model's rule and the core's were the same rule: a
    // modelled take against a fact the core closed by its take. A take held against an
    // "Auto Price Down" fact is not a miss of the take model, it is a rule the model does not
    // have yet (phase 2), and it stays unanswered until it does.
    // The exit group is judged from the FACTUAL entry, whatever the entry group modelled: the
    // sell rules measure from the buy, and a fill the model placed a few ticks off would shift
    // every level of a correctly reproduced line. The entry group has its own verdict above.
    let fact_fill = Fill {
        t_ms: deal.buy_ms,
        price: deal.buy_price,
    };
    let walked = ExitModel::new(exit).walk(deal, ticks, fact_fill);
    let closed = walked.exit;
    let (exit_ok, exit_dev, line_points) = if closed.kind == ExitKind::OpenAtWindowEnd {
        // The core closed it; the model never did inside the same tape: a miss of the exit
        // group, not an unanswered question.
        (Some(false), None, None)
    } else if exit_rule_matches(closed.kind, &deal.sell_reason) {
        let dev = deviation_pct(closed.price, deal.sell_price);
        let tolerance = if closed.kind == ExitKind::Stop {
            STOP_PRICE_TOLERANCE
        } else {
            PRICE_TOLERANCE
        };
        let price_ok = dev.is_some_and(|d| d.abs() <= tolerance * 100.0);
        let points = exit_points.filter(|p| !p.is_empty()).map(|archived| {
            let moves = archived_replacements(archived);
            (matched_points(&walked.points, &moves), moves.len())
        });
        let line_ok = points.is_none_or(|(matched, total)| matched == total);
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

/// How many archived moves the modelled line re-placed at, within the tolerances.
fn matched_points(modelled: &[LinePoint], archived: &[(i64, f64)]) -> usize {
    archived
        .iter()
        .filter(|&&(t, p)| {
            modelled.iter().any(|m| {
                (m.t_ms - t).abs() <= POINT_TIME_TOLERANCE_MS
                    && deviation_pct(m.price, p).is_some_and(|d| d.abs() <= PRICE_TOLERANCE * 100.0)
            })
        })
        .count()
}

/// The core's `sellreason` for a position its take closed.
pub const REASON_TAKE: &str = "Sell Price";

/// The core's `sellreason` prefixes for a position its moving line closed.
pub const REASONS_LINE: [&str; 3] = ["Auto Price Down", "Sell Level", "SellShot"];

/// The core's `sellreason` prefix for a position its stop closed.
pub const REASON_STOP: &str = "StopLoss";

/// Whether the model's exit rule is the one the core's `sellreason` names, so the two prices
/// are comparable: the take against "Sell Price", the moving line against the PriceDown /
/// SellLevel / SellShot reasons, the stop against "StopLoss …".
fn exit_rule_matches(kind: ExitKind, sell_reason: &str) -> bool {
    let reason = sell_reason.trim();
    let starts = |prefix: &str| {
        reason.len() >= prefix.len() && reason[..prefix.len()].eq_ignore_ascii_case(prefix)
    };
    match kind {
        ExitKind::Take => reason.eq_ignore_ascii_case(REASON_TAKE),
        ExitKind::Line => REASONS_LINE.iter().any(|r| starts(r)),
        ExitKind::Stop => starts(REASON_STOP),
        ExitKind::OpenAtWindowEnd => false,
    }
}

/// Share of ✓ over verdicts that answered, as `(hits, answered)`; the caption prints it and
/// the gate compares it with the threshold. Unanswered verdicts are out of both counts.
pub fn share(verdicts: impl IntoIterator<Item = Option<bool>>) -> (usize, usize) {
    verdicts
        .into_iter()
        .flatten()
        .fold((0, 0), |(hits, n), ok| (hits + usize::from(ok), n + 1))
}
