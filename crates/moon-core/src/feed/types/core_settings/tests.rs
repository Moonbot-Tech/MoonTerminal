use super::*;

/// Regression target: the Moonbot default window end (`0.9999`, printed as 23:59) must not round up
/// past the last minute of the day, which a bare `as u16` cast after `round` would do.
#[test]
fn day_end_fraction_clamps_to_last_minute() {
    assert_eq!(day_fraction_to_minutes(0.9999), 1439);
}

#[test]
fn midnight_and_noon_convert_both_ways() {
    assert_eq!(day_fraction_to_minutes(0.0), 0);
    assert_eq!(day_fraction_to_minutes(0.5), 720);
    assert!((minutes_to_day_fraction(720) - 0.5).abs() < f64::EPSILON);
    assert_eq!(day_fraction_to_minutes(minutes_to_day_fraction(1439)), 1439);
}

/// A hand-edited or sentinel value reaches a time control, so it clamps instead of wrapping, and
/// anything non-finite reads as midnight rather than as an arbitrary end of the day.
#[test]
fn out_of_band_fractions_clamp() {
    assert_eq!(day_fraction_to_minutes(-3.0), 0);
    assert_eq!(day_fraction_to_minutes(7.5), 1439);
    assert_eq!(day_fraction_to_minutes(f64::NAN), 0);
    assert_eq!(day_fraction_to_minutes(f64::INFINITY), 0);
    assert_eq!(day_fraction_to_minutes(f64::NEG_INFINITY), 0);
}
