//! Coverage for "the reader is using this chart" and the KeepInChart timer it holds back.

use std::time::{Duration, Instant};

use super::{HOVER_RECHECK, IN_USE_WINDOW, TtlStep, UseTrack, ttl_step};

fn touched(at: Instant) -> UseTrack {
    UseTrack {
        pointer_over: false,
        last_input: Some(at),
    }
}

#[test]
fn untouched_chart_is_idle() {
    assert_eq!(UseTrack::default().hold_for(Instant::now()), None);
}

#[test]
fn recent_input_holds_for_the_rest_of_the_window() {
    let t0 = Instant::now();
    let left = touched(t0).hold_for(t0 + Duration::from_secs(30));
    assert_eq!(left, Some(IN_USE_WINDOW - Duration::from_secs(30)));
}

#[test]
fn input_older_than_the_window_is_idle() {
    let t0 = Instant::now();
    assert_eq!(touched(t0).hold_for(t0 + IN_USE_WINDOW), None);
}

#[test]
fn pointer_over_holds_without_input() {
    let track = UseTrack {
        pointer_over: true,
        last_input: None,
    };
    assert_eq!(track.hold_for(Instant::now()), Some(HOVER_RECHECK));
}

#[test]
fn deadline_ahead_waits_for_it() {
    assert_eq!(
        ttl_step(Some(10_000.0), 4_000.0, None),
        Some(TtlStep::Wait(Duration::from_millis(6_000)))
    );
}

#[test]
fn no_deadline_needs_no_timer() {
    assert_eq!(ttl_step(None, 4_000.0, Some(HOVER_RECHECK)), None);
}

#[test]
fn expired_chart_in_use_is_kept_and_rechecked() {
    let t0 = Instant::now();
    let hold = touched(t0).hold_for(t0 + Duration::from_secs(100));
    assert_eq!(
        ttl_step(Some(1_000.0), 2_000.0, hold),
        Some(TtlStep::Wait(Duration::from_secs(20)))
    );
}

#[test]
fn expired_idle_chart_is_closed() {
    let t0 = Instant::now();
    let hold = touched(t0).hold_for(t0 + Duration::from_secs(600));
    assert_eq!(ttl_step(Some(1_000.0), 2_000.0, hold), Some(TtlStep::Prune));
}
