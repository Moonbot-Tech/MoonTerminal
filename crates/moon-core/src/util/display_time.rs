//! User-selected civil-time conversion over UTC Unix timestamps.
//!
//! Persistence, protocols, logs, and market-data minutes keep UTC instants. This module is the
//! shared seam for UI dates and analytical grouping that must follow one selected IANA zone,
//! including historical daylight-saving transitions.

use chrono::{DateTime, Datelike, Days, Duration, LocalResult, NaiveDate, NaiveDateTime, Offset};
use chrono::{TimeZone as _, Timelike as _, Utc};
use chrono_tz::Tz;

/// How a local picker value resolves when the wall clock repeats during a fall-back transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalBoundary {
    /// The first occurrence, used by inclusive lower bounds.
    Lower,
    /// The last occurrence, used by inclusive or exclusive upper bounds.
    Upper,
}

/// Convert UTC Unix seconds into the selected IANA zone.
///
/// Args:
///     secs: UTC Unix timestamp in seconds.
///     zone: Selected display zone.
///
/// Returns:
///     Zone-aware date and time, or `None` outside chrono's representable range.
pub fn at(secs: i64, zone: Tz) -> Option<DateTime<Tz>> {
    DateTime::from_timestamp(secs, 0).map(|value| value.with_timezone(&zone))
}

/// Convert UTC Unix milliseconds into the selected IANA zone.
///
/// Args:
///     millis: UTC Unix timestamp in milliseconds.
///     zone: Selected display zone.
///
/// Returns:
///     Zone-aware date and time, or `None` outside chrono's representable range.
pub fn at_millis(millis: i64, zone: Tz) -> Option<DateTime<Tz>> {
    DateTime::from_timestamp_millis(millis).map(|value| value.with_timezone(&zone))
}

/// Resolve a whole-minute civil picker value into a UTC Unix instant.
///
/// Ambiguous fall-back minutes use the occurrence selected by `boundary`. A spring-forward gap
/// clamps to the first valid local minute after the gap, so a picker never invents an instant by
/// reinterpreting nonexistent wall time as UTC.
///
/// Args:
///     local: Civil date and whole-minute clock value shown by the picker.
///     zone: Selected display zone.
///     boundary: Lower or upper range edge, which selects an ambiguous occurrence.
///
/// Returns:
///     UTC Unix seconds, or `None` when no valid local instant exists within the following 48 hours.
pub fn unix_from_local(local: NaiveDateTime, zone: Tz, boundary: LocalBoundary) -> Option<i64> {
    for minute in 0..=48 * 60 {
        let candidate = local.checked_add_signed(Duration::minutes(minute))?;
        match zone.from_local_datetime(&candidate) {
            LocalResult::Single(value) => return Some(value.timestamp()),
            LocalResult::Ambiguous(first, second) => {
                let value = match boundary {
                    LocalBoundary::Lower => first.min(second),
                    LocalBoundary::Upper => first.max(second),
                };
                return Some(value.timestamp());
            }
            LocalResult::None => {}
        }
    }
    None
}

/// Resolve the first real instant of a civil date in the selected zone.
///
/// Historical zones occasionally jump at midnight or skip a complete date. Resolution therefore
/// shares the picker gap policy instead of assuming every date owns a `00:00` instant.
///
/// Args:
///     date: Civil date whose beginning is required.
///     zone: Selected display zone.
///
/// Returns:
///     UTC Unix seconds of the first valid minute on or after the date, or `None` at chrono limits.
pub fn day_start(date: NaiveDate, zone: Tz) -> Option<i64> {
    unix_from_local(date.and_hms_opt(0, 0, 0)?, zone, LocalBoundary::Lower)
}

/// Resolve a civil date only when that date exists in the selected zone.
///
/// A midnight gap may resolve later on the same date, while a complete date skip must remain
/// distinguishable from the following date.
///
/// Args:
///     date: Civil date whose first real instant is required.
///     zone: Selected display zone.
///
/// Returns:
///     First real instant belonging to `date`, or `None` when the date is fully skipped.
pub fn exact_day_start(date: NaiveDate, zone: Tz) -> Option<i64> {
    let start = day_start(date, zone)?;
    (self::date(start, zone) == Some(date)).then_some(start)
}

/// Resolve the nearest existing civil date in one requested direction.
///
/// Args:
///     date: First civil date to try.
///     zone: Selected display zone.
///     direction: Negative to search earlier dates, non-negative to search later dates.
///
/// Returns:
///     First existing civil-day start in the requested direction, or `None` at chrono limits.
pub fn resolve_day_start(date: NaiveDate, zone: Tz, direction: i64) -> Option<i64> {
    let step = if direction < 0 { -1 } else { 1 };
    let mut candidate = date;
    for _ in 0..=366 {
        if let Some(start) = exact_day_start(candidate, zone) {
            return Some(start);
        }
        let next = shift_date(candidate, step);
        if next == candidate {
            break;
        }
        candidate = next;
    }
    None
}

/// Shift from one existing civil day by a count of other existing civil days.
///
/// Fully skipped historical dates do not consume a step. This keeps presets and fixed-row calendar
/// windows at their promised number of real days across dateline changes.
///
/// Args:
///     start: UTC instant belonging to the starting civil date.
///     days: Signed number of existing civil days to move.
///     zone: Selected display zone.
///
/// Returns:
///     Target existing civil-day start, or `None` at chrono limits.
pub fn shift_day_start(start: i64, days: i64, zone: Tz) -> Option<i64> {
    let direction = if days < 0 { -1 } else { 1 };
    let mut current = exact_day_start(self::date(start, zone)?, zone)?;
    for _ in 0..days.unsigned_abs() {
        let current_date = self::date(current, zone)?;
        let candidate = shift_date(current_date, direction);
        if candidate == current_date {
            return None;
        }
        current = resolve_day_start(candidate, zone, direction)?;
    }
    Some(current)
}

/// Shift a civil date without assuming a fixed number of UTC seconds per day.
///
/// Args:
///     date: Civil date to move.
///     days: Signed number of calendar dates.
///
/// Returns:
///     Shifted date, or the input date at chrono's representable boundary.
pub fn shift_date(date: NaiveDate, days: i64) -> NaiveDate {
    if days >= 0 {
        date.checked_add_days(Days::new(days as u64))
            .unwrap_or(date)
    } else {
        date.checked_sub_days(Days::new(days.unsigned_abs()))
            .unwrap_or(date)
    }
}

/// First civil day of the calendar month preceding `date`'s month.
///
/// A month never wraps within one year alone — January's preceding month is December of the PRIOR
/// year — so this is the one place that year rollback lives, shared by every "previous calendar
/// month" preset instead of each caller re-deriving the carry. Month LENGTH (28/29/30/31) never
/// enters this function: callers resolve a preset's upper bound from the following month's own
/// start rather than counting days, so a 31-to-30-day boundary needs no special case here either.
///
/// Args:
///     date: Civil date whose preceding month is required.
///
/// Returns:
///     First day of the preceding calendar month, or `date` itself at chrono's representable
///     boundary.
pub fn prev_month_start(date: NaiveDate) -> NaiveDate {
    let (year, month) = if date.month() == 1 {
        (date.year() - 1, 12)
    } else {
        (date.year(), date.month() - 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(date)
}

/// Both civil-month starts a "previous calendar month" preset needs, from today's civil date.
///
/// Every such preset (Analytics, Report, the Profit Monitor) agrees on exactly these two dates
/// before diverging into its own inclusive/exclusive upper-bound convention — this factors out
/// only that shared pair, never the bound conventions themselves, which stay in each caller.
///
/// Args:
///     today: Civil date to derive the current calendar month's start from.
///
/// Returns:
///     `(previous month's first day, current month's first day)`.
pub fn prev_and_cur_month_start(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let cur_month_start = today.with_day(1).unwrap_or(today);
    (prev_month_start(cur_month_start), cur_month_start)
}

/// Return the selected zone's civil date at one UTC instant.
///
/// Args:
///     secs: UTC Unix timestamp in seconds.
///     zone: Selected display zone.
///
/// Returns:
///     Local civil date, or `None` outside chrono's representable range.
pub fn date(secs: i64, zone: Tz) -> Option<NaiveDate> {
    at(secs, zone).map(|value| value.date_naive())
}

/// Return the current offset for one historical instant rather than for process launch time.
///
/// Args:
///     secs: UTC Unix timestamp in seconds.
///     zone: Selected display zone.
///
/// Returns:
///     Signed seconds east of UTC, or zero outside chrono's representable range.
pub fn offset_seconds(secs: i64, zone: Tz) -> i32 {
    at(secs, zone)
        .map(|value| value.offset().fix().local_minus_utc())
        .unwrap_or(0)
}

/// Format UTC Unix seconds as `YYYY-MM-DD HH:MM` in the selected zone.
///
/// Args:
///     secs: UTC Unix timestamp in seconds; non-positive values mean unknown.
///     zone: Selected display zone.
///
/// Returns:
///     Formatted civil time or an empty string for an unknown/out-of-range value.
pub fn format_minute(secs: i64, zone: Tz) -> String {
    if secs <= 0 {
        return String::new();
    }
    at(secs, zone)
        .map(|value| value.format(MINUTE_FORMAT).to_string())
        .unwrap_or_default()
}

/// The one date-and-minute pattern [`format_minute`] prints.
const MINUTE_FORMAT: &str = "%Y-%m-%d %H:%M";

/// Format UTC Unix seconds as `YYYY-MM-DD` in the selected zone.
///
/// Args:
///     secs: UTC Unix timestamp in seconds; non-positive values mean unknown.
///     zone: Selected display zone.
///
/// Returns:
///     Formatted civil date or an empty string for an unknown/out-of-range value.
pub fn format_date(secs: i64, zone: Tz) -> String {
    if secs <= 0 {
        return String::new();
    }
    at(secs, zone)
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Format UTC Unix seconds as `YYYY-MM-DD HH:MM:SS` in the selected zone.
///
/// Args:
///     secs: UTC Unix timestamp in seconds.
///     zone: Selected display zone.
///
/// Returns:
///     Formatted civil time or an empty string outside chrono's representable range.
pub fn format_second(secs: i64, zone: Tz) -> String {
    at(secs, zone)
        .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

/// Format a stored UTC log timestamp as a selected-zone clock with milliseconds.
///
/// Args:
///     text: Stored `YYYY-MM-DD HH:MM:SS.mmm` UTC timestamp.
///     zone: Selected application-wide IANA display zone.
///
/// Returns:
///     Selected-zone `HH:MM:SS.mmm`, or the original clock suffix when parsing fails.
pub fn format_utc_millis_clock(text: &str, zone: Tz) -> String {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.3f")
        .map(|value| {
            value
                .and_utc()
                .with_timezone(&zone)
                .format("%H:%M:%S%.3f")
                .to_string()
        })
        .unwrap_or_else(|_| text.rsplit(' ').next().unwrap_or(text).to_string())
}

/// Format a chart instant as local clock time, adding `DD.MM` when it is not today locally.
///
/// Args:
///     unix_ms: Event timestamp in UTC Unix milliseconds.
///     zone: Selected display zone.
///     with_seconds: Whether the clock includes seconds.
///     now_ms: Current UTC Unix milliseconds used for the local-day comparison.
///
/// Returns:
///     `HH:MM[:SS]`, or `DD.MM HH:MM[:SS]` for another local date.
pub fn format_chart_clock(unix_ms: i64, zone: Tz, with_seconds: bool, now_ms: i64) -> String {
    let Some(value) = at_millis(unix_ms, zone) else {
        return String::new();
    };
    let clock = if with_seconds {
        value.format("%H:%M:%S").to_string()
    } else {
        value.format("%H:%M").to_string()
    };
    if at_millis(now_ms, zone).is_some_and(|now| now.date_naive() == value.date_naive()) {
        clock
    } else {
        format!("{} {clock}", value.format("%d.%m"))
    }
}

/// Return the start of the civil bucket containing one UTC instant.
///
/// The supported analytical grids are whole local hours, days, or epoch-aligned seven-day spans.
/// Repeated fall-back hours intentionally share one displayed bucket; a skipped spring hour has no
/// bucket. The returned value is the UTC instant of the bucket's first occurrence.
///
/// Args:
///     secs: UTC Unix timestamp in seconds.
///     bucket_secs: Civil bucket width in seconds.
///     zone: Selected display zone.
///
/// Returns:
///     UTC Unix seconds of the bucket start, or `None` for an invalid timestamp or bucket width.
pub fn bucket_start(secs: i64, bucket_secs: i64, zone: Tz) -> Option<i64> {
    if bucket_secs <= 0 {
        return None;
    }
    let local = at(secs, zone)?.naive_local();
    let local_axis = local.and_utc().timestamp();
    let bucket_axis = local_axis.div_euclid(bucket_secs) * bucket_secs;
    let bucket_local = DateTime::<Utc>::from_timestamp(bucket_axis, 0)?.naive_utc();
    unix_from_local(bucket_local, zone, LocalBoundary::Lower)
}

/// Advance one civil analytical bucket without assuming its UTC duration.
///
/// Args:
///     start: UTC instant returned by [`bucket_start`] or this function.
///     bucket_secs: Civil bucket width in seconds.
///     zone: Selected display zone.
///
/// Returns:
///     UTC instant of the next civil bucket, or `None` at chrono limits.
pub fn next_bucket(start: i64, bucket_secs: i64, zone: Tz) -> Option<i64> {
    if bucket_secs <= 0 {
        return None;
    }
    let local = at(start, zone)?.naive_local();
    let local_axis = local.and_utc().timestamp();
    let boundary_axis = local_axis.div_euclid(bucket_secs) * bucket_secs;
    let next_axis = boundary_axis.checked_add(bucket_secs)?;
    let next_local = DateTime::<Utc>::from_timestamp(next_axis, 0)?.naive_utc();
    unix_from_local(next_local, zone, LocalBoundary::Lower)
}

/// Return local minute-of-day for a historical UTC instant.
///
/// Args:
///     secs: UTC Unix timestamp in seconds.
///     zone: Selected display zone.
///
/// Returns:
///     `0..=1439`, or `None` outside chrono's representable range.
pub fn minute_of_day(secs: i64, zone: Tz) -> Option<i64> {
    at(secs, zone).map(|value| i64::from(value.hour() * 60 + value.minute()))
}

/// Return local minute-of-week for a historical UTC instant, Monday as day zero.
///
/// Args:
///     secs: UTC Unix timestamp in seconds.
///     zone: Selected display zone.
///
/// Returns:
///     `0..=10079`, or `None` outside chrono's representable range.
pub fn minute_of_week(secs: i64, zone: Tz) -> Option<i64> {
    at(secs, zone).map(|value| {
        i64::from(value.weekday().num_days_from_monday()) * 1_440
            + i64::from(value.hour() * 60 + value.minute())
    })
}

#[cfg(test)]
mod tests;
