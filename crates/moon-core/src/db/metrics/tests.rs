use super::*;

#[test]
fn profit_factor_covers_the_three_regimes() {
    // Wins over losses.
    assert!((profit_factor(16.0, 6.0) - 16.0 / 6.0).abs() < 1e-12);
    // Wins but no losses saturate to the 99 sentinel, not infinity.
    assert_eq!(profit_factor(10.0, 0.0), 99.0);
    // Neither wins nor losses is 0, not a divide.
    assert_eq!(profit_factor(0.0, 0.0), 0.0);
}

#[test]
fn winrate_is_zero_on_empty() {
    assert_eq!(winrate(0, 0), 0.0);
    assert!((winrate(1, 2) - 50.0).abs() < 1e-12);
    assert!((winrate(2, 4) - 50.0).abs() < 1e-12);
}

/// The tally's drawdown must follow the CUMULATIVE CURVE, not the individual results.
///
/// The oracle is hand-computed from the sequence below: the curve runs 0 -> +10 -> +4 -> -6 ->
/// -1, its peak is +10, and its deepest point is -6, so the maximum drawdown is 16 — a number no
/// single trade in the sequence carries.
///
/// Breakage this pins: comparing against the worst single loss (`max_dd.max(-profit)`), or
/// updating the peak AFTER measuring the fall. Both read plausibly and both understate the
/// drawdown of a losing streak, which is exactly the figure a trader judges a suggested filter
/// by — and it is the figure the tuner now shows for the trades held back from the search.
#[test]
fn drawdown_measures_the_cumulative_curve() {
    let mut t = Tally::default();
    for profit in [10.0, -6.0, -10.0, 5.0] {
        t.push(profit);
    }
    assert_eq!(t.n, 4);
    assert_eq!(t.wins, 2);
    assert!((t.profit - -1.0).abs() < 1e-12);
    assert!((t.wsum - 15.0).abs() < 1e-12);
    assert!((t.lsum - 16.0).abs() < 1e-12);
    assert!((t.max_dd - 16.0).abs() < 1e-12, "max_dd={}", t.max_dd);
    assert!((t.avg() - -0.25f64).abs() < 1e-12, "avg={}", t.avg());
}

/// A zero result is a loss, and an empty tally divides by nothing.
#[test]
fn a_zero_result_counts_as_a_loss() {
    let mut t = Tally::default();
    t.push(0.0);
    assert_eq!(t.wins, 0);
    assert_eq!(t.n, 1);
    assert_eq!(t.profit_factor(), 0.0);

    let empty = Tally::default();
    assert_eq!(empty.winrate(), 0.0);
    assert_eq!(empty.avg(), 0.0);
    assert_eq!(empty.max_dd, 0.0);
}

#[test]
fn improvement_margin_has_a_floor_of_one() {
    // Below |base| = 1 the margin is pinned to 1e-9, so a tiny baseline cannot make the
    // threshold vanish and let summation noise win.
    assert_eq!(improvement_margin(0.0), 1e-9);
    assert_eq!(improvement_margin(0.5), 1e-9);
    // Above the floor it scales with the baseline.
    assert!((improvement_margin(1000.0) - 1e-6).abs() < 1e-15);
}
