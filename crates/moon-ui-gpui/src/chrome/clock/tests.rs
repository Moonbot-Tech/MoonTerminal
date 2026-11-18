//! Regression tests for shared clock presentation modes.

use chrono::{TimeZone, Utc};

use super::{ClockPrecision, cities, clock_menu_rows, clock_parts, picker_right_label};

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

/// Opening the lazy picker must preserve the eager menu order, labels and checked/disabled rows.
#[test]
fn opened_clock_menu_matches_eager_city_rows() {
    let _locale = crate::test_locale::force("en");
    let now = Utc.with_ymd_and_hms(2026, 8, 6, 18, 42, 9).unwrap();
    for (zone, has_system_row) in [
        (chrono_tz::Europe::Warsaw, false),
        ("Etc/GMT+7".parse().unwrap(), true),
    ] {
        let rows = clock_menu_rows(zone, now);
        assert_eq!(
            rows.len(),
            cities::CITIES.len() + usize::from(has_system_row)
        );
        assert_eq!(rows.iter().filter(|row| row.selected).count(), 1);
        if has_system_row {
            let system = &rows[0];
            assert_eq!(system.key, "tz-system");
            assert_eq!(
                system.label,
                rust_i18n::t!("header.clock_system", zone = zone.name())
            );
            assert_eq!(system.right, picker_right_label(zone, now));
            assert!(system.selected && system.city.is_none());
        }
        for (row, city) in rows[usize::from(has_system_row)..]
            .iter()
            .zip(cities::CITIES)
        {
            assert_eq!(row.key, format!("tz-{}", city.code));
            assert_eq!(row.label, format!("{}  {}", city.code, city.name()));
            assert_eq!(row.right, picker_right_label(city.zone, now));
            assert_eq!(row.selected, city.zone == zone);
            assert_eq!(row.city.map(|city| city.zone), Some(city.zone));
        }
    }
}
