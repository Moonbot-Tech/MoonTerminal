//! Which zone a sleep schedule is read in.
//!
//! Explicit imports: the backend parent re-exports `gpui::*`, whose own `test` shadows the
//! built-in attribute and makes `#[test]` expand recursively.

use chrono_tz::Tz;

use super::quiet_zone;

/// The default answer: the machine's own zone, whatever the header clock shows.
///
/// Breakage: following the visible clock means a trader watching New York falls asleep on New
/// York's schedule, which is the report this flag came from.
#[test]
fn the_machines_zone_wins_by_default() {
    let zone = quiet_zone(true, || Some(Tz::Europe__Kyiv), Some("America/New_York"));
    assert_eq!(zone, Tz::Europe__Kyiv);
}

/// Switched off, the schedule follows the visible clock exactly as it did before the flag.
#[test]
fn the_header_clock_wins_when_the_flag_is_off() {
    let zone = quiet_zone(false, || Some(Tz::Europe__Kyiv), Some("America/New_York"));
    assert_eq!(zone, Tz::America__New_York);
}

/// A machine that cannot name its own zone falls back to the visible clock, not to UTC.
///
/// Breakage: falling back to UTC moves the night by the whole offset while the flag claims the
/// schedule is local — the exact silent shift this change exists to remove.
#[test]
fn an_unknown_system_zone_falls_back_to_the_header_clock() {
    let zone = quiet_zone(true, || None, Some("America/New_York"));
    assert_eq!(zone, Tz::America__New_York);
}

/// With neither a system zone nor a saved clock there is nothing left but UTC.
#[test]
fn nothing_known_is_utc() {
    assert_eq!(quiet_zone(true, || None, None), Tz::UTC);
    assert_eq!(quiet_zone(false, || None, None), Tz::UTC);
}

/// An unresolvable saved zone id is not a zone.
#[test]
fn a_broken_saved_zone_id_is_utc() {
    assert_eq!(quiet_zone(true, || None, Some("Mars/Olympus")), Tz::UTC);
}

/// With the flag off the machine's zone is never even asked for.
///
/// Breakage: resolving it eagerly costs a platform call — a WinRT activation on Windows, a
/// process-wide cache reset on macOS — ten times a second, for a value that is then discarded.
#[test]
fn the_system_zone_is_not_resolved_when_it_is_not_wanted() {
    let mut asked = false;
    let zone = quiet_zone(
        false,
        || {
            asked = true;
            Some(Tz::Europe__Kyiv)
        },
        Some("America/New_York"),
    );
    assert_eq!(zone, Tz::America__New_York);
    assert!(
        !asked,
        "the platform must not be queried for a discarded value"
    );
}
