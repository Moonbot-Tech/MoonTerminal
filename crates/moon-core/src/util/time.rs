//! Shared Unix-time helpers: whole seconds for network deadlines, `f64` milliseconds for the chart
//! tick timeline, and `i64` milliseconds for logs and the database.
//!
//! This module also owns the crate's UTC Gregorian calendar implementation (`civil_from_days`),
//! consumed by `db` report timestamps and config/strategy backup snapshot directory names.
//! User-selected civil display belongs to `display_time` instead.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix time in whole seconds (`u64`) for absolute network scheduling deadlines.
/// Returns `0` if the system clock precedes the Unix epoch.
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Current Unix time in milliseconds (`f64`), on the same scale as market tick `time_ms` values.
/// Returns `0.0` if `duration_since(UNIX_EPOCH)` fails because the system time precedes the epoch.
pub fn now_unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Current Unix time in whole milliseconds (`i64`) for log timestamps and database records.
/// Returns `0` if `duration_since(UNIX_EPOCH)` fails because the system time precedes the epoch.
pub fn now_unix_ms_i64() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// How long an answer from [`local_utc_offset_ms`] is reused.
///
/// The read costs a zone lookup and a transition search, and it sits on the chart's market-revision
/// path — several panes, several times a second. A minute of staleness is invisible: the value
/// changes twice a year, and a daylight-saving jump is picked up within that minute.
const LOCAL_OFFSET_TTL_MS: i64 = 60_000;

/// This machine's offset from UTC right now, in milliseconds; `+2h` reads as `7_200_000`.
///
/// Exists for values that arrive already expressed in the CLIENT's local wall clock rather than in
/// UTC — the protocol's funding time is one (`apply_delphi_local_funding_shift` adds this very
/// offset while reading the wire). Subtracting it puts such a value back on the one scale
/// everything else here uses, so a countdown against [`now_unix_ms_i64`] is not off by a zone.
///
/// Deliberately NOT `chrono::Local`, which is the obvious way to write this: its Windows path ends
/// in an `unwrap` of an `Option` that the zone API can legitimately decline
/// (`TzInfo::for_year` → `MappedLocalTime::None`), and this is read from the chart's update path,
/// where a panic takes the whole application down. The system zone plus `chrono-tz` answers the
/// same question and has no such edge.
///
/// Returns:
///     Offset in milliseconds, or `0` when the platform states no usable zone — a terminal that
///     cannot learn its own zone reads the wire value as UTC, which is what it did before.
pub fn local_utc_offset_ms() -> i64 {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(i64, i64)>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(None));
    let now = now_unix_ms_i64();
    if let Some((at, offset)) = *cache.lock().unwrap_or_else(|e| e.into_inner()) {
        if now.saturating_sub(at) < LOCAL_OFFSET_TTL_MS {
            return offset;
        }
    }
    let offset = resolve_local_utc_offset_ms(now);
    *cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, offset));
    offset
}

/// The uncached reading behind [`local_utc_offset_ms`].
fn resolve_local_utc_offset_ms(now_ms: i64) -> i64 {
    use chrono::Offset as _;
    let Some(utc) = chrono::DateTime::from_timestamp_millis(now_ms) else {
        return 0;
    };
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|id| id.parse::<chrono_tz::Tz>().ok())
        .map(|zone| i64::from(utc.with_timezone(&zone).offset().fix().local_minus_utc()) * 1000)
        .unwrap_or(0)
}

/// Whole Unix milliseconds (`i64`) of an arbitrary `SystemTime`, for stamping a value whose clock
/// reading the caller already holds. Returns `0` for a time before the epoch, like its siblings.
pub fn unix_ms_i64_of(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Convert days since the Unix epoch to `(year, month, day)` in the proleptic Gregorian calendar.
///
/// This is the crate's single copy of Howard Hinnant's civil-from-days algorithm.
/// `db::fmt_unix*` and `config::backup` use it so UTC report contracts and snapshot directory names
/// cannot drift apart. User-facing strategy-version labels use `display_time` instead.
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
/// Two properties are required by the config and strategy backup domains:
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
mod tests;
