//! What a point must keep so that every trade it buys can close while it lasts (the developer,
//! 2026-09-24): "something must work on every trade — the stop, or the trailing without a take
//! profit; a trade closes by its stop or its sell". The second and third stops live on the first
//! (`UseStopLoss`), and a trailing with a take profit stands nowhere until the take profit is
//! passed, so neither guards a trade on its own.

use std::collections::HashMap;

use super::{PreparedDeal, Tally, params_of, point_of, results};
use crate::db::tuner::ticks::EntryParams;
use crate::db::tuner::ticks::exit::ExitParams;
use crate::db::tuner::ticks::settings::ModelSettings;

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

/// The tally of a point that closes every deal it buys inside the tape — by its stop or its
/// sell — else `None`: a point that leaves one open is not one the search may pick, since the
/// loss it would carry past the tape is on no record and dropping the deal would only reward it
/// (the developer, 2026-09-24).
pub(super) fn closed_tally(
    deals: &[PreparedDeal],
    of_deal: &[usize],
    params: &[(EntryParams, ExitParams)],
) -> Option<Tally> {
    let mut tally = Tally::default();
    for (result, open) in results(deals, of_deal, params) {
        if open {
            return None;
        }
        if let Some((money, _)) = result {
            tally.push(money);
        }
    }
    Some(tally)
}

/// The tally of a point over `deals` — the deals it closed — and how many it bought and left
/// open inside the tape, which the tally cannot hold.
pub(super) fn tally_counting_open(
    deals: &[PreparedDeal],
    of_deal: &[usize],
    params: &[(EntryParams, ExitParams)],
) -> (Tally, usize) {
    let mut tally = Tally::default();
    let mut open = 0;
    for (result, left_open) in results(deals, of_deal, params) {
        open += usize::from(left_open);
        if let Some((money, _)) = result {
            tally.push(money);
        }
    }
    (tally, open)
}

#[cfg(test)]
mod tests;
