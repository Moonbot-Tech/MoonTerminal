//! Tests for the scan side of the threshold search: chronological ordering, the train/holdout
//! split, and the walk-forward folds cut inside the fitting region.
//!
//! Both are pure functions over the scanned columns, so they are checked here directly rather
//! than through a database — the SQL they sit next to is exercised by the tuner's own DB tests.

use super::{
    ComposeDecision, EDGES_MAX, EDGES_MAX_LIGHT, RESTARTS_MAX, RESTARTS_MAX_LIGHT, RESTARTS_MIN,
    chronological_order, compose, composed_set, edges_max_for, fold_cuts, restarts_max_for,
    train_split,
};
use crate::db::tuner::FIELDS;

/// Trades with distinct timestamps, so a split can land anywhere.
fn distinct(n: usize) -> Vec<i64> {
    (0..n as i64).collect()
}

/// A split must leave trades on BOTH sides, whatever fraction is asked for.
///
/// The oracle is the definition itself, applied at the edges: the two figures must sum to the
/// sample and neither may be empty once a split was requested at all.
///
/// Breakage this pins: dropping the `clamp(1, n - 1)` in `threshold_search/mod.rs:train_split`.
/// A 95% split of a 20-trade scope would then hold back a single trade — or, with rounding, none
/// at all — and the tuner would print an "out of sample" verdict computed over nothing while
/// looking exactly like a real one.
#[test]
fn a_requested_split_always_leaves_trades_on_both_sides() {
    for n in [2usize, 3, 7, 20, 1000] {
        for pct in [1u32, 5, 50, 70, 95, 99] {
            let train = train_split(&distinct(n), f64::from(pct) / 100.0);
            assert!(
                train >= 1 && train < n,
                "n={n} pct={pct}: train part {train} leaves one side empty"
            );
        }
    }
}

/// Asking for the whole period, or for nonsense, must mean "no split".
///
/// Breakage: making `train_frac` a fraction the caller must pre-clamp, so a default of 1.0 (or a
/// NaN out of a parsed setting) silently holds part of the period back and the suggestion starts
/// answering for a period the user never restricted.
#[test]
fn an_unsplit_search_keeps_the_whole_sample() {
    assert_eq!(train_split(&distinct(100), 1.0), 100);
    assert_eq!(train_split(&distinct(100), 1.5), 100);
    assert_eq!(train_split(&distinct(100), f64::NAN), 100);
    // Too few trades to divide at all.
    assert_eq!(train_split(&distinct(1), 0.5), 1);
    assert_eq!(train_split(&[], 0.5), 0);
}

/// Powerful machines accept depth 512 while the protected tier remains capped at 128.
///
/// Breakage this pins: reverting `threshold_search/mod.rs:EDGES_MAX` to 256 after exposing 512 in
/// the UI. The joint search would silently clamp the selected value and display a setting it never
/// executes.
#[test]
fn machine_depth_ceilings_match_the_user_visible_policy() {
    assert_eq!(edges_max_for(true), 512);
    assert_eq!(edges_max_for(false), 128);
    assert_eq!(edges_max_for(true), EDGES_MAX);
    assert_eq!(edges_max_for(false), EDGES_MAX_LIGHT);
}

/// The split must never cut through trades that share a close timestamp.
///
/// Within one timestamp the sample is ordered by PROFIT, because that is the only total order
/// available. A boundary landing inside such a group would therefore divide it by the very
/// quantity the holdout is meant to judge — losses to one side, wins to the other — and the
/// out-of-sample figure would be decided by the sort rather than by the suggested ranges.
///
/// The oracle is the timestamps themselves: whatever index comes back must sit where the
/// timestamp actually changes.
///
/// Breakage this pins: removing the snap from `threshold_search/mod.rs:train_split` and
/// returning the raw proportional index. With ten same-second trades straddling the boundary,
/// five losses would deterministically land in train and five wins in the holdout, and the
/// feature would report a healthy out-of-sample result it manufactured itself.
#[test]
fn a_split_never_cuts_a_group_of_equally_timed_trades() {
    // Twenty trades, of which rows 5..15 all closed in the same second.
    let closes: Vec<i64> = (0..20)
        .map(|k: i64| if (5..15).contains(&k) { 100 } else { k })
        .collect();
    for pct in [30u32, 40, 50, 60, 70] {
        let train = train_split(&closes, f64::from(pct) / 100.0);
        assert!(
            train >= 1 && train < closes.len(),
            "pct={pct}: split at {train} leaves one side empty"
        );
        assert_ne!(
            closes[train],
            closes[train - 1],
            "pct={pct}: split at {train} cuts through one timestamp"
        );
    }
}

/// The walk-forward folds must be ANCHORED: every fold fits from the first trade, the windows
/// tile the trailing half in order without overlapping, and the last one ends exactly at the
/// fitting boundary.
///
/// The expected boundaries are derived by hand from the stated geometry — with 12 trades and 3
/// folds, `12 * (3 + j) / 6` gives 6, 8, 10 and the final endpoint is 12 — rather than read back
/// from the function. That is what makes this more than a shape check: a plausible wrong formula
/// such as `train_n * j / k` yields 0, 4, 8 and would satisfy every "inside the region, ordered,
/// no timestamp split" predicate while measuring folds on stretches they had already fitted.
///
/// Breakage this pins: `threshold_search/mod.rs:fold_cuts` changing its cut formula, letting a
/// window overlap its neighbour, or snapping the final endpoint — which would either drop the
/// last group of trades or reach past the fitting region into the held-back tail.
#[test]
fn folds_are_anchored_and_tile_the_second_half_in_order() {
    let closes = distinct(12);
    let cuts = fold_cuts(&closes, 12, 3);
    assert_eq!(
        cuts,
        vec![(6, 8), (8, 10), (10, 12)],
        "the anchored geometry must place the fits at half the region and tile the rest"
    );
    // Every fold is measured strictly after what it fitted on, and nothing reaches past the
    // fitting boundary into the tail the user is shown as out-of-sample.
    let wide = fold_cuts(&distinct(100), 60, 3);
    assert_eq!(
        wide.len(),
        3,
        "a region this wide must yield every fold asked for"
    );
    for (fit_end, validate_end) in wide {
        assert!(
            0 < fit_end && fit_end < validate_end && validate_end <= 60,
            "fold ({fit_end}, {validate_end}) escapes the fitting region"
        );
    }
}

/// Folds that cannot be cut must be dropped, not returned degenerate.
///
/// Breakage this pins: `fold_cuts` returning windows it could not honestly place — an empty
/// validation window scores every candidate set at exactly zero, so composition would compare
/// zeroes and accept whichever field it happened to reach first.
#[test]
fn folds_that_cannot_be_cut_are_dropped() {
    // One timestamp for the whole region: there is no boundary to cut between.
    assert!(
        fold_cuts(&vec![7i64; 40], 40, 3).is_empty(),
        "a region closed within one timestamp cannot be cut into folds"
    );
    // Too few trades for the requested folds: whatever survives must still be well formed.
    // The counts are derived by hand from the same geometry — with `n` trades and `k` folds the
    // cuts are `n * (k + j) / 2k`, deduplicated — so a silently empty result cannot pass as one
    // that was merely well formed.
    for (n, k, kept) in [(2usize, 3usize, 1usize), (3, 3, 2), (4, 3, 2), (5, 2, 2)] {
        let cuts = fold_cuts(&distinct(n), n, k);
        assert_eq!(
            cuts.len(),
            kept,
            "n={n} k={k}: wrong number of folds survived"
        );
        for (fit_end, validate_end) in cuts {
            assert!(
                0 < fit_end && fit_end < validate_end && validate_end <= n,
                "n={n} k={k}: degenerate fold ({fit_end}, {validate_end})"
            );
        }
    }
    assert!(
        fold_cuts(&distinct(10), 10, 0).is_empty(),
        "no folds asked for"
    );
}

/// A machine below the heavy-search bar accepts strictly fewer restarts, and both ceilings stay
/// inside what the search will run.
///
/// Both branches are exercised in one run through [`restarts_max_for`]; a real machine only ever
/// takes one of them, so without that the light ceiling would be unchecked on a development
/// machine and the heavy one unchecked in CI.
///
/// Breakage this pins: reducing the heavy branch in `mod.rs:RESTARTS_MAX` back to 50,000,
/// returning the same ceiling for both classes, or swapping them. The first would silently remove
/// the requested 100k range; the others would let a small machine spend minutes on a knob it has
/// no cores to spare for — which is the whole reason the two ceilings exist.
#[test]
fn a_small_machine_accepts_fewer_restarts_than_a_big_one() {
    // Read through the accessor, not off the constants: what must hold is a property of the
    // POLICY, and two literals compared with each other is a statement about neither branch.
    let (light, heavy) = (restarts_max_for(false), restarts_max_for(true));
    assert!(
        light < heavy,
        "the bar must actually withhold restarts: {light} against {heavy}"
    );
    assert!(
        light >= RESTARTS_MIN,
        "the light ceiling must still leave a usable range: {light}"
    );
    assert_eq!(
        heavy, 100_000,
        "the capable-machine quality dial must reach the requested 100k boundary"
    );
    assert_eq!(
        light, 10_000,
        "raising the capable-machine ceiling must not enlarge the light tier"
    );
    assert_eq!(heavy, RESTARTS_MAX);
    assert_eq!(light, RESTARTS_MAX_LIGHT);
}

/// A sample that closed entirely within one timestamp has no later period to hold back.
#[test]
fn a_sample_with_one_timestamp_cannot_be_split() {
    assert_eq!(train_split(&[42; 10], 0.7), 10);
}

/// The scan's row order must be chronological and TOTAL, so equally timed rows cannot reorder
/// between runs.
///
/// The oracle is a fixture whose rows are deliberately shuffled and share timestamps: the
/// expected permutation is derived by hand from the ordering rule, not read back from the code.
///
/// Breakage this pins: dropping the `then_with` tie-break chain in
/// `threshold_search/mod.rs:chronological_order`. Rows sharing a `closedate` would then keep
/// whatever order SQLite handed over, which decides both which side of the train/holdout cut
/// they land on and the drawdown of the part they land in — so the same search on the same data
/// would quietly answer differently between runs.
#[test]
fn equally_timed_rows_are_ordered_by_their_own_values() {
    // Rows 0 and 2 share a timestamp and a profit and are separated ONLY by their field value;
    // rows 1 and 3 share a timestamp and a field value and are separated ONLY by profit. Each
    // pair is also written in the order a sort with that link missing would leave it in, so
    // neither link can be dropped without the expected permutation changing.
    let closes = vec![200i64, 100, 200, 100];
    let profits = vec![5.0, 2.0, 5.0, -1.0];
    let vals = vec![vec![9.0, 0.0, 4.0, 0.0]];

    // By hand: closedate 100 first — of those, profit -1 before +2, so row 3 then row 1. Then
    // closedate 200 — equal profits, so the field value decides: 4.0 before 9.0, row 2 then 0.
    assert_eq!(
        chronological_order(&closes, &profits, &vals),
        vec![3, 1, 2, 0]
    );
}

/// The declined set is named from its OWN mask, not from the ranges the refit applied.
///
/// `ComposedSet::fields` is deliberately keyed to the applied ranges, and under
/// `NoAdditionalFilters` — the only verdict that renders the declined set — there are none. So the
/// fixture gives `composed_set` an EMPTY `applied` list, exactly as production does on that path,
/// and still requires the declined columns to come out.
///
/// Breakage this pins: building `RejectedSet::fields` from `applied` the way `ComposedSet::fields`
/// is built. The row would name no fields at all and would read as "a set was declined" with
/// nothing to say which — a sentence the user cannot act on, and one that no other test notices
/// because the numbers beside it stay correct.
#[test]
fn a_declined_set_names_its_columns_from_its_own_mask() {
    let chosen = vec![false; FIELDS.len()];
    let mut mask = vec![false; FIELDS.len()];
    mask[0] = true;
    mask[2] = true;
    let out = compose::ComposeOutcome {
        chosen,
        decision: ComposeDecision::NoAdditionalFilters,
        support: vec![0u8; FIELDS.len()],
        folds: 5,
        gate_robust: true,
        rejected: Some(compose::RejectedCandidate {
            mask,
            inner_lift: 116.41,
            gate_lift: -129.91,
            inner_folds: 3,
            gate_folds: 2,
        }),
    };
    let set = composed_set(&out, &[]);
    let declined = set
        .rejected
        .expect("a carried rejection must survive into the reported set");
    assert_eq!(
        declined.fields,
        vec![FIELDS[0].col, FIELDS[2].col],
        "the declined set must name the columns of its own mask, even with no applied ranges"
    );
    assert_eq!(
        (
            declined.inner_lift,
            declined.gate_lift,
            declined.inner_folds,
            declined.gate_folds
        ),
        (116.41, -129.91, 3, 2),
        "both figures and both fold counts must reach the surface unchanged"
    );
    assert!(
        set.fields.is_empty(),
        "the CHOSEN set stays keyed to the applied ranges, which is what makes the two differ"
    );
}
