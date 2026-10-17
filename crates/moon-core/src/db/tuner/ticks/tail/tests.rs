//! The horizon floor: one rule for the terminal and the station.

use super::{
    MAX_MIN_TAIL_S, TAIL_MS, clamp_min_tail_s, floor_ms, record_tail_ms, required_with_floor,
};
use crate::market::trade_replay::{Coverage, replay_window_ms};

/// Recapping by the terminal margin would hide station tails longer than thirty seconds.
#[test]
fn floor_ms_is_independent_of_the_margin() {
    assert_eq!(floor_ms(60), 60_000);
    assert_eq!(
        floor_ms(MAX_MIN_TAIL_S + 50),
        i64::from(MAX_MIN_TAIL_S) * 1_000
    );
}

/// The recorded tail must stay at least the margin, the clamped setting and the model's pad.
/// Dropping any one records a deal the terminal still calls short, or a tape the exit model
/// cannot judge.
#[test]
fn record_tail_ms_is_never_under_the_margin_the_setting_or_the_model_pad() {
    let cases = [
        (60_u32, 90_000_i64, 90_000),
        (10, 5_000, TAIL_MS),
        (120, 0, 120_000),
        (0, -1_000, TAIL_MS),
        (
            MAX_MIN_TAIL_S + 50,
            1_000,
            i64::from(MAX_MIN_TAIL_S) * 1_000,
        ),
    ];
    for (setting_s, margin_ms, expected) in cases {
        let got = record_tail_ms(setting_s, margin_ms);
        assert_eq!(got, expected, "setting {setting_s} margin {margin_ms}");
    }
}

/// A deal whose tape ends 30 s past the close is short of a 60 s floor and fit at a 30 s
/// floor. A thirty-second window must not shrink the higher floor back to the model's pad.
#[test]
fn a_thirty_second_tail_is_short_of_a_sixty_second_floor() {
    let open = 100 * 60_000;
    let close = open + 20_000;
    let window = replay_window_ms(open, close, 30_000).expect("window");
    let held = Coverage::one((open - TAIL_MS, close + 30_000));
    assert!(
        !held.covers(&required_with_floor(&window, 60_000)),
        "60 s floor"
    );
    assert!(
        held.covers(&required_with_floor(&window, 30_000)),
        "30 s floor"
    );
    assert!(
        Coverage::one((open - TAIL_MS, close + 60_000))
            .covers(&required_with_floor(&window, 60_000))
    );
}

/// The setting cannot ask past the store's longest margin. The cap itself stays; one past
/// it is the cap.
#[test]
fn clamp_min_tail_s_caps_at_the_store_maximum() {
    assert_eq!(clamp_min_tail_s(0), 0);
    assert_eq!(clamp_min_tail_s(60), 60);
    assert_eq!(clamp_min_tail_s(MAX_MIN_TAIL_S), MAX_MIN_TAIL_S);
    assert_eq!(clamp_min_tail_s(MAX_MIN_TAIL_S + 1), MAX_MIN_TAIL_S);
}
