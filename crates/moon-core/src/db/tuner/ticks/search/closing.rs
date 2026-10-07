//! What a point must keep so that every trade it buys can close while it lasts (the developer,
//! 2026-09-24): "something must work on every trade — the stop, or the trailing without a take
//! profit; a trade closes by its stop or its sell". The second and third stops live on the first
//! (`UseStopLoss`), and a trailing with a take profit stands nowhere until the take profit is
//! passed, so neither guards a trade on its own.

use std::collections::HashMap;

use super::{Bases, Point, PreparedDeal, SearchParams, Tally, params_of, point_of, results};
use crate::db::tuner::ticks::exit::ExitParams;
use crate::db::tuner::ticks::settings::ModelSettings;
use crate::db::tuner::ticks::{EntryParams, Fill};

/// Whether every strategy of the point keeps a guard that stands from the fill on: a stop, or a
/// trailing stop without a take profit.
pub(super) fn protected(per_base: &[(EntryParams, ExitParams)]) -> bool {
    per_base.iter().all(|(_, exit)| guarded(exit))
}

/// How many of the strategies `owns` a variant's `values` would leave with nothing standing to
/// close a trade — the rule the search refuses a point by ([`protected`]), asked of a variant as
/// it will be written, typed or found.
///
/// Args:
///     owns: Each strategy's own values, once per strategy.
///     defaults: The live schema's numeric defaults.
///     kind: The strategies' kind, for the entry model; the guard is the exit's either way.
///     values: The variant's changes, in strategy spelling.
///     model: The model's own settings.
pub fn unguarded_strategies<'a>(
    owns: impl IntoIterator<Item = &'a HashMap<String, String>>,
    defaults: &HashMap<String, f64>,
    kind: &str,
    values: &[(String, String)],
    model: ModelSettings,
) -> usize {
    let point = point_of(values);
    let held = HashMap::new();
    owns.into_iter()
        .filter(|own| {
            let (_, exit) = params_of(own, &held, defaults, &point, kind, model.sanitized());
            !guarded(&exit)
        })
        .count()
}

/// One strategy's guard.
fn guarded(exit: &ExitParams) -> bool {
    exit.stop_loss_pct != 0.0
        || (exit.trailing_pct != 0.0 && exit.trailing_take_profit_pct.is_none())
}

/// One point's replay of a slice, read the two ways the search needs it: the trades it closed
/// inside the tape, and how many deals it bought and left open there.
#[derive(Clone, Debug, Default)]
pub(super) struct Replayed {
    /// The deals the point bought and closed inside the tape, in order.
    pub(super) closed: Tally,
    /// The deals the point bought and closed neither inside the tape nor provably in a hole of
    /// it ([`crate::db::tuner::ticks::Outcome::left_open`]).
    pub(super) open: usize,
}

impl Replayed {
    /// The tally of a point that closes every deal it buys inside the tape — by its stop or its
    /// sell — else `None`: such a point is the only one the search may ANSWER, since the loss an
    /// open deal would carry past the tape is on no record and dropping the deal would only
    /// reward it (the developer, 2026-09-24).
    pub(super) fn closed_all(&self) -> Option<&Tally> {
        (self.open == 0).then_some(&self.closed)
    }
}

/// Replay `deals` under `params` on the entry `fills` read for them
/// ([`super::fills::FillCache`]).
pub(super) fn replay(
    deals: &[PreparedDeal],
    of_deal: &[usize],
    params: &[(EntryParams, ExitParams)],
    fills: &[Option<Fill>],
) -> Replayed {
    let mut replayed = Replayed::default();
    for (result, open, _) in results(deals, of_deal, params, Some(fills)) {
        if open {
            replayed.open += 1;
        } else if let Some((money, _)) = result {
            replayed.closed.push(money);
        }
    }
    replayed
}

/// The sample less the deals the strategies as they stand leave open inside the tape — the
/// variant's held edits over them and nothing else moved, completed as every point is
/// (`deps::Dependents::complete`), over the same bases the search then scores on: restart 0's
/// point less the fields it pins on their grids (`pinned`) — with the kept deals' indices into
/// `bases.owns`, and the dropped deals' ids.
///
/// Such a deal is no point's doing: a gap in its tape, a rule the model does not have. Held in the
/// sample it would refuse every point, the strategy itself among them, and the search could not
/// move even the one field it was asked about (LinKvo, 2026-09-24: "the stops are in the strategy
/// and must stay; only the selected field is searched"). A point is still refused when it leaves
/// open a deal the strategies as they stand close ([`Replayed::closed_all`]).
///
/// Args:
///     deals: The sample, cut at its horizon.
///     bases: The sample's bases ([`Bases::of`] over `deals`), kept by the search as they are.
///     params: The search's parameters: the held edits, the defaults, the kind, the model.
///     deps: The search's completion ([`super::deps::dependents_of`]) over `bases`.
pub(super) fn closable_at_base(
    deals: &[PreparedDeal],
    bases: &Bases<'_>,
    params: &SearchParams<'_>,
    deps: &super::deps::Dependents,
) -> (Vec<PreparedDeal>, Vec<usize>, Vec<i64>) {
    let point = deps.complete(&Point::new(), &bases.owns, params.held, params.defaults);
    let base = bases.params(
        params.held,
        params.defaults,
        &point,
        params.kind,
        params.model.sanitized(),
    );
    let mut kept = Vec::with_capacity(deals.len());
    let mut of_kept = Vec::with_capacity(deals.len());
    let mut left_open = Vec::new();
    for (((_, open, _), deal), &base_of) in results(deals, &bases.of_deal, &base, None)
        .into_iter()
        .zip(deals)
        .zip(&bases.of_deal)
    {
        if open {
            left_open.push(deal.deal.report_uid);
        } else {
            kept.push(deal.clone());
            of_kept.push(base_of);
        }
    }
    if !left_open.is_empty() {
        log::info!(
            target: crate::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks search: {} of {} deal(s) the strategies as they stand leave open inside the tape are out of the sample: {:?}",
            left_open.len(),
            deals.len(),
            left_open
        );
    }
    (kept, of_kept, left_open)
}

#[cfg(test)]
mod tests;
