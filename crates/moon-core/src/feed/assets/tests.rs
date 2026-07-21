use super::source_priced;

/// A global-sourced account needs a usable base-currency rate.
#[test]
fn global_source_requires_a_finite_positive_rate() {
    assert!(source_priced(false, false, false, 1.0));
    assert!(!source_priced(false, false, false, 0.0));
    assert!(!source_priced(false, false, false, -1.0));
    assert!(!source_priced(false, false, false, f64::NAN));
    assert!(!source_priced(false, false, false, f64::INFINITY));
}

/// A corrupt `btc_full` disqualifies the global source even when the rate is perfect.
///
/// The trap this guards: `NaN.abs() > 1e-9` is false, so a non-finite `btc_full` silently took
/// the "field not published" fallback and reported available funds as though they were equity
/// — locked balance and unrealized PnL dropped, at full rendering strength.
#[test]
fn corrupt_full_disqualifies_the_global_source() {
    assert!(!source_priced(false, false, true, 1.0));
    assert!(!source_priced(false, true, true, 1.0));
}

/// A coin-margined account is judged by its wallets, which `btc_full` never fed — so the same
/// corruption must NOT disqualify it, or a healthy COIN-M account would read as unpriced.
#[test]
fn coin_margined_source_is_judged_by_its_wallets() {
    assert!(source_priced(true, true, true, 0.0));
    assert!(!source_priced(true, false, false, 1.0));
}
