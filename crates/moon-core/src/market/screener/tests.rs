//! Regression coverage for the Screener retained-range delta projection.

use moonproto::state::DerivedDeltaSnapshot;

use super::ScreenerRow;

/// Removing or swapping any assignment in `screener.rs:ScreenerRow::apply_range_deltas` must fail
/// the distinct-value assertion; otherwise a Screener column displays zero or another period's
/// movement instead of its matching retained-range delta.
#[test]
fn every_screener_delta_uses_its_matching_range_period() {
    let mut row = ScreenerRow::default();
    row.apply_range_deltas(Some(DerivedDeltaSnapshot {
        one_minute: 1.0,
        fifteen_minutes: 15.0,
        one_hour: 60.0,
        three_hours: 180.0,
        twenty_four_hours: 1_440.0,
        seventy_two_hours: 4_320.0,
        ..DerivedDeltaSnapshot::default()
    }));

    assert_eq!(
        [
            row.d_24h, row.d_3h, row.d_1h, row.d_15m, row.d_1m, row.d_72h
        ],
        [1_440.0, 180.0, 60.0, 15.0, 1.0, 4_320.0]
    );
}
