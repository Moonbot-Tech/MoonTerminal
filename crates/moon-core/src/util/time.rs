//! Shared Unix-time helpers for feed/live, feed/synth, session/order_lines, applog, and db
//! consumers. Before the refactor, those areas kept five copies of `now_ms` using the same
//! `SystemTime::now() - UNIX_EPOCH` formula. Their common helpers were consolidated here: `f64`
//! milliseconds for the chart tick timeline and `i64` milliseconds for logs and the database.
//!
//! This module also owns the crate's single Gregorian calendar implementation
//! (`civil_from_days`), consumed by `db` for report timestamps, `strat_db` for compact version
//! labels, and `config::backup` for snapshot directory names.

use std::time::{SystemTime, UNIX_EPOCH};

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
mod tests;
