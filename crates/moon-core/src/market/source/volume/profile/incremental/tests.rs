//! Sequence-prefix and floating-point parity for retained price contributions.

use super::*;

/// Removing a time prefix must keep a late resend after the first matching input.
#[test]
fn chart_market_time_expiry_matches_the_original_sequence_suffix() {
    let mut tape = Tape::default();
    for at in [10, 20, 5, 30] {
        tape.push(at, 0);
    }
    tape.trim(10, 15);
    assert_eq!(
        tape.rows.iter().copied().collect::<Vec<_>>(),
        vec![(1, 20), (2, 5), (3, 30)]
    );
    tape.trim(10, 25);
    assert_eq!(tape.rows.iter().copied().collect::<Vec<_>>(), vec![(3, 30)]);
}

/// Capacity expiry uses sequence count even when the time seek skipped many retained inputs.
#[test]
fn chart_market_retention_counts_skipped_inputs_and_expires_the_correct_bin_addends() {
    let mut tape = Tape::default();
    for at in [1, 2, 3, 10, 11] {
        tape.push(at, 10);
    }
    tape.trim(3, 10);
    assert_eq!(tape.first(), 3);
    for at in [12, 13] {
        tape.push(at, 10);
    }
    tape.trim(3, 10);
    assert_eq!(
        tape.rows.iter().copied().collect::<Vec<_>>(),
        vec![(4, 11), (5, 12), (6, 13)]
    );
}

/// Reordering mini and trade sums or subtracting expired values changes these exact totals.
#[test]
fn chart_market_bins_preserve_order_and_remove_expired_rounding_dust() {
    let mut bins = Bins::default();
    bins.add(true, 0, 1, 1e20, 0.0);
    bins.add(false, 0, 1, 1.0, 0.0);
    bins.add(true, 1, 1, 1.0, 0.0);
    assert_eq!(
        bins.totals(0, 0).collect::<Vec<_>>(),
        vec![(1, (1e20, 0.0))]
    );
    assert_eq!(bins.totals(1, 0).collect::<Vec<_>>(), vec![(1, (2.0, 0.0))]);
    assert!(bins.totals(2, 1).next().is_none());
    let mut ordered = Bins::default();
    ordered.add(true, 0, 1, 1.0, 0.0);
    ordered.add(false, 0, 1, 1e16, 0.0);
    ordered.add(true, 1, 1, 1.0, 0.0);
    assert_eq!(
        ordered.totals(0, 0).collect::<Vec<_>>(),
        vec![(1, (1e16 + 2.0, 0.0))]
    );
}
