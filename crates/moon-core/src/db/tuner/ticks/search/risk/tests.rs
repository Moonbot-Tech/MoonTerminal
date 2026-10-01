use super::*;

fn tally(results: &[f64]) -> Tally {
    let mut tally = Tally::default();
    for &r in results {
        tally.push(r);
    }
    tally
}

/// A drawdown up to 20 % deeper than the base's passes, one past it does not; the base passes
/// its own limit.
#[test]
fn the_drawdown_may_be_a_share_deeper_than_the_base() {
    let limits = RiskLimits {
        drawdown_pct: Some(20.0),
        winrate_pct: None,
    };
    // Base: +5, −10 → drawdown 10. Within: −12. Past: −13.
    let base = tally(&[5.0, -10.0, 8.0]);
    assert!(limits.allows(&base, &base));
    assert!(limits.allows(&tally(&[5.0, -12.0, 30.0]), &base));
    assert!(!limits.allows(&tally(&[5.0, -13.0, 30.0]), &base));
}

/// A win rate down to 20 % under the base's passes, one under it does not.
#[test]
fn the_win_rate_may_be_a_share_lower_than_the_base() {
    let limits = RiskLimits {
        drawdown_pct: None,
        winrate_pct: Some(20.0),
    };
    // Base: 5 of 5 won (100 %). 4 of 5 (80 %) passes, 3 of 5 (60 %) does not.
    let base = tally(&[1.0; 5]);
    assert!(limits.allows(&tally(&[1.0, 1.0, 1.0, 1.0, -1.0]), &base));
    assert!(!limits.allows(&tally(&[1.0, 1.0, 1.0, -1.0, -1.0]), &base));
}

/// No limit set leaves every point free.
#[test]
fn no_limit_refuses_nothing() {
    let base = tally(&[1.0; 5]);
    let worse = tally(&[-50.0, -50.0]);
    assert!(RiskLimits::default().allows(&worse, &base));
    let standard = RiskLimits {
        drawdown_pct: Some(DEFAULT_WORSE_PCT),
        winrate_pct: Some(DEFAULT_WORSE_PCT),
    };
    assert!(!standard.allows(&worse, &base));
}
