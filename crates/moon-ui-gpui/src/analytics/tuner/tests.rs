use super::{StratMode, STRAT_MODES};
use moon_core::config::layout::StratColsByMode;

/// Each axis must address its OWN slot. Two axes sharing one is the copy-paste that makes
/// the whole per-axis layout pointless — and it would look like "my columns keep changing
/// when I switch tabs", which is exactly what this feature exists to stop.
#[test]
fn each_axis_owns_its_column_slot() {
    let mut cols = StratColsByMode::default();
    for (i, mode) in STRAT_MODES.into_iter().enumerate() {
        *mode.cols_slot(&mut cols) = i as u16 + 1;
    }
    assert_eq!((cols.filter, cols.coins, cols.time), (1, 2, 3));
    // And reading back returns what that axis wrote, not a neighbour's.
    for (i, mode) in STRAT_MODES.into_iter().enumerate() {
        assert_eq!(*mode.cols_slot(&mut cols), i as u16 + 1);
    }
}

/// Only the coin axis spends width on the coin-list columns — that difference is the
/// reason the mask is per axis at all.
#[test]
fn coin_axis_defaults_to_showing_the_lists() {
    assert_ne!(
        StratMode::Coins.default_cols(),
        StratMode::Filters.default_cols()
    );
    assert_eq!(
        StratMode::Filters.default_cols(),
        StratMode::Time.default_cols()
    );
}
