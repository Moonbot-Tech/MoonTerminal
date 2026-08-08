//! The shared "from"/"to" date+time bounds of the Report filter and the Analytics period bar.
//!
//! Both panels pick a range with two MoonUI `MoonDateTimePicker` fields, which resolve to whole
//! minutes. This module owns the one rule the two must agree on: which clock time an untouched
//! field means, and how a picked minute turns into a filter bound. Report wants an INCLUSIVE upper
//! bound (`closedate <= ?`) and Analytics an EXCLUSIVE one (`[from, to)`), so both are derived
//! here rather than open-coded twice and left to drift apart.

use chrono::{NaiveDateTime, NaiveTime};
use chrono_tz::Tz;
use gpui::{App, Context, Window};
use moon_ui::MoonDateTimePickerState;

use crate::design;
use moon_core::util::display_time::{self, LocalBoundary};

/// One picker step in seconds: the fields pick whole minutes, never seconds.
pub const MINUTE: i64 = 60;

/// Rendered date half of a bound field.
const DATE_FORMAT: &str = "%d.%m.%y";

/// Rendered time half of a bound field.
const TIME_FORMAT: &str = "%H:%M";

/// The widest value a field can render: every "dd.mm.yy HH:MM" digit at its widest.
const WIDEST_VALUE: &str = "00.00.00 00:00";

/// Reference size of the field's own text at zero Font-slider delta.
///
/// MoonUI draws a Medium picker field at `text_sm` — 0.875 rem — and the rem comes from the
/// theme's UI base size of 12. [`design::ui_text_width`] applies the active font scale itself, so
/// this stays the UNSCALED number.
const VALUE_TEXT_SIZE: f32 = 10.5;

/// Chrome around the value: 12 px of input padding either side, the gap before the trailing
/// affordance, the clear button itself, and a margin.
///
/// Fixed pixels rather than a scaled value because MoonUI's `input_px`/icon metrics for a Medium
/// field are fixed pixels too. Deliberately generous: the estimate sums glyph advances without
/// kerning, and being a few pixels short costs a whole wrapped line.
const CHROME_W: f32 = 64.0;

/// The width one bound field needs to keep "dd.mm.yy HH:MM" on a single line.
///
/// Measured with the active UI font rather than guessed: the field clips its label and wraps
/// before it clips, so a field a few pixels too narrow drops the time half onto an invisible
/// second line. Growing with the Font slider is the same reason — the text inside grows with it.
///
/// Args:
///     cx: Application context providing the text system and active font scale.
///
/// Returns:
///     The field width in pixels.
pub fn field_width(cx: &App) -> f32 {
    design::ui_text_width(cx, WIDEST_VALUE, VALUE_TEXT_SIZE, 400.0, false) + CHROME_W
}

/// Which edge of a range a field holds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// The lower, inclusive edge.
    From,
    /// The upper edge, inclusive of the whole minute it names.
    To,
}

impl Bound {
    /// The clock time a field carries while the user has set only a day.
    ///
    /// 00:00 on the lower edge and 23:59 on the upper one. That is what makes the SAME day picked
    /// on both fields mean the whole day: 00:00:00 through 23:59:59, exactly the range the old
    /// date-only fields produced.
    pub fn default_time(self) -> NaiveTime {
        let (hour, minute) = match self {
            Bound::From => (0, 0),
            Bound::To => (23, 59),
        };
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap_or_default()
    }
}

/// Build one bound field, empty and holding that edge's default clock time.
///
/// Args:
///     bound: Which edge this field holds, which fixes its default clock time.
///     window: Window the picker's internal calendar subscribes to.
///     cx: Context of the new picker state.
///
/// Returns:
///     A picker state rendering "dd.mm.yy HH:MM" once a day is chosen.
pub fn bound_picker(
    bound: Bound,
    window: &mut Window,
    cx: &mut Context<MoonDateTimePickerState>,
) -> MoonDateTimePickerState {
    let mut state = MoonDateTimePickerState::new(window, cx)
        .date_format(DATE_FORMAT)
        .time_format(TIME_FORMAT);
    state.set_time(bound.default_time(), cx);
    state
}

/// Convert Unix seconds into the civil value shown by a bound field.
///
/// Args:
///     secs: Absolute Unix seconds.
///     zone: User-selected display time zone.
///
/// Returns:
///     Local wall-clock value, or `None` when the timestamp is out of range.
pub fn dt_of_secs(secs: i64, zone: Tz) -> Option<NaiveDateTime> {
    display_time::at(secs, zone).map(|dt| dt.naive_local())
}

/// Convert a bound field's civil value into absolute Unix seconds.
///
/// Args:
///     dt: Local wall-clock value selected by the user.
///     zone: User-selected display time zone.
///     bound: Range edge that determines the repeated-hour occurrence.
///
/// Returns:
///     Absolute Unix seconds after applying the shared ambiguity and gap policy.
pub fn secs_of_dt(dt: NaiveDateTime, zone: Tz, bound: Bound) -> Option<i64> {
    let boundary = match bound {
        Bound::From => LocalBoundary::Lower,
        Bound::To => LocalBoundary::Upper,
    };
    display_time::unix_from_local(dt, zone, boundary)
}

/// The last second an upper bound covers, for an inclusive `<=` filter.
///
/// A picked minute belongs to the range as a whole, so 23:59 reaches 23:59:59 and an equal
/// from/to pair still selects that one minute instead of nothing.
pub fn inclusive_end(secs: i64) -> i64 {
    secs + MINUTE - 1
}

/// The first second past an upper bound, for an exclusive `[from, to)` range.
pub fn exclusive_end(secs: i64) -> i64 {
    secs + MINUTE
}

/// The value an upper field must show for an inclusive stored bound.
///
/// Floors to the minute grid the field picks on, so a bound stored by an older build — a whole day
/// as `midnight + 86_399` — reads back as that day's 23:59 rather than as an unrepresentable value.
///
/// Args:
///     secs: Inclusive absolute upper bound in UTC Unix seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Civil picker value for the covered minute, or `None` outside chrono's range.
pub fn field_of_inclusive(secs: i64, zone: Tz) -> Option<NaiveDateTime> {
    dt_of_secs(secs.div_euclid(MINUTE) * MINUTE, zone)
}

/// The value an upper field must show for an exclusive stored bound.
///
/// The exclusive edge itself is outside the range, so the field names the last minute inside it.
///
/// Args:
///     secs: Exclusive absolute upper bound in UTC Unix seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Civil picker value for the final included minute, or `None` outside chrono's range.
pub fn field_of_exclusive(secs: i64, zone: Tz) -> Option<NaiveDateTime> {
    field_of_inclusive(secs - 1, zone)
}

#[cfg(test)]
mod tests;
