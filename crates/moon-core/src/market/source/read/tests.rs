//! Exact source-time and late-resend coverage for live marker seed reads.

use super::visit_tick_rows;
use moonproto::{MoonTime, state::TradeHistoryRow};

/// A late resend behind a newer append must still reach the matcher, at original millisecond precision.
#[test]
fn visitor_keeps_late_resends_and_excludes_both_interval_boundaries_correctly() {
    let base = 1_700_086_401_000;
    let rows = [-1, 100, 3_000, 766, 1_000, 1].map(|offset| TradeHistoryRow {
        time: MoonTime::from_unix_millis(base + offset),
        price: 0.14275,
        qty: 1.0,
    });
    let mut seen = Vec::new();
    visit_tick_rows(rows.iter(), base, base + 1_000, |time, _| seen.push(time));
    assert_eq!(seen, vec![base + 100, base + 766, base + 1]);
}
