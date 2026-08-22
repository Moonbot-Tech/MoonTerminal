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
//! because two individually weak fields may form a strong interaction.
//!
//! The decisive figure is LIFT, not summed profit: a candidate's held-out profit MINUS what the
//! same number of average base-passing trades earn unfiltered on the same fold. Summed profit
//! cannot decide this question at all, because the empty mask retains every base-passing trade and
//! therefore collects the whole window — a filter that is better PER TRADE competes with a
//! superset of its own trades and loses by construction. Lift is exactly zero for the empty mask,
//! so a selective filter and "no filter" meet on equal terms. A single fold counts lift as evidence
//! only above a retention floor, since keeping three lucky trades maximizes it otherwise; aggregate
//! ranking has no such floor. Close candidates then compare profit factor, lower drawdown, and
//! absolute profit. Backward elimination gives cost-free fields back, making the smaller set the
//! complexity tie-break — but it never removes the last field.
//!
//! Two final folds see only the selected subset and choose symmetrically among three normal paths:
//! that reduced set, every admitted field, or no additional field filters. Nothing earlier may
//! answer "no additional filters" on a tie: a selection stage that resolves ties toward the empty
//! set decides the very question these reserved folds exist to answer. When the gate itself
//! separates nothing, the ranked-best path is still applied and the outcome says the evidence was
//! inconclusive rather than claiming a winner.

use std::cmp::Ordering;
use std::ops::Range;

use super::handle::SearchHandle;
use super::search::Search;
use super::ComposeDecision;
use crate::db::metrics::improvement_margin;

mod ranking;

#[cfg(test)]
use ranking::{adaptive_width, worst_fold};
use ranking::{
    beam_candidates, inner_winner, mask_order, ranking_key_order, ranking_keys, RankingKey,
    ScoredMask,
};

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

/// Smallest SHARE of a fold's base-passing validate rows an outcome must retain before its lift
/// counts as evidence.
///
/// One tenth, matching the search's own automatic `min_n` (`mod.rs`: one tenth of what the descent
/// fits on) so both halves of a fold ask for the same selectivity. Without a floor, lift is
/// maximized by keeping the three best trades in the window.
const LIFT_MIN_RETENTION: f64 = 0.10;

/// Absolute floor under [`LIFT_MIN_RETENTION`], for folds too short for a share to mean anything.
///
/// Below roughly twenty outcomes a mean and a drawdown are decided by two or three trades, so the
/// share alone would admit a fold's worth of noise as a decisive lift.
///
/// It is a statement about ONE measurement, which is why only [`quality_order`] applies it and
/// [`aggregate_order`] does not: a fold this thin abstains from its own vote, but it must not drag
/// an average of a dozen folds and seeds under the same bar.
const LIFT_MIN_TRADES: f64 = 20.0;

/// Largest restart count spent fitting one candidate on one ranking fold.
///
/// A GUARD above the accepted range, not a budget: [`super::RESTARTS_MAX`] over
/// [`RANKING_RESTART_DIVISOR`] is 3125, so inside the range the user can actually request this
/// clamp never binds and the ranking budget is exactly their setting divided by the divisor. It
/// used to sit at 512, where it bound for every setting from 16_384 upward — the visible 100_000
/// silently became 512 per candidate and 102 per seed vote, so raising the setting could not
/// change a single ranking decision. It stays only so a future rise in `RESTARTS_MAX` cannot turn
/// one click into an unbounded beam.
const RANKING_RESTARTS_MAX: usize = 4096;

/// User restarts represented by one ranking restart before the guard applies.
///
/// The beam ranks thousands of masks on several folds, so a candidate cannot be fitted with the
/// budget the final refit gets: measured on a 1289-trade scope with 32 logical cores, one full
/// 100_000-restart fit per candidate per fold costs hours for one click, while a thirty-second of
/// it costs minutes. This is the affordable point, not a free choice.
const RANKING_RESTART_DIVISOR: usize = 32;

/// Restarts one seed group must be able to spend before its vote means anything.
///
/// A group of one restart is a single random start — or, in group zero, the deterministic greedy
/// one — so five of those are five coin flips wearing the label of a consensus. Below this the
/// group count is reduced rather than the evidence pretended into existence.
const SEED_GROUP_MIN_RESTARTS: usize = 8;

/// Final folds reserved from beam ranking and used only to gate its one winner.
const GATE_FOLDS: usize = 2;

/// One walk-forward fold: a search fitted on a prefix, and the stretch it is measured on.
pub(super) struct Fold {
    /// Search prepared over `0..fit_end`, with its own quantile edges.
    pub(super) search: Search,
    /// Rows immediately after the fit prefix — never seen while fitting.
    pub(super) validate: Range<usize>,
    /// The VALIDATE window scored with no additional field ranges at all — the caller's fixed
    /// filters and nothing else.
    ///
    /// Every lift on this fold is measured against this one tally, so it is a property of the
    /// fold rather than of a candidate: it does not depend on the mask, the seed or the step, and
    /// recomputing it per candidate would answer identically thousands of times per run. It is
    /// built by [`Self::new`] so no caller can scope it to the FIT prefix by accident, which
    /// would bias every lift in one direction while every number still looked plausible.
    base: crate::db::metrics::Tally,
}

impl Fold {
    /// Build one fold and measure the unfiltered baseline its candidates are judged against.
    ///
    /// Args:
    ///     search: Search prepared over the fold's own fit prefix.
    ///     validate: Rows immediately after that prefix.
    ///
    /// Returns:
    ///     The fold, carrying its own unfiltered validate tally.
    pub(super) fn new(search: Search, validate: Range<usize>) -> Self {
        let base = search.tally(&[], validate.clone());
        Self {
            search,
            validate,
            base,
        }
    }
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

/// The beam's set, and the two figures that say why the gate did not take it.
///
/// It exists because "no additional filters" is the ordinary answer and reads as the search having
/// found nothing, when in fact a set WAS found and then failed on the folds it had never seen.
/// Those two lifts are the whole story, and lift is the quantity this module ranks on — zero by
/// definition for the empty mask, so a signed pair is directly readable.
///
/// The caveat that must never be dropped: the two lifts come from DIFFERENT restart streams and
/// different budgets ([`ComposeParams::ranking_restarts`] against
/// [`ComposeParams::gate_restarts`]) over different folds. They are evidence about WHERE the set
/// held up, not a controlled A/B, and no surface may word them as one.
pub(super) struct RejectedCandidate {
    /// The beam subset the gate declined, as a field mask.
    pub(super) mask: Vec<bool>,
    /// Mean lift over the inner selection folds — the stretch it was fitted and ranked on.
    pub(super) inner_lift: f64,
    /// Mean lift over the reserved gate folds — the stretch it had never seen.
    pub(super) gate_lift: f64,
    /// Inner folds `inner_lift` is a mean over.
    pub(super) inner_folds: u8,
    /// Reserved folds `gate_lift` is a mean over.
    pub(super) gate_folds: u8,
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
    /// Whether the reserved gate folds robustly separated any path from another.
    ///
    /// `false` is not a failure and not an error path — the decision above is still the best
    /// supported answer — but it is a different claim from "this path won", and the surface
    /// showing it has to be able to say so.
    pub(super) gate_robust: bool,
    /// The beam set the gate declined, when it declined a real one.
    ///
    /// `None` whenever there is nothing to explain: the subset WON, or the beam produced no set at
    /// all. See [`RejectedCandidate`] for why the two lifts are not a controlled comparison.
    pub(super) rejected: Option<RejectedCandidate>,
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
    /// Held-out profit ABOVE what this many base-passing trades earn unfiltered on the same fold.
    ///
    /// The decisive quantity, and the reason the empty mask no longer wins by construction: it
    /// retains every base-passing trade and so collects the whole window's profit, which any
    /// selective filter competes with a fraction of. Lift removes exactly that arithmetic — it is
    /// zero for the empty mask by definition, positive for a filter whose retained trades beat the
    /// fold's own average trade, and it stays in profit units, so the same risk-scaled band
    /// applies to it unchanged.
    lift: f64,
    /// Share of the fold's base-passing validate rows this outcome retained.
    ///
    /// Lift alone is maximized by keeping three lucky trades, so it counts as evidence only above
    /// a floor. Carried per seed and averaged like every other metric rather than recomputed from
    /// means, because the base count differs between folds.
    retention: f64,
}

/// Measure one held-out tally against the fold's own unfiltered baseline.
///
/// The ONE place lift is computed. It deliberately replaces a `From<Tally>` impl: a conversion
/// that cannot see the baseline is exactly the shape of the bug this metric exists to remove, and
/// a caller reaching for `.into()` would silently produce a lift of zero for a real filter.
///
/// Args:
///     tally: Held-out rows scored under one fitted seed outcome.
///     base: The same rows scored with no additional field ranges at all.
///
/// Returns:
///     Finite metrics used by ranking and robust acceptance.
fn quality_on(tally: crate::db::metrics::Tally, base: &crate::db::metrics::Tally) -> Quality {
    let trades = tally.n as f64;
    // An empty baseline leaves nothing to be better than, so the fold contributes no lift rather
    // than an infinity. `Tally::avg` already answers zero there, and `retention` is zero for the
    // same reason, which the floor then rejects.
    let retention = if base.n > 0 {
        trades / base.n as f64
    } else {
        0.0
    };
    Quality {
        profit: tally.profit,
        profit_factor: tally.profit_factor(),
        max_dd: tally.max_dd,
        trades,
        lift: tally.profit - base.avg() * trades,
        retention,
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
        out.lift += value.lift;
        out.retention += value.retention;
        count += 1;
    }
    if count > 0 {
        let divisor = count as f64;
        out.profit /= divisor;
        out.profit_factor /= divisor;
        out.max_dd /= divisor;
        out.trades /= divisor;
        out.lift /= divisor;
        out.retention /= divisor;
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
    /// Absolute held-out profit preference; higher is better.
    ///
    /// This slot used to hold RETAINED TRADES, which the empty mask wins unconditionally by
    /// construction: it keeps every base-passing row, so no filter can ever match it and the vote
    /// was a constant. Absolute profit is won by neither side structurally — a filter that removes
    /// losers exceeds the unfiltered total, and on a losing stretch the unfiltered total is
    /// negative — and it is the figure the user actually banks, which is the sanity check a
    /// per-trade primary needs beside it.
    profit: Ordering,
}

impl SecondaryPreference {
    /// Combine the three metrics into an equal-weight pairwise vote.
    ///
    /// Returns:
    ///     Positive when the left outcome wins more metrics, negative when the right does.
    fn balance(self) -> i8 {
        [self.profit_factor, self.drawdown, self.profit]
            .into_iter()
            .map(|ordering| match ordering {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
            .sum()
    }
}

/// Compare PF, drawdown, and absolute profit under their shared robust margins.
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
    let profit_margin = risk_scale(a, b, |q| q.profit) * PROFIT_BAND_FRAC;
    let profit = if a.profit > b.profit + profit_margin {
        Ordering::Greater
    } else if b.profit > a.profit + profit_margin {
        Ordering::Less
    } else {
        Ordering::Equal
    };
    SecondaryPreference {
        profit_factor,
        drawdown,
        profit,
    }
}

/// The risk scale one comparison is banded against: the larger magnitude of the compared figure,
/// floored by both drawdowns and by one unit.
///
/// Shared by the lift comparison and the absolute-profit vote so the same pair is never banded two
/// different ways depending on which predicate is asking.
///
/// Args:
///     a: Left outcome.
///     b: Right outcome.
///     figure: The metric being compared.
///
/// Returns:
///     A strictly positive scale.
fn risk_scale(a: Quality, b: Quality, figure: impl Fn(&Quality) -> f64) -> f64 {
    figure(&a)
        .abs()
        .max(figure(&b).abs())
        .max(a.max_dd)
        .max(b.max_dd)
        .max(1.0)
}

/// Whether an outcome retained enough of its fold to let its lift count as evidence.
///
/// Args:
///     q: Outcome to test.
///
/// Returns:
///     `true` once both the absolute floor and the retained share are cleared.
fn lift_is_measurable(q: Quality) -> bool {
    q.trades >= LIFT_MIN_TRADES && q.retention >= LIFT_MIN_RETENTION
}

/// Compare LIFT against the pair's own risk-scaled band.
///
/// Lift, not absolute profit, is what makes this comparison fair: the empty mask retains every
/// base-passing trade and therefore collects the whole window's profit, so an absolute-sum
/// predicate asks a selective filter to beat the sum of a superset of its own trades. Lift is that
/// sum minus what the same NUMBER of average base-passing trades would have earned, so it is
/// exactly zero for the empty mask and positive for a filter whose retained trades are better per
/// trade.
///
/// The retention floor is deliberately NOT applied here. Its callers want it at different levels —
/// [`quality_order`] applies it to a single outcome, [`aggregate_order`] must not apply it to a
/// mean of many, and `ranking::ranking_keys_from_qualities` applies it only when it is told the
/// qualities are single measurements — so the floor lives with them and this stays the pure
/// comparison.
///
/// Args:
///     a: Left outcome.
///     b: Right outcome.
///
/// Returns:
///     Lift ordering outside the material band, otherwise `Equal`.
fn lift_order(a: Quality, b: Quality) -> Ordering {
    let delta = a.lift - b.lift;
    let band = risk_scale(a, b, |q| q.lift) * PROFIT_BAND_FRAC;
    if delta > band {
        Ordering::Greater
    } else if delta < -band {
        Ordering::Less
    } else {
        Ordering::Equal
    }
}

/// Compare two single-fold outcomes, abstaining when either lacks enough retained data.
///
/// The retention floor applies only to one fold's measurement: below it, neither outcome has
/// evidence for this fold and the comparison returns `Equal`. Measurable outcomes use
/// [`balanced_order`], while [`aggregate_order`] deliberately compares means without this floor.
///
/// Args:
///     a: Left outcome.
///     b: Right outcome.
///
/// Returns:
///     Which outcome is better under the balanced comparison.
fn quality_order(a: Quality, b: Quality) -> Ordering {
    // Below the retention floor there is no evidence, so this ABSTAINS rather than deciding —
    // in either direction. The whole comparison stops here, secondaries included: a thin outcome
    // wins those by construction, since four trades that happen to land right show a huge profit
    // factor and almost no drawdown, and letting them through would readmit exactly what the
    // floor rejects. Ruling AGAINST it would be the opposite mistake — "not measured" is not
    // "worse", and with a strict fold majority one collapsed fold would veto a good set.
    if !lift_is_measurable(a) || !lift_is_measurable(b) {
        return Ordering::Equal;
    }
    balanced_order(a, b)
}

/// Compare two AGGREGATES — means taken over every fold and every seed group.
///
/// Identical to [`quality_order`] except that the retention floor does NOT apply, and that is the
/// whole point of having two functions. The floor answers "is this ONE measurement meaningful?",
/// and an average of ten or fifteen measurements is not one measurement: holding it to a single
/// outcome's floor lets one collapsed fold drag the mean under the bar and veto a set whose lift
/// is strongly positive everywhere else. Measured on a real scope, a field set with a mean lift of
/// +37 across the selection folds was rejected for exactly that reason.
///
/// The per-fold floor still does its job, because the fold vote runs through [`quality_order`]:
/// a fold too thin to measure abstains instead of being counted for either side.
///
/// Args:
///     a: Left aggregate.
///     b: Right aggregate.
///
/// Returns:
///     Which aggregate is better under the balanced comparison.
fn aggregate_order(a: Quality, b: Quality) -> Ordering {
    balanced_order(a, b)
}

/// The balanced comparison itself: material lift, then the equal-weight secondary vote, then lift
/// above floating-point noise.
///
/// Shared by [`quality_order`] and [`aggregate_order`] so the two can differ in exactly ONE thing
/// — whether the retention floor applies — and in nothing else.
///
/// Args:
///     a: Left outcome.
///     b: Right outcome.
///
/// Returns:
///     Which outcome is better.
fn balanced_order(a: Quality, b: Quality) -> Ordering {
    let material_lift = lift_order(a, b);
    if !material_lift.is_eq() {
        return material_lift;
    }

    let votes = secondary_preference(a, b).balance();
    match votes.cmp(&0) {
        // The secondaries split evenly, so the last word goes back to lift.
        Ordering::Equal => {
            let lift_delta = a.lift - b.lift;
            let noise = improvement_margin(risk_scale(a, b, |q| q.lift));
            if lift_delta > noise {
                Ordering::Greater
            } else if lift_delta < -noise {
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
    if !aggregate_order(summary(candidate), summary(incumbent)).is_gt() {
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
    !aggregate_order(summary(without), summary(incumbent)).is_lt()
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
    // Resolved ONCE, outside the fold loop and shared by both branches: the empty mask replicates
    // its single score this many times, so a count that disagreed with what the seeded branch
    // actually ran would compare evidence vectors of different lengths and `seed_consensus` would
    // silently refuse every candidate.
    let active_groups = active_seed_groups(p.seed_groups, requested_restarts);
    folds
        .iter()
        .map(|fold| {
            // An empty mask is seed-invariant. Score it once and replicate the evidence shape so
            // comparisons remain aligned without spending the requested restart budget on copies.
            if !mask.iter().any(|chosen| *chosen) {
                let outcome = fold.search.run_masked(mask, 1, seed, handle)?;
                let applied = fold.search.applied_ranges(&outcome.sel, p.round);
                let score = quality_on(
                    fold.search.tally(&applied, fold.validate.clone()),
                    &fold.base,
                );
                return Some(FoldScores {
                    seeds: vec![score; active_groups],
                });
            }
            let seeds = fold
                .search
                .run_masked_seed_groups(mask, requested_restarts, seed, active_groups, handle)?
                .into_iter()
                .map(|outcome| {
                    let applied = fold.search.applied_ranges(&outcome.sel, p.round);
                    quality_on(
                        fold.search.tally(&applied, fold.validate.clone()),
                        &fold.base,
                    )
                })
                .collect();
            Some(FoldScores { seeds })
        })
        .collect()
}

/// Independent seed groups a given restart budget can honestly supply.
///
/// Three rules, in order: never more groups than the non-zero requested count, never a group
/// thinner than [`SEED_GROUP_MIN_RESTARTS`], and never an EVEN count. A zero request is treated as
/// one group so the scoring path still has a valid evidence shape. The last rule matters as much as the
/// others — `seed_consensus` asks for a STRICT majority, so an even split resolves against the
/// candidate, and a vote that can only be lost is not a vote. That is the same reason
/// [`SEED_GROUPS`] itself is odd.
///
/// Args:
///     requested_groups: Groups the composition budget asked for; zero is treated as one.
///     restarts: Total restart budget the groups must divide.
///
/// Returns:
///     A positive, odd group count no larger than `requested_groups.max(1)`.
fn active_seed_groups(requested_groups: usize, restarts: usize) -> usize {
    let affordable = restarts / SEED_GROUP_MIN_RESTARTS;
    let groups = requested_groups.max(1).min(affordable.max(1));
    // Round DOWN to odd; one group is the floor, and one group is already odd.
    if groups.is_multiple_of(2) {
        groups - 1
    } else {
        groups
    }
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

/// One of the three search paths selected by the outer gate.
#[derive(Debug, PartialEq, Eq)]
struct GateChoice {
    /// User-facing meaning of the selected mask.
    decision: ComposeDecision,
    /// Fields admitted into the final full-budget refit.
    mask: Vec<bool>,
    /// Whether ANY path robustly beat another on the reserved gate folds.
    ///
    /// `false` means the gate separated nothing and the winner was settled by the deterministic
    /// balanced ranking alone — real evidence, but weaker than a pairwise win. It is carried
    /// beside the decision rather than folded into it because "which path won" and "how strong the
    /// evidence was" are independent questions: collapsing them would force the caller to drop the
    /// path name in order to report the doubt, which is precisely how an inconclusive gate used to
    /// reach the user as a confident "no additional filters".
    robust: bool,
}

/// Select between a strict subset, all admitted fields, and no additional filters.
///
/// Only one beam subset reaches this function, so the gate is not reused to rank the frontier.
/// The three paths are ranked symmetrically: first by how many alternatives they beat on a strict
/// majority of gate folds, then by the deterministic balanced ranking when pairwise comparisons
/// are inconclusive. With two reserved folds, a pairwise win must repeat on both rather than ride
/// one lucky validation stretch, so no pairwise win at all is the ORDINARY outcome — which is why
/// that case is reported through [`GateChoice::robust`] instead of being dressed as a verdict.
///
/// The complexity and mask tie-breaks below are the visible total order; they are unreachable in
/// practice because [`ranking_keys`] already assigns every candidate a distinct rank. Keeping them
/// costs nothing and states the intended order at the point a future change might need it.
///
/// Args:
///     no_filters: Empty mask and its gate scores.
///     all_fields: All admitted fields and their gate scores.
///     subset: One beam-selected mask and its gate scores.
///
/// Returns:
///     Decision and mask supported by the reserved gate folds, and whether anything separated them.
fn gate_choice(
    no_filters: (&[bool], &ScoreSet),
    all_fields: (&[bool], &ScoreSet),
    subset: (&[bool], &ScoreSet),
) -> GateChoice {
    if all_fields.0 == no_filters.0 {
        // Nothing was admitted at all, so there is only one path. A degenerate case with one
        // answer is not an inconclusive one.
        return GateChoice {
            decision: ComposeDecision::NoAdditionalFilters,
            mask: no_filters.0.to_vec(),
            robust: true,
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
    // Only an EQUAL mask is skipped, and only because a duplicate path would compare against
    // itself. The subset can no longer BE the empty mask — `inner_winner` never answers one — so
    // the guard that used to drop `ReducedSet` whenever it did is gone with it.
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
        robust: wins.iter().any(|count| *count > 0),
    }
}

/// Remove fields whose absence costs nothing on the inner selection folds.
///
/// It never removes the LAST field. Elimination runs on the inner folds, so walking a set down to
/// empty here would answer "no additional filters" from the same stage that already picked the
/// set — pre-empting the reserved gate folds, which are the only place that answer is allowed to
/// come from. A set of one that deserves to go is dropped by the gate, on evidence it never saw.
///
/// Args:
///     folds: Inner folds already used for candidate ranking.
///     mask: Reduced winner to simplify.
///     p: Composition search settings.
///     step: Last seed-stream step consumed before shrinking.
///     handle: Cancellation and progress channel.
///
/// Returns:
///     Simplified mask, final consumed step, and the inner-fold scores of the mask returned, or
///     `None` when cancelled. The scores cost nothing extra: the exit below is taken only after a
///     pass that removed nothing, so `incumbent` is already exactly this mask's evidence.
fn shrink_mask(
    folds: &[Fold],
    mut mask: Vec<bool>,
    p: &ComposeParams,
    mut step: usize,
    handle: &SearchHandle,
) -> Option<(Vec<bool>, usize, ScoreSet)> {
    loop {
        step += 1;
        let mut incumbent = score_set(folds, &mask, p, p.ranking_restarts, step, handle)?;
        let mut removed = false;
        let mut held = mask.iter().filter(|chosen| **chosen).count();
        let mut tried = 0usize;
        for fi in 0..mask.len() {
            if !mask[fi] {
                continue;
            }
            // The last field is never offered up: see the note on this function.
            if held <= 1 {
                break;
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
                held -= 1;
            } else {
                mask[fi] = true;
            }
        }
        if !removed {
            return Some((mask, step, incumbent));
        }
    }
}

/// Keep the beam's set beside the verdict when the reserved folds declined it.
///
/// A beam set that lost to one of the two controls is the answer to the question the verdict
/// provokes — "so the search found nothing?" — and it is thrown away everywhere else. It is
/// carried only when there is something to carry: the subset must have LOST, must be a real
/// non-empty set, and must differ from the mask that won. Nothing here changes the decision; a
/// caller that ignores it sees exactly the composition it saw before.
///
/// Args:
///     choice: What the reserved gate folds decided.
///     subset: The beam's shrunk mask that was offered to the gate.
///     subset_inner: That mask's evidence on the inner selection folds.
///     subset_gate: That same mask's evidence on the reserved gate folds.
///     inner_folds: How many inner folds `subset_inner` spans.
///     gate_folds: How many reserved folds `subset_gate` spans.
///
/// Returns:
///     The declined set with both lifts, or `None` when nothing was declined.
fn rejected_candidate(
    choice: &GateChoice,
    subset: &[bool],
    subset_inner: &ScoreSet,
    subset_gate: &ScoreSet,
    inner_folds: usize,
    gate_folds: usize,
) -> Option<RejectedCandidate> {
    if choice.decision == ComposeDecision::ReducedSet
        || !subset.iter().any(|chosen| *chosen)
        || subset == choice.mask
    {
        return None;
    }
    Some(RejectedCandidate {
        mask: subset.to_vec(),
        inner_lift: summary(subset_inner).lift,
        gate_lift: summary(subset_gate).lift,
        inner_folds: inner_folds.min(u8::MAX as usize) as u8,
        gate_folds: gate_folds.min(u8::MAX as usize) as u8,
    })
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
            // Concurrent publishers: the beam scores one depth's masks in parallel.
            handle.advance_stage(depth, done, total);
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
    // No finalist at all means the beam had nothing to admit, which is the one honest route to the
    // empty mask that does not run through a tie.
    let subset = inner_winner(inner_control, ranked).unwrap_or_else(|| no_filters.clone());
    let (subset, next_step, subset_inner) = shrink_mask(inner_folds, subset, p, step, handle)?;
    step = next_step + 1;

    // The final folds are an OUTER gate: only the already-selected subset reaches them.
    // Comparing every beam candidate here would merely overfit more validation stretches.
    handle.set_stage(step, 0, 3);
    let no_filters_gate = score_set(gate_folds, &no_filters, p, p.gate_restarts, step, handle)?;
    handle.set_stage(step, 1, 3);
    let all_fields_gate = score_set(gate_folds, &all_fields, p, p.gate_restarts, step, handle)?;
    handle.set_stage(step, 2, 3);
    // Scored on its own, always. The old shortcut aliased the subset's gate evidence to the empty
    // mask's whenever the two masks were equal, which — with `inner_winner` answering the empty
    // mask on every tie — made the gate compare the empty mask against ITSELF and report the
    // result as a decision. `inner_winner` can no longer answer it, so the alias has nothing left
    // to save and everything to hide.
    let subset_gate = score_set(gate_folds, &subset, p, p.gate_restarts, step, handle)?;
    let choice = gate_choice(
        (&no_filters, &no_filters_gate),
        (&all_fields, &all_fields_gate),
        (&subset, &subset_gate),
    );

    let rejected = rejected_candidate(
        &choice,
        &subset,
        &subset_inner,
        &subset_gate,
        inner_folds.len(),
        gate_folds.len(),
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
            active_seed_groups(p.seed_groups, p.ranking_restarts),
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
        gate_robust: choice.robust,
        rejected,
    })
}
