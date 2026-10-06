//! Generation tests for By-time scope and report invalidation.

use super::TimeTunerState;
use super::{fmt_min, fmt_week_ep};

/// A day wrap or symmetric endpoint shortening changes the strategy schedule input text.
#[test]
fn time_inputs_keep_unwrapped_hours_and_asymmetric_week_boundaries() {
    for (minutes, expected) in [
        (0, "00:00"),
        (61, "01:01"),
        (1439, "23:59"),
        (1440, "24:00"),
    ] {
        assert_eq!(fmt_min(minutes), expected);
    }
    for (minute, is_end, expected) in [
        (0, false, "1"),
        (1439, true, "1"),
        (1424, false, "1.23:44"),
        (10079, true, "7"),
        (1440, true, "2.00:00"),
    ] {
        assert_eq!(fmt_week_ep(minute, is_end), expected);
    }
}

/// `time/state.rs:TimeTunerState::invalidate` must advance both request generations; removing the
/// `seq` bump lets a pre-scope profile completion clear `dirty` under the newly selected strategy.
#[test]
fn scope_change_retires_profile_and_suggestion_requests() {
    let mut state = TimeTunerState::load();
    let (seq, sugg_seq) = (state.seq, state.sugg_seq);

    state.invalidate();

    assert_ne!(state.seq, seq);
    assert_ne!(state.sugg_seq, sugg_seq);
    assert!(state.dirty);
}

/// `time/state.rs:TimeTunerState::mark_report_stale` retires suggestions but keeps profile state.
///
/// Removing the suggestion-generation bump lets an optimizer pinned before a report commit write
/// a stale schedule into saveable v1 fields after the report inputs have changed.
#[test]
fn report_change_retires_suggestion_but_keeps_profile_generation() {
    let mut state = TimeTunerState::load();
    let (seq, sugg_seq) = (state.seq, state.sugg_seq);

    state.mark_report_stale();

    assert_eq!(state.seq, seq);
    assert_ne!(state.sugg_seq, sugg_seq);
    assert!(state.dirty);
}

/// `time/state.rs:TimeTunerState::apply_current_read` must preserve a confirmed Save baseline
/// when an automatic refresh cannot read the strategy row; clearing it would make the next Save
/// propose empty schedules and false ignore-flag changes.
#[test]
fn automatic_missing_schedule_read_preserves_the_save_baseline() {
    let mut state = TimeTunerState::load();
    state.apply_current_read(
        Some((["1.00:00-5.23:59".into(), "08:00-20:00".into()], true, true)),
        false,
    );

    state.apply_current_read(None, true);

    assert_eq!(state.current_raw[0], "1.00:00-5.23:59");
    assert_eq!(state.current_raw[1], "08:00-20:00");
    assert!(state.ignore_cur);
    assert!(state.ign_filters_cur);
}
