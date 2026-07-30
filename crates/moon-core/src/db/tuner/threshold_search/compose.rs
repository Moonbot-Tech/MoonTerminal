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
//! * a completed reduced set is accepted only if it helps in a strict MAJORITY of folds, not
//!   merely on average — weak intermediate branches may survive long enough to expose a useful
//!   interaction, but one fold's lucky streak cannot admit their final set;
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
//!
//! A bounded beam keeps several alternative paths alive, including temporarily weak singletons
//! that may form a strong pair. Up to three folds rank those paths; two later folds see only the
//! one subset and decide which of three paths wins: that strict subset, all admitted fields, or
//! no additional field filters.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::ops::Range;

use super::handle::SearchHandle;
use super::search::Search;
use super::ComposeDecision;
use crate::db::metrics::improvement_margin;

#[cfg(test)]
mod tests;

/// Parallel branches retained at each composition depth.
///
/// Eight keeps twice as many alternative interactions alive as the initial beam without turning
/// 24 fields into an exponential search. Growing through all 24 fields produces at most 2,232
/// scored masks before slot constraints and duplicate children reduce that number.
const BEAM_WIDTH: usize = 8;

/// Largest restart count spent fitting one candidate on one ranking fold.
///
/// Candidate selection is a coarse ordering pass. The final refit still receives the user's full
/// restart count, while this cap gives the beam a hard work bound independent of a 50k setting.
const RANKING_RESTARTS_MAX: usize = 256;

/// User restarts represented by one ranking restart before the hard cap applies.
const RANKING_RESTART_DIVISOR: usize = 32;

/// Final folds reserved from beam ranking and used only to gate its one winner.
const GATE_FOLDS: usize = 2;

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
    pub(super) ranking_restarts: usize,
    /// Full user restart budget used for the two outer-gate comparisons.
    ///
    /// The ordinary all-field control benefits more from additional starts than a reduced mask,
    /// so gating both on the ranking budget would make the ordinary option artificially weak.
    pub(super) gate_restarts: usize,
    /// Base seed; every step derives its own stream from it.
    pub(super) seed: u64,
    /// Whether bounds are rounded outward — the user's own setting, applied here too so what is
    /// SELECTED is what will be APPLIED.
    pub(super) round: bool,
    /// Deepest set the beam may build.
    pub(super) max_fields: usize,
    /// Parallel branches retained at each depth.
    pub(super) beam_width: usize,
}

/// The composed set.
pub(super) struct ComposeOutcome {
    /// Per field, whether it is in the chosen set.
    pub(super) chosen: Vec<bool>,
    /// Which independently evaluated path produced `chosen`.
    pub(super) decision: ComposeDecision,
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
    /// Deepest set the beam may build.
    pub max_fields: usize,
    /// Parallel branches retained at each depth.
    pub beam_width: usize,
    /// Logical cores this was built for, so the UI can explain the numbers it is showing.
    pub cores: usize,
}

impl ComposeBudget {
    /// Restarts one candidate ranking run gets, out of the count the user asked for.
    ///
    /// One thirty-second of it, capped at [`RANKING_RESTARTS_MAX`]: beam search ranks hundreds of
    /// masks, so the former quarter-budget would multiply a 50k request into millions of ranking
    /// restarts. Never zero — a run of no restarts finds nothing and would read as "this scope has
    /// no answer" rather than as a budget too small to say. The outer gate and final refit are not
    /// scaled here.
    ///
    /// Args:
    ///     user_restarts: Restart count used by the later full-budget gate and refit.
    ///
    /// Returns:
    ///     Restart count allowed for one candidate on one fold.
    pub fn ranking_restarts(&self, user_restarts: usize) -> usize {
        (user_restarts / RANKING_RESTART_DIVISOR).clamp(super::RESTARTS_MIN, RANKING_RESTARTS_MAX)
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
/// set by what makes the ANSWER trustworthy — three inner folds and two untouched gates. The
/// beam is bounded by [`BEAM_WIDTH`] rather than an arbitrary set-size ceiling, so every admitted
/// field count remains reachable while several competing interactions survive each layer.
///
/// Args:
///     cores: Logical processors available to the search.
///
/// Returns:
///     Fixed composition limits, or `None` below the heavy-search bar.
fn budget_for(cores: usize) -> Option<ComposeBudget> {
    if cores < super::search::HEAVY_SEARCH_MIN_CORES {
        return None;
    }
    Some(ComposeBudget {
        folds: 5,
        max_fields: super::super::FIELDS.len(),
        beam_width: BEAM_WIDTH,
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
///
/// Args:
///     folds: Walk-forward folds to score.
///     mask: Fields the fold searches may use.
///     p: Seed and rounding settings shared by this composition.
///     requested_restarts: Restart budget for each non-empty mask on each fold.
///     step: Deterministic restart-stream discriminator.
///     handle: Cancellation and progress channel.
///
/// Returns:
///     Profit on each fold's validation stretch, or `None` when cancelled.
fn score_set(
    folds: &[Fold],
    mask: &[bool],
    p: &ComposeParams,
    requested_restarts: usize,
    step: usize,
    handle: &SearchHandle,
) -> Option<Vec<f64>> {
    let seed = super::search::restart_seed(p.seed, step);
    // An empty set assigns no range whatever the random start, so every restart returns the same
    // outcome. Ranking the incumbent at step one — the one score always taken over an empty mask
    // — would otherwise repeat a single answer up to thousands of times per fold.
    let restarts = if mask.iter().any(|m| *m) {
        requested_restarts
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
/// The descent enforces that limit internally too, so this changes no answer — it stops the beam
/// from spending a branch, and one of the user's `max_fields`, on a field that could never carry a
/// range anyway.
///
/// Args:
///     mask: Fields already present in the branch.
///     is_slot: Fields that consume a Delta2/Delta3 slot.
///     locked_slots: Slots already occupied by fixed filters.
///
/// Returns:
///     Whether no further slot field can be admitted.
fn slots_are_full(mask: &[bool], is_slot: &[bool], locked_slots: usize) -> bool {
    let used = mask
        .iter()
        .zip(is_slot)
        .filter(|(chosen, slot)| **chosen && **slot)
        .count();
    locked_slots + used >= 2
}

/// One field-set mask and its scores on a common set of folds.
struct ScoredMask {
    /// Fields admitted by this branch.
    mask: Vec<bool>,
    /// Validation profit on each fold.
    scores: Vec<f64>,
}

/// Deterministic mask order, preferring lower field indices.
///
/// Args:
///     a: Left mask.
///     b: Right mask.
///
/// Returns:
///     Lexicographic ordering of the masks' chosen field indices.
fn mask_order(a: &[bool], b: &[bool]) -> Ordering {
    a.iter()
        .enumerate()
        .filter_map(|(index, chosen)| chosen.then_some(index))
        .cmp(
            b.iter()
                .enumerate()
                .filter_map(|(index, chosen)| chosen.then_some(index)),
        )
}

/// Rank masks by validation mean, with a stable field-index tie break.
///
/// Args:
///     a: Left scored mask.
///     b: Right scored mask.
///
/// Returns:
///     Ordering suitable for best-first sorting.
fn ranked_order(a: &ScoredMask, b: &ScoredMask) -> Ordering {
    mean(&b.scores)
        .total_cmp(&mean(&a.scores))
        .then_with(|| mask_order(&a.mask, &b.mask))
}

/// Build a bounded beam of alternative field sets, including temporarily weak branches.
///
/// Every mask in one depth is scored with the same `step`, so the production scorer derives the
/// same restart stream for comparable candidates. Masks are retained by rank even when they do
/// not yet beat the empty set: two individually weak fields can still form a useful pair.
///
/// Args:
///     candidate: Fields the beam may add.
///     is_slot: Fields that consume one of the two Delta2/Delta3 slots.
///     locked_slots: Slots already consumed by fixed filters.
///     max_fields: Deepest set the beam may build.
///     width: Branches retained after each depth.
///     score: Deterministic scorer for a mask and layer progress.
///
/// Returns:
///     Retained masks from every completed depth, or `None` when scoring was cancelled.
fn beam_candidates<F>(
    candidate: &[bool],
    is_slot: &[bool],
    locked_slots: usize,
    max_fields: usize,
    width: usize,
    mut score: F,
) -> Option<Vec<Vec<bool>>>
where
    F: FnMut(&[bool], usize, usize, usize) -> Option<Vec<f64>>,
{
    if width == 0 || max_fields == 0 {
        return Some(Vec::new());
    }
    let mut frontier = vec![vec![false; candidate.len()]];
    let mut retained = Vec::new();
    for depth in 1..=max_fields {
        let mut seen = HashSet::new();
        let mut expanded = Vec::new();
        for parent in &frontier {
            let slots_full = slots_are_full(parent, is_slot, locked_slots);
            for fi in 0..candidate.len() {
                if !candidate[fi] || parent[fi] || (slots_full && is_slot[fi]) {
                    continue;
                }
                let mut mask = parent.clone();
                mask[fi] = true;
                if seen.insert(mask.clone()) {
                    expanded.push(mask);
                }
            }
        }
        if expanded.is_empty() {
            break;
        }
        let total = expanded.len();
        let mut scored = Vec::with_capacity(total);
        for (done, mask) in expanded.into_iter().enumerate() {
            scored.push(ScoredMask {
                scores: score(&mask, depth, done, total)?,
                mask,
            });
        }
        scored.sort_by(ranked_order);
        scored.truncate(width);
        frontier = scored.into_iter().map(|ranked| ranked.mask).collect();
        retained.extend(frontier.iter().cloned());
    }
    Some(retained)
}

/// Pick the strongest reduced mask that robustly beats the strongest inner control.
///
/// Args:
///     control_scores: Fold profits of all-fields search or no filters, whichever is stronger.
///     candidates: Beam finalists re-scored on one common restart stream.
///     empty: Empty mask returned when no finalist clears the control.
///
/// Returns:
///     Best acceptable reduced mask, or `empty`.
fn inner_winner(
    control_scores: &[f64],
    mut candidates: Vec<ScoredMask>,
    empty: &[bool],
) -> Vec<bool> {
    candidates.retain(|candidate| accepts(control_scores, &candidate.scores));
    candidates.sort_by(|a, b| {
        mean(&b.scores)
            .total_cmp(&mean(&a.scores))
            .then_with(|| {
                a.mask
                    .iter()
                    .filter(|chosen| **chosen)
                    .count()
                    .cmp(&b.mask.iter().filter(|chosen| **chosen).count())
            })
            .then_with(|| mask_order(&a.mask, &b.mask))
    });
    candidates
        .into_iter()
        .next()
        .map_or_else(|| empty.to_vec(), |candidate| candidate.mask)
}

/// One of the three search paths selected by the outer gate.
#[derive(Debug, PartialEq, Eq)]
struct GateChoice {
    /// User-facing meaning of the selected mask.
    decision: ComposeDecision,
    /// Fields admitted into the final full-budget refit.
    mask: Vec<bool>,
}

/// Select between a strict subset, all admitted fields, and no additional filters.
///
/// Only one beam subset reaches this function, so the gate is not reused to rank the frontier.
/// The three paths are ranked symmetrically: first by how many alternatives they beat on a strict
/// majority of gate folds, then by mean gate profit when pairwise comparisons are inconclusive.
/// An exact tie prefers fewer fields. With two reserved folds, a pairwise win must repeat on both
/// rather than ride one lucky validation stretch.
///
/// Args:
///     no_filters: Empty mask and its gate scores.
///     all_fields: All admitted fields and their gate scores.
///     subset: One beam-selected mask and its gate scores.
///
/// Returns:
///     Decision and mask supported by the reserved gate folds.
fn gate_choice(
    no_filters: (&[bool], &[f64]),
    all_fields: (&[bool], &[f64]),
    subset: (&[bool], &[f64]),
) -> GateChoice {
    if all_fields.0 == no_filters.0 {
        return GateChoice {
            decision: ComposeDecision::NoAdditionalFilters,
            mask: no_filters.0.to_vec(),
        };
    }
    let mut choices = vec![
        (
            ComposeDecision::NoAdditionalFilters,
            no_filters.0,
            no_filters.1,
        ),
        (
            ComposeDecision::AllAllowedFields,
            all_fields.0,
            all_fields.1,
        ),
    ];
    if subset.0 != no_filters.0 && subset.0 != all_fields.0 {
        choices.push((ComposeDecision::ReducedSet, subset.0, subset.1));
    }

    let mut wins = vec![0usize; choices.len()];
    for candidate in 0..choices.len() {
        for alternative in 0..choices.len() {
            if candidate != alternative && accepts(choices[alternative].2, choices[candidate].2) {
                wins[candidate] += 1;
            }
        }
    }

    let field_count = |mask: &[bool]| mask.iter().filter(|chosen| **chosen).count();
    let mut best = 0usize;
    for candidate in 1..choices.len() {
        let ordering = wins[candidate]
            .cmp(&wins[best])
            .then_with(|| mean(choices[candidate].2).total_cmp(&mean(choices[best].2)))
            .then_with(|| field_count(choices[best].1).cmp(&field_count(choices[candidate].1)));
        if ordering.is_gt() {
            best = candidate;
        }
    }
    GateChoice {
        decision: choices[best].0,
        mask: choices[best].1.to_vec(),
    }
}

/// Remove fields whose absence costs nothing on the inner selection folds.
///
/// Args:
///     folds: Inner folds already used for candidate ranking.
///     mask: Reduced winner to simplify.
///     p: Composition search settings.
///     step: Last seed-stream step consumed before shrinking.
///     handle: Cancellation and progress channel.
///
/// Returns:
///     Simplified mask and final consumed step, or `None` when cancelled.
fn shrink_mask(
    folds: &[Fold],
    mut mask: Vec<bool>,
    p: &ComposeParams,
    mut step: usize,
    handle: &SearchHandle,
) -> Option<(Vec<bool>, usize)> {
    loop {
        step += 1;
        let mut incumbent = score_set(folds, &mask, p, p.ranking_restarts, step, handle)?;
        let mut removed = false;
        let held = mask.iter().filter(|chosen| **chosen).count();
        let mut tried = 0usize;
        for fi in 0..mask.len() {
            if !mask[fi] {
                continue;
            }
            handle.set_stage(step, tried, held);
            tried += 1;
            mask[fi] = false;
            let without = score_set(folds, &mask, p, p.ranking_restarts, step, handle);
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
            return Some((mask, step));
        }
    }
}

/// Compose the field set with a bounded beam, then choose among three equal search paths.
///
/// Args:
///     folds: Chronological walk-forward folds; the final two are reserved as gates.
///     p: Restart, seed, rounding, and depth limits.
///     handle: Cancellation and progress channel shared with the caller.
///
/// Returns:
///     Chosen mask with fold support, or `None` when stopped before a complete answer.
pub(super) fn compose(
    folds: &[Fold],
    p: &ComposeParams,
    handle: &SearchHandle,
) -> Option<ComposeOutcome> {
    let first = folds.first()?;
    if folds.len() <= GATE_FOLDS {
        return None;
    }
    let (inner_folds, gate_folds) = folds.split_at(folds.len() - GATE_FOLDS);
    if inner_folds.len() < 2 {
        return None;
    }
    let nf = first.search.free_mask().len();
    let locked_slots = first.search.locked_slots();
    let is_slot = first.search.slot_flags();
    // A field is a candidate only where EVERY fold can search it. The folds share one `locked`,
    // so they already agree; intersecting rather than trusting that keeps a future fold-building
    // change from quietly handing the beam a field one fold would ignore.
    let candidate: Vec<bool> = (0..nf)
        .map(|fi| folds.iter().all(|fold| fold.search.free_mask()[fi]))
        .collect();
    let no_filters = vec![false; nf];
    let all_fields = candidate.clone();
    let finalists = beam_candidates(
        &candidate,
        is_slot,
        locked_slots,
        p.max_fields,
        p.beam_width,
        |mask, depth, done, total| {
            handle.set_stage(depth, done, total);
            score_set(inner_folds, mask, p, p.ranking_restarts, depth, handle)
        },
    )?;

    // Re-score every retained finalist on one COMMON stream. Scores used to choose a frontier at
    // different depths came from different streams and are intentionally not compared here.
    let mut step = p.max_fields + 1;
    let no_filters_inner = score_set(
        inner_folds,
        &no_filters,
        p,
        p.ranking_restarts,
        step,
        handle,
    )?;
    let all_fields_inner = score_set(
        inner_folds,
        &all_fields,
        p,
        p.ranking_restarts,
        step,
        handle,
    )?;
    let inner_control = if accepts(&no_filters_inner, &all_fields_inner) {
        &all_fields_inner
    } else {
        &no_filters_inner
    };
    let total = finalists.len();
    let mut ranked = Vec::with_capacity(total);
    for (done, mask) in finalists.into_iter().enumerate() {
        handle.set_stage(step, done, total);
        ranked.push(ScoredMask {
            scores: score_set(inner_folds, &mask, p, p.ranking_restarts, step, handle)?,
            mask,
        });
    }
    let subset = inner_winner(inner_control, ranked, &no_filters);
    let (subset, next_step) = shrink_mask(inner_folds, subset, p, step, handle)?;
    step = next_step + 1;

    // The final folds are an OUTER gate: only the already-selected subset reaches them.
    // Comparing every beam candidate here would merely overfit more validation stretches.
    handle.set_stage(step, 0, 3);
    let no_filters_gate = score_set(gate_folds, &no_filters, p, p.gate_restarts, step, handle)?;
    handle.set_stage(step, 1, 3);
    let all_fields_gate = score_set(gate_folds, &all_fields, p, p.gate_restarts, step, handle)?;
    handle.set_stage(step, 2, 3);
    let subset_gate = if subset == no_filters {
        no_filters_gate.clone()
    } else {
        score_set(gate_folds, &subset, p, p.gate_restarts, step, handle)?
    };
    let choice = gate_choice(
        (&no_filters, &no_filters_gate),
        (&all_fields, &all_fields_gate),
        (&subset, &subset_gate),
    );

    // Measured under the FINAL set rather than accumulated during the search, so the number
    // beside a field describes the set the user is being shown and cannot disagree with it.
    step += 1;
    let mut support = vec![0u8; nf];
    let seed = super::search::restart_seed(p.seed, step);
    for (i, fold) in folds.iter().enumerate() {
        handle.set_stage(step, i, folds.len());
        let outcome = fold
            .search
            .run_masked(&choice.mask, p.ranking_restarts, seed, handle)?;
        for (fi, _, _) in fold.search.applied_ranges(&outcome.sel, p.round) {
            support[fi] = support[fi].saturating_add(1);
        }
    }
    Some(ComposeOutcome {
        chosen: choice.mask,
        decision: choice.decision,
        support,
        folds: folds.len().min(u8::MAX as usize) as u8,
    })
}
