//! Единый источник unix-времени. До рефактора каждый модуль (feed/live, feed/synth,
//! session/order_lines, applog, db) держал свою копию `now_ms` — одна и та же формула
//! `SystemTime::now() - UNIX_EPOCH` в пяти местах. Свели сюда: f64-мс для шкалы тиков
//! чарта, i64-мс для логов/БД.
//!
//! Здесь же живёт единственная в крейте реализация григорианского календаря
//! (`civil_from_days`) — её потребляют `db` (форматирование меток отчётов),
//! `strat_db` (краткая подпись версии) и `config::backup` (имя папки снапшота).

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

/// Days since the unix epoch -> `(year, month, day)` in the proleptic Gregorian calendar (UTC).
///
/// Howard Hinnant's civil-from-days algorithm. The one copy in this crate: `db::fmt_unix*`,
/// `strat_db::stats::short_date` and `config::backup` all route through it, so a date rendered in
/// a report, a strategy version label and a backup folder name cannot disagree.
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

/// Lowest stamp `utc_stamp_compact` will emit — the clamp for a pre-year-0 timestamp.
const STAMP_MIN: &str = "00000101-000000";
/// Highest stamp `utc_stamp_compact` will emit — the clamp for a post-year-9999 timestamp.
const STAMP_MAX: &str = "99991231-235959";

/// unix-ms -> `YYYYMMDD-HHMMSS` in UTC: a filename-legal, fixed-width timestamp.
///
/// Two properties callers depend on, both load-bearing for `config::backup`:
/// - **Filename-legal on Windows**, which forbids `:` — so no `HH:MM:SS` form can be used.
/// - **Lexicographic order equals chronological order**, because the output is fixed-width,
///   zero-padded and most-significant-first. That is what lets snapshot pruning sort by NAME and
///   never consult mtime (which a file copy or a cloud sync can rewrite).
///
/// UTC rather than local time on purpose: local time runs BACKWARDS for an hour at a DST
/// fall-back, which would break the ordering property once a year.
///
/// Supported range is years 0000-9999. `{y:04}` is a MINIMUM width, not a fixed one, so a year
/// outside that range would change the string LENGTH and silently destroy both properties above;
/// such timestamps clamp to a boundary stamp instead of emitting a malformed name.
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
    //! Contract for the compact UTC stamp used to name backup snapshots.

    use super::{utc_stamp_compact, STAMP_MAX, STAMP_MIN};

    /// The stamp must be fixed-width with the separator at a fixed offset.
    ///
    /// Protects the layout against the plausible edit: someone reformats this to the
    /// human-readable `DD.MM.YYYY_HH-MM-SS` used elsewhere in the app (`analytics/period.rs`,
    /// `analytics/toolbar.rs`) because it reads nicer in a folder listing. That form is
    /// day-major, so snapshot pruning — which sorts by name — would start deleting by day of
    /// month rather than by date.
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

    /// The epoch renders as a known constant, pinning the calendar conversion itself.
    ///
    /// The oracle is independent of this code: 1970-01-01T00:00:00Z is the definition of the
    /// unix epoch, not a value read back from the function.
    #[test]
    fn the_epoch_renders_as_the_start_of_1970() {
        assert_eq!(utc_stamp_compact(0), "19700101-000000");
    }

    /// Sorting stamps as STRINGS must equal sorting the instants they came from.
    ///
    /// This is the property snapshot pruning relies on to keep the newest N. The mutation that
    /// breaks it is any variable-width or non-most-significant-first format.
    #[test]
    fn string_order_matches_chronological_order() {
        let base = 1_753_100_000_000_i64;
        let one_second = utc_stamp_compact(base + 1_000);
        let one_year = utc_stamp_compact(base + 366 * 86_400 * 1_000);
        let now = utc_stamp_compact(base);

        assert!(now < one_second, "{now} should sort before {one_second}");
        assert!(
            one_second < one_year,
            "{one_second} should sort before {one_year}"
        );
    }

    /// A timestamp outside years 0000-9999 must clamp, never emit a different-length name.
    ///
    /// A 16-char name would be rejected by the snapshot reader and would sort wrongly against
    /// every 15-char sibling; a negative year would emit a leading `-`, which sorts BEFORE every
    /// digit and would make the oldest-looking entry the newest.
    #[test]
    fn a_year_outside_the_supported_range_clamps_instead_of_changing_width() {
        assert_eq!(utc_stamp_compact(i64::MIN / 2), STAMP_MIN);
        assert_eq!(utc_stamp_compact(i64::MAX / 2), STAMP_MAX);
        assert_eq!(STAMP_MIN.len(), 15);
        assert_eq!(STAMP_MAX.len(), 15);
    }
}
