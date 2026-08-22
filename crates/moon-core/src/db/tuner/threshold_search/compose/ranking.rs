//! Candidate ranking and adaptive beam traversal for threshold composition.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::atomic::{self, AtomicUsize};

use rayon::prelude::*;

use super::{
    accepts, aggregate_order, lift_is_measurable, lift_order, mean_quality, slots_are_full,
    summary, Quality, ScoreSet,
};

/// Deterministic total-order key used only to rank a common candidate pool.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RankingKey {
    /// Unique best-first position assigned by the material-lift dependency order.
    pub(super) rank: usize,
}

/// One field-set mask and its scores on a common set of folds.
pub(super) struct ScoredMask {
    /// Fields admitted by this branch.
    pub(super) mask: Vec<bool>,
    /// Fold-major validation evidence for the branch.
    pub(super) scores: ScoreSet,
    /// Total-order key assigned across the branch's current comparison pool.
    pub(super) key: RankingKey,
}

/// Deterministic mask order, preferring lower field indices.
///
/// Args:
///     a: Left mask.
///     b: Right mask.
///
/// Returns:
///     Lexicographic ordering of the masks' chosen field indices.
pub(super) fn mask_order(a: &[bool], b: &[bool]) -> Ordering {
    a.iter()
        .enumerate()
        .filter_map(|(index, chosen)| chosen.then_some(index))
        .cmp(
            b.iter()
                .enumerate()
                .filter_map(|(index, chosen)| chosen.then_some(index)),
        )
}

/// Build transitive ranking keys over one common candidate pool, from qualities already reduced.
///
/// Material-LIFT comparisons form a directed acyclic graph because every edge points from higher
/// exact lift to lower exact lift: an edge needs `a.lift - b.lift` to clear a band that is at
/// least `PROFIT_BAND_FRAC`, hence strictly positive, so `a.lift > b.lift` exactly, and `>` on a
/// fixed multiset of reals is irreflexive and transitive. That argument depends on NOTHING about
/// how the qualities were reduced, which is why this function takes them directly: a weakest-fold
/// quality ranks under exactly the same guarantee as a seed/fold mean.
///
/// [`super::lift_order`] itself carries no retention floor, because its aggregate callers must not
/// have one — see [`super::aggregate_order`]. When the qualities handed in are SINGLE MEASUREMENTS
/// rather than means, `single_measurements` restores exactly the floor [`super::quality_order`]
/// applies: an outcome
/// that retained too little draws no outgoing edge, so it can never eliminate a measured rival, and
/// it loses the first tie-break to any measured quality, so it can never be picked as the best.
/// Removing edges only shrinks the graph, so acyclicity survives unchanged.
///
/// A deterministic topological selection therefore guarantees that secondary metrics can never
/// move a candidate ahead of another candidate it materially trails. Currently eligible candidates
/// meet head-to-head under the same balanced close-lift rule used by acceptance; this avoids a
/// pool-wide loss total that could reverse an unchanged pair merely because another strong branch
/// was present. Exact metrics and input order resolve tied or cyclic pairwise preferences
/// deterministically.
///
/// What it does NOT give is a maximum: inside one rank step the eligible set has no lift edges
/// among its members by construction, so `preferred` below collapses to the intransitive secondary
/// vote and rank 0 is a deterministic function of the input order rather than a maximal element.
/// When no maximal element exists no rule can return one; what rank 0 guarantees is that the
/// chosen candidate is never one another candidate MATERIALLY BEATS ON LIFT.
///
/// Args:
///     qualities: One aggregate per candidate, all reduced the same way from one restart stream.
///     single_measurements: Whether each quality is ONE measurement rather than a mean of many,
///         which is the only case the retention floor may be applied to.
///
/// Returns:
///     One ranking key per input quality, in input order.
fn ranking_keys_from_qualities(
    qualities: &[Quality],
    single_measurements: bool,
) -> Vec<RankingKey> {
    let count = qualities.len();
    let measured = |q: Quality| !single_measurements || lift_is_measurable(q);
    let mut outgoing = vec![Vec::new(); count];
    let mut indegree = vec![0usize; count];
    for candidate in 0..count {
        // An unmeasurable single outcome has no evidence to beat anyone with, exactly as it has
        // none to vote with in `quality_order`. It keeps its INCOMING edges: being outranked by a
        // measured rival is not a claim about it, and it must still sink below one.
        if !measured(qualities[candidate]) {
            continue;
        }
        for alternative in 0..count {
            if candidate == alternative {
                continue;
            }
            let quality = qualities[candidate];
            let other = qualities[alternative];
            match lift_order(quality, other) {
                Ordering::Greater => {
                    outgoing[candidate].push(alternative);
                    indegree[alternative] += 1;
                }
                Ordering::Less | Ordering::Equal => {}
            }
        }
    }

    let preferred = |candidate: usize, incumbent: usize| {
        // A measured outcome outranks an unmeasured one before any metric is consulted: below the
        // floor the metrics are four lucky trades wearing a profit factor. This key is inert
        // unless `single_measurements` is set, and inert again once every quality clears the floor.
        measured(qualities[candidate])
            .cmp(&measured(qualities[incumbent]))
            .then_with(|| aggregate_order(qualities[candidate], qualities[incumbent]))
            // Exact lift breaks a balanced tie. Above this line the floor has already had its say,
            // and for AGGREGATES it never applies at all - see `aggregate_order`.
            .then_with(|| {
                qualities[candidate]
                    .lift
                    .total_cmp(&qualities[incumbent].lift)
            })
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
            .expect("material-lift edges always form an acyclic graph");
        removed[best] = true;
        keys[best].rank = rank;
        for dependent in &outgoing[best] {
            indegree[*dependent] -= 1;
        }
    }
    keys
}

/// The same ranking, with the quality vector taken from each candidate's seed/fold mean.
///
/// See [`ranking_keys_from_qualities`] for the acyclicity argument and for what rank 0 does and
/// does not promise. The retention floor is switched OFF here and must stay off: these qualities
/// are means over every fold and every seed, and holding a mean of a dozen measurements to a single
/// measurement's floor is the mistake [`aggregate_order`] exists to avoid.
///
/// Args:
///     scores: Fold-major score sets belonging to one common restart stream.
///
/// Returns:
///     One ranking key per input score set, in input order.
pub(super) fn ranking_keys(scores: &[&ScoreSet]) -> Vec<RankingKey> {
    let qualities: Vec<Quality> = scores.iter().map(|scores| summary(scores)).collect();
    ranking_keys_from_qualities(&qualities, false)
}

/// Compare ranking keys with `Greater` meaning the left candidate is preferred.
///
/// Args:
///     a: Left candidate key.
///     b: Right candidate key.
///
/// Returns:
///     Deterministic total ordering from balanced rank to exact metrics.
pub(super) fn ranking_key_order(a: &RankingKey, b: &RankingKey) -> Ordering {
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
pub(super) fn adaptive_width(scored: &[ScoredMask], min_width: usize, max_width: usize) -> usize {
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
/// One depth's masks are scored CONCURRENTLY. They are independent by construction — a mask's
/// score depends on the folds, the mask and the step, never on another mask — and the results are
/// collected in mask order rather than completion order, so the ranking that follows is the same
/// one a sequential scoring would have produced. The width is where the machine's cores belong:
/// scoring one mask fans out over its restarts and then joins, so a depth of hundreds of masks
/// used to pay hundreds of joins, each one idling every worker that finished early while the
/// slowest restart of that mask converged.
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
pub(super) fn beam_candidates<F>(
    candidate: &[bool],
    is_slot: &[bool],
    locked_slots: usize,
    max_fields: usize,
    min_width: usize,
    max_width: usize,
    score: F,
) -> Option<Vec<Vec<bool>>>
where
    F: Fn(&[bool], usize, usize, usize) -> Option<ScoreSet> + Sync,
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
        // Progress counts masks STARTED, as the sequential index did, but drawn from a shared
        // counter. Workers can take consecutive numbers and publish them in the opposite order, so
        // the scorer publishes through `SearchHandle::advance_stage`, which drops a number lower
        // than the one already standing for this depth — a caption counting DOWN reads as work
        // being redone. It reports work, never a decision, so it is no part of the answer either
        // way.
        let started = AtomicUsize::new(0);
        let mut scored: Vec<ScoredMask> = super::super::search::install(|| {
            expanded
                .into_par_iter()
                .map(|mask| {
                    let done = started.fetch_add(1, atomic::Ordering::Relaxed);
                    score(&mask, depth, done, total).map(|scores| ScoredMask {
                        scores,
                        mask,
                        key: RankingKey::default(),
                    })
                })
                .collect::<Option<Vec<_>>>()
        })?;
        assign_ranking(&mut scored);
        scored.sort_by(ranked_order);
        let retained_width = adaptive_width(&scored, min_width, max_width);
        scored.truncate(retained_width);
        frontier = scored.into_iter().map(|ranked| ranked.mask).collect();
        retained.extend(frontier.iter().cloned());
    }
    Some(retained)
}

/// Pick the reduced mask the inner folds lean to, preferring one that robustly beats the control.
///
/// It used to answer the EMPTY mask when no finalist cleared the inner control, and that single
/// fallback decided most runs: the empty mask then reached [`super::gate_choice`] as the "subset", which
/// dropped `ReducedSet` from the pool and scored the empty mask against ITSELF twice. So a
/// selection stage was quietly deciding the question the reserved gate folds exist to answer.
///
/// Handing the best-ranked finalist over instead is not a weakening of the walk-forward test — it
/// is what makes the test happen. The two gate folds are reserved and untouched, and the subset
/// still has to beat both the empty mask and the all-fields control ON THEM.
///
/// Args:
///     control_scores: Fold-major quality evidence of the stronger complete control.
///     candidates: Beam finalists re-scored on one common restart stream.
///
/// Returns:
///     Best reduced mask, or `None` when the beam produced no finalist at all.
pub(super) fn inner_winner(
    control_scores: &ScoreSet,
    candidates: Vec<ScoredMask>,
) -> Option<Vec<bool>> {
    // Partition rather than filter: candidates clearing the control are strictly preferred, but an
    // empty preferred set falls back to the whole pool, never to no set.
    let (mut cleared, mut rest): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|candidate| accepts(control_scores, &candidate.scores));
    let pool = if cleared.is_empty() {
        &mut rest
    } else {
        &mut cleared
    };
    assign_ranking(pool);
    // NOT dead, and not merely cosmetic: this sort is the TIE-BREAK CARRIER. The last tie-break
    // inside [`ranking_keys_from_qualities`] prefers the LOWER INPUT INDEX, so pre-sorting the pool
    // by the mean-based total order below makes "lower index" mean "better mean rank" — the
    // documented tie-break, preserved exactly. Delete this line as unused and the tie-break
    // silently becomes beam/scan order, changing answers with every test still green.
    pool.sort_by(ranked_order);
    // Exactly ONE candidate reaches the reserved gate folds, so the one to send is the one most
    // likely to SURVIVE an unseen stretch — not the one with the best average. Ranking the
    // finalists by their mean rewards a set that caught one lucky fold, which is the very
    // coincidence `accepts`' fold-majority clause exists to reject; applying the same scepticism
    // to the RANKING means comparing each candidate's WEAKEST fold. That is a harder test than
    // the mean, not an easier one, and it is where the walk-forward evidence is thinnest. The
    // BEAM deliberately keeps ranking on the mean: a branch that is weak everywhere must still
    // survive to pair up with another weak field.
    //
    // Why the selection below is neither a SORT nor a FOLD. `aggregate_order` is deliberately not
    // a total order: `lift_order` calls a difference inside a pair-dependent risk band a tie, and
    // inside that band `secondary_preference` decides by majority vote over three independently
    // banded metrics. Both are intransitive, and together they produce real cycles — A beats B and
    // B beats C on the secondaries while C beats A on a lift gap wide enough to clear the band.
    // Handing such a relation to `sort_by` ABORTS the process: Rust's small-sort path detects the
    // violation and panics, and this binary is `panic=abort` in a `windows_subsystem` target, so
    // the terminal vanishes with no window and no dialog. That already happened once.
    //
    // A `reduce` does not panic, but it is wrong in principle: a fold returns a maximum only when
    // a maximal element EXISTS. Measured on a real 1289-trade scope, the finalist pool had ZERO
    // maximal elements — a cycle spanned the top — and the fold returned a scan-order artifact. At
    // 1321 trades on the same scope there happened to be two byte-identical maxima, so the fold was
    // accidentally right. Luck, not a property.
    //
    // What runs instead: each candidate is reduced to its `worst_fold` quality, and
    // `ranking_keys_from_qualities` ranks those. Its edges come ONLY from `lift_order`, and every
    // edge implies strictly greater exact lift, so the graph is a DAG whatever the secondaries do —
    // the selection can neither cycle nor panic. When no maximal element exists no rule can return
    // one, so the guarantee is stated as what it is: the winner can never be a candidate that
    // another candidate MATERIALLY BEATS ON LIFT, which is exactly what the fold could return.
    // Every candidate gets a distinct rank — the same property [`gate_choice`] leans on for its
    // own tie-breaks — and the outcome is a deterministic function of the input order, which the
    // sort above fixes to the mean-based one.
    //
    // These qualities are SINGLE measurements — one fold each — so the retention floor applies,
    // which is the one thing that separates this call from `ranking_keys` above. A candidate whose
    // weakest fold is too thin to measure neither eliminates a measured rival nor wins the pool on
    // four lucky trades.
    let qualities: Vec<Quality> = pool
        .iter()
        .map(|candidate| worst_fold(&candidate.scores))
        .collect();
    let keys = ranking_keys_from_qualities(&qualities, true);
    // An empty pool yields no keys and therefore no rank 0, which is the `None` this returns.
    let best = keys.iter().position(|key| key.rank == 0)?;
    Some(pool[best].mask.clone())
}

/// The weakest fold a candidate showed, as that fold's own seed-mean.
///
/// Folds are the unit of temporal evidence, so "weakest" is asked of a fold rather than of a seed:
/// one unlucky restart inside an otherwise strong stretch is noise, while a whole stretch the set
/// could not hold up on is the thing a reserved gate fold is about to test.
///
/// A fold below the measurability floor abstains from being the weakest, the same way it abstains
/// from its own seed vote in [`super::quality_order`]. Otherwise the selection this feeds would be decided
/// by the thinnest stretch in the run: a fold that retained four trades shows a wild lift in
/// whichever direction those four landed, and sinking it to the last rank hands that noise to
/// [`inner_winner`] as the candidate's temporal evidence. When NO fold is measurable there is
/// nothing to prefer, so the whole vector is ranked and the answer is the weakest of them —
/// unmeasured, but honestly the only thing the candidate showed.
///
/// Args:
///     scores: Fold-major evidence for one candidate.
///
/// Returns:
///     The lowest-ranked measurable fold's aggregate, or a default for empty evidence.
pub(super) fn worst_fold(scores: &ScoreSet) -> Quality {
    let folds: Vec<Quality> = scores
        .iter()
        .map(|fold| mean_quality(fold.seeds.iter().copied()))
        .collect();
    let measurable: Vec<Quality> = folds
        .iter()
        .copied()
        .filter(|fold| lift_is_measurable(*fold))
        .collect();
    let ranked = if measurable.is_empty() {
        &folds
    } else {
        &measurable
    };
    // The same reason `inner_winner` no longer folds over `aggregate_order`, one level down: a
    // `reduce` returns a MINIMUM only when a minimal element exists, and the relation is
    // intransitive, so with two or three folds in a cycle the fold returns whichever one the scan
    // happened to hold. Ranking them and taking the LAST rank cannot cycle — the edges are
    // lift-only and therefore acyclic — and guarantees the fold returned is never one that
    // materially beats another fold on lift.
    let keys = ranking_keys_from_qualities(ranked, true);
    keys.iter()
        .position(|key| key.rank + 1 == ranked.len())
        .map_or_else(Quality::default, |worst| ranked[worst])
}
