//! Independent small sequences catch input-order drawdown and shifting-cohort regressions.

use super::*;

/// Ignoring replay tails or admitting a shared boundary leaks later holdout prices.
#[test]
fn embargo_excludes_overlapping_and_touching_windows() {
    assert!(before_holdout(109, Some(110)));
    assert!(!before_holdout(110, Some(110)));
    assert!(!before_holdout(120, Some(110)));
    assert!(before_holdout(120, None));
}

/// Computing drawdown in report input order would report a smaller loss than exit order here.
#[test]
fn tally_follows_exit_time_instead_of_input_order() {
    let tally = ordered_tally(vec![(30, 1, 1, -4.0), (10, 1, 2, 3.0), (20, 1, 3, -5.0)]);
    assert_eq!(tally.n, 3);
    assert_eq!(tally.profit, -6.0);
    assert_eq!(tally.max_dd, 9.0);
    assert_eq!(tally.profit_factor(), 3.0 / 9.0);
}

/// The split is by identity count, not a model-dependent eligibility tally or search helper.
#[test]
fn cohort_boundary_is_count_based() {
    assert_eq!(cohort_cut(10), 7);
    assert_eq!(cohort_cut(5), 4);
    assert_eq!(cohort_cut(0), 0);
}

/// Recutting a reduced cohort would leak the frozen holdout into optimization.
#[test]
fn frozen_cohort_overrides_count_cut() {
    let labels = BTreeMap::from([
        ((1, 10), "holdout".to_owned()),
        ((1, 11), "train".to_owned()),
    ]);
    assert!(!is_train(Some(&labels), (1, 10), 0, cohort_cut(2)));
    assert!(is_train(Some(&labels), (1, 11), 1, 1));
    assert!(is_train(None, (1, 10), 0, 1));
    assert!(!is_train(None, (1, 11), 1, 1));
}
