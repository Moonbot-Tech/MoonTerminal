//! Regression tests for shared clock presentation modes.

use chrono::{TimeZone, Utc};

use super::{ClockPrecision, clock_parts};

/// `clock.rs:clock_parts` must remove only seconds for `ClockPrecision::Minutes`; returning the
/// full time makes the narrow Profit Monitor reclaim the width that its stacked controls released,
/// while dropping the city code hides which IANA zone defines the monitor's calendar periods.
#[test]
fn compact_clock_removes_only_seconds() {
    let now = Utc.with_ymd_and_hms(2026, 8, 6, 18, 42, 9).unwrap();

    assert_eq!(
        clock_parts(chrono_tz::Europe::Warsaw, now, ClockPrecision::Seconds),
        ("20:42:09".to_string(), "WAW".to_string())
    );
    assert_eq!(
        clock_parts(chrono_tz::Europe::Warsaw, now, ClockPrecision::Minutes),
        ("20:42".to_string(), "WAW".to_string())
    );
}
