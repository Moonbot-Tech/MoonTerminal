//! Contract for compact UTC timestamps used in snapshot directory names.

use super::{local_utc_offset_ms, utc_stamp_compact, STAMP_MAX, STAMP_MIN};

/// The timestamp has fixed width and a separator at a fixed offset.
///
/// The plausible breakage is switching for readability to `DD.MM.YYYY_HH-MM-SS`, as used in
/// `analytics/period.rs` and `analytics/toolbar.rs`. That format puts the day first, so pruning
/// by name would order entries by day-of-month rather than date.
#[test]
fn the_stamp_is_fixed_width_with_the_separator_at_a_fixed_offset() {
    for ms in [0_i64, 1, 1_753_100_000_000, 4_102_444_800_000] {
        let s = utc_stamp_compact(ms);
        assert_eq!(s.len(), 15, "stamp `{s}` is not 15 chars");
        assert_eq!(&s[8..9], "-", "stamp `{s}` has no separator at index 8");
        assert!(
            s.bytes()
                .enumerate()
                .all(|(i, b)| i == 8 || b.is_ascii_digit()),
            "stamp `{s}` holds a non-digit outside index 8"
        );
    }
}

/// The Unix epoch renders as a known constant, pinning the calendar conversion too.
///
/// The oracle is independent of the code: 1970-01-01T00:00:00Z defines the Unix epoch rather
/// than being a value read back from this function.
#[test]
fn the_epoch_renders_as_the_start_of_1970() {
    assert_eq!(utc_stamp_compact(0), "19700101-000000");
}

/// Sorting timestamps as STRINGS matches the order of the source instants.
///
/// Snapshot pruning relies on this property to retain the newest N entries.
///
/// The instants make the DAY OF MONTH decrease across the boundary (Jan 31 -> Feb 1). This
/// distinguishes formats: an increasing day would let `DD.MM.YYYY_HH-MM-SS` sort correctly by
/// accident, while this pair yields `31.01.2026` > `01.02.2026` and fails. Removing leading
/// zeroes also reverses this pair.
#[test]
fn string_order_matches_chronological_order_across_a_month_boundary() {
    // 2026-01-31 12:00:00Z, followed by the next day.
    let jan31 = 1_769_860_800_000_i64;
    let feb01 = jan31 + 86_400_000;

    let a = utc_stamp_compact(jan31);
    let b = utc_stamp_compact(feb01);

    // Pin the conversion too so an incorrect fixture cannot make the test pass by accident.
    assert_eq!(a, "20260131-120000");
    assert_eq!(b, "20260201-120000");
    assert!(
        a < b,
        "{a} must sort before {b} despite the smaller day number"
    );
}

/// A time outside years 0000-9999 clamps to a boundary without changing the name width.
///
/// The snapshot reader rejects a 16-character name, which also sorts incorrectly beside a
/// 15-character name. A negative year adds a leading `-` that sorts BEFORE digits and makes an
/// old entry appear new.
#[test]
fn a_year_outside_the_supported_range_clamps_instead_of_changing_width() {
    assert_eq!(utc_stamp_compact(i64::MIN / 2), STAMP_MIN);
    assert_eq!(utc_stamp_compact(i64::MAX / 2), STAMP_MAX);
    assert_eq!(STAMP_MIN.len(), 15);
    assert_eq!(STAMP_MAX.len(), 15);
}

/// The offset must be a real zone offset, never a panic and never nonsense.
///
/// Breakage: the obvious `chrono::Local::now()` unwraps an `Option` the Windows zone API can
/// decline, and this is read from the chart's update path — where a panic ends the process.
#[test]
fn the_local_offset_is_a_plausible_zone_offset() {
    let offset = local_utc_offset_ms();
    assert!(
        offset.abs() <= 14 * 3_600_000,
        "no zone is further than 14 hours from UTC: {offset}"
    );
    // Zones are whole minutes; anything else means the reading came from the wrong unit.
    assert_eq!(offset % 60_000, 0, "offset must be whole minutes: {offset}");
    // Cached and uncached must agree, or a caption would flicker between two answers.
    assert_eq!(offset, local_utc_offset_ms());
}
