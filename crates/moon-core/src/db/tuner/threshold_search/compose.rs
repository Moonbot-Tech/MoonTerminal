//! Select which fields to filter, not only where their thresholds belong.
//!
//! Composition stays inside the fitting region. Anchored walk-forward folds fit thresholds on a
//! growing prefix and score them on the unseen stretch that follows, leaving the UI's final
//! holdout untouched. Each fold divides one fixed restart budget among up to five independent
//! seed groups. Seeds vote inside their fold before folds vote, so one lucky time stretch cannot
//! dominate the temporal evidence.
//!
//! A bounded adaptive beam keeps eight branches when ranks eight and nine are clearly separated
//! and up to sixteen when that boundary is ambiguous. Weak intermediate branches remain eligible
//! because two individually weak fields may form a strong interaction. Material risk-scaled
//! profit gaps are decisive; close candidates also compare profit factor, lower drawdown, and
//! retained trade count. Backward elimination gives cost-free fields back, making the smaller set
//! the complexity tie-break.
//!
//! Two final folds see only the selected subset and choose symmetrically among three normal paths:
//! that reduced set, every admitted field, or no additional field filters.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::ops::Range;

use super::handle::SearchHandle;
use super::search::Search;
use super::ComposeDecision;
use crate::db::metrics::improvement_margin;

#[cfg(test)]
mod tests;

/// Branches retained at an unambiguous composition depth.
///
/// Ambiguous boundaries may expand to [`BEAM_WIDTH_MAX`], but a decisive layer contracts here.
const BEAM_WIDTH_MIN: usize = 8;

/// Hard ceiling for an ambiguous composition depth.
const BEAM_WIDTH_MAX: usize = 16;

/// Independent restart groups used for each fold score when the budget can supply them.
///
/// The groups divide one fixed budget. An odd maximum prevents a tie when all groups run; smaller
/// budgets vote over only the groups they can supply.
const SEED_GROUPS: usize = 5;

/// Profit differences inside this share of the compared risk scale let secondary metrics decide.
const PROFIT_BAND_FRAC: f64 = 0.05;

/// Profit-factor difference below which two outcomes are treated as tied.
const PF_MARGIN: f64 = 0.02;

/// Largest restart count spent fitting one candidate on one ranking fold.
///
/// Candidate selection is a coarse ordering pass. The final refit still receives the user's full
/// restart count, while this cap gives the beam a hard work bound independent of a large setting.
const RANKING_RESTARTS_MAX: usize = 512;

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
    /// Branches retained when the beam boundary is decisive.
    pub(super) beam_width_min: usize,
    /// Hard branch ceiling when the beam boundary is ambiguous.
    pub(super) beam_width_max: usize,
    /// Independent restart groups requested for every fold score.
    pub(super) seed_groups: usize,
}

/// The composed set.
pub(super) struct ComposeOutcome {
    /// Per field, whether it is in the chosen set.
    pub(super) chosen: Vec<bool>,
    /// Which independently evaluated path produced `chosen`.
    pub(super) decision: ComposeDecision,
    /// Per field, how many folds had a strict seed majority apply it under the final set.
    pub(super) support: Vec<u8>,
    /// Folds the figures rest on, so a caller can render "2/3" without assuming how many there
    /// were.
    pub(super) folds: u8,
}

/// How wide a composition may go on this machine.
///
/// Machine-fixed only. The per-run ranking share is [`Self::ranking_restarts`] rather than a
/// field, so a caller that only wants to know whether this machine composes and caption the fixed
/// policy does not have to invent a restart count.
pub struct ComposeBudget {
    /// Folds the fitting region is cut into.
    pub folds: usize,
    /// Deepest set the beam may build.
    pub max_fields: usize,
    /// Branches retained when the beam boundary is decisive.
    pub beam_width_min: usize,
    /// Hard branch ceiling when the beam boundary is ambiguous.
    pub beam_width_max: usize,
    /// Maximum independent seed groups used inside each fold.
    pub seed_groups: usize,
    /// Logical cores this was built for, so the UI can explain the numbers it is showing.
    pub cores: usize,
}

impl ComposeBudget {
    /// Restarts one candidate ranking run gets, out of the count the user asked for.
    ///
    /// One thirty-second of it, capped at [`RANKING_RESTARTS_MAX`]: beam search ranks hundreds of
    /// masks, so the former quarter-budget would multiply a large request into millions of
    /// ranking restarts. Never zero — a run of no restarts finds nothing and would read as "this
    /// scope has no answer" rather than as a budget too small to say. The outer gate and final
    /// refit are not scaled here.
    ///
    /// The floor stays at one restart rather than one per seed group so this stage never spends
    /// work the user did not request. Small settings therefore vote with fewer groups.
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
/// One tier above the bar: a machine with twice the bar's cores composes no wider. Three inner
/// folds rank candidates and two untouched folds gate the result. The beam uses
/// [`BEAM_WIDTH_MIN`] at a robust boundary and at most [`BEAM_WIDTH_MAX`] at an ambiguous one, so
/// every admitted field count remains reachable while the work stays bounded.
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
        beam_width_min: BEAM_WIDTH_MIN,
        beam_width_max: BEAM_WIDTH_MAX,
        seed_groups: SEED_GROUPS,
        cores,
    })
}

/// Metrics produced by one seed group on one fold's unseen stretch.
#[derive(Clone, Copy, Debug, Default)]
struct Quality {
    /// Held-out profit.
    profit: f64,
    /// Held-out profit factor.
    profit_factor: f64,
    /// Held-out maximum drawdown.
    max_dd: f64,
    /// Held-out trades retained by the ranges.
    trades: f64,
}

impl From<crate::db::metrics::Tally> for Quality {
    /// Convert one chronological tally into the comparison metrics composition uses.
    ///
    /// Args:
    ///     tally: Held-out rows scored under one fitted seed outcome.
    ///
    /// Returns:
    ///     Finite metrics used by ranking and robust acceptance.
    fn from(tally: crate::db::metrics::Tally) -> Self {
        Self {
            profit: tally.profit,
            profit_factor: tally.profit_factor(),
            max_dd: tally.max_dd,
            trades: tally.n as f64,
        }
    }
}

/// Seed-group scores belonging to one chronological walk-forward fold.
#[derive(Clone, Debug)]
struct FoldScores {
    /// Independent restart-group outcomes. Their vote is resolved before folds vote.
    seeds: Vec<Quality>,
}

/// Fold-major score hierarchy; folds remain the unit of temporal evidence.
type ScoreSet = Vec<FoldScores>;

/// Arithmetic mean of a quality collection.
///
/// Args:
///     values: Seed or fold summaries to aggregate.
///
/// Returns:
///     Mean metrics, or zeroes for an empty collection.
fn mean_quality(values: impl IntoIterator<Item = Quality>) -> Quality {
    let mut out = Quality::default();
    let mut count = 0usize;
    for value in values {
        out.profit += value.profit;
        out.profit_factor += value.profit_factor;
        out.max_dd += value.max_dd;
        out.trades += value.trades;
        count += 1;
    }
    if count > 0 {
        let divisor = count as f64;
        out.profit /= divisor;
        out.profit_factor /= divisor;
        out.max_dd /= divisor;
        out.trades /= divisor;
    }
    out
}

/// Mean metrics across every seed inside every fold.
///
/// Args:
///     scores: Fold-major seed outcomes.
///
/// Returns:
///     Aggregate quality used as the non-temporal half of an acceptance decision.
fn summary(scores: &ScoreSet) -> Quality {
    mean_quality(scores.iter().flat_map(|fold| fold.seeds.iter().copied()))
}

/// Robust per-metric preferences for one close-profit pair.
#[derive(Clone, Copy, Debug)]
struct SecondaryPreference {
    /// Profit-factor preference; higher is better.
    profit_factor: Ordering,
    /// Drawdown preference; lower is better.
    drawdown: Ordering,
    /// Retained-trade preference; higher is better.
    trades: Ordering,
}

impl SecondaryPreference {
    /// Combine the three metrics into an equal-weight pairwise vote.
    ///
    /// Returns:
    ///     Positive when the left outcome wins more metrics, negative when the right does.
    fn balance(self) -> i8 {
        [self.profit_factor, self.drawdown, self.trades]
            .into_iter()
            .map(|ordering| match ordering {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
            .sum()
    }
}

/// Compare PF, drawdown, and retained trades under their shared robust margins.
///
/// Args:
///     a: Left outcome.
///     b: Right outcome.
///
/// Returns:
///     Independent preferences for the three secondary metrics.
fn secondary_preference(a: Quality, b: Quality) -> SecondaryPreference {
    let profit_factor = if a.profit_factor > b.profit_factor + PF_MARGIN {
        Ordering::Greater
    } else if b.profit_factor > a.profit_factor + PF_MARGIN {
        Ordering::Less
    } else {
        Ordering::Equal
    };
    let dd_margin = a.max_dd.max(b.max_dd).max(1.0) * PROFIT_BAND_FRAC;
    let drawdown = if a.max_dd + dd_margin < b.max_dd {
        Ordering::Greater
    } else if b.max_dd + dd_margin < a.max_dd {
        Ordering::Less
    } else {
        Ordering::Equal
    };
    let trade_margin = a.trades.max(b.trades).max(1.0) * PROFIT_BAND_FRAC;
    let trades = if a.trades > b.trades + trade_margin {
        Ordering::Greater
    } else if b.trades > a.trades + trade_margin {
        Ordering::Less
    } else {
        Ordering::Equal
    };
    SecondaryPreference {
        profit_factor,
        drawdown,
        trades,
    }
}

/// Compare profit only when its gap clears the pair's own risk-scaled band.
///
/// Args:
///     a: Left outcome.
///     b: Right outcome.
///
/// Returns:
///     Profit ordering outside the material band, otherwise `Equal`.
fn material_profit_order(a: Quality, b: Quality) -> Ordering {
    let scale = a
        .profit
        .abs()
        .max(b.profit.abs())
        .max(a.max_dd)
        .max(b.max_dd)
        .max(1.0);
    let delta = a.profit - b.profit;
    let band = scale * PROFIT_BAND_FRAC;
    if delta > band {
        Ordering::Greater
    } else if delta < -band {
        Ordering::Less
    } else {
        Ordering::Equal
    }
}

/// Compare two close-profit outcomes using PF, drawdown, and trade count.
///
/// Profit remains decisive outside a five-percent risk-scaled band. Inside the band the three
/// secondary metrics vote equally; only when they tie does profit above floating-point noise
/// break the tie. This is a robust PAIRWISE predicate, not a sorting comparator.
///
/// Args:
///     a: Left outcome.
///     b: Right outcome.
///
/// Returns:
///     Which outcome is better under the balanced comparison.
fn quality_order(a: Quality, b: Quality) -> Ordering {
    let scale = a
        .profit
        .abs()
        .max(b.profit.abs())
        .max(a.max_dd)
        .max(b.max_dd)
        .max(1.0);
    let profit_delta = a.profit - b.profit;
    let material_profit = material_profit_order(a, b);
    if !material_profit.is_eq() {
        return material_profit;
    }

    let votes = secondary_preference(a, b).balance();
    match votes.cmp(&0) {
        Ordering::Equal => {
            let noise = improvement_margin(scale);
            if profit_delta > noise {
                Ordering::Greater
            } else if profit_delta < -noise {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        }
        ordering => ordering,
    }
}

/// Whether the candidate wins a strict majority of seed groups inside one fold.
///
/// Args:
///     incumbent: Seed outcomes for the current path.
///     candidate: Matching seed outcomes for the proposed path.
///
/// Returns:
///     `true` only when most independent groups prefer the candidate.
fn seed_consensus(incumbent: &FoldScores, candidate: &FoldScores) -> bool {
    if candidate.seeds.len() != incumbent.seeds.len() || incumbent.seeds.is_empty() {
        return false;
    }
    let wins = incumbent
        .seeds
        .iter()
        .zip(&candidate.seeds)
        .filter(|(before, after)| quality_order(**after, **before).is_gt())
        .count();
    2 * wins > incumbent.seeds.len()
}

/// Resolve which fields a strict majority of one fold's seed outcomes applied.
///
/// Counted over the groups that actually ran, never over [`SEED_GROUPS`] — a fold the restart
/// budget could not supply in full votes with the groups it got.
///
/// Args:
///     field_count: Number of tuner fields.
///     applied_by_seed: Applied field indices from every active seed outcome.
///
/// Returns:
///     One boolean per field; empty evidence supports nothing.
fn seed_majority_support(field_count: usize, applied_by_seed: &[Vec<usize>]) -> Vec<bool> {
    let mut counts = vec![0usize; field_count];
    for applied in applied_by_seed {
        for field in applied {
            counts[*field] += 1;
        }
    }
    counts
        .into_iter()
        .map(|count| !applied_by_seed.is_empty() && 2 * count > applied_by_seed.len())
        .collect()
}

/// Whether a candidate is robustly better without flattening seed votes across time.
///
/// Seeds vote inside each fold first; folds then vote as the independent time periods. An
/// aggregate quality win is required as well, so a bare fold majority cannot hide one collapse.
///
/// Args:
///     incumbent: Fold-major scores of the current path.
///     candidate: Matching fold-major scores of the proposed path.
///
/// Returns:
///     Whether the candidate is supported by both aggregate quality and most folds.
fn accepts(incumbent: &ScoreSet, candidate: &ScoreSet) -> bool {
    if candidate.len() != incumbent.len() || incumbent.is_empty() {
        return false;
    }
    if !quality_order(summary(candidate), summary(incumbent)).is_gt() {
        return false;
    }
    let wins = incumbent
        .iter()
        .zip(candidate)
        .filter(|(before, after)| seed_consensus(before, after))
        .count();
    2 * wins > incumbent.len()
}

/// Whether removing a field costs no robust quality.
///
/// Args:
///     incumbent: Scores with the field.
///     without: Scores after removing it.
///
/// Returns:
///     `true` when the smaller set is not worse in aggregate and the incumbent lacks a majority
///     of fold-level seed consensuses over it.
fn drops(incumbent: &ScoreSet, without: &ScoreSet) -> bool {
    if without.len() != incumbent.len() || incumbent.is_empty() {
        return false;
    }
    let incumbent_wins = without
        .iter()
        .zip(incumbent)
        .filter(|(smaller, larger)| seed_consensus(smaller, larger))
        .count();
    !quality_order(summary(without), summary(incumbent)).is_lt()
        && 2 * incumbent_wins <= incumbent.len()
}

/// Score one candidate set: per fold, independent seed-group tallies on its unseen stretch.
///
/// Returns `None` when the run was stopped — a partial score vector is not comparable with a
/// complete one, and picking a winner from mixed evidence is worse than not answering.
///
/// Args:
///     folds: Walk-forward folds to score.
///     mask: Fields the fold searches may use.
///     p: Seed-group, rounding, and composition-budget settings.
///     requested_restarts: Restart budget for each non-empty mask on each fold.
///     step: Deterministic restart-stream discriminator.
///     handle: Cancellation and progress channel.
///
/// Returns:
///     Fold-major quality scores, or `None` when cancelled.
fn score_set(
    folds: &[Fold],
    mask: &[bool],
    p: &ComposeParams,
    requested_restarts: usize,
    step: usize,
    handle: &SearchHandle,
) -> Option<ScoreSet> {
    let seed = super::search::restart_seed(p.seed, step);
    folds
        .iter()
        .map(|fold| {
            let requested_groups = p.seed_groups.max(1);
            let active_groups = requested_groups.min(requested_restarts).max(1);
            // An empty mask is seed-invariant. Score it once and replicate the evidence shape so
            // comparisons remain aligned without spending the requested restart budget on copies.
            if !mask.iter().any(|chosen| *chosen) {
                let outcome = fold.search.run_masked(mask, 1, seed, handle)?;
                let applied = fold.search.applied_ranges(&outcome.sel, p.round);
                let score = Quality::from(fold.search.tally(&applied, fold.validate.clone()));
                return Some(FoldScores {
                    seeds: vec![score; active_groups],
                });
            }
            let seeds = fold
                .search
                .run_masked_seed_groups(mask, requested_restarts, seed, requested_groups, handle)?
                .into_iter()
                .map(|outcome| {
                    let applied = fold.search.applied_ranges(&outcome.sel, p.round);
                    Quality::from(fold.search.tally(&applied, fold.validate.clone()))
                })
                .collect();
            Some(FoldScores { seeds })
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

/// Deterministic total-order key used only to rank a common candidate pool.
#[derive(Clone, Copy, Debug, Default)]
struct RankingKey {
    /// Unique best-first position assigned by the material-profit dependency order.
    rank: usize,
}

/// One field-set mask and its scores on a common set of folds.
struct ScoredMask {
    /// Fields admitted by this branch.
    mask: Vec<bool>,
    /// Fold-major validation evidence for the branch.
    scores: ScoreSet,
    /// Total-order key assigned across the branch's current comparison pool.
    key: RankingKey,
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

/// Build transitive ranking keys over one common candidate pool.
///
/// Material-profit comparisons form a directed acyclic graph because every edge points from
/// higher exact profit to lower exact profit. A deterministic topological selection therefore
/// guarantees that secondary metrics can never move a candidate ahead of another candidate it
/// materially trails. Currently eligible candidates meet head-to-head under the same balanced
/// close-profit rule used by acceptance; this avoids a pool-wide loss total that could reverse
/// an unchanged pair merely because another strong branch was present. Exact metrics and input
/// order resolve tied or cyclic pairwise preferences deterministically.
///
/// Args:
///     scores: Fold-major score sets belonging to one common restart stream.
///
/// Returns:
///     One ranking key per input score set, in input order.
fn ranking_keys(scores: &[&ScoreSet]) -> Vec<RankingKey> {
    let qualities: Vec<Quality> = scores.iter().map(|scores| summary(scores)).collect();
    let count = qualities.len();
    let mut outgoing = vec![Vec::new(); count];
    let mut indegree = vec![0usize; count];
    for candidate in 0..count {
        for alternative in 0..count {
            if candidate == alternative {
                continue;
            }
            let quality = qualities[candidate];
            let other = qualities[alternative];
            match material_profit_order(quality, other) {
                Ordering::Greater => {
                    outgoing[candidate].push(alternative);
                    indegree[alternative] += 1;
                }
                Ordering::Less | Ordering::Equal => {}
            }
        }
    }

    let preferred = |candidate: usize, incumbent: usize| {
        quality_order(qualities[candidate], qualities[incumbent])
            .then_with(|| {
                qualities[candidate]
                    .profit
                    .total_cmp(&qualities[incumbent].profit)
            })
            .then_with(|| {
                qualities[candidate]
                    .profit_factor
                    .total_cmp(&qualities[incumbent].profit_factor)
            })
            .then_with(|| {
                qualities[incumbent]
                    .max_dd
                    .total_cmp(&qualities[candidate].max_dd)
            })
            .then_with(|| {
                qualities[candidate]
                    .trades
                    .total_cmp(&qualities[incumbent].trades)
            })
            .then_with(|| incumbent.cmp(&candidate))
            .is_gt()
    };

    let mut removed = vec![false; count];
    let mut keys = vec![RankingKey::default(); count];
    for rank in 0..count {
        let best = (0..count)
            .filter(|candidate| !removed[*candidate] && indegree[*candidate] == 0)
            .reduce(|incumbent, candidate| {
                if preferred(candidate, incumbent) {
                    candidate
                } else {
                    incumbent
                }
            })
            .expect("material-profit edges always form an acyclic graph");
        removed[best] = true;
        keys[best].rank = rank;
        for dependent in &outgoing[best] {
            indegree[*dependent] -= 1;
        }
    }
    keys
}

/// Compare ranking keys with `Greater` meaning the left candidate is preferred.
///
/// Args:
///     a: Left candidate key.
///     b: Right candidate key.
///
/// Returns:
///     Deterministic total ordering from balanced rank to exact metrics.
fn ranking_key_order(a: &RankingKey, b: &RankingKey) -> Ordering {
    b.rank.cmp(&a.rank)
}

/// Assign ranking keys across the exact pool that will be sorted.
///
/// Args:
///     candidates: Common-stream candidates to key in place.
fn assign_ranking(candidates: &mut [ScoredMask]) {
    let keys = {
        let scores: Vec<&ScoreSet> = candidates
            .iter()
            .map(|candidate| &candidate.scores)
            .collect();
        ranking_keys(&scores)
    };
    for (candidate, key) in candidates.iter_mut().zip(keys) {
        candidate.key = key;
    }
}

/// Rank masks by balanced quality, with stable complexity and field-index tie breaks.
///
/// Args:
///     a: Left scored mask.
///     b: Right scored mask.
///
/// Returns:
///     Ordering suitable for best-first sorting.
fn ranked_order(a: &ScoredMask, b: &ScoredMask) -> Ordering {
    ranking_key_order(&b.key, &a.key)
        .then_with(|| {
            a.mask
                .iter()
                .filter(|chosen| **chosen)
                .count()
                .cmp(&b.mask.iter().filter(|chosen| **chosen).count())
        })
        .then_with(|| mask_order(&a.mask, &b.mask))
}

/// Choose the retained width at one already-ranked beam depth.
///
/// Eight branches are enough when candidate eight robustly beats candidate nine. An ambiguous
/// boundary keeps up to sixteen so interacting fields get another depth to separate. The choice
/// is recomputed at every depth, allowing a widened beam to contract again.
///
/// Args:
///     scored: Best-first candidates on one common stream.
///     min_width: Decisive-layer width.
///     max_width: Ambiguous-layer hard ceiling.
///
/// Returns:
///     Number of candidates to retain from this depth.
fn adaptive_width(scored: &[ScoredMask], min_width: usize, max_width: usize) -> usize {
    if scored.len() <= min_width {
        return scored.len();
    }
    if accepts(&scored[min_width].scores, &scored[min_width - 1].scores) {
        min_width
    } else {
        scored.len().min(max_width.max(min_width))
    }
}

/// Build an adaptive bounded beam of alternative field sets, including weak branches.
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
///     min_width: Branches retained when the depth boundary is decisive.
///     max_width: Hard branch ceiling for an ambiguous depth.
///     score: Deterministic scorer for a mask and layer progress.
///
/// Returns:
///     Retained masks from every completed depth, or `None` when scoring was cancelled.
fn beam_candidates<F>(
    candidate: &[bool],
    is_slot: &[bool],
    locked_slots: usize,
    max_fields: usize,
    min_width: usize,
    max_width: usize,
    mut score: F,
) -> Option<Vec<Vec<bool>>>
where
    F: FnMut(&[bool], usize, usize, usize) -> Option<ScoreSet>,
{
    if min_width == 0 || max_fields == 0 {
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
                key: RankingKey::default(),
            });
        }
        assign_ranking(&mut scored);
        scored.sort_by(ranked_order);
        let retained_width = adaptive_width(&scored, min_width, max_width);
        scored.truncate(retained_width);
        frontier = scored.into_iter().map(|ranked| ranked.mask).collect();
        retained.extend(frontier.iter().cloned());
    }
    Some(retained)
}

/// Pick the strongest reduced mask that robustly beats the strongest inner control.
///
/// Args:
///     control_scores: Fold-major quality evidence of the stronger complete control.
///     candidates: Beam finalists re-scored on one common restart stream.
///     empty: Empty mask returned when no finalist clears the control.
///
/// Returns:
///     Best acceptable reduced mask, or `empty`.
fn inner_winner(
    control_scores: &ScoreSet,
    mut candidates: Vec<ScoredMask>,
    empty: &[bool],
) -> Vec<bool> {
    candidates.retain(|candidate| accepts(control_scores, &candidate.scores));
    assign_ranking(&mut candidates);
    candidates.sort_by(ranked_order);
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
/// majority of gate folds, then by the deterministic balanced ranking when pairwise comparisons
/// are inconclusive. An exact tie prefers fewer fields. With two reserved folds, a pairwise win
/// must repeat on both rather than ride one lucky validation stretch.
///
/// Args:
///     no_filters: Empty mask and its gate scores.
///     all_fields: All admitted fields and their gate scores.
///     subset: One beam-selected mask and its gate scores.
///
/// Returns:
///     Decision and mask supported by the reserved gate folds.
fn gate_choice(
    no_filters: (&[bool], &ScoreSet),
    all_fields: (&[bool], &ScoreSet),
    subset: (&[bool], &ScoreSet),
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

    let keys = ranking_keys(&choices.iter().map(|choice| choice.2).collect::<Vec<_>>());
    let field_count = |mask: &[bool]| mask.iter().filter(|chosen| **chosen).count();
    let mut best = 0usize;
    for candidate in 1..choices.len() {
        let ordering = wins[candidate]
            .cmp(&wins[best])
            .then_with(|| ranking_key_order(&keys[candidate], &keys[best]))
            .then_with(|| field_count(choices[best].1).cmp(&field_count(choices[candidate].1)))
            .then_with(|| mask_order(choices[best].1, choices[candidate].1));
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

/// Compose the field set with an adaptive bounded beam, then choose among three equal paths.
///
/// Args:
///     folds: Chronological walk-forward folds; the final two are reserved as gates.
///     p: Restart, seed-group, rounding, depth, and adaptive-beam limits.
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
        p.beam_width_min,
        p.beam_width_max,
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
    let control_keys = ranking_keys(&[&no_filters_inner, &all_fields_inner]);
    let inner_control = if accepts(&no_filters_inner, &all_fields_inner)
        || (!accepts(&all_fields_inner, &no_filters_inner)
            && ranking_key_order(&control_keys[1], &control_keys[0]).is_gt())
    {
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
            key: RankingKey::default(),
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
        if !choice.mask.iter().any(|chosen| *chosen) {
            continue;
        }
        let outcomes = fold.search.run_masked_seed_groups(
            &choice.mask,
            p.ranking_restarts,
            seed,
            p.seed_groups.max(1),
            handle,
        )?;
        let applied_by_seed: Vec<Vec<usize>> = outcomes
            .iter()
            .map(|outcome| {
                fold.search
                    .applied_ranges(&outcome.sel, p.round)
                    .into_iter()
                    .map(|(fi, _, _)| fi)
                    .collect()
            })
            .collect();
        for (fi, backed) in seed_majority_support(nf, &applied_by_seed)
            .into_iter()
            .enumerate()
        {
            if backed {
                support[fi] = support[fi].saturating_add(1);
            }
        }
    }
    Some(ComposeOutcome {
        chosen: choice.mask,
        decision: choice.decision,
        support,
        folds: folds.len().min(u8::MAX as usize) as u8,
    })
}
