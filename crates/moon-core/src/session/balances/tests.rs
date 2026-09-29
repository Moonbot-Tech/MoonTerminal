use super::*;

/// One reading.
fn figures(state: BalanceState, free: f64, total: f64) -> BalanceFigures {
    BalanceFigures { state, free, total }
}

/// Awaiting and unpriced cores stay out of the sum and are counted apart; stale ones count.
///
/// Breakage: summing an unusable reading as zero states "the account is empty" when the truth is
/// "nothing has reported yet".
#[test]
fn only_usable_readings_are_summed() {
    let sum = aggregate_balance_figures(&[
        figures(BalanceState::Live, 1.0, 10.0),
        figures(BalanceState::Stale, 2.0, 20.0),
        figures(BalanceState::Awaiting, 0.0, 0.0),
        figures(BalanceState::Unpriced, 5.0, 50.0),
        figures(BalanceState::Live, f64::NAN, 1.0),
    ]);
    assert_eq!(sum.free, Some(3.0));
    assert_eq!(sum.total, Some(30.0));
    assert_eq!(
        (
            sum.counted,
            sum.stale,
            sum.awaiting,
            sum.unpriced,
            sum.excluded
        ),
        (2, 1, 1, 2, 3)
    );
}

/// Nothing counted is no figure, not zero.
#[test]
fn an_empty_sum_has_no_totals() {
    let sum = aggregate_balance_figures(&[]);
    assert_eq!((sum.free, sum.total, sum.counted), (None, None, 0));
}

/// A core that is not configured takes the default total setting.
#[test]
fn an_unconfigured_core_counts_by_default() {
    assert_eq!(core_total_mode(&[], 7), TotalMode::default());
}
