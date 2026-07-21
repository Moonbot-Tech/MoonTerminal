//! Единый источник unix-времени. До рефактора каждый модуль (feed/live, feed/synth,
//! session/order_lines, applog, db) держал свою копию `now_ms` — одна и та же формула
//! `SystemTime::now() - UNIX_EPOCH` в пяти местах. Свели сюда: f64-мс для шкалы тиков
//! чарта, i64-мс для логов/БД.
//!
//! This module also owns the crate's single Gregorian calendar implementation
//! (`civil_from_days`), consumed by `db` for report timestamps, `strat_db` for compact version
//! labels, and `config::backup` for snapshot directory names.

use std::time::{SystemTime, UNIX_EPOCH};

/// Текущее unix-время в мс (f64). Та же шкала, что `time_ms` тиков рынка.
pub fn now_unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Текущее unix-время в целых мс (i64) — для меток логов и записей БД.
pub fn now_unix_ms_i64() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Convert days since the Unix epoch to `(year, month, day)` in the proleptic Gregorian calendar.
///
/// This is the crate's single copy of Howard Hinnant's civil-from-days algorithm.
/// `db::fmt_unix*`, `strat_db::stats::short_date`, and `config::backup` all use it so report dates,
/// strategy-version labels, and snapshot directory names cannot drift apart.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Lower `utc_stamp_compact` bound for times before year 0.
const STAMP_MIN: &str = "00000101-000000";
/// Upper `utc_stamp_compact` bound for times after year 9999.
const STAMP_MAX: &str = "99991231-235959";

/// Convert Unix milliseconds to a filename-safe UTC timestamp, `YYYYMMDD-HHMMSS`.
///
/// Two properties are required by `config::backup`:
/// - **Windows compatibility**, because `:` is forbidden and rules out `HH:MM:SS`.
/// - **Lexicographic order equals chronological order**, provided by fixed width, leading zeroes,
///   and most-significant components first. Snapshot pruning therefore sorts by NAME instead of
///   mtime, which file copying and cloud synchronization can change.
///
/// UTC is used instead of local time because the clock moves backward during a daylight-saving
/// transition and would violate ordering.
///
/// Years 0000-9999 are supported. `{y:04}` specifies a minimum rather than a fixed width, so a year
/// outside the range would change the string LENGTH and violate both properties. Such times clamp
/// to a boundary timestamp instead of producing an invalid name.
pub fn utc_stamp_compact(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    if y < 0 {
        return STAMP_MIN.to_string();
    }
    if y > 9999 {
        return STAMP_MAX.to_string();
    }
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

#[cfg(test)]
mod tests {
    //! Contract for compact UTC timestamps used in snapshot directory names.

    use super::{utc_stamp_compact, STAMP_MAX, STAMP_MIN};

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
}
