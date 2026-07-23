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

#[test]
fn improvement_margin_has_a_floor_of_one() {
    // Below |base| = 1 the margin is pinned to 1e-9, so a tiny baseline cannot make the
    // threshold vanish and let summation noise win.
    assert_eq!(improvement_margin(0.0), 1e-9);
    assert_eq!(improvement_margin(0.5), 1e-9);
    // Above the floor it scales with the baseline.
    assert!((improvement_margin(1000.0) - 1e-6).abs() < 1e-15);
}
