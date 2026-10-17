//! Filtering regressions independent of the displayed market spelling.

use super::{Entry, matches_coin_filter};
use moon_core::market::ScreenerRow;

/// Cached search forms must allow either market or coin matches without changing display text.
#[test]
fn filter_matches_uppercased_market_or_coin() {
    let entry = Entry {
        row: ScreenerRow {
            market: "alphaUsdt".into(),
            coin: "beta".into(),
            ..Default::default()
        },
        core_name: "synthetic".into(),
        market_uc: "ALPHAUSDT".into(),
        coin_uc: "BETA".into(),
    };
    assert!(matches_coin_filter(&entry, "ALPHA"));
    assert!(matches_coin_filter(&entry, "BETA"));
    assert!(!matches_coin_filter(&entry, "GAMMA"));
}

/// Treating an empty query as absent data would hide every market when the input is cleared.
#[test]
fn empty_filter_keeps_all() {
    let entry = Entry {
        row: ScreenerRow::default(),
        core_name: "synthetic".into(),
        market_uc: String::new(),
        coin_uc: "BETA".into(),
    };
    assert!(matches_coin_filter(&entry, ""));
}
