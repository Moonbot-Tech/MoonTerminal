//! Explicit imports, never `use super::*`: this module re-exports `gpui::*`, whose own `test`
//! shadows the built-in attribute and makes `#[test]` expand into itself. The caption tests carry
//! the same note for the same reason.

use super::{QUICK_PERIODS, QUICK_TRADES};

/// The offered periods must run from shortest to longest.
///
/// Breakage: they used to be emitted as two runs — the fixed windows, then the extra minute counts
/// — so the list read `1м 3м 5м 15м 30м 2м 10м …`. A reader picks a period by its LENGTH, and a
/// list that is not ordered by it is one they have to search instead of scan.
#[test]
fn the_offered_periods_are_ordered_by_length() {
    let lengths: Vec<i64> = QUICK_PERIODS.iter().map(|w| w.millis()).collect();
    let mut sorted = lengths.clone();
    sorted.sort_unstable();
    assert_eq!(lengths, sorted, "periods must be listed shortest first");
    assert!(
        lengths.windows(2).all(|w| w[0] != w[1]),
        "two rows offering the same period would be one the reader cannot tell apart"
    );
}

/// Every offered trade count is distinct and ascending, for the same reason.
#[test]
fn the_offered_trade_counts_ascend() {
    let mut sorted = QUICK_TRADES;
    sorted.sort_unstable();
    assert_eq!(QUICK_TRADES, sorted);
}
