//! Unit tests for the city table and its time math.
//!
//! Explicit imports, never `use super::*`: the ancestors re-export `gpui::*`, whose own `test`
//! shadows the built-in attribute and makes `#[test]` expand recursively.

use super::{
    CITIES, by_zone_id, current_offset_min, local_hms, migrate_offset_min, reconcile_target,
    utc_city,
};
use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::{OffsetComponents, Tz};

/// Parse an RFC 3339 instant for a test oracle.
fn at(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .expect("test instant parses")
        .with_timezone(&Utc)
}

/// The zone's STANDARD offset in minutes, summer time excluded — the key the city table is ordered
/// by. Lives here because only the ordering test needs it; production reads current offsets.
///
/// Deliberately not "the offset in January": south of the equator January IS the daylight-saving
/// side, so a January reading calls Sydney +11 when its standard offset is +10.
fn base_offset_min(zone: Tz, now_utc: DateTime<Utc>) -> i32 {
    zone.offset_from_utc_datetime(&now_utc.naive_utc())
        .base_utc_offset()
        .num_minutes() as i32
}

/// Summer time must come from the tz database, in both hemispheres.
///
/// The oracle is hand-known wall-clock fact, not arithmetic borrowed from the code under test: at
/// noon UTC on 1 July 2026 Warsaw reads 14:00 (CEST, +2 standard plus an hour) and New York 08:00
/// (EDT); on 1 January 2026 the same instant reads 13:00 in Warsaw and 07:00 in New York. Sydney
/// is the sign flip — 22:00 in July (standard +10) against 23:00 in January (+11, its summer) —
/// and Tokyo, which observes no summer time at all, reads 21:00 in both. Mutation this catches:
/// caching a fixed offset in place of the zone lookup, which would leave seasonal clocks stale.
#[test]
fn a_citys_clock_follows_its_summer_time_rules() {
    let july = at("2026-07-01T12:00:00Z");
    let january = at("2026-01-01T12:00:00Z");

    assert_eq!(local_hms(Tz::Europe__Warsaw, july), "14:00:00");
    assert_eq!(local_hms(Tz::Europe__Warsaw, january), "13:00:00");
    assert_eq!(local_hms(Tz::America__New_York, july), "08:00:00");
    assert_eq!(local_hms(Tz::America__New_York, january), "07:00:00");

    // Southern hemisphere: January is the daylight-saving side, so the offset moves the other way.
    assert_eq!(local_hms(Tz::Australia__Sydney, july), "22:00:00");
    assert_eq!(local_hms(Tz::Australia__Sydney, january), "23:00:00");

    // Control: a zone with no summer time must not move.
    assert_eq!(local_hms(Tz::Asia__Tokyo, july), "21:00:00");
    assert_eq!(local_hms(Tz::Asia__Tokyo, january), "21:00:00");
}

/// Codes are the header label and the localization key, so a duplicate would make one city
/// unlabelable and silently steal the other's translated name.
#[test]
fn every_code_is_a_unique_three_letter_token() {
    let mut codes: Vec<&str> = CITIES.iter().map(|c| c.code).collect();
    let total = codes.len();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), total, "duplicate city code in CITIES");

    for c in CITIES {
        assert!(
            c.code.len() == 3 && c.code.chars().all(|ch| ch.is_ascii_uppercase()),
            "{} is not a three-letter uppercase code",
            c.code
        );
    }
}

/// The persisted key is the zone id, so two rows sharing a zone would make the saved selection
/// ambiguous — `by_zone_id` could only ever return one of them.
#[test]
fn every_zone_is_unique_and_resolvable_by_its_id() {
    let mut ids: Vec<&str> = CITIES.iter().map(|c| c.zone.name()).collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total, "duplicate zone in CITIES");

    for c in CITIES {
        let found = by_zone_id(c.zone.name()).expect("every table zone resolves");
        assert_eq!(found.code, c.code);
    }
    assert!(by_zone_id("Europe/Atlantis").is_none());
}

/// UTC stays the neutral first row: it is the fallback for an unknown saved zone and the default
/// for a config that never opened the picker.
#[test]
fn utc_is_the_first_row_and_the_fallback() {
    assert_eq!(utc_city().code, "UTC");
    assert_eq!(utc_city().zone, Tz::UTC);
    assert_eq!(CITIES[0].code, "UTC");
}

/// The menu reads west to east; a row inserted in the wrong place is invisible in review and
/// obvious to a user scanning the list.
#[test]
fn the_table_runs_west_to_east_after_utc() {
    let now = at("2026-07-01T12:00:00Z");
    let mut prev = i32::MIN;
    for c in &CITIES[1..] {
        let off = base_offset_min(c.zone, now);
        assert!(
            off >= prev,
            "{} at {off} min breaks the west-to-east order",
            c.code
        );
        prev = off;
    }
}

/// Migrating a compatibility offset must preserve its wall clock at the migration instant.
///
/// The oracle is the wall clock itself, not the mapping: for representative valid seeds, the
/// migrated city's time must equal the time computed independently from the offset. Mutation this
/// catches: matching STANDARD offsets, which can hand a saved `+2` to Athens while it reads `+3`.
#[test]
fn migrating_a_legacy_offset_keeps_the_clock_where_it_was() {
    // Deliberately a summer instant in the northern hemisphere: that is when standard and current
    // offsets disagree across most of the table, so the wrong rule cannot pass here.
    let now = at("2026-07-01T12:00:00Z");

    for off in [-300, -180, 60, 120, 180, 330, 480, 540, 600] {
        let city = migrate_offset_min(off, now)
            .unwrap_or_else(|| panic!("no curated city reachable from offset {off}"));
        let expected = local_hms(Tz::UTC, now + chrono::Duration::minutes(off.into()));
        assert_eq!(
            local_hms(city.zone, now),
            expected,
            "offset {off} migrated to {}, which reads a different time",
            city.code
        );
    }
}

/// A default or out-of-schema offset must not silently invent a selected city.
///
/// Zero is the untouched default of every config that never opened the picker — resolving it to a
/// city would relabel all of them. A value outside the compatibility range is not a saved choice.
/// Both leave the clock on UTC.
#[test]
fn a_legacy_offset_migrates_only_when_it_was_actually_chosen() {
    let now = at("2026-07-01T12:00:00Z");

    assert!(
        migrate_offset_min(0, now).is_none(),
        "an untouched default must stay on UTC, not become a city"
    );
    for off in [999, -13 * 60, 15 * 60] {
        assert!(
            migrate_offset_min(off, now).is_none(),
            "{off} is outside the old picker's range and cannot be a saved choice"
        );
    }
}

/// Reconciling an already-saved zone must still refresh its compatibility offset mirror across a
/// summer-time transition.
///
/// Regression target: `chrome::clock::reconcile_clock_zone` regaining an early return when a
/// zone is already saved (`if b.layout.header_clock_zone.is_some() { return; }` at the top) —
/// or, equivalently, `backend::set_header_clock_zone` dropping the
/// `|| self.layout.header_clock_offset_min != offset_min` half of its guard. Either edit stops
/// the mirror from tracking the chosen city's own transitions, leaving compatibility readers an
/// hour stale. The oracle is `reconcile_target` computing a DIFFERENT offset for the same saved
/// zone on either side of a known DST date (Warsaw's CEST/CET switch), a wall-clock fact
/// independent of how the function is implemented.
#[test]
fn reconciling_an_already_saved_zone_still_refreshes_its_legacy_offset() {
    let winter = at("2026-01-01T12:00:00Z");
    let summer = at("2026-07-01T12:00:00Z");
    let zone_id = Tz::Europe__Warsaw.name();

    let (winter_city, winter_offset) =
        reconcile_target(Some(zone_id), 60, winter).expect("an already-saved zone must resolve");
    let (summer_city, summer_offset) =
        reconcile_target(Some(zone_id), 60, summer).expect("an already-saved zone must resolve");

    assert_eq!(winter_city.code, "WAW");
    assert_eq!(summer_city.code, "WAW");
    assert_ne!(
        winter_offset, summer_offset,
        "the legacy offset mirror for an already-saved zone must track its own DST transitions, \
         not stay pinned to whatever it was when the zone was first chosen"
    );
}

/// A valid compatibility offset that no curated city currently observes still lands nearby.
///
/// The schema permits `+14`, which no curated city currently observes. Falling back to UTC would
/// move the clock by fourteen hours, so the nearest-city rule bounds the visible drift.
#[test]
fn an_offset_no_city_stands_at_lands_on_the_nearest_one() {
    let now = at("2026-07-01T12:00:00Z");
    let city = migrate_offset_min(14 * 60, now).expect("the far edge still resolves");
    let drift = (current_offset_min(city.zone, now) - 14 * 60).abs();
    assert!(
        drift <= 120,
        "{} is {drift} min from the saved offset — too far to call nearest",
        city.code
    );
}
