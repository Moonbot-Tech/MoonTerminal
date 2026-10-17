//! Regression checks for shared report-axis cache identities.

use super::*;

/// Failing to return the same allocation would restore per-tick axis allocation.
#[test]
fn axis_cache_hits_on_same_epoch_and_zone() {
    let axis = Arc::new(moon_core::db::ReportAxis::from_measured(
        Default::default(),
        chrono_tz::UTC,
    ));
    let entries = vec![(chrono_tz::UTC, (1, 2), axis.clone())];
    let hit = axis_cache_lookup(&entries, chrono_tz::UTC, (1, 2)).unwrap();
    assert!(Arc::ptr_eq(&hit, &axis));
}

/// Ignoring the fleet epoch would keep corrected report timestamps on the old offset.
#[test]
fn axis_cache_misses_when_offset_epoch_moves() {
    let axis = Arc::new(moon_core::db::ReportAxis::from_measured(
        Default::default(),
        chrono_tz::UTC,
    ));
    let entries = vec![(chrono_tz::UTC, (1, 2), axis)];
    assert!(axis_cache_lookup(&entries, chrono_tz::UTC, (2, 2)).is_none());
}

/// Ignoring the zone would reuse another window's civil-day boundaries.
#[test]
fn axis_cache_keeps_separate_entries_per_zone() {
    let utc = Arc::new(moon_core::db::ReportAxis::from_measured(
        Default::default(),
        chrono_tz::UTC,
    ));
    let warsaw = Arc::new(moon_core::db::ReportAxis::from_measured(
        Default::default(),
        chrono_tz::Europe::Warsaw,
    ));
    let entries = vec![
        (chrono_tz::UTC, (1, 2), utc.clone()),
        (chrono_tz::Europe::Warsaw, (1, 2), warsaw.clone()),
    ];
    assert!(Arc::ptr_eq(
        &axis_cache_lookup(&entries, chrono_tz::UTC, (1, 2)).unwrap(),
        &utc
    ));
    assert!(Arc::ptr_eq(
        &axis_cache_lookup(&entries, chrono_tz::Europe::Warsaw, (1, 2)).unwrap(),
        &warsaw
    ));
    assert!(axis_cache_lookup(&entries, chrono_tz::Asia::Tokyo, (1, 2)).is_none());
}

/// Omitting either configured id or durable uid would retain a removed core's report mapping.
#[test]
fn axis_cache_misses_when_server_uid_set_changes() {
    let original = axis_servers_key([(41, 51), (42, 52)].into_iter());
    let axis = Arc::new(moon_core::db::ReportAxis::from_measured(
        Default::default(),
        chrono_tz::UTC,
    ));
    let entries = vec![(chrono_tz::UTC, (1, original), axis)];
    for mapping in [[(41, 53), (42, 52)], [(43, 51), (42, 52)]] {
        let changed = axis_servers_key(mapping.into_iter());
        assert!(axis_cache_lookup(&entries, chrono_tz::UTC, (1, changed)).is_none());
    }
}
