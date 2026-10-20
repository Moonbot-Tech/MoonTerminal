use super::evaluate;
use crate::db::tuner::{TimeAxes, TimeWindow};

/// Holdout losses cannot change the trained bounds; positive profit can still trail Fact.
#[test]
fn fit_only_oldest_rows_and_compare_same_tail() {
    let axes = TimeAxes {
        day: true,
        ..Default::default()
    };
    let train = vec![(0, 1, 10.0, 1), (0, 30, -20.0, 2), (0, 59, 11.0, 3)];
    let mut rows = train.clone();
    rows.extend([(0, 1, 2.0, 4), (0, 30, 20.0, 5), (0, 59, 3.0, 6)]);
    rows.reverse();
    let fit = evaluate(rows, Some(1), false, axes, 0.5);
    assert_eq!(fit.suggestion.tod, Some(TimeWindow::Day(59, 1)));
    assert_eq!(fit.train.as_ref().unwrap().profit, 21.0);
    assert_eq!(fit.holdout.as_ref().unwrap().profit, 5.0);
    assert_eq!(fit.fact_holdout.as_ref().unwrap().profit, 25.0);
    assert!(fit.below_fact());
    let full = evaluate(train.clone(), Some(1), false, axes, 1.0);
    assert!(full.holdout.is_none());
    let small = evaluate(train, Some(4), false, axes, 1.0);
    assert!(small.train.is_none());
}

/// Equal-close groups stay together and drawdown follows UTC close, never civil schedule time.
#[test]
fn timestamp_groups_and_drawdown_are_chronological() {
    let axes = TimeAxes {
        day: true,
        ..Default::default()
    };
    let rows = vec![
        (0, 20, -4.0, 10),
        (0, 1, 10.0, 20),
        (0, 5, -3.0, 20),
        (0, 30, 1.0, 30),
    ];
    let result = evaluate(rows, Some(4), false, axes, 1.0);
    // Equal-close losses precede wins, just as in By filter: -4, -3, +10, +1.
    assert_eq!(result.train.unwrap().max_dd, 7.0);
    let ties = vec![(0, 20, 4.0, 10), (0, 1, 10.0, 10)];
    let result = evaluate(ties, Some(1), false, axes, 0.5);
    assert!(result.holdout.is_none());
}

/// The saved rounded window wins on its own rows, and its KPI includes precisely those rows.
#[test]
fn rounded_kpi_matches_final_bounds() {
    let axes = TimeAxes {
        day: true,
        ..Default::default()
    };
    let rows = vec![
        (0, 1, 20.0, 1),
        (0, 4, -30.0, 2),
        (0, 12, 10.0, 3),
        (0, 16, -50.0, 4),
    ];
    let result = evaluate(rows, Some(1), true, axes, 1.0);
    assert_eq!(result.suggestion.tod, Some(TimeWindow::Day(10, 15)));
    assert_eq!(result.train.as_ref().unwrap().profit, 10.0);
    assert_eq!(result.train.as_ref().unwrap().n, 1);
}
