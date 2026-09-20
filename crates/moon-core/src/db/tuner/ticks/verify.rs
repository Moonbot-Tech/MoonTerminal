//! Reproducing the fact — the check every search has to pass before it may run.
//!
//! The model is run with the strategy's parameters AS THEY WERE at the trade and its answer is
//! held against the report row, per group: did the entry fill where the core's did, did the
//! exit land where the core's did. The share of ✓ over the sample is the caption's "model ✓
//! K %", and a group below the caller's threshold is not searched at all — a search over a model
//! that cannot reproduce what happened optimizes noise.
//!
//! A group the model does not cover answers `None`, not `false`: a Spread's entry is not wrong,
//! it is not modelled, and phase 1's exit is not modelled wherever the take did not close it.

use super::{Deal, EntryParams, ExitKind, ExitParams, Fill, PRICE_TOLERANCE, simulate};
use crate::feed::types::Tick;

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
    /// How the modelled position closed, when it filled.
    pub exit_kind: Option<ExitKind>,
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
pub fn verify(
    deal: &Deal,
    ticks: &[Tick],
    entry: &EntryParams,
    exit: &ExitParams,
    entry_start: Option<(i64, f64)>,
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
    let (exit_ok, exit_dev) = match outcome.exit {
        Some(exit) if exit_rule_matches(exit.kind, &deal.sell_reason) => {
            let dev = deviation_pct(exit.price, deal.sell_price);
            let ok = dev.is_some_and(|d| d.abs() <= PRICE_TOLERANCE * 100.0);
            (Some(ok), dev)
        }
        _ => (None, None),
    };
    Verdict {
        entry: entry_ok,
        entry_dev_pct: entry_dev,
        exit: exit_ok,
        exit_dev_pct: exit_dev,
        fill: outcome.fill,
        exit_kind: outcome.exit.map(|e| e.kind),
    }
}

/// The core's `sellreason` for a position its take closed.
pub const REASON_TAKE: &str = "Sell Price";

/// Whether the model's exit rule is the one the core's `sellreason` names, so the two prices
/// are comparable. Only the take is modelled today; the moving line and the stop join in
/// phase 2 with their own reasons (`Auto Price Down`, `StopLoss …`).
fn exit_rule_matches(kind: ExitKind, sell_reason: &str) -> bool {
    match kind {
        ExitKind::Take => sell_reason.trim().eq_ignore_ascii_case(REASON_TAKE),
        ExitKind::Line | ExitKind::Stop | ExitKind::Fact | ExitKind::OpenAtWindowEnd => false,
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
