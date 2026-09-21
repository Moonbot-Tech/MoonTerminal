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
//! each of which the model must have made too within [`POINT_TIME_TOLERANCE_MS`] and
//! [`PRICE_TOLERANCE`]. A model that lands on the right price by a different path has not
//! reproduced the rule. Which PRINT the model would have sold on is not judged: that is the
//! queue at the level (the spec's §7), which the tape does not carry — a print at the level
//! sold the core's line on ARX and left it standing on COOL the same day. A stop is the one
//! exit judged by its firing: it is a market order on the print, not a resting line.

use super::exit::ExitModel;
use super::line::LinePoint;
use super::mshot::MshotParams;
use super::{Deal, EntryParams, Exit, ExitKind, ExitParams, Fill, PRICE_TOLERANCE, simulate};
use crate::feed::types::Tick;

/// How far apart a modelled and an archived replacement may be in time and still be the same
/// move: the archive stamps the core's own moment, the model the print that triggered it.
pub const POINT_TIME_TOLERANCE_MS: i64 = 1_000;

/// Tolerance on a STOP's price: the core's stop is a market order, and the report's
/// `sellprice` is what the book gave for it, while the model knows only the print that
/// fired it. Measured on the live tape (2026-09-20): 0.16–0.28 % between the two on a spike.
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
    /// fact, a line against a stop the model never fired) — the two prices are not comparable
    /// then. `Some(false)` when no level stood at the close at all.
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
    // every level of a correctly reproduced line. The entry group has its own verdict above.
    let fact_fill = Fill {
        t_ms: deal.buy_ms,
        price: deal.buy_price,
    };
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
    let fact_stopped = reason_starts_with(deal.sell_reason.trim(), REASON_STOP);
    let closed = match walked.exit.kind {
        ExitKind::Stop
            if walked.exit.t_ms <= deal.close_ms + POINT_TIME_TOLERANCE_MS || fact_stopped =>
        {
            walked.exit
        }
        _ => {
            // A point the model stamps up to its own latency after the close is a move due
            // before it — the core's stamp is its moment, the model's the print plus latency.
            let mut placed: Vec<&LinePoint> = walked
                .points
                .iter()
                .filter(|p| p.t_ms <= deal.close_ms + exit.latency_ms.max(0.0) as i64)
                .collect();
            // In time order: the take is stamped when it is armed, after any timer step
            // that fell due inside the sell delay.
            placed.sort_by_key(|p| p.t_ms);
            match placed.last() {
                Some(level) => Exit {
                    t_ms: deal.close_ms,
                    price: level.price,
                    kind: if placed.len() > 1 {
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
    let (exit_ok, exit_dev, line_points) = if closed.kind == ExitKind::OpenAtWindowEnd {
        // No line stood at the close: a miss of the exit group, not an unanswered question.
        (Some(false), None, None)
    } else if exit_rule_matches(closed.kind, &deal.sell_reason) {
        let dev = deviation_pct(closed.price, deal.sell_price);
        let tolerance = if closed.kind == ExitKind::Stop {
            STOP_PRICE_TOLERANCE
        } else {
            PRICE_TOLERANCE
        };
        // Within the tolerance either way, or a limit's fill on the better side of its level:
        // `dev` is the model against the fact, so a fact above the modelled sell (a long) or
        // below the modelled buy-back (a short) reads as a negative deviation of the model.
        let improved = |d: f64| match closed.kind {
            ExitKind::Stop => false,
            _ => {
                let better = if deal.is_long() { -d } else { d };
                better > 0.0 && better <= FILL_IMPROVEMENT_TOLERANCE * 100.0
            }
        };
        // The archive's last point AT the sale — within the model's latency of the close, at
        // the price the core sold at (GUN 2026-09-21: 31 ms before it, at the average fill)
        // — is the fill filed as a point, not a move of the line; a re-placement any earlier,
        // or at another price, is a move the model has to have made.
        let fill_window_ms = exit.latency_ms.max(0.0) as i64;
        let points = exit_points.filter(|p| !p.is_empty()).map(|archived| {
            let mut moves = archived_replacements(archived);
            if moves.len() > 1
                && moves.last().is_some_and(|&(t, p)| {
                    (t - deal.close_ms).abs() <= fill_window_ms
                        && deviation_pct(p, deal.sell_price)
                            .is_some_and(|d| d.abs() <= PRICE_TOLERANCE * 100.0)
                })
            {
                moves.pop();
            }
            (matched_points(&walked.points, &moves), moves.len())
        });
        let line_ok = points.is_none_or(|(matched, total)| matched == total);
        let corroborated = points.is_some_and(|(matched, total)| matched == total);
        let price_ok =
            dev.is_some_and(|d| d.abs() <= tolerance * 100.0 || (corroborated && improved(d)));
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

/// How far a modelled entry may sit from the fact and still be the same order, per cent: the
/// corridor's own width (`MShotPrice − MShotPriceMin` with the trade's modifiers), floored at
/// [`PRICE_TOLERANCE`] — see the module doc.
pub fn entry_tolerance_pct(params: &MshotParams, deal: &Deal) -> f64 {
    let (near, far) = params.bounds_pct(&deal.deltas);
    (far - near).max(PRICE_TOLERANCE * 100.0)
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
    let starts = |prefix: &str| reason_starts_with(reason, prefix);
    match kind {
        ExitKind::Take => reason.eq_ignore_ascii_case(REASON_TAKE),
        ExitKind::Line => REASONS_LINE.iter().any(|r| starts(r)),
        ExitKind::Stop => starts(REASON_STOP),
        ExitKind::OpenAtWindowEnd => false,
    }
}

/// Whether a `sellreason` starts with an ASCII prefix, case-insensitively — on characters,
/// never bytes: the reason is database text, and a slice at a byte inside a multi-byte
/// character would panic.
fn reason_starts_with(reason: &str, prefix: &str) -> bool {
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
