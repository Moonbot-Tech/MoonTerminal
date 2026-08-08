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
///     step_ms: Civil spacing between round labels in milliseconds.
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
    let local_axis = local_left.naive_local().and_utc().timestamp_millis();
    let mut boundary = local_axis.div_euclid(step) * step;
    if boundary < local_axis {
        boundary = boundary.saturating_add(step);
    }

    let mut ticks = Vec::new();
    for _ in 0..4096 {
        let Some(local) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(boundary)
            .map(|value| value.naive_utc())
        else {
            break;
        };
        let candidates = match zone.from_local_datetime(&local) {
            LocalResult::Single(value) => [Some(value.timestamp_millis()), None],
            LocalResult::Ambiguous(first, second) => {
                let first = first.timestamp_millis();
                let second = second.timestamp_millis();
                [Some(first.min(second)), Some(first.max(second))]
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
        boundary = boundary.saturating_add(step);
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

#[cfg(test)]
mod tests;
