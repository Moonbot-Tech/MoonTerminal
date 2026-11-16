use super::{PageTarget, step, stored_clock, tail_start};
use crate::panels::log::VIEW_LIMIT;

/// Stepping past either end, or skipping lines between two screens, would hide part of the day
/// behind a range label that claims it was shown.
#[test]
fn a_step_moves_one_screen_and_stops_at_both_ends() {
    let total = 3 * VIEW_LIMIT + 7;
    let tail = tail_start(total);
    assert_eq!(tail, 2 * VIEW_LIMIT + 7);

    // From the tail back to the first line: whole screens, then whatever is left.
    assert_eq!(
        step(tail, VIEW_LIMIT, total, PageTarget::Earlier),
        VIEW_LIMIT + 7
    );
    assert_eq!(
        step(VIEW_LIMIT + 7, VIEW_LIMIT, total, PageTarget::Earlier),
        7
    );
    assert_eq!(step(7, VIEW_LIMIT, total, PageTarget::Earlier), 0);
    assert_eq!(step(0, VIEW_LIMIT, total, PageTarget::Earlier), 0);

    // Forward continues at the line after the last one shown.
    assert_eq!(step(0, VIEW_LIMIT, total, PageTarget::Later), VIEW_LIMIT);
    assert_eq!(step(tail, VIEW_LIMIT, total, PageTarget::Later), tail);
    // A short window, as a time jump near the end leaves, has nothing after it.
    assert_eq!(step(total - 3, 3, total, PageTarget::Later), total - 3);
}

/// A time read off the rows must find the line it was read from: the rows show the display zone,
/// the file is searched in its own clock.
#[test]
fn a_typed_time_is_taken_in_the_display_zone() {
    let moscow: chrono_tz::Tz = "Europe/Moscow".parse().unwrap();
    let new_york: chrono_tz::Tz = "America/New_York".parse().unwrap();
    let day = "2026-01-02";

    assert_eq!(&stored_clock(*b"15:00:00", day, moscow), b"12:00:00");
    // Shown after midnight, stored late the same file day.
    assert_eq!(&stored_clock(*b"02:30:10", day, moscow), b"23:30:10");
    // A zone behind UTC: winter is five hours, summer four.
    assert_eq!(&stored_clock(*b"19:00:00", day, new_york), b"00:00:00");
    assert_eq!(
        &stored_clock(*b"19:00:00", "2026-07-02", new_york),
        b"23:00:00"
    );
    assert_eq!(
        &stored_clock(*b"15:00:00", day, chrono_tz::UTC),
        b"15:00:00"
    );
    // A file whose name carries no day cannot be placed in a zone; the time is used as typed.
    assert_eq!(&stored_clock(*b"15:00:00", "", moscow), b"15:00:00");
}

/// A file shorter than one screen starts at its first line.
#[test]
fn a_short_file_has_one_window() {
    assert_eq!(tail_start(0), 0);
    assert_eq!(tail_start(VIEW_LIMIT), 0);
    assert_eq!(step(0, 12, 12, PageTarget::Later), 0);
}
