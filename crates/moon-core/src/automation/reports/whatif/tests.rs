//! The holdout split and the before/after window of an edit.

use super::*;

/// The held-out part is the LATER trades: a check on the past would prove nothing.
#[test]
fn the_holdout_is_the_latest_part() {
    let trades: Vec<Trade> = (0..10).map(|i| Trade::sample(1, 7, "A", i, 1.0)).collect();
    let (train, holdout) = split(&trades, 30);
    assert_eq!((train.len(), holdout.len()), (7, 3));
    assert!(train.iter().all(|t| t.close_ms < holdout[0].close_ms));
    let (all, none) = split(&trades, 0);
    assert_eq!((all.len(), none.len()), (10, 0));
}

/// Regression target: an extreme `valid_from_ms` overflowing the window arithmetic.
#[test]
fn an_extreme_edit_time_is_refused_not_overflowed() {
    let scope = super::super::Scope {
        cores: vec![1],
        ..Default::default()
    };
    let args = EffectArgs {
        core: "1".into(),
        strategy_id: "7".into(),
        valid_from_ms: i64::MAX,
        window_days: 90,
    };
    let error = effect(&scope, args).unwrap_err().to_string();
    assert!(error.contains("future"), "{error}");
}
