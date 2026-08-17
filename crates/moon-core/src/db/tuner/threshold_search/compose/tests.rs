//! Tests for the field-set composition.
//!
//! The acceptance rules are pure functions over fold-score vectors, so they are tested directly:
//! no sample, no search, no database, and an oracle that is a hand-written vector rather than
//! anything the code under test produced.
//!
//! The fold-isolation test is the one that needs a real search behind it, because what it pins is
//! a property of how a fold is BUILT rather than of how a set is chosen.

use std::sync::Arc;
use std::time::Instant;

use super::*;
use crate::db::metrics::Tally;
use crate::db::tuner::threshold_search::search::Columns;
use crate::db::tuner::{FieldClass, FIELDS};

/// Deterministic value generator, independent of the search's own PRNG so a fixture does not
/// shift when that stream changes.
struct Lcg(u64);

impl Lcg {
    /// Advance and return the next 64-bit state.
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }

    /// Uniform value in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Per field, whether it occupies a Delta2/Delta3 slot.
fn slot_flags() -> Vec<bool> {
    FIELDS
        .iter()
        .map(|s| s.class == FieldClass::DeltaSlot)
        .collect()
}

/// A sample of `n` trades whose profit genuinely depends on the first two columns, so a search
/// over it has something to find rather than only noise to fit.
fn sample(n: usize, seed: u64) -> (Vec<f64>, Vec<Vec<f64>>, Vec<i64>) {
    let mut rng = Lcg(seed | 1);
    let nf = FIELDS.len();
    let mut profits = Vec::with_capacity(n);
    let mut vals = vec![Vec::with_capacity(n); nf];
    let mut closes = Vec::with_capacity(n);
    for t in 0..n {
        let a = rng.unit() * 100.0;
        let b = rng.unit() * 100.0;
        // Trades in the upper band of the first column pay; everything else bleeds.
        let edge = if a > 60.0 { 4.0 } else { -1.5 };
        profits.push(edge + (rng.unit() - 0.5) * 3.0);
        for (fi, col) in vals.iter_mut().enumerate() {
            col.push(match fi {
                0 => a,
                1 => b,
                _ => rng.unit() * 50.0,
            });
        }
        // Distinct, increasing timestamps so every cut has a boundary to snap to.
        closes.push(1_700_000_000 + t as i64);
    }
    (profits, vals, closes)
}

// ─────────────────────────────── the acceptance rules ───────────────────────────────

/// Build one hand-authored comparison outcome on a fold whose unfiltered baseline pays ZERO per
/// trade, where lift is profit by definition and the whole window was retained.
///
/// That baseline is what keeps every fixture written before lift existed meaning exactly what its
/// author wrote: with a base rate of zero, `lift = profit - 0 * trades = profit`. Use
/// [`quality_lift`] wherever the point of the fixture is a baseline that pays something.
///
/// Args:
///     profit: Held-out profit.
///     profit_factor: Held-out profit factor.
///     max_dd: Held-out maximum drawdown.
///     trades: Held-out retained trade count.
///
/// Returns:
///     Quality evidence independent of the production search.
fn quality(profit: f64, profit_factor: f64, max_dd: f64, trades: f64) -> Quality {
    Quality {
        profit,
        profit_factor,
        max_dd,
        trades,
        lift: profit,
        retention: 1.0,
    }
}

/// Build one hand-authored comparison outcome with lift and retention stated independently.
///
/// Args:
///     lift: Held-out profit above the fold's own unfiltered rate for this many trades.
///     profit: Held-out profit.
///     profit_factor: Held-out profit factor.
///     max_dd: Held-out maximum drawdown.
///     trades: Held-out retained trade count.
///     retention: Share of the fold's base-passing validate rows retained.
///
/// Returns:
///     Quality evidence independent of the production search.
fn quality_lift(
    lift: f64,
    profit: f64,
    profit_factor: f64,
    max_dd: f64,
    trades: f64,
    retention: f64,
) -> Quality {
    Quality {
        profit,
        profit_factor,
        max_dd,
        trades,
        lift,
        retention,
    }
}

/// Build one-seed folds from hand-authored profit values.
///
/// Args:
///     profits: One held-out profit per fold.
///
/// Returns:
///     Fold-major evidence with neutral, identical secondary metrics.
fn profit_scores(profits: &[f64]) -> ScoreSet {
    profits
        .iter()
        .map(|profit| FoldScores {
            seeds: vec![quality(*profit, 1.0, 10.0, 100.0)],
        })
        .collect()
}

/// Build one fold from an explicit collection of seed outcomes.
///
/// Args:
///     seeds: Independent outcomes belonging to the same time fold.
///
/// Returns:
///     Fold evidence suitable for assembling hierarchy fixtures.
fn seed_fold(seeds: &[Quality]) -> FoldScores {
    FoldScores {
        seeds: seeds.to_vec(),
    }
}

/// A candidate must earn its place in MOST folds, not just on average.
///
/// The oracle is a hand-written pair of vectors: one fold improves enormously while two get
/// worse, so the mean rises and the majority does not. Nothing here is derived from the code
/// under test.
///
/// Breakage this pins: `compose.rs:accepts` dropping its majority clause and deciding on the
/// mean alone. The inner winner or outer gate could then accept a path that caught one lucky
/// stretch, which is precisely the coincidence out-of-sample scoring exists to reject.
#[test]
fn a_field_only_joins_when_it_helps_in_most_folds() {
    let incumbent = profit_scores(&[100.0, 100.0, 100.0]);
    // Mean rises from 100 to 140, yet two folds out of three got worse.
    let lucky_one_fold = profit_scores(&[420.0, 99.0, 99.0]);
    assert!(
        summary(&lucky_one_fold).profit > summary(&incumbent).profit,
        "the fixture must be a candidate whose MEAN improves, or it tests nothing"
    );
    assert!(
        !accepts(&incumbent, &lucky_one_fold),
        "a candidate winning one fold and losing two must be refused"
    );
    // The same total improvement spread across the folds is a real pattern, and is taken.
    assert!(
        accepts(&incumbent, &profit_scores(&[110.0, 108.0, 112.0])),
        "a candidate that helps in every fold must be accepted"
    );
    // Better in a majority but worse on average is still not worth taking.
    assert!(
        !accepts(&incumbent, &profit_scores(&[101.0, 101.0, 40.0])),
        "a candidate that wrecks one fold must not ride in on a bare majority"
    );
}

/// Seed votes are resolved inside folds before chronological folds vote.
///
/// The candidate wins five of nine individual seed comparisons, but those wins are concentrated
/// as 3+1+1 and therefore produce only one fold win. Flattening all nine votes would incorrectly
/// accept it even though two of three time periods reject it.
///
/// Breakage this pins: replacing `compose.rs:seed_consensus` plus the fold majority in `accepts`
/// with one flat seed-fold win count. A strategy could then pass because one historical stretch
/// had three lucky restarts while both other stretches disagreed.
#[test]
fn seed_votes_cannot_outvote_the_chronological_fold_hierarchy() {
    let base = quality(100.0, 1.0, 10.0, 100.0);
    let better = quality(110.0, 1.0, 10.0, 100.0);
    let worse = quality(90.0, 1.0, 10.0, 100.0);
    let incumbent = vec![
        seed_fold(&[base, base, base]),
        seed_fold(&[base, base, base]),
        seed_fold(&[base, base, base]),
    ];
    let candidate = vec![
        seed_fold(&[better, better, better]),
        seed_fold(&[better, worse, worse]),
        seed_fold(&[better, worse, worse]),
    ];
    assert!(
        quality_order(summary(&candidate), summary(&incumbent)).is_gt(),
        "the flat aggregate must improve or the hierarchy is not the deciding guard"
    );
    assert!(
        !accepts(&incumbent, &candidate),
        "five flattened seed wins cannot replace a majority of chronological folds"
    );
}

/// Production fold scoring must carry every requested seed group into the vote hierarchy.
///
/// The fixture spends [`SEED_GROUPS`] times [`SEED_GROUP_MIN_RESTARTS`] ranking restarts, so every
/// requested group can own the minimum honest evidence and the returned shape is observable
/// without relying on a tuned profit answer.
///
/// Breakage this pins: replacing `requested_groups` with `1` in `compose.rs:score_set`. The pure
/// consensus tests would still pass on hand-built evidence, while real composition silently
/// reverted to a single seed.
#[test]
fn score_set_uses_every_budgeted_seed_group_in_production_scoring() {
    let (profits, vals, closes) = sample(600, 20260731);
    let cols = Arc::new(Columns { profits, vals });
    let locked = vec![None; FIELDS.len()];
    let folds = super::super::build_folds(&cols, &locked, &slot_flags(), &closes, 10, 16, 500, 3);
    let p = ComposeParams {
        ranking_restarts: SEED_GROUPS * SEED_GROUP_MIN_RESTARTS,
        gate_restarts: SEED_GROUPS * SEED_GROUP_MIN_RESTARTS,
        seed: 0x5EED,
        round: false,
        max_fields: FIELDS.len(),
        beam_width_min: 8,
        beam_width_max: 16,
        seed_groups: SEED_GROUPS,
    };
    let mut mask = vec![false; FIELDS.len()];
    mask[0] = true;
    let scores = score_set(
        &folds,
        &mask,
        &p,
        SEED_GROUPS * SEED_GROUP_MIN_RESTARTS,
        1,
        &SearchHandle::new(),
    )
    .expect("an uncancelled full-group score answers");
    assert!(
        scores.iter().all(|fold| fold.seeds.len() == SEED_GROUPS),
        "every production fold must preserve all {SEED_GROUPS} seed votes"
    );
}

/// A support badge counts a fold only after that fold's seed majority agrees.
///
/// Fields zero and one appear in two of three seed outcomes and are supported once. Field two
/// appears in one seed only and must contribute no support, while empty evidence supports none.
///
/// Breakage this pins: changing `compose.rs:seed_majority_support` from strict majority to
/// `count > 0`. One random restart could then add a full fold to a field's confidence badge even
/// when the other two fitted outcomes never used it.
#[test]
fn fold_support_requires_a_seed_majority() {
    assert_eq!(
        seed_majority_support(3, &[vec![0, 1], vec![0], vec![1, 2]]),
        [true, true, false],
        "each field needs two of three seeds, and a fold contributes at most once"
    );
    assert_eq!(
        seed_majority_support(3, &[]),
        [false, false, false],
        "missing seed evidence cannot support a field"
    );
}

/// Secondary metrics decide close profit contests, while material profit remains decisive.
///
/// Breakage this pins: changing `compose.rs:quality_order` back to profit-only ordering. The first
/// assertion would then select a noisily higher-profit path with much worse PF, drawdown, and
/// coverage. Removing the profit band would break the second assertion and let secondary metrics
/// disguise a material loss.
#[test]
fn balanced_quality_decides_only_inside_the_profit_band() {
    let stable = quality(100.0, 1.40, 20.0, 200.0);
    let noisy = quality(102.0, 1.05, 45.0, 120.0);
    assert!(
        quality_order(stable, noisy).is_gt(),
        "PF, drawdown, and trades must overturn a two-percent profit difference"
    );
    let material_profit = quality(130.0, 1.05, 45.0, 120.0);
    assert!(
        quality_order(material_profit, stable).is_gt(),
        "a profit difference outside the risk-scaled band must remain decisive"
    );
}

/// Beam sorting applies secondary metrics without pool-wide profit buckets or loss totals.
///
/// A distant third branch must not move the close pair across an arbitrary global boundary. The
/// second fixture straddled a global bucket edge in the former implementation even though its
/// pairwise profit gap is below five percent. A strong close-profit third branch also must not
/// reverse that pair merely because it contributes a different number of robust metric losses to
/// each side.
///
/// Breakage this pins: restoring a pool-wide profit bucket in `compose.rs:ranking_keys`, removing
/// the head-to-head quality comparison from its topological selection, summing pool-wide
/// secondary losses, or omitting material-profit dependency edges. Those edits either rank the
/// noisier close path first or let secondary metrics hide a material loss.
#[test]
fn beam_ranking_keeps_close_pairs_stable_when_the_pool_changes() {
    let stable = vec![seed_fold(&[quality(100.0, 1.40, 20.0, 200.0)])];
    let noisy = vec![seed_fold(&[quality(102.0, 1.05, 45.0, 120.0)])];
    let distant = vec![seed_fold(&[quality(0.0, 1.0, 10.0, 100.0)])];
    let keys = ranking_keys(&[&stable, &noisy, &distant]);
    assert!(
        ranking_key_order(&keys[0], &keys[1]).is_gt(),
        "an unrelated low-profit branch cannot erase the close pair's secondary evidence"
    );

    let strong = vec![seed_fold(&[quality(101.0, 2.0, 15.0, 300.0)])];
    let strong_keys = ranking_keys(&[&stable, &noisy, &strong]);
    assert!(
        ranking_key_order(&strong_keys[0], &strong_keys[1]).is_gt(),
        "a strong third branch cannot reverse the unchanged close pair through pooled losses"
    );

    let boundary_stable = vec![seed_fold(&[quality(4.9, 1.40, 1.0, 200.0)])];
    let boundary_noisy = vec![seed_fold(&[quality(5.1, 1.05, 1.0, 120.0)])];
    let high = vec![seed_fold(&[quality(100.0, 1.0, 10.0, 100.0)])];
    let boundary_keys = ranking_keys(&[&boundary_stable, &boundary_noisy, &high]);
    assert!(
        ranking_key_order(&boundary_keys[0], &boundary_keys[1]).is_gt(),
        "two close profits cannot become profit-first only because they cross a global boundary"
    );

    let materially_better = vec![seed_fold(&[quality(130.0, 1.01, 45.0, 120.0)])];
    let materially_stable = vec![seed_fold(&[quality(100.0, 1.40, 20.0, 200.0)])];
    let material_keys = ranking_keys(&[&materially_better, &materially_stable, &distant]);
    assert!(
        ranking_key_order(&material_keys[0], &material_keys[1]).is_gt(),
        "secondary metrics must never move a material profit loser across its dependency edge"
    );
}

/// A field that costs nothing to remove is removed — where "nothing" means less than the
/// precision the fold sums carry.
///
/// The discriminating case is the boundary PAIR around that tolerance, since a cost of exactly
/// zero is decided the same way with or without it: at this fixture's scale the floor is 7e-8, so
/// a cost of 1e-8 must still be free while 1e-6 must not. Both numbers are derived from
/// `improvement_margin`'s stated definition, not read back from `drops`.
///
/// Breakage this pins: replacing the `improvement_margin(scale)` check in
/// `compose.rs:quality_order` with an exact nonzero-profit comparison. A field whose removal costs
/// a float rounding error would then be kept forever, so backward elimination would stop at a
/// needlessly narrow set.
#[test]
fn a_field_that_costs_nothing_is_given_back() {
    let incumbent = profit_scores(&[50.0, 60.0, 70.0]);
    assert!(
        drops(&incumbent, &incumbent),
        "removing a field that changes nothing must be free, so the smaller set wins"
    );
    assert!(
        drops(&incumbent, &profit_scores(&[51.0, 61.0, 71.0])),
        "removing a field that IMPROVES the score must obviously be taken"
    );
    assert!(
        drops(&incumbent, &profit_scores(&[50.0, 60.0, 70.0 - 1e-8])),
        "a cost below the precision the sums carry is not a cost"
    );
    assert!(
        !drops(&incumbent, &profit_scores(&[50.0, 60.0, 70.0 - 1e-6])),
        "a cost above that floor is real and the field must be kept"
    );
    assert!(
        !drops(&incumbent, &profit_scores(&[50.0, 60.0, 40.0])),
        "a field the score actually depends on must be kept"
    );
}

/// A temporarily weak singleton must remain reachable long enough to form a strong pair.
///
/// The oracle is a hand-written score table, independent of the threshold search: every singleton
/// loses money while fields 0+1 together score 30 on every fold. Width two is
/// enough to retain those two best losing branches and discover their synergy.
///
/// Breakage this pins: `compose.rs:beam_candidates` filtering a layer through `accepts` before
/// retaining its frontier. That one-line shortcut would discard every singleton here, so the
/// profitable pair could never be generated and the user-visible composition would stay empty.
#[test]
fn a_weak_singleton_can_survive_to_form_a_strong_pair() {
    let candidate = [true, true, true];
    let is_slot = [false, false, false];
    let scores = |mask: &[bool]| match mask {
        [true, false, false] => profit_scores(&[-1.0, -1.0, -1.0]),
        [false, true, false] => profit_scores(&[-2.0, -2.0, -2.0]),
        [false, false, true] => profit_scores(&[-3.0, -3.0, -3.0]),
        [true, true, false] => profit_scores(&[30.0, 30.0, 30.0]),
        [true, false, true] => profit_scores(&[11.0, 11.0, 11.0]),
        [false, true, true] => profit_scores(&[12.0, 12.0, 12.0]),
        other => panic!("unexpected mask {other:?}"),
    };
    let masks = beam_candidates(&candidate, &is_slot, 0, 2, 2, 2, |mask, _, _, _| {
        Some(scores(mask))
    })
    .expect("the deterministic scorer never cancels");
    let empty_scores = profit_scores(&[0.0, 0.0, 0.0]);
    assert!(
        masks
            .iter()
            .filter(|mask| mask.iter().filter(|chosen| **chosen).count() == 1)
            .all(|mask| !accepts(&empty_scores, &scores(mask))),
        "the fixture must make every retained singleton lose on its own"
    );
    let ranked = masks
        .into_iter()
        .map(|mask| ScoredMask {
            scores: scores(&mask),
            mask,
            key: RankingKey::default(),
        })
        .collect();
    assert_eq!(
        inner_winner(&empty_scores, ranked).as_deref(),
        Some(&[true, true, false][..]),
        "the beam must carry the losing branches into their jointly profitable pair"
    );
}

/// The beam expands only when the eighth-place boundary is genuinely ambiguous.
///
/// The clear fixture has a material profit gap between candidates eight and nine, while the
/// ambiguous fixtures are exact ties. These hand-authored ranked layers independently define the
/// expected widths, including the hard ceiling and the short-layer boundary.
///
/// Breakage this pins: making `compose.rs:adaptive_width` always return eight or always return
/// sixteen. The former loses interacting branches at noisy depths; the latter doubles work even
/// after a later depth has separated the candidates and should contract.
#[test]
fn adaptive_beam_expands_caps_and_contracts_at_each_depth() {
    let scored = |profits: &[f64]| {
        profits
            .iter()
            .map(|profit| ScoredMask {
                mask: vec![false],
                scores: profit_scores(&[*profit, *profit, *profit]),
                key: RankingKey::default(),
            })
            .collect::<Vec<_>>()
    };

    let clear = scored(&[100.0, 99.0, 98.0, 97.0, 96.0, 95.0, 94.0, 93.0, 70.0]);
    assert_eq!(
        adaptive_width(&clear, 8, 16),
        8,
        "a robust eighth-over-ninth win must contract to the fast beam"
    );
    assert_eq!(
        adaptive_width(&scored(&[100.0; 12]), 8, 16),
        12,
        "an ambiguous boundary must retain every available branch below the cap"
    );
    assert_eq!(
        adaptive_width(&scored(&[100.0; 20]), 8, 16),
        16,
        "ambiguity must never exceed the hard beam ceiling"
    );
    assert_eq!(
        adaptive_width(&scored(&[100.0; 5]), 8, 16),
        5,
        "a short layer cannot manufacture nonexistent branches"
    );
}

/// The beam builder must apply the adaptive width rather than only compute it.
///
/// Five equal singleton branches have an ambiguous boundary at width two and therefore retain
/// the configured maximum of four. This drives the production wiring around `adaptive_width`.
///
/// Breakage this pins: replacing `scored.truncate(retained_width)` in
/// `compose.rs:beam_candidates` with `scored.truncate(min_width)`. The pure policy test above
/// would remain green while the actual beam never widened.
#[test]
fn beam_candidates_applies_the_adaptive_width() {
    let candidates = [true; 5];
    let slots = [false; 5];
    let retained = beam_candidates(&candidates, &slots, 0, 1, 2, 4, |_, _, _, _| {
        Some(profit_scores(&[100.0, 100.0, 100.0]))
    })
    .expect("the deterministic scorer never cancels");
    assert_eq!(
        retained.len(),
        4,
        "an ambiguous production layer must expand from two branches to four"
    );
}

/// The outer gate must report the exact path whose mask it selected.
///
/// Every score is hand-written independent evidence. The cases cover a strict subset, all
/// admitted fields, no additional filters, a beam mask equal to all fields, and the zero-admitted
/// boundary where all-fields and no-filters are the same mask.
///
/// Breakage this pins: `compose.rs:gate_choice` labelling an all-fields mask as `ReducedSet` after
/// removing the strict-subset check, or restoring the asymmetric sequence that defaulted to no
/// filters whenever neither the subset nor all-fields beat both alternatives. The thresholds
/// would remain numerically plausible while the new user-facing result line described either a
/// different path or a route another candidate beat on every gate fold.
#[test]
fn the_outer_gate_reports_each_distinct_decision() {
    let no_filters = [false, false, false];
    let all_fields = [true, true, true];
    let subset = [true, false, true];

    let chosen_subset = gate_choice(
        (&no_filters, &profit_scores(&[100.0, 100.0])),
        (&all_fields, &profit_scores(&[110.0, 110.0])),
        (&subset, &profit_scores(&[130.0, 125.0])),
    );
    assert_eq!(
        chosen_subset,
        GateChoice {
            decision: ComposeDecision::ReducedSet,
            mask: subset.to_vec(),
            robust: true,
        },
        "a strict subset that beats both complete alternatives is its own decision"
    );

    let chosen_all = gate_choice(
        (&no_filters, &profit_scores(&[100.0, 100.0])),
        (&all_fields, &profit_scores(&[140.0, 135.0])),
        (&subset, &profit_scores(&[130.0, 120.0])),
    );
    assert_eq!(
        chosen_all,
        GateChoice {
            decision: ComposeDecision::AllAllowedFields,
            mask: all_fields.to_vec(),
            robust: true,
        },
        "all admitted fields must be named when their search wins"
    );

    let chosen_none = gate_choice(
        (&no_filters, &profit_scores(&[100.0, 100.0])),
        (&all_fields, &profit_scores(&[90.0, 90.0])),
        (&subset, &profit_scores(&[95.0, 95.0])),
    );
    assert_eq!(
        chosen_none,
        GateChoice {
            decision: ComposeDecision::NoAdditionalFilters,
            mask: no_filters.to_vec(),
            robust: true,
        },
        "no additional filters is a normal winning path"
    );

    let chosen_non_transitive = gate_choice(
        (&no_filters, &profit_scores(&[100.0, 100.0])),
        (&all_fields, &profit_scores(&[200.0, 0.0])),
        (&subset, &profit_scores(&[120.0, 120.0])),
    );
    assert_eq!(
        chosen_non_transitive,
        GateChoice {
            decision: ComposeDecision::ReducedSet,
            mask: subset.to_vec(),
            robust: true,
        },
        "one inconclusive alternative must not make the gate default to a route the subset beat"
    );

    let beam_reached_all = gate_choice(
        (&no_filters, &profit_scores(&[100.0, 100.0])),
        (&all_fields, &profit_scores(&[130.0, 130.0])),
        (&all_fields, &profit_scores(&[130.0, 130.0])),
    );
    assert_eq!(
        beam_reached_all.decision,
        ComposeDecision::AllAllowedFields,
        "a beam path equal to all fields is not a reduced set"
    );

    let no_admitted = gate_choice(
        (&no_filters, &profit_scores(&[100.0, 100.0])),
        (&no_filters, &profit_scores(&[100.0, 100.0])),
        (&no_filters, &profit_scores(&[100.0, 100.0])),
    );
    assert_eq!(
        no_admitted.decision,
        ComposeDecision::NoAdditionalFilters,
        "with no admitted fields the two identical masks have one unambiguous meaning"
    );
}

/// Composition exists exactly on machines at or above the bar, and never asks for a budget of
/// nothing when it does.
///
/// The oracle is a boundary PAIR either side of the documented core threshold, taken from the
/// bar's own constant rather than a number restated here: one core below it there must be no
/// budget at all, and at it there must be a usable one.
///
/// Breakage this pins six ways. `compose.rs:budget_for` returning a budget below the bar — the
/// feature would run on exactly the machines it was gated off, since a caller decides whether to
/// compose by whether a budget came back. Reducing `folds` below four would make every production
/// call skip composition because two inner and two gate folds are mandatory. Restoring a fixed
/// `max_fields: 6` would silently make larger admitted sets unreachable again. Restoring
/// `beam_width_min: 4` would discard half the competing interactions the widened search promises;
/// losing the 16-branch ceiling would make ambiguity either invisible or unbounded. Reducing
/// `seed_groups` would restore dependence on fewer restart streams. Finally, `ranking_restarts`
/// dividing without its one-restart floor would report "this period has no answer" at low settings;
/// reducing `RANKING_RESTARTS_MAX` below `RESTARTS_MAX / RANKING_RESTART_DIVISOR` would again
/// clamp the user's restart setting before it reaches every Beam candidate.
#[test]
fn composition_has_a_budget_exactly_above_the_core_bar() {
    let bar = super::super::search::HEAVY_SEARCH_MIN_CORES;
    for cores in 0..bar {
        assert!(
            budget_for(cores).is_none(),
            "cores {cores}: below the bar composition must not be offered a budget at all"
        );
    }
    for cores in [bar, bar + 1, 16, 64] {
        let b = budget_for(cores)
            .unwrap_or_else(|| panic!("cores {cores}: at or above the bar there is a budget"));
        assert!(
            b.folds >= 4,
            "cores {cores}: {} folds cannot supply two inner and two gate folds",
            b.folds
        );
        assert_eq!(
            b.max_fields,
            FIELDS.len(),
            "cores {cores}: every admitted field count must be reachable"
        );
        assert_eq!(
            (b.beam_width_min, b.beam_width_max, b.seed_groups),
            (8, 16, 5),
            "cores {cores}: composition must expose its adaptive beam and seed consensus"
        );
        for restarts in [1usize, 2, 7, 100, 20_000] {
            assert!(
                b.ranking_restarts(restarts) >= super::super::RESTARTS_MIN,
                "cores {cores}, restarts {restarts}: ranking budget fell to {}",
                b.ranking_restarts(restarts)
            );
        }
        let full_user_budget = b.ranking_restarts(super::super::RESTARTS_MAX);
        assert_eq!(
            full_user_budget,
            super::super::RESTARTS_MAX / RANKING_RESTART_DIVISOR,
            "a 100k user budget must reach each Beam candidate at the configured divisor"
        );
        assert!(
            full_user_budget < RANKING_RESTARTS_MAX,
            "the ranking guard must not bind inside the accepted user-restart range"
        );
        let scaled = [1_000usize, 10_000, super::super::RESTARTS_MAX]
            .map(|restarts| b.ranking_restarts(restarts));
        assert!(
            scaled.windows(2).all(|pair| pair[0] < pair[1]),
            "larger accepted user budgets must strictly increase ranking evidence"
        );
    }
    // Width is set by what makes the answer trustworthy, not by the hardware, so a bigger machine
    // buys speed rather than a wider search.
    let at_bar = budget_for(bar).expect("a budget at the bar");
    let far_above = budget_for(64).expect("a budget well above the bar");
    assert_eq!(
        (
            at_bar.folds,
            at_bar.max_fields,
            at_bar.beam_width_min,
            at_bar.beam_width_max,
            at_bar.seed_groups,
            at_bar.ranking_restarts(800)
        ),
        (
            far_above.folds,
            far_above.max_fields,
            far_above.beam_width_min,
            far_above.beam_width_max,
            far_above.seed_groups,
            far_above.ranking_restarts(800)
        ),
        "past the bar the budget must stop growing"
    );
}

/// A better retained-trade rate must make a selective filter beat the empty mask.
///
/// The hand-authored evidence starts from an empty baseline paying +1.00 per trade over one
/// hundred trades. The candidate retains forty trades at +1.80 each, so its +32 lift follows
/// directly from `72 - 1.00 * 40`, independently of the production comparator.
///
/// Breakage this pins: changing `compose.rs:lift_order` from `a.lift - b.lift` to
/// `a.profit - b.profit`. The empty mask would win by construction on its +100 total, and the
/// tuner would silently return "no additional filters" despite a materially better filter.
#[test]
fn lift_orders_a_better_per_trade_filter_above_the_empty_mask() {
    let empty = quality_lift(0.0, 100.0, 1.0, 10.0, 100.0, 1.0);
    let candidate = quality_lift(32.0, 72.0, 1.0, 10.0, 40.0, 0.40);
    let empty_scores = vec![seed_fold(&[empty]); 3];
    let candidate_scores = vec![seed_fold(&[candidate]); 3];

    assert!(
        quality_order(candidate, empty).is_gt(),
        "a +1.80 retained-trade rate must beat the empty mask's +1.00 despite a lower sum"
    );
    assert!(
        accepts(&empty_scores, &candidate_scores),
        "three independently stronger folds must accept the selective filter"
    );
}
/// A worse retained-trade rate must make a selective filter lose despite a higher sum.
///
/// The hand-authored empty baseline loses -0.60 per trade over one hundred trades. The candidate
/// loses -0.80 over forty trades, so its -32 total is higher than -60 but its -8 lift follows
/// independently from `-32 - (-0.60 * 40)` and is worse than the baseline.
///
/// Breakage this pins: changing `compose.rs:lift_order` from `a.lift - b.lift` to
/// `a.profit - b.profit`. The absolute-total rule would accept a per-trade-worse filter merely
/// because its smaller retained loss is numerically higher.
#[test]
fn lift_orders_a_worse_per_trade_filter_below_the_empty_mask() {
    let empty = quality_lift(0.0, -60.0, 1.0, 10.0, 100.0, 1.0);
    let candidate = quality_lift(-8.0, -32.0, 1.0, 10.0, 40.0, 0.40);
    let empty_scores = vec![seed_fold(&[empty]); 3];
    let candidate_scores = vec![seed_fold(&[candidate]); 3];

    assert!(
        quality_order(candidate, empty).is_lt(),
        "a -0.80 retained-trade rate must lose to the empty mask's -0.60 rate"
    );
    assert!(
        !accepts(&empty_scores, &candidate_scores),
        "three independently weaker folds must reject the selective filter"
    );
}
/// The best real finalist must reach the outer gate even when none clears the inner control.
///
/// The hand-authored control holds +100 in every fold while the two finalists hold +90 and +80,
/// so every finalist independently fails `accepts`. Their score table still makes the +90 mask
/// the best actual subset for the reserved gate folds to judge.
///
/// Breakage this pins: replacing `compose.rs:inner_winner`'s fallback pool with only `cleared`.
/// The empty pool would return `None`, composition would substitute the empty mask, and the tuner
/// would report "no additional filters" without a reduced-set gate comparison.
#[test]
fn inner_winner_keeps_a_ranked_finalist_when_none_clear_the_control() {
    let control_scores = profit_scores(&[100.0, 100.0, 100.0]);
    let best_mask = vec![true, false, false];
    let ranked = vec![
        ScoredMask {
            scores: profit_scores(&[90.0, 90.0, 90.0]),
            mask: best_mask.clone(),
            key: RankingKey::default(),
        },
        ScoredMask {
            scores: profit_scores(&[80.0, 80.0, 80.0]),
            mask: vec![false, true, false],
            key: RankingKey::default(),
        },
    ];
    assert!(
        ranked
            .iter()
            .all(|candidate| !accepts(&control_scores, &candidate.scores)),
        "the fixture must leave every finalist below the inner control"
    );
    assert_eq!(
        inner_winner(&control_scores, ranked).as_deref(),
        Some(best_mask.as_slice()),
        "an empty cleared pool must still supply the best non-empty finalist to the outer gate"
    );
}
/// The strongest weakest fold must choose the gate finalist over the higher mean.
///
/// Against a zero control, candidate A wins two folds by +100 but collapses to -10 in the third,
/// while candidate B earns +40 in all three. Both independently clear the control; A's mean is
/// higher, but B's +40 weakest fold is the hand-authored stronger unseen-stretch evidence.
///
/// Breakage this pins: replacing `compose.rs:inner_winner`'s `worst_fold` sort closure with
/// `ranked_order`. The higher-mean collapsed candidate would reach the reserved gate folds, where
/// it can turn a robust reduced set into an apparent "no additional filters" result.
#[test]
fn inner_winner_prefers_the_stronger_weakest_fold_over_the_higher_mean() {
    let control_scores = profit_scores(&[0.0, 0.0, 0.0]);
    let collapsed_high_mean = profit_scores(&[100.0, 100.0, -10.0]);
    let steady_lower_mean = profit_scores(&[40.0, 40.0, 40.0]);
    let steady_mask = vec![false, true, false];
    assert!(
        accepts(&control_scores, &collapsed_high_mean)
            && accepts(&control_scores, &steady_lower_mean),
        "both candidates must clear the control before weakest-fold ranking decides between them"
    );
    assert!(
        aggregate_order(summary(&collapsed_high_mean), summary(&steady_lower_mean)).is_gt(),
        "the fixture must give the collapsed candidate the higher mean"
    );
    assert!(
        aggregate_order(
            worst_fold(&steady_lower_mean),
            worst_fold(&collapsed_high_mean)
        )
        .is_gt(),
        "the fixture must give the steady candidate the stronger weakest fold"
    );
    let ranked = vec![
        ScoredMask {
            scores: collapsed_high_mean,
            mask: vec![true, false, false],
            key: RankingKey::default(),
        },
        ScoredMask {
            scores: steady_lower_mean,
            mask: steady_mask.clone(),
            key: RankingKey::default(),
        },
    ];
    assert_eq!(
        inner_winner(&control_scores, ranked).as_deref(),
        Some(steady_mask.as_slice()),
        "the reserved gate must receive the candidate with the stronger weakest fold"
    );
}

/// Three finalists whose weakest folds form a top-spanning cycle, so NO maximal element exists.
///
/// Lift is the only metric that draws a dependency edge, so the fixture puts every adjacent pair
/// inside the risk band — where the intransitive drawdown vote decides — while the outer pair
/// clears it. `strong` beats `weak` on material lift; `mid` beats `strong` and `weak` beats `mid`
/// on the secondary vote. Nothing is maximal.
///
/// Returns:
///     The three candidates in the order `(strong, mid, weak)`, matching the scan order the old
///     fold walked.
fn cycling_finalists() -> Vec<ScoredMask> {
    // 109.7 / 104.9 / 100.0: each neighbouring gap is under the pair's own 5% band while the
    // 9.7 outer gap clears it. Drawdown runs the other way, so the secondary vote reverses the
    // pairs the band tied.
    let fold = |lift: f64, max_dd: f64| {
        vec![seed_fold(&[quality_lift(lift, lift, 1.0, max_dd, 100.0, 1.0)]); 3]
    };
    vec![
        ScoredMask {
            scores: fold(109.7, 30.0),
            mask: vec![true, false, false],
            key: RankingKey::default(),
        },
        ScoredMask {
            scores: fold(104.9, 20.0),
            mask: vec![false, true, false],
            key: RankingKey::default(),
        },
        ScoredMask {
            scores: fold(100.0, 10.0),
            mask: vec![false, false, true],
            key: RankingKey::default(),
        },
    ]
}

/// A finalist pool with no maximal element must still answer, and never with a candidate another
/// finalist materially beats on lift.
///
/// The fixture is asserted to CYCLE before anything is asserted about the winner: without that
/// first block the test would pass over any ordinary pool and prove nothing. The oracle is
/// implementation-independent and is deliberately NOT an identity — under a cycle the identity
/// legitimately follows the documented input-order tie-break, which is why the rotated pool is
/// checked for the same PROPERTY rather than for the same mask.
///
/// Breakage this pins: restoring `compose.rs:inner_winner`'s `reduce` over `aggregate_order`. A
/// fold returns a maximum only when a maximal element exists; on this pool it walks
/// `strong -> mid -> weak` and answers `weak`, which `strong` beats by 9.7 of lift — measured on a
/// real 1289-trade scope, that is exactly the scan-order artifact this selection removes. Handing
/// the same relation to `sort_by` instead aborts the process outright.
#[test]
fn inner_winner_answers_a_pool_with_no_maximal_element() {
    let cycle = cycling_finalists();
    let worst: Vec<Quality> = cycle
        .iter()
        .map(|candidate| worst_fold(&candidate.scores))
        .collect();
    assert!(
        aggregate_order(worst[0], worst[2]).is_gt()
            && aggregate_order(worst[2], worst[1]).is_gt()
            && aggregate_order(worst[1], worst[0]).is_gt(),
        "the fixture must genuinely cycle, or this test proves nothing about an intransitive pool"
    );
    assert!(
        (0..worst.len()).all(|candidate| (0..worst.len()).any(|other| aggregate_order(
            worst[other],
            worst[candidate]
        )
        .is_gt())),
        "every candidate must be beaten by another, so the pool has no maximal element at all"
    );
    assert!(
        lift_order(worst[0], worst[2]).is_gt(),
        "the fixture's outer pair must clear the risk band, or the oracle below cannot fail"
    );

    // A control nothing clears, so the pool is the whole fixture in its written order.
    let control_scores = vec![seed_fold(&[quality_lift(1000.0, 1000.0, 3.0, 5.0, 500.0, 1.0)]); 3];
    for rotation in 0..3 {
        let mut candidates = cycling_finalists();
        candidates.rotate_left(rotation);
        let pool: Vec<(Vec<bool>, Quality)> = candidates
            .iter()
            .map(|candidate| (candidate.mask.clone(), worst_fold(&candidate.scores)))
            .collect();
        let winner = inner_winner(&control_scores, candidates)
            .unwrap_or_else(|| panic!("rotation {rotation}: a non-empty pool must answer"));
        let chosen = pool
            .iter()
            .find(|(mask, _)| *mask == winner)
            .expect("the winner must be one of the finalists")
            .1;
        assert!(
            pool.iter()
                .all(|(_, other)| !lift_order(*other, chosen).is_gt()),
            "rotation {rotation}: no finalist may be sent to the reserved folds while another \
             materially beats it on lift"
        );
    }
}

/// The mean-based tie-break survives, and it lives in the sort above the selection.
///
/// Both candidates have a BYTE-IDENTICAL weakest fold, so the weakest-fold ranking cannot separate
/// them and the answer is decided entirely by the input order the pool was left in. That order is
/// set by `pool.sort_by(ranked_order)` at `compose.rs:inner_winner`, whose deletion — the obvious
/// "this sort is unused now" cleanup — flips this answer to the worse-mean candidate while every
/// other test stays green.
///
/// What it does NOT pin, and must not be read as pinning: that the selection ranks the WEAKEST
/// FOLD at all. On this fixture the mean and the weakest fold agree, so it passes just as well
/// against the old `reduce` over `aggregate_order`. The weakest-fold SELECTION is proven by
/// [`inner_winner_prefers_the_stronger_weakest_fold_over_the_higher_mean`] and by
/// [`inner_winner_ignores_an_unmeasurably_thin_weakest_fold`]; this test only proves what happens
/// once that selection has run out of separating power.
///
/// Breakage this pins, and ONLY this: removing that `pool.sort_by(ranked_order)` line, or removing
/// the `assign_ranking(pool)` that fills the key it reads.
#[test]
fn inner_winner_breaks_an_identical_weakest_fold_by_the_better_mean() {
    let control_scores = profit_scores(&[0.0, 0.0, 0.0]);
    let flat = profit_scores(&[50.0, 50.0, 50.0]);
    let higher_mean = profit_scores(&[50.0, 200.0, 200.0]);
    let higher_mean_mask = vec![false, true, false];
    let (flat_worst, rich_worst) = (worst_fold(&flat), worst_fold(&higher_mean));
    assert_eq!(
        (flat_worst.lift, flat_worst.profit, flat_worst.max_dd),
        (rich_worst.lift, rich_worst.profit, rich_worst.max_dd),
        "the fixture must give both candidates the same weakest fold, or the mean never decides"
    );
    assert!(
        aggregate_order(summary(&higher_mean), summary(&flat)).is_gt(),
        "the fixture must give one candidate the better mean"
    );
    // Written worst-mean FIRST, so only the sort can put the better-mean candidate at index 0.
    let ranked = vec![
        ScoredMask {
            scores: flat,
            mask: vec![true, false, false],
            key: RankingKey::default(),
        },
        ScoredMask {
            scores: higher_mean,
            mask: higher_mean_mask.clone(),
            key: RankingKey::default(),
        },
    ];
    assert_eq!(
        inner_winner(&control_scores, ranked).as_deref(),
        Some(higher_mean_mask.as_slice()),
        "an unbreakable weakest-fold tie must fall to the better mean-ranked candidate"
    );
}

/// A fold too thin to measure must not be the weakest fold a candidate is judged on.
///
/// Candidate A holds a steady +40 lift over 120 trades in all three folds. Candidate B holds +120
/// over 150 trades twice and then a fold that retained FOUR trades at 2% — below both
/// `LIFT_MIN_TRADES` and `LIFT_MIN_RETENTION`, so `quality_order` would make it abstain from its
/// own seed vote. Every number is hand-authored; nothing is derived from the code under test.
///
/// Breakage this pins: ranking single measurements without the retention floor — passing `false`
/// for `single_measurements` at `compose.rs:inner_winner`, or dropping the measurable-fold filter
/// in `compose.rs:worst_fold`. B's weakest fold becomes the four-trade one at -30, `lift_order`
/// draws an edge A -> B, and a candidate leading by +80 of lift on every fold that MEASURED
/// anything is eliminated from the reserved gate by four trades. The mirror of the same flaw
/// promotes a candidate whose best-looking evidence is a 2%-retention fold.
#[test]
fn inner_winner_ignores_an_unmeasurably_thin_weakest_fold() {
    let steady = |lift: f64, trades: f64, retention: f64| {
        vec![seed_fold(&[quality_lift(lift, lift, 1.0, 10.0, trades, retention)]); 3]
    };
    let thin_fold = quality_lift(-30.0, -30.0, 1.0, 10.0, 4.0, 0.02);
    assert!(
        !lift_is_measurable(thin_fold),
        "the fixture's third fold must be BELOW the floor, or this test proves nothing"
    );
    let steady_lower = steady(40.0, 120.0, 0.6);
    let mut stronger_with_thin_fold = steady(120.0, 150.0, 0.75);
    stronger_with_thin_fold[2] = seed_fold(&[thin_fold]);

    assert_eq!(
        worst_fold(&stronger_with_thin_fold).lift,
        120.0,
        "the weakest MEASURED fold is the +120 one; the four-trade fold has no evidence to be \
         weakest with"
    );
    assert!(
        lift_order(worst_fold(&steady_lower), thin_fold).is_gt(),
        "the fixture must let the thin fold lose to the steady candidate, or the pre-fix \
         elimination it reproduces could not happen"
    );

    // A control nothing clears, so the pool is both candidates in the order written below.
    let control_scores = profit_scores(&[1_000.0, 1_000.0, 1_000.0]);
    let thin_mask = vec![false, true, false];
    let ranked = vec![
        ScoredMask {
            scores: steady_lower,
            mask: vec![true, false, false],
            key: RankingKey::default(),
        },
        ScoredMask {
            scores: stronger_with_thin_fold,
            mask: thin_mask.clone(),
            key: RankingKey::default(),
        },
    ];
    assert_eq!(
        inner_winner(&control_scores, ranked).as_deref(),
        Some(thin_mask.as_slice()),
        "an unmeasurably thin fold must not eliminate the candidate that leads every measured fold"
    );
}

/// The aggregate ranking must stay floor-FREE, so the single-measurement floor cannot leak into it.
///
/// One candidate's MEAN retained four trades at 2% while leading on lift by +160 — as a single
/// measurement it would be rejected outright, and as an aggregate it must still take rank 0. That
/// is not a hypothetical: a mean of ten folds under the bar is exactly the case `aggregate_order`
/// documents rejecting a +37-lift set over.
///
/// Breakage this pins: passing `true` for `single_measurements` at `compose.rs:ranking_keys`, or
/// applying `lift_is_measurable` unconditionally inside `ranking_keys_from_qualities`. The beam
/// and the outer gate would then rank on a floor that was only ever meant for one measurement.
#[test]
fn aggregate_ranking_keeps_no_retention_floor() {
    let aggregate = |lift: f64, trades: f64, retention: f64| {
        vec![seed_fold(&[quality_lift(lift, lift, 1.0, 10.0, trades, retention)]); 3]
    };
    let thin_but_leading = aggregate(200.0, 4.0, 0.02);
    let measured_but_trailing = aggregate(40.0, 150.0, 0.75);
    assert!(
        !lift_is_measurable(summary(&thin_but_leading))
            && lift_is_measurable(summary(&measured_but_trailing)),
        "the fixture must straddle the floor, or a leak into the aggregate path would be invisible"
    );
    assert!(
        lift_order(summary(&thin_but_leading), summary(&measured_but_trailing)).is_gt(),
        "the fixture must give the thin aggregate a materially better lift"
    );

    let keys = ranking_keys(&[&thin_but_leading, &measured_but_trailing]);
    assert_eq!(
        keys[0].rank, 0,
        "the aggregate ranking must still answer on lift alone, with no retention floor applied"
    );
}

/// The declined beam set is carried with the gate figure from the GATE folds, and only when there
/// is something to explain.
///
/// The shape is the owner's real one: a set that earned its keep on the folds it was fitted on and
/// gave it back on the two nobody fitted it to. That is why "no additional filters" is right and
/// why the window has to be able to say so.
///
/// Breakage this pins two ways. Swapping the two score sets in
/// `compose.rs:rejected_candidate` — the row would then claim the set lost where it was fitted and
/// won where it was not, inverting the whole explanation while both numbers still look plausible.
/// And dropping any of the three `None` guards: a set that WON would be reported as declined, or
/// an empty mask would be named as a set that was tried.
#[test]
fn a_declined_beam_set_is_carried_with_its_reserved_fold_figure() {
    let subset = vec![true, false, true];
    let no_filters = vec![false, false, false];
    let inner = profit_scores(&[116.41, 116.41, 116.41]);
    let gate = profit_scores(&[-129.91, -129.91]);
    let lost = GateChoice {
        decision: ComposeDecision::NoAdditionalFilters,
        mask: no_filters.clone(),
        robust: true,
    };
    let carried = rejected_candidate(&lost, &subset, &inner, &gate, 3, 2)
        .expect("a real set the gate declined must be carried beside the verdict");
    assert_eq!(
        (
            carried.mask.clone(),
            carried.inner_folds,
            carried.gate_folds
        ),
        (subset.clone(), 3, 2),
        "the declined set must be reported as its own mask over its own fold counts"
    );
    assert!(
        carried.inner_lift > 0.0 && carried.gate_lift < 0.0,
        "inner {} and gate {}: the two lifts must come from their own fold groups, or the row \
         inverts the explanation",
        carried.inner_lift,
        carried.gate_lift
    );

    let won = GateChoice {
        decision: ComposeDecision::ReducedSet,
        mask: subset.clone(),
        robust: true,
    };
    assert!(
        rejected_candidate(&won, &subset, &inner, &gate, 3, 2).is_none(),
        "a set the gate TOOK was not declined and must not be reported as one"
    );
    assert!(
        rejected_candidate(&lost, &no_filters, &inner, &gate, 3, 2).is_none(),
        "the empty mask is not a set that was tried"
    );
    let same = GateChoice {
        decision: ComposeDecision::AllAllowedFields,
        mask: subset.clone(),
        robust: true,
    };
    assert!(
        rejected_candidate(&same, &subset, &inner, &gate, 3, 2).is_none(),
        "a beam set equal to the winning mask was not declined, whatever the path is called"
    );
}

// ─────────────────────────────── real-snapshot diagnostic ───────────────────────────────

/// One exact strategy+core scope out of a real snapshot, in chronological order.
struct Scope {
    /// Stable strategy identifier from the snapshot.
    strategy_id: i64,
    /// Stable core identifier from the snapshot.
    core_uid: i64,
    /// Human-readable scope description.
    label: String,
    /// Chronologically ordered trade profits.
    profits: Vec<f64>,
    /// Chronologically ordered values for every tuner field.
    vals: Vec<Vec<f64>>,
    /// Chronologically ordered close timestamps.
    closes: Vec<i64>,
}

/// Measured result returned by one exact real-snapshot scope.
struct ScopeReport {
    /// Stable strategy identifier.
    strategy_id: i64,
    /// Stable core identifier.
    core_uid: i64,
    /// Holdout tally with no additional field ranges.
    no_filters: Tally,
    /// Holdout tally after admitting every searchable field.
    all_fields: Tally,
    /// Holdout tally of the selected composition.
    composed: Tally,
    /// Time spent inside composition alone, excluding controls and reporting.
    composition_seconds: f64,
}

/// Sum independent scope tallies for aggregate profit-factor reporting.
///
/// Args:
///     tallies: Scope-level chronological tallies.
///
/// Returns:
///     Summed counts, wins, profits, wins/losses, and honestly additive per-scope drawdowns.
fn sum_tallies<'a>(tallies: impl Iterator<Item = &'a Tally>) -> Tally {
    tallies.fold(Tally::default(), |mut total, tally| {
        total.n += tally.n;
        total.wins += tally.wins;
        total.profit += tally.profit;
        total.wsum += tally.wsum;
        total.lsum += tally.lsum;
        total.max_dd += tally.max_dd;
        total
    })
}

/// Positive integer override for the opt-in real-snapshot diagnostic.
///
/// Invalid or absent values use `default`, so an accidental shell value cannot turn a local
/// benchmark into an unbounded run.
///
/// Args:
///     name: Environment variable to read.
///     default: Value used when the variable is absent or invalid.
///
/// Returns:
///     Positive configured value or `default`.
fn bench_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

/// The largest strategy+core scopes in the snapshot, biggest first.
///
/// Args:
///     want: How many scopes to return.
///     min_trades: Smallest scope worth reporting on.
///
/// Returns:
///     One [`Scope`] per strategy+core pair, or an empty vector when no snapshot was named.
fn snapshot_scopes(want: usize, min_trades: usize) -> Vec<Scope> {
    let Ok(path) = std::env::var("MOON_TUNER_BENCH_DB") else {
        return Vec::new();
    };
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let mut pick = conn
        .prepare(
            "SELECT strategyid, core_uid, COUNT(*) n FROM orders_rep \
             WHERE closedate > 0 AND COALESCE(deleted,0) = 0 AND COALESCE(emulator,0) = 0 \
             GROUP BY strategyid, core_uid HAVING n >= ?1 \
             ORDER BY n DESC, strategyid, core_uid LIMIT ?2",
        )
        .expect("the snapshot must carry orders_rep");
    let picked: Vec<(i64, i64, usize)> = pick
        .query_map(rusqlite::params![min_trades as i64, want as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as usize))
        })
        .expect("scope query")
        .filter_map(Result::ok)
        .collect();

    let nf = FIELDS.len();
    let cols = FIELDS
        .iter()
        .map(|s| format!("\"{}\"", s.col))
        .collect::<Vec<_>>()
        .join(", ");
    // The SAME row predicates as the pick above and as the running window's defaults. Without
    // them this measured a row set the product never scans: on the author's own replica every
    // scope but two was entirely emulated or deleted trades, so the unfiltered query answered
    // with thousands of rows that `suggest` would never see.
    let sql = format!(
        "SELECT {cols}, COALESCE(profitbtc,0), COALESCE(closedate,0) FROM orders_rep \
         WHERE closedate > 0 AND COALESCE(deleted,0) = 0 AND COALESCE(emulator,0) = 0 \
         AND strategyid = ?1 AND core_uid = ?2"
    );
    let mut stmt = conn.prepare(&sql).expect("scope rows");
    picked
        .into_iter()
        .map(|(sid, core, n)| {
            let mut rows = stmt
                .query(rusqlite::params![sid, core])
                .expect("scope rows");
            let (mut profits, mut closes) = (Vec::new(), Vec::new());
            let mut vals = vec![Vec::new(); nf];
            while let Some(r) = rows.next().expect("scope row") {
                profits.push(r.get::<_, f64>(nf).expect("profit"));
                closes.push(r.get::<_, i64>(nf + 1).expect("closedate"));
                for (fi, col) in vals.iter_mut().enumerate() {
                    let v = r.get::<_, Option<f64>>(fi).expect("field");
                    col.push(v.filter(|v| v.is_finite()).unwrap_or(0.0));
                }
            }
            let order = super::super::chronological_order(&closes, &profits, &vals);
            let gather = |c: &[f64]| order.iter().map(|t| c[*t]).collect::<Vec<f64>>();
            Scope {
                strategy_id: sid,
                core_uid: core,
                label: format!("strategy {sid} core {core} ({n} trades)"),
                profits: gather(&profits),
                vals: vals.iter().map(|c| gather(c)).collect(),
                closes: order.iter().map(|t| closes[*t]).collect(),
            }
        })
        .collect()
}

/// Measure composition against all-fields joint search on raw-USDT rows from a REAL report.
///
/// Opt-in, because it needs a snapshot and takes minutes:
/// `MOON_TUNER_BENCH_DB=<snapshot> cargo test -p moon-core compose -- --ignored --nocapture`
///
/// This controlled diagnostic evaluates every exact strategy+core pair separately. It
/// intentionally does not reconstruct the Analytics window's period, side, emulator, deleted,
/// liquidation, or Percent-metric filters. Its output measures the selector on the snapshot, not
/// the exact row set of an arbitrary saved UI view.
///
/// The selected profits remain a diagnostic: the fold count and cut points are a judgement call,
/// and freezing today's answer into a threshold would make an evolving snapshot fail spuriously.
/// The harness still asserts that the requested number of separate scopes exists, so a grouping
/// regression cannot silently replace the requested comparison with a shorter printed report.
#[test]
#[ignore]
fn composition_against_all_fields_on_a_real_snapshot() {
    let want = bench_usize("MOON_TUNER_BENCH_SCOPES", 4).max(1);
    let min_trades = bench_usize("MOON_TUNER_BENCH_MIN_TRADES", 600);
    let scopes = snapshot_scopes(want, min_trades);
    assert_eq!(
        scopes.len(),
        want,
        "MOON_TUNER_BENCH_DB must hold {want} separate strategy+core scopes worth tuning"
    );
    let reports: Vec<ScopeReport> = scopes.into_iter().map(report_one_scope).collect();
    let no_filters = sum_tallies(reports.iter().map(|report| &report.no_filters));
    let all_fields = sum_tallies(reports.iter().map(|report| &report.all_fields));
    let composed = sum_tallies(reports.iter().map(|report| &report.composed));
    let ids: Vec<(i64, i64)> = reports
        .iter()
        .map(|report| (report.strategy_id, report.core_uid))
        .collect();
    println!("\n[aggregate] scopes {ids:?}");
    for (label, tally) in [
        ("no filter", &no_filters),
        ("all fields", &all_fields),
        ("composed", &composed),
    ] {
        println!(
            "[aggregate {label}] profit {:+.4}, trades {}, pf {:.4}, sum per-scope dd {:.4}",
            tally.profit,
            tally.n,
            tally.profit_factor(),
            tally.max_dd
        );
    }
    println!(
        "[aggregate timing] composition {:.2}s",
        reports
            .iter()
            .map(|report| report.composition_seconds)
            .sum::<f64>()
    );
}

/// Run no-filters, all-fields, and composition over one exact tuner scope.
///
/// Args:
///     scope: Chronological rows for one exact strategy+core pair.
///
/// Returns:
///     Holdout control/composition metrics and composition-only runtime for aggregation.
fn report_one_scope(scope: Scope) -> ScopeReport {
    let started = Instant::now();
    let Scope {
        strategy_id,
        core_uid,
        label,
        profits,
        vals,
        closes,
    } = scope;
    let total = profits.len();
    let cols = Arc::new(Columns { profits, vals });
    // Match the UI's "Search all" boundary: fields with nowhere to save a threshold remain
    // excluded unless a user explicitly enables them one by one.
    let locked: Vec<Option<(Option<f64>, Option<f64>)>> = FIELDS
        .iter()
        .map(|field| {
            if field.mapped() {
                None
            } else {
                Some((None, None))
            }
        })
        .collect();
    let ne = bench_usize("MOON_TUNER_BENCH_EDGES", 32)
        .clamp(super::super::EDGES_MIN, super::super::edges_max());
    let restarts = bench_usize("MOON_TUNER_BENCH_RESTARTS", 2_000)
        .clamp(super::super::RESTARTS_MIN, super::super::restarts_max());
    let train_pct = bench_usize("MOON_TUNER_BENCH_TRAIN_PCT", 90).clamp(1, 99);
    let train_n = super::super::train_split(&closes, train_pct as f64 / 100.0);
    assert!(
        train_n < total,
        "the real-snapshot diagnostic requires a non-empty holdout"
    );
    let min_n = (train_n / 10).max(1);
    let handle = SearchHandle::new();
    let full = Search::new(cols.clone(), &locked, slot_flags(), min_n, ne, train_n)
        .expect("the snapshot is larger than its min_n");

    let report = |what: &str, mask: &[bool]| {
        let out = full
            .run_masked(mask, restarts, 0x5EED, &handle)
            .expect("an uncancelled search answers");
        let applied = full.applied_ranges(&out.sel, true);
        let (tr, ho) = (
            full.tally(&applied, 0..train_n),
            full.tally(&applied, train_n..total),
        );
        let names: Vec<&str> = applied.iter().map(|(fi, _, _)| FIELDS[*fi].col).collect();
        println!(
            "[{what}] fields {} {:?}\n         in sample {:+.4} over {}\n         \
             OUT OF SAMPLE {:+.4} over {} (pf {:.2}, dd {:.4})",
            names.len(),
            names,
            tr.profit,
            tr.n,
            ho.profit,
            ho.n,
            ho.profit_factor(),
            ho.max_dd
        );
        ho
    };

    println!(
        "\n[scope] {label} - train {train_n}, holdout {}",
        total - train_n
    );
    // The base rate: what the period pays with no filter at all. Without it the two searches
    // cannot be read — "out of sample +33" means one thing against a baseline of -200 and the
    // opposite against a baseline of +400.
    let base = report("no filter", &vec![false; FIELDS.len()]);
    let all_fields = report("all fields", full.free_mask());

    let budget = budget_for(super::super::search::logical_cores()).expect(
        "the composition diagnostic needs a machine the feature actually runs on - see \
         HEAVY_SEARCH_MIN_CORES",
    );
    let beam_width_min =
        bench_usize("MOON_TUNER_BENCH_BEAM_MIN", budget.beam_width_min).clamp(1, FIELDS.len());
    let beam_width_max = bench_usize("MOON_TUNER_BENCH_BEAM_MAX", budget.beam_width_max)
        .clamp(beam_width_min, FIELDS.len());
    let seed_groups =
        bench_usize("MOON_TUNER_BENCH_SEEDS", budget.seed_groups).min(restarts.max(1));
    println!(
        "[budget] cores {} -> ranking restarts {}, folds {}, max fields {}, beam {}-{}, seeds {}",
        budget.cores,
        budget.ranking_restarts(restarts),
        budget.folds,
        budget.max_fields,
        beam_width_min,
        beam_width_max,
        seed_groups
    );
    // Through the production path, not a local rebuild of it: a diagnostic that cuts its own
    // folds can report on a geometry the product no longer builds.
    let folds = super::super::build_folds(
        &cols,
        &locked,
        &slot_flags(),
        &closes,
        min_n,
        ne,
        train_n,
        budget.folds,
    );
    println!(
        "[folds] {:?}",
        folds
            .iter()
            .map(|f| (f.search.train_n(), f.validate.end))
            .collect::<Vec<_>>()
    );
    // What each control RETAINS on every fold's validation stretch, which is the quantity the
    // retention floor is stated against. Printed because a floor that silently becomes the
    // binding selectivity bar looks exactly like a selector that found nothing.
    for (what, mask) in [
        ("no filter", vec![false; FIELDS.len()]),
        ("all fields", full.free_mask().to_vec()),
    ] {
        let retained: Vec<String> = folds
            .iter()
            .map(|f| {
                let out = f
                    .search
                    .run_masked(&mask, budget.ranking_restarts(restarts), 0x5EED, &handle)
                    .expect("an uncancelled search answers");
                let applied = f.search.applied_ranges(&out.sel, true);
                let kept = f.search.tally(&applied, f.validate.clone()).n;
                let base = f.search.tally(&[], f.validate.clone()).n;
                format!("{kept}/{base}")
            })
            .collect();
        println!("[retention {what}] {}", retained.join(" "));
    }
    let p = ComposeParams {
        ranking_restarts: budget.ranking_restarts(restarts),
        gate_restarts: restarts,
        seed: 0x5EED,
        round: true,
        max_fields: budget.max_fields,
        beam_width_min,
        beam_width_max,
        seed_groups,
    };
    // The vote, laid out. A composition that answers "no additional filters" is indistinguishable
    // from one that never had a measurable candidate, so the aggregates the decision is actually
    // taken on are printed rather than inferred.
    {
        let (inner, gate) = folds.split_at(folds.len() - 2);
        let no_filters = vec![false; FIELDS.len()];
        let all_fields = full.free_mask().to_vec();
        // An arbitrary field set named on the command line, so a set a previous run produced can
        // be re-scored under the CURRENT predicate. Without it, "the selector no longer picks
        // that set" cannot be told apart from "that set never had the evidence".
        let named: Vec<bool> = std::env::var("MOON_TUNER_BENCH_FIELDS")
            .ok()
            .map(|names| {
                let wanted: Vec<&str> = names.split(',').map(str::trim).collect();
                FIELDS
                    .iter()
                    .zip(full.free_mask())
                    .map(|(field, free)| *free && wanted.contains(&field.col))
                    .collect()
            })
            .unwrap_or_default();
        for (label, set, restarts_for) in [
            ("inner", inner, p.ranking_restarts),
            ("gate", gate, p.gate_restarts),
        ] {
            let mut masks = vec![("no filter", &no_filters), ("all fields", &all_fields)];
            if named.iter().any(|chosen| *chosen) {
                masks.push(("named set", &named));
            }
            for (what, mask) in masks {
                let Some(scores) = score_set(set, mask, &p, restarts_for, 1, &handle) else {
                    continue;
                };
                let q = summary(&scores);
                let per_fold: Vec<String> = scores
                    .iter()
                    .map(|f| {
                        let m = mean_quality(f.seeds.iter().copied());
                        format!("{:.0}tr/{:.0}%", m.trades, m.retention * 100.0)
                    })
                    .collect();
                println!(
                    "[vote {label} {what}] lift {:+.2}, profit {:+.2}, trades {:.1},                      retention {:.1}%, pf {:.2}, dd {:.2} | folds {}",
                    q.lift,
                    q.profit,
                    q.trades,
                    q.retention * 100.0,
                    q.profit_factor,
                    q.max_dd,
                    per_fold.join(" ")
                );
            }
        }
    }
    let composition_started = Instant::now();
    let out = compose(&folds, &p, &handle).expect("an uncancelled composition answers");
    let composition_seconds = composition_started.elapsed().as_secs_f64();
    let support: Vec<(&str, u8)> = out
        .chosen
        .iter()
        .enumerate()
        .filter(|(_, c)| **c)
        .map(|(fi, _)| (FIELDS[fi].col, out.support[fi]))
        .collect();
    println!(
        "[composition] decision {:?}; support out of {} folds: {support:?}",
        out.decision, out.folds
    );
    let composed = report("composed", &out.chosen);
    println!(
        "[verdict] out of sample vs the no-filter baseline {:+.4}: \
         all fields {:+.4}, composed {:+.4}; composition {:.2}s, elapsed {:.2?}",
        base.profit,
        all_fields.profit - base.profit,
        composed.profit - base.profit,
        composition_seconds,
        started.elapsed()
    );
    ScopeReport {
        strategy_id,
        core_uid,
        no_filters: base,
        all_fields,
        composed,
        composition_seconds,
    }
}

// ─────────────────────────────── fold isolation ───────────────────────────────

/// A fold's thresholds must be fitted without any knowledge of the rows it is then measured on.
///
/// Rewriting the fold's VALIDATION rows onto a scale its fitting prefix never held moves every
/// quantile edge that stretch would contribute. If the fold saw it, its chosen ranges move with
/// it. The oracle is that rewrite — a controlled mutation of the fixture, owing nothing to the
/// code under test — and the assertion is that the fitted side does not budge while the measured
/// side does.
///
/// The folds come from `mod.rs:build_folds`, the production path, and NOT from a fixture helper
/// of this file's own. That is the whole point: what is under test is the `fit_end` that function
/// hands each fold's `Search`, so a test that assembles its own folds would prove only that
/// `Search` honours the window it was given — which was never in doubt, and which leaves the
/// argument that decides the leak untested.
///
/// Breakage this pins: `mod.rs:build_folds` constructing a fold's `Search` with the whole fitting
/// region as its `train_n` instead of that fold's own `fit_end` — the obvious "why derive the
/// edges K times" optimization. Nothing would crash and every number would still look plausible;
/// the fold scores would simply get better, the composed set would be chosen on the stretches
/// meant to judge it, and the figure the window prints as out-of-sample would be a fitted one
/// wearing an out-of-sample label.
#[test]
fn a_folds_thresholds_are_fitted_without_its_validation_rows() {
    let (profits, vals, closes) = sample(600, 20260729);
    // By hand from the anchored geometry — `600 * (2 + j) / 4` is 300 then 450, and the final
    // endpoint is the region itself: folds (300, 450) and (450, 600).
    let (train_n, k) = (600usize, 2usize);
    let (fit_end, validate_end) = (300usize, 450usize);
    let mut rewritten = vals.clone();
    for col in rewritten.iter_mut() {
        for v in col[fit_end..validate_end].iter_mut() {
            *v = *v * 3.0 + 1000.0;
        }
    }
    let build = |vals: Vec<Vec<f64>>| {
        let cols = Arc::new(Columns {
            profits: profits.clone(),
            vals,
        });
        let locked = vec![None; FIELDS.len()];
        super::super::build_folds(&cols, &locked, &slot_flags(), &closes, 10, 16, train_n, k)
    };
    let built = build(vals);
    let altered_folds = build(rewritten);
    assert_eq!(
        built.len(),
        2,
        "the fixture must produce the two folds above"
    );
    let (plain, altered) = (&built[0], &altered_folds[0]);
    // The window the fold was actually built over, read back from the production path rather
    // than from the fixture: under the named mutation this is the whole region.
    assert_eq!(
        (plain.search.train_n(), plain.validate.clone()),
        (fit_end, fit_end..validate_end),
        "the first fold must fit on its own prefix and be measured on the stretch after it"
    );

    let mask = vec![true; FIELDS.len()];
    let handle = SearchHandle::new();
    let run = |f: &Fold| {
        let out = f
            .search
            .run_masked(&mask, 4, 0xF01D, &handle)
            .expect("an uncancelled search over a sufficient fixture always answers");
        let applied = f.search.applied_ranges(&out.sel, false);
        let profit = f.search.tally(&applied, f.validate.clone()).profit;
        (applied, profit)
    };
    let (plain_ranges, plain_profit) = run(plain);
    let (altered_ranges, altered_profit) = run(altered);

    assert_eq!(
        plain_ranges, altered_ranges,
        "the fitted ranges moved, so the fold read the rows it is measured on"
    );
    assert_ne!(
        plain_profit.to_bits(),
        altered_profit.to_bits(),
        "the fixture must actually change what the validation rows are worth, or this test \
         cannot distinguish a fold that peeked from one that did not"
    );
}

// ─────────────────────── one pinned real scope, end to end ───────────────────────

/// Signed integer override for the pinned-scope diagnostic.
///
/// Strategy identifiers are hashes and routinely negative, so the unsigned reader above cannot
/// carry one. An absent or unparseable value answers `None`, which the diagnostic reports as a
/// missing scope rather than silently benchmarking a different one.
///
/// Args:
///     name: Environment variable to read.
///
/// Returns:
///     The parsed identifier, or `None`.
fn bench_i64(name: &str) -> Option<i64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

/// Load ONE exact strategy+core scope, filtered the way the running window filters it.
///
/// The scope query mirrors `analytics::Query`'s own row predicates for the app defaults — closed
/// trades only, `deleted = 0`, real trades only (`emulator = 0`) — and reads `profitbtc`, which
/// is what `unified_from_mode` projects as `pnl` under `ProjectionMode::Native`. Without those
/// predicates the diagnostic measures a row set the product never scans.
///
/// Args:
///     strategy_id: Stable strategy identifier.
///     core_uid: Stable core identifier.
///
/// Returns:
///     The chronologically ordered scope, or `None` when no snapshot was named.
fn pinned_scope(strategy_id: i64, core_uid: i64) -> Option<Scope> {
    let path = std::env::var("MOON_TUNER_BENCH_DB").ok()?;
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    let nf = FIELDS.len();
    let cols = FIELDS
        .iter()
        .map(|s| format!("\"{}\"", s.col))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {cols}, COALESCE(profitbtc,0), COALESCE(closedate,0) FROM orders_rep \
         WHERE closedate > 0 AND COALESCE(deleted,0) = 0 AND COALESCE(emulator,0) = 0 \
         AND strategyid = ?1 AND core_uid = ?2"
    );
    let mut stmt = conn.prepare(&sql).expect("scope rows");
    let mut rows = stmt
        .query(rusqlite::params![strategy_id, core_uid])
        .expect("scope rows");
    let (mut profits, mut closes) = (Vec::new(), Vec::new());
    let mut vals = vec![Vec::new(); nf];
    while let Some(r) = rows.next().expect("scope row") {
        profits.push(r.get::<_, f64>(nf).expect("profit"));
        closes.push(r.get::<_, i64>(nf + 1).expect("closedate"));
        for (fi, col) in vals.iter_mut().enumerate() {
            let v = r.get::<_, Option<f64>>(fi).expect("field");
            col.push(v.filter(|v| v.is_finite()).unwrap_or(0.0));
        }
    }
    let n = profits.len();
    let order = super::super::chronological_order(&closes, &profits, &vals);
    let gather = |c: &[f64]| order.iter().map(|t| c[*t]).collect::<Vec<f64>>();
    Some(Scope {
        strategy_id,
        core_uid,
        label: format!("strategy {strategy_id} core {core_uid} ({n} trades)"),
        profits: gather(&profits),
        vals: vals.iter().map(|c| gather(c)).collect(),
        closes: order.iter().map(|t| closes[*t]).collect(),
    })
}

/// Reproduce ONE user-reported scope on both paths: the plain threshold search and composition.
///
/// Opt-in, because it needs a real snapshot and takes minutes:
/// `MOON_TUNER_BENCH_DB=<snapshot> MOON_TUNER_BENCH_STRATEGY=<id> MOON_TUNER_BENCH_CORE=<uid>
/// cargo test -p moon-core pinned_scope -- --ignored --nocapture`
///
/// The point of a PINNED scope, next to the aggregate diagnostic above, is that a report from the
/// running window names one strategy on one core. Comparing against the snapshot's largest scopes
/// answers a different question, and a selector regression that only shows up on the scope a user
/// actually tunes would never appear in that aggregate.
///
/// The comparison itself is PRINTED, never asserted, exactly as in the aggregate diagnostic above:
/// the fold count and cut points are a judgement call, and freezing today's profits into a
/// threshold would make an evolving snapshot fail spuriously. It is therefore not a behavioural
/// oracle and pins no breakage — it would pass against any selector. The one assertion it does
/// carry is the fixture guard: a pinned scope that resolved to zero rows would print an empty
/// report that reads exactly like a run in which composition found nothing.
#[test]
#[ignore]
fn a_pinned_scope_compares_composition_against_the_plain_search() {
    let (Some(strategy_id), Some(core_uid)) = (
        bench_i64("MOON_TUNER_BENCH_STRATEGY"),
        bench_i64("MOON_TUNER_BENCH_CORE"),
    ) else {
        println!("[x] MOON_TUNER_BENCH_STRATEGY / MOON_TUNER_BENCH_CORE unset - nothing pinned");
        return;
    };
    let Some(scope) = pinned_scope(strategy_id, core_uid) else {
        println!("[x] MOON_TUNER_BENCH_DB unset - the pinned diagnostic needs a snapshot");
        return;
    };
    assert!(
        !scope.profits.is_empty(),
        "the pinned scope must hold trades - check the strategy and core identifiers"
    );
    report_one_scope(scope);
}

/// How stable is ONE plain-search answer? Re-run the all-fields path across several base seeds.
///
/// Opt-in, and cheap next to composition — a plain search is one fan-out, not thousands:
/// `MOON_TUNER_BENCH_DB=<snapshot> MOON_TUNER_BENCH_STRATEGY=<id> MOON_TUNER_BENCH_CORE=<uid>
/// cargo test -p moon-core plain_search_spread -- --ignored --nocapture`
///
/// This exists to keep a single plain-search figure from being read as a target. The plain search
/// maximizes IN-SAMPLE profit over a hundred thousand random restarts and reports whatever the
/// winning restart happens to earn on the holdout; that held-out number was never optimized for
/// and never averaged, so two seeds over the same rows can disagree by more than any selector
/// change. Composition's answer has to be judged against that SPREAD, not against one draw of it.
///
/// Prints, never asserts, and deliberately carries no oracle at all: the spread is a property of
/// the snapshot rather than of any selector, so it pins no breakage and would pass against any
/// version of this code. Read it as the measuring stick the other numbers are judged against.
#[test]
#[ignore]
fn a_plain_search_spread_shows_how_much_one_seed_decides() {
    let (Some(strategy_id), Some(core_uid)) = (
        bench_i64("MOON_TUNER_BENCH_STRATEGY"),
        bench_i64("MOON_TUNER_BENCH_CORE"),
    ) else {
        println!("[x] MOON_TUNER_BENCH_STRATEGY / MOON_TUNER_BENCH_CORE unset - nothing pinned");
        return;
    };
    let Some(scope) = pinned_scope(strategy_id, core_uid) else {
        println!("[x] MOON_TUNER_BENCH_DB unset - the spread diagnostic needs a snapshot");
        return;
    };
    let total = scope.profits.len();
    let closes = scope.closes;
    let cols = Arc::new(Columns {
        profits: scope.profits,
        vals: scope.vals,
    });
    let locked: Vec<Option<(Option<f64>, Option<f64>)>> = FIELDS
        .iter()
        .map(|field| {
            if field.mapped() {
                None
            } else {
                Some((None, None))
            }
        })
        .collect();
    let ne = bench_usize("MOON_TUNER_BENCH_EDGES", 32)
        .clamp(super::super::EDGES_MIN, super::super::edges_max());
    let restarts = bench_usize("MOON_TUNER_BENCH_RESTARTS", 2_000)
        .clamp(super::super::RESTARTS_MIN, super::super::restarts_max());
    let train_pct = bench_usize("MOON_TUNER_BENCH_TRAIN_PCT", 90).clamp(1, 99);
    let train_n = super::super::train_split(&closes, train_pct as f64 / 100.0);
    let seeds = bench_usize("MOON_TUNER_BENCH_SPREAD_SEEDS", 8);
    let handle = SearchHandle::new();
    let full = Search::new(
        cols,
        &locked,
        slot_flags(),
        (train_n / 10).max(1),
        ne,
        train_n,
    )
    .expect("the snapshot is larger than its min_n");
    println!(
        "\n[spread] {} - train {train_n}, holdout {}, restarts {restarts}, edges {ne}",
        scope.label,
        total - train_n
    );
    let mut holdouts = Vec::with_capacity(seeds);
    for i in 0..seeds {
        // Seeds spelled out from the index rather than drawn: the whole point is a run another
        // machine can repeat and get the same spread.
        let seed = 0x5EED_u64.wrapping_add(i as u64 * 0x9E37_79B9_7F4A_7C15);
        let out = full
            .run_masked(full.free_mask(), restarts, seed, &handle)
            .expect("an uncancelled search answers");
        let applied = full.applied_ranges(&out.sel, true);
        let (tr, ho) = (
            full.tally(&applied, 0..train_n),
            full.tally(&applied, train_n..total),
        );
        println!(
            "[seed {i}] fields {} -> in sample {:+.4} over {}, OUT OF SAMPLE {:+.4} over {}",
            applied.len(),
            tr.profit,
            tr.n,
            ho.profit,
            ho.n
        );
        holdouts.push(ho.profit);
    }
    let mean = holdouts.iter().sum::<f64>() / holdouts.len().max(1) as f64;
    let lo = holdouts.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = holdouts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let sd = (holdouts.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
        / holdouts.len().max(1) as f64)
        .sqrt();
    println!(
        "[spread] out of sample over {} seeds: mean {mean:+.4}, sd {sd:.4}, min {lo:+.4}, max {hi:+.4}",
        holdouts.len()
    );
}
