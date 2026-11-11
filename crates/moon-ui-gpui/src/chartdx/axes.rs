//! Selected-zone chart-axis helpers shared by every chart text path.

use std::sync::{LazyLock, RwLock};

use chrono::{LocalResult, TimeZone as _};
use chrono_tz::Tz;

/// Process-wide display zone mirrored from the single persisted header-clock selection.
static DISPLAY_ZONE: LazyLock<RwLock<Tz>> = LazyLock::new(|| RwLock::new(Tz::UTC));

/// Publish a selected display zone to retained chart renderers.
///
/// Args:
///     zone: IANA zone selected by the shared header clock.
///
/// Returns:
///     Nothing; subsequent chart text preparation reads the published zone.
pub fn set_display_zone(zone: Tz) {
    if let Ok(mut current) = DISPLAY_ZONE.write() {
        *current = zone;
    }
}

/// Return the currently selected display zone.
///
/// Returns:
///     Published IANA zone, or UTC after lock poisoning.
pub fn display_zone() -> Tz {
    DISPLAY_ZONE.read().map(|zone| *zone).unwrap_or(Tz::UTC)
}

/// Return selected-zone civil boundaries that fall inside one chart window.
///
/// Args:
///     left_ms: Inclusive UTC window start in Unix milliseconds.
///     right_ms: Inclusive UTC window end in Unix milliseconds.
///     step_ms: Civil spacing between round labels in milliseconds; a month step from
///         `moon_chart::axes::time_step` means first-of-month midnights, and a week step
///         starts on Mondays.
///
/// Returns:
///     UTC instants whose selected-zone wall clocks land on the requested round boundaries.
pub fn aligned_ticks_ms(left_ms: f64, right_ms: f64, step_ms: f64) -> Vec<f64> {
    aligned_ticks_ms_in_zone(left_ms, right_ms, step_ms, display_zone())
}

/// Calculate chart ticks against an explicit zone so DST behavior is deterministic in tests.
///
/// Args:
///     left_ms: Inclusive UTC window start in Unix milliseconds.
///     right_ms: Inclusive UTC window end in Unix milliseconds.
///     step_ms: Civil spacing between round labels in milliseconds.
///     zone: IANA zone used to resolve each boundary independently.
///
/// Returns:
///     Deduplicated UTC instants for valid civil boundaries inside the window.
fn aligned_ticks_ms_in_zone(left_ms: f64, right_ms: f64, step_ms: f64, zone: Tz) -> Vec<f64> {
    let left = left_ms as i64;
    let right = right_ms as i64;
    let step = step_ms.round() as i64;
    if step <= 0 || right < left {
        return Vec::new();
    }
    let Some(local_left) = moon_core::util::display_time::at_millis(left, zone) else {
        return Vec::new();
    };
    // Wall-clock milliseconds: the local reading of the left edge, counted as if it were UTC.
    let local_axis = local_left.naive_local().and_utc().timestamp_millis();
    match moon_chart::axes::calendar_months(step_ms / 1000.0) {
        Some(months) => resolve_boundaries(
            month_boundaries(local_axis, months),
            left,
            right,
            zone,
            true,
        ),
        None => {
            // Anchored on a Monday so week steps start the week; every clock and day step up to
            // two days divides the four-day offset, so their lattice is the epoch one unchanged.
            let mut boundary =
                (local_axis - WEEK_ANCHOR_MS).div_euclid(step) * step + WEEK_ANCHOR_MS;
            if boundary < local_axis {
                boundary = boundary.saturating_add(step);
            }
            let boundaries = std::iter::successors(Some(boundary), move |b| b.checked_add(step));
            resolve_boundaries(boundaries, left, right, zone, step >= DAY_MS)
        }
    }
}

/// One civil day in milliseconds.
const DAY_MS: i64 = 86_400_000;

/// 1970-01-05 00:00, the first Monday after the Unix epoch, in milliseconds.
const WEEK_ANCHOR_MS: i64 = 4 * DAY_MS;

/// How far past a midnight skipped by a DST gap a date boundary looks for the first real instant.
const MIDNIGHT_GAP_SEARCH_MS: i64 = 3 * 3_600_000;

/// Granularity of that search; every real offset change lands on a quarter hour.
const MIDNIGHT_GAP_PROBE_MS: i64 = 15 * 60_000;

/// Wall-clock midnights on the first of every `months`-th month, from the first at or after
/// `local_axis`.
///
/// Months are counted from year zero, so three months fall on quarters, six on half-years and
/// twelve on 1 January.
///
/// Args:
///     local_axis: Wall-clock milliseconds of the window's left edge.
///     months: Month count per step, at least one.
///
/// Returns:
///     Wall-clock milliseconds of each boundary, ascending.
fn month_boundaries(local_axis: i64, months: u32) -> impl Iterator<Item = i64> {
    use chrono::Datelike as _;
    let step = i64::from(months.max(1));
    let first_of = |index: i64| {
        let year = i32::try_from(index.div_euclid(12)).ok()?;
        let month = u32::try_from(index.rem_euclid(12)).ok()? + 1;
        chrono::NaiveDate::from_ymd_opt(year, month, 1)
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .map(|midnight| midnight.and_utc().timestamp_millis())
    };
    let start = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(local_axis).map(|value| {
        let index = i64::from(value.year()) * 12 + i64::from(value.month0());
        let index = index.div_euclid(step) * step;
        if first_of(index).is_some_and(|ms| ms < local_axis) {
            index + step
        } else {
            index
        }
    });
    std::iter::successors(start, move |index| index.checked_add(step)).map_while(first_of)
}

/// Resolve ascending wall-clock boundaries in a zone and keep the instants inside the window.
///
/// Clock boundaries: an ambiguous fall-back boundary yields both instants, one skipped by a
/// spring-forward gap yields none. Date boundaries (`whole_days`) stand for the day itself, so
/// they yield one instant each: the earlier of an ambiguous pair, and for a midnight a DST gap
/// skips (zones that change at 00:00, such as Africa/Cairo) the first real instant after it —
/// dropping it would lose that day's or month's only caption.
///
/// Args:
///     boundaries: Ascending wall-clock milliseconds.
///     left: Inclusive UTC window start in Unix milliseconds.
///     right: Inclusive UTC window end in Unix milliseconds.
///     zone: IANA zone the wall clock is read in.
///     whole_days: Whether every boundary is a local midnight standing for a date.
///
/// Returns:
///     Sorted, deduplicated UTC instants inside the window.
fn resolve_boundaries(
    boundaries: impl Iterator<Item = i64>,
    left: i64,
    right: i64,
    zone: Tz,
    whole_days: bool,
) -> Vec<f64> {
    let resolve = |boundary: i64| {
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(boundary)
            .map(|value| zone.from_local_datetime(&value.naive_utc()))
    };
    let mut ticks = Vec::new();
    for boundary in boundaries.take(4096) {
        let Some(mut resolved) = resolve(boundary) else {
            break;
        };
        if whole_days && matches!(resolved, LocalResult::None) {
            let mut probe = MIDNIGHT_GAP_PROBE_MS;
            while matches!(resolved, LocalResult::None) && probe <= MIDNIGHT_GAP_SEARCH_MS {
                resolved = resolve(boundary + probe).unwrap_or(LocalResult::None);
                probe += MIDNIGHT_GAP_PROBE_MS;
            }
        }
        let candidates = match resolved {
            LocalResult::Single(value) => [Some(value.timestamp_millis()), None],
            LocalResult::Ambiguous(first, second) => {
                let first = first.timestamp_millis();
                let second = second.timestamp_millis();
                let later = (!whole_days).then_some(first.max(second));
                [Some(first.min(second)), later]
            }
            LocalResult::None => [None, None],
        };
        if candidates[0].is_some_and(|unix| unix > right) {
            break;
        }
        for unix in candidates.into_iter().flatten() {
            if unix >= left && unix <= right {
                ticks.push(unix as f64);
            }
        }
    }
    ticks.sort_by(f64::total_cmp);
    ticks.dedup();
    ticks
}

/// Format a chart instant in the selected zone, adding a date outside selected-zone today.
///
/// Args:
///     unix_ms: Event timestamp in UTC Unix milliseconds.
///     with_seconds: Whether to include seconds.
///     now_ms: Current UTC Unix milliseconds used for the civil-day comparison.
///
/// Returns:
///     Selected-zone clock label, optionally prefixed by `DD.MM`.
pub fn format_clock_dated(unix_ms: f64, with_seconds: bool, now_ms: f64) -> String {
    moon_core::util::display_time::format_chart_clock(
        unix_ms as i64,
        display_zone(),
        with_seconds,
        now_ms as i64,
    )
}

/// Format a day- or month-step axis caption in the selected zone: `DD.MM`, or the year on
/// 1 January.
///
/// Args:
///     unix_ms: Caption instant in UTC Unix milliseconds.
///
/// Returns:
///     The date caption.
pub fn format_date_caption(unix_ms: f64) -> String {
    moon_core::util::display_time::format_chart_date(unix_ms as i64, display_zone())
}

/// Format a chart instant in the selected zone WITH milliseconds, adding a date outside
/// selected-zone today.
///
/// The millisecond sibling of [`format_clock_dated`]. Separate rather than a flag, so a
/// second-resolution value has no code path that could print a fraction the core never gave.
///
/// Args:
///     unix_ms: Event timestamp in UTC Unix milliseconds.
///     now_ms: Current UTC Unix milliseconds used for the civil-day comparison.
///
/// Returns:
///     Selected-zone `HH:MM:SS.mmm` label, optionally prefixed by `DD.MM`.
pub fn format_clock_dated_ms(unix_ms: i64, now_ms: i64) -> String {
    moon_core::util::display_time::format_chart_clock_ms(unix_ms, display_zone(), now_ms)
}

#[cfg(test)]
mod tests;
