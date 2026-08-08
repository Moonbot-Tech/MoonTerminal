//! Regression tests for shared clock presentation modes.

use chrono::{TimeZone, Utc};

use super::{ClockPrecision, clock_parts, resolved_header_clock_zone};

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

/// Restricting resolution to `cities::by_zone_id` turns a valid first-run system zone outside the
/// picker into UTC, so every panel disagrees with the operating system after restart.
#[test]
fn uncurated_persisted_iana_zone_remains_exact() {
    assert_eq!(
        resolved_header_clock_zone(Some("Europe/Prague")),
        chrono_tz::Europe::Prague
    );
    assert_eq!(
        resolved_header_clock_zone(Some("Europe/Atlantis")),
        chrono_tz::UTC
    );
}
