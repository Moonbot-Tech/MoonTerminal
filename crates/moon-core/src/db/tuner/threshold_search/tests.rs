//! Tests for the scan side of the threshold search: how the sample is cut in two, and how it is
//! ordered before it is cut.
//!
//! Both are pure functions over the scanned columns, so they are checked here directly rather
//! than through a database — the SQL they sit next to is exercised by the tuner's own DB tests.

use super::{chronological_order, train_split};

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
