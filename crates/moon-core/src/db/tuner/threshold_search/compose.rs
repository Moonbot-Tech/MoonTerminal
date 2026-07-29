//! Choosing WHICH fields to filter on, not just where to put their thresholds.
//!
//! The descent in [`super::search`] already decides per field whether a range beats leaving it
//! alone, so a set of sorts falls out of it for free. What it cannot do is judge that set: it
//! maximizes profit on the very trades it fits, and with two dozen fields and hundreds of
//! quantile edges the best-fitting set is reliably the one that memorized the sample. Ask it to
//! consider every field and it will happily use every field.
//!
//! So the set is chosen HERE, and it is chosen on trades the fitting never saw. The fitting
//! region is cut into anchored walk-forward folds ([`super::fold_cuts`]): each fold fits on a
//! growing prefix and is measured on the stretch immediately after it. A candidate set is worth
//! what it earns on those measured stretches, averaged.
//!
//! Two rules do the real work:
//!
//! * a field joins only if it helps in a strict MAJORITY of folds, not merely on average — one
//!   fold's lucky streak can carry a mean, and that is how a wrapper selector talks itself into
//!   noise;
//! * when dropping a field costs nothing, it goes. Ties go to the SMALLER set, and that is the
//!   whole complexity penalty. Writing it as a penalty term instead would need a coefficient
//!   trading profit against field count — a number in someone's currency, varying by scope over
//!   orders of magnitude, that the user would have to tune. Needing to tune it is the complaint
//!   this feature exists to answer.
//!
//! The period the user is SHOWN as out-of-sample is not read here at all. Selection consumes
//! whatever it is measured on — a few dozen looks at the same stretch and it starts fitting that
//! too — so composition stays entirely inside the fitting region, and the held-back tail remains
//! the one number nobody optimized against.

use std::ops::Range;

use super::handle::SearchHandle;
use super::search::Search;
use crate::db::metrics::improvement_margin;

#[cfg(test)]
mod tests;

/// One walk-forward fold: a search fitted on a prefix, and the stretch it is measured on.
pub(super) struct Fold {
    /// Search prepared over `0..fit_end`, with its own quantile edges.
    pub(super) search: Search,
    /// Rows immediately after the fit prefix — never seen while fitting.
    pub(super) validate: Range<usize>,
}

/// What one composition run is allowed to do.
pub(super) struct ComposeParams {
    /// Restarts each candidate ranking run gets. Deliberately smaller than the final refit's:
    /// ranking needs to order candidates, not to squeeze the last basis point out of one.
    pub(super) restarts: usize,
    /// Base seed; every step derives its own stream from it.
    pub(super) seed: u64,
    /// Whether bounds are rounded outward — the user's own setting, applied here too so what is
    /// SELECTED is what will be APPLIED.
    pub(super) round: bool,
    /// Fields the forward pass may accept.
    pub(super) max_fields: usize,
}

/// The composed set.
pub(super) struct ComposeOutcome {
    /// Per field, whether it is in the chosen set.
    pub(super) chosen: Vec<bool>,
    /// Per field, how many folds gave it a range under the FINAL set — how reproducible its
    /// contribution is across time, which is the only confidence figure here that was not
    /// selected for.
    pub(super) support: Vec<u8>,
    /// Folds the figures rest on, so a caller can render "2/3" without assuming how many there
    /// were.
    pub(super) folds: u8,
}

/// How wide a composition may go on this machine.
///
/// Machine-fixed only. The per-run ranking share is [`Self::ranking_restarts`] rather than a
/// field, so a caller that only wants to know WHETHER this machine composes — and to caption the
/// three numbers below — does not have to invent a restart count to ask.
pub struct ComposeBudget {
    /// Folds the fitting region is cut into.
    pub folds: usize,
    /// Fields the forward pass may accept.
    pub max_fields: usize,
    /// Logical cores this was built for, so the UI can explain the numbers it is showing.
    pub cores: usize,
}

impl ComposeBudget {
    /// Restarts one candidate ranking run gets, out of the count the user asked for.
    ///
    /// A quarter of it: ranking has to ORDER candidates, not squeeze the last basis point out of
    /// one, and it pays that cost once per candidate per fold. Never zero — a run of no restarts
    /// finds nothing and would read as "this scope has no answer" rather than as a budget too
    /// small to say. The FINAL refit is not scaled here at all.
    pub fn ranking_restarts(&self, user_restarts: usize) -> usize {
        (user_restarts / 4).max(super::RESTARTS_MIN)
    }
}

/// The composition budget for THIS machine, or `None` when it is below
/// [`super::search::HEAVY_SEARCH_MIN_CORES`].
///
/// `None` is what makes the feature disappear rather than shrink: the UI has no budget to caption
/// a switch with, and the search has none to run on.
pub fn composition_budget() -> Option<ComposeBudget> {
    budget_for(super::search::logical_cores())
}

/// [`composition_budget`] with the core count supplied, so the policy itself is testable.
///
/// One tier above the bar: a machine with twice the bar's cores composes no wider. The width is
/// set by what makes the ANSWER trustworthy — three folds and up to six fields — not by what the
/// hardware could stand, and a bigger machine spends its cores finishing sooner.
fn budget_for(cores: usize) -> Option<ComposeBudget> {
    if cores < super::search::HEAVY_SEARCH_MIN_CORES {
        return None;
    }
    Some(ComposeBudget {
        folds: 3,
        max_fields: 6,
        cores,
    })
}

/// Mean of a fold score vector; zero for an empty one.
fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// The noise floor two fold-score vectors must clear to count as different.
///
/// Scaled to the LARGEST fold profit in play rather than to the mean being compared. The folds
/// routinely straddle zero, so their mean can cancel down to almost nothing while the numbers it
/// was built from are in the thousands — and a tolerance derived from that cancelled mean would
/// be far below the precision the sum actually carries.
fn noise_floor(a: &[f64], b: &[f64]) -> f64 {
    let scale = a.iter().chain(b).fold(0.0f64, |acc, v| acc.max(v.abs()));
    improvement_margin(scale)
}

/// Whether a candidate set is worth taking over the incumbent.
///
/// Two conditions, both required: it must earn more on average, and it must be better in a
/// strict MAJORITY of folds. The majority clause is the one that matters — an average alone is
/// carried by a single fold where the field happened to catch a run, which is exactly the
/// coincidence out-of-sample scoring is supposed to filter out.
///
/// Pure, so the rule can be tested without a sample, a search, or a database.
fn accepts(incumbent: &[f64], candidate: &[f64]) -> bool {
    if candidate.len() != incumbent.len() || incumbent.is_empty() {
        return false;
    }
    if mean(candidate) <= mean(incumbent) + noise_floor(incumbent, candidate) {
        return false;
    }
    let wins = incumbent
        .iter()
        .zip(candidate)
        .filter(|(before, after)| after > before)
        .count();
    2 * wins > incumbent.len()
}

/// Whether a field can be dropped: doing so must cost nothing beyond noise.
///
/// `>=`, not `>`. A field that earns its keep exactly is not earning it, and preferring the
/// smaller set on a tie IS the complexity penalty.
fn drops(incumbent: &[f64], without: &[f64]) -> bool {
    if without.len() != incumbent.len() || incumbent.is_empty() {
        return false;
    }
    mean(without) + noise_floor(incumbent, without) >= mean(incumbent)
}

/// Score one candidate set: per fold, the profit its ranges earn on that fold's unseen stretch.
///
/// Returns `None` when the run was stopped — a partial score vector is not comparable with a
/// complete one, and picking a winner from mixed evidence is worse than not answering.
fn score_set(
    folds: &[Fold],
    mask: &[bool],
    p: &ComposeParams,
    step: usize,
    handle: &SearchHandle,
) -> Option<Vec<f64>> {
    let seed = super::search::restart_seed(p.seed, step);
    // An empty set assigns no range whatever the random start, so every restart returns the same
    // outcome. Ranking the incumbent at step one — the one score always taken over an empty mask
    // — would otherwise repeat a single answer up to thousands of times per fold.
    let restarts = if mask.iter().any(|m| *m) {
        p.restarts
    } else {
        1
    };
    folds
        .iter()
        .map(|fold| {
            let outcome = fold.search.run_masked(mask, restarts, seed, handle)?;
            let applied = fold.search.applied_ranges(&outcome.sel, p.round);
            Some(fold.search.tally(&applied, fold.validate.clone()).profit)
        })
        .collect()
}

/// Fields a set may still take on, honouring the two-slot limit on Delta2/Delta3 fields.
///
/// `is_slot` is borrowed from the folds' own searches rather than carried alongside them: two
/// copies of one per-field flag vector would have to agree for this to match the descent's own
/// slot rule, and nothing would check that they did.
///
/// The descent enforces that limit internally too, so this changes no answer — it stops the
/// forward pass from spending a step, and one of the user's `max_fields`, on a field that could
/// never carry a range anyway.
fn slots_are_full(mask: &[bool], is_slot: &[bool], locked_slots: usize) -> bool {
    let used = mask
        .iter()
        .zip(is_slot)
        .filter(|(chosen, slot)| **chosen && **slot)
        .count();
    locked_slots + used >= 2
}

/// Compose the field set: grow it while growing helps, then shrink it while shrinking is free.
///
/// Returns `None` when the run was stopped before it had a complete answer.
pub(super) fn compose(
    folds: &[Fold],
    p: &ComposeParams,
    handle: &SearchHandle,
) -> Option<ComposeOutcome> {
    let first = folds.first()?;
    let nf = first.search.free_mask().len();
    let locked_slots = first.search.locked_slots();
    let is_slot = first.search.slot_flags();
    // A field is a candidate only where EVERY fold can search it. The folds share one `locked`,
    // so they already agree; intersecting rather than trusting that keeps a future fold-building
    // change from quietly handing the forward pass a field one fold would ignore.
    let candidate: Vec<bool> = (0..nf)
        .map(|fi| folds.iter().all(|f| f.search.free_mask()[fi]))
        .collect();

    let mut mask = vec![false; nf];
    // Every score in this function is taken at the CURRENT step's seed, so nothing is carried
    // between steps: a set's worth is only ever compared with another set measured on the same
    // random starts.
    let mut step = 0usize;

    // ── forward: take the best field while one is worth taking ──
    while mask.iter().filter(|m| **m).count() < p.max_fields {
        step += 1;
        let slots_full = slots_are_full(&mask, is_slot, locked_slots);
        let pool: Vec<usize> = (0..nf)
            .filter(|fi| candidate[*fi] && !mask[*fi] && !(slots_full && is_slot[*fi]))
            .collect();
        if pool.is_empty() {
            break;
        }
        // The incumbent is re-scored on THIS step's stream. Comparing it at the seed of an
        // earlier step would put the two sides of the decision on different random starts, and
        // the difference between them would carry the seed's luck as well as the field's.
        let incumbent = score_set(folds, &mask, p, step, handle)?;
        let mut best: Option<(usize, Vec<f64>)> = None;
        for (i, fi) in pool.iter().enumerate() {
            handle.set_stage(step, i, pool.len());
            mask[*fi] = true;
            let scored = score_set(folds, &mask, p, step, handle);
            mask[*fi] = false;
            let scored = scored?;
            // Admissibility FIRST, ranking second. Ranking by mean and testing the winner
            // afterwards lets one fold's outlier decide the whole step: a field that earns a
            // fortune in a single fold and loses in the rest has the highest mean, fails the
            // majority rule, and — being the only candidate ever tested — ends the forward pass
            // while a field that satisfies both rules sits untaken in the same pool. That is the
            // exact coincidence the majority rule exists to reject, given veto power instead.
            if !accepts(&incumbent, &scored) {
                continue;
            }
            // Strict improvement only, so among equal candidates the LOWEST field index wins —
            // the same tie-break the restart merge uses, and the reason a rerun of one seed
            // returns one answer rather than whichever candidate the loop happened to reach.
            if best
                .as_ref()
                .is_none_or(|(_, b)| mean(&scored) > mean(b) + noise_floor(b, &scored))
            {
                best = Some((*fi, scored));
            }
        }
        // Nothing in the pool clears both rules against the incumbent: the set is done growing.
        let Some((fi, _)) = best else { break };
        mask[fi] = true;
    }

    // ── backward: give back every field the set turned out not to need ──
    //
    // The forward pass takes fields one at a time against the set it had at the time, so a field
    // that earned its place early can be made redundant by a better one arriving later. Without
    // this pass that field stays in the strategy forever, doing nothing but narrowing it.
    loop {
        step += 1;
        let mut incumbent = score_set(folds, &mask, p, step, handle)?;
        let mut removed = false;
        let mut tried = 0usize;
        let held = mask.iter().filter(|m| **m).count();
        for fi in 0..nf {
            if !mask[fi] {
                continue;
            }
            handle.set_stage(step, tried, held);
            tried += 1;
            mask[fi] = false;
            let without = score_set(folds, &mask, p, step, handle);
            let Some(without) = without else {
                mask[fi] = true;
                return None;
            };
            if drops(&incumbent, &without) {
                incumbent = without;
                removed = true;
            } else {
                mask[fi] = true;
            }
        }
        if !removed {
            break;
        }
    }

    // ── support: how many folds actually gave each chosen field a range ──
    //
    // Measured under the FINAL set rather than accumulated during the search, so the number
    // beside a field describes the set the user is being shown and cannot disagree with it.
    step += 1;
    let mut support = vec![0u8; nf];
    let seed = super::search::restart_seed(p.seed, step);
    for (i, fold) in folds.iter().enumerate() {
        handle.set_stage(step, i, folds.len());
        let outcome = fold.search.run_masked(&mask, p.restarts, seed, handle)?;
        for (fi, _, _) in fold.search.applied_ranges(&outcome.sel, p.round) {
            support[fi] = support[fi].saturating_add(1);
        }
    }
    Some(ComposeOutcome {
        chosen: mask,
        support,
        folds: folds.len().min(u8::MAX as usize) as u8,
    })
}
