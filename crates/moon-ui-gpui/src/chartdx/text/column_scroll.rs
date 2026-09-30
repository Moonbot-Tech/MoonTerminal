//! Which rows of a scrolled label column are on screen, and how a wheel event moves it.
//!
//! Pure arithmetic, kept apart from the caption builder so the window a column shows and the
//! offset a wheel notch produces are decided in one place.

use std::ops::Range;

use moon_core::config::label_scroll_first_max;

/// The visible part of a column of `above + range.len() + below` rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::chartdx) struct Window {
    /// Rows hidden above the window; a "N above" line is drawn when non-zero.
    pub above: usize,
    /// Indices of the rows drawn.
    pub range: Range<usize>,
    /// Rows hidden below the window; a "N more" line is drawn when non-zero.
    pub below: usize,
}

/// The first row a column of label row `row` is scrolled to; 0 when it is not scrolled.
pub(crate) fn first_of(scroll: &[(usize, u32)], row: usize) -> u32 {
    scroll
        .iter()
        .find(|(r, _)| *r == row)
        .map_or(0, |(_, first)| *first)
}

/// Clamp a requested first row into `0..=label_scroll_first_max(total)`.
pub(in crate::chartdx) fn clamp_first(first: i64, total: usize) -> u32 {
    let max = i64::try_from(label_scroll_first_max(total)).unwrap_or(i64::MAX);
    u32::try_from(first.clamp(0, max)).unwrap_or(u32::MAX)
}

/// The rows a column of `total` rows shows from `first`, within `budget` lines.
///
/// The indicator lines count against the budget. An unscrolled column that fits shows every row
/// and no indicator, so it is drawn exactly as it was before scrolling existed. `budget` is
/// expected to leave room for both indicators and one row (at least 3).
pub(in crate::chartdx) fn visible_window(total: usize, first: u32, budget: usize) -> Window {
    debug_assert!(
        budget >= 3,
        "a window needs room for both indicators and one row"
    );
    let first = clamp_first(i64::from(first), total) as usize;
    if first == 0 && total <= budget {
        return Window {
            above: 0,
            range: 0..total,
            below: 0,
        };
    }
    let mut room = budget.saturating_sub(usize::from(first > 0));
    if total - first > room {
        room = room.saturating_sub(1);
    }
    let end = (first + room).min(total);
    Window {
        above: first,
        range: first..end,
        below: total - end,
    }
}

/// Rows one wheel event moves a column by; positive = towards the next rows (wheel down).
///
/// `dy` is the event's vertical distance as the chart's wheel handler reads it, in lines when
/// `lines` is set and in pixels otherwise. A line event of a whole notch or more is exactly one
/// step whatever its magnitude, and drops any fraction left over; smaller line fractions (a
/// high-resolution wheel) and pixel distances (a trackpad, `line_px` per step) accumulate in
/// `accum` and pay out one step each time a whole unit is crossed. gpui reports wheel down as a
/// negative y, hence the sign flip.
pub(crate) fn notch_steps(dy: f32, lines: bool, accum: &mut f32, line_px: f32) -> i32 {
    if dy == 0.0 || !dy.is_finite() {
        return 0;
    }
    if lines {
        if dy.abs() >= 1.0 {
            *accum = 0.0;
            return -(dy.signum() as i32);
        }
        *accum += dy;
        if accum.abs() >= 1.0 {
            let sign = accum.signum();
            *accum -= sign;
            return -(sign as i32);
        }
        return 0;
    }
    if !(line_px > 0.0) || !line_px.is_finite() {
        return 0;
    }
    if !accum.is_finite() {
        *accum = 0.0;
    }
    *accum += dy;
    let steps = (*accum / line_px).trunc();
    *accum -= steps * line_px;
    -(steps as i32)
}

#[cfg(test)]
mod tests;
