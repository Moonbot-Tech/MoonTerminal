//! Checks that every platform's wheel lines come back as the notches the hand turned.

use super::{LineScale, MAX_NOTCHES_PER_EVENT, notches};

/// Reading macOS's accelerated line count as notches must fail: a slow detent of 0.1 lines is
/// still a whole notch, and a fast one of 7.5 lines is not seven.
#[test]
fn a_mac_event_is_one_notch_whatever_its_accelerated_size() {
    assert_eq!(notches(0.1, LineScale::OneNotchPerEvent), 1.0);
    assert_eq!(notches(-7.5, LineScale::OneNotchPerEvent), -1.0);
}

/// Dividing by a fixed three must fail: a Windows user on "1 line per notch" gets one doubling per
/// click, not a third of one, and a high-resolution wheel's third of a notch stays a third.
#[test]
fn a_windows_notch_is_one_whatever_the_lines_setting() {
    assert_eq!(notches(1.0, LineScale::PerNotch(1.0)), 1.0);
    assert_eq!(notches(-6.0, LineScale::PerNotch(3.0)), -2.0);
    assert!((notches(1.0, LineScale::PerNotch(3.0)) - 1.0 / 3.0).abs() < 1e-6);
}

/// A page-scroll setting, a garbage delta or an unreadable setting must not jump the chart across
/// its history: one event stays within the cap, and a bad scale falls back to three lines.
#[test]
fn a_bad_delta_or_setting_stays_within_one_bounded_step() {
    let page = u32::MAX as f32;
    assert!((notches(page, LineScale::PerNotch(page)) - 1.0).abs() < 1e-6);
    assert_eq!(
        notches(1_000.0, LineScale::PerNotch(3.0)),
        MAX_NOTCHES_PER_EVENT
    );
    assert_eq!(notches(3.0, LineScale::PerNotch(0.0)), 1.0);
    assert_eq!(notches(f32::NAN, LineScale::PerNotch(3.0)), 0.0);
}
