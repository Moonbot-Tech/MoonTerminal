//! Regression coverage for the width-derived chart time-label target.

use super::time_label_target;

/// Axis and cursor prices must remain readable below the former four-decimal floor.
#[test]
fn microprices_use_satoshis_and_preserve_the_market_grid() {
    assert_eq!(super::format_price(0.00000012, Some(1e-8)), "12 sat");
    assert_eq!(super::format_price(0.0000001234, Some(1e-10)), "12.34 sat");
    assert_eq!(super::format_price(0.0000000001, Some(1e-10)), "0.01 sat");
    assert_eq!(super::format_price(0.00001, Some(1e-8)), "1000 sat");
    assert_eq!(super::format_price(0.00001234, Some(1e-8)), "0.00001234");
    assert_eq!(super::format_price(-0.00000012, Some(1e-8)), "-12 sat");
}

/// Known ticks retain exchange precision; an unavailable tick must not turn microprices into zero.
#[test]
fn price_labels_share_grid_precision_and_an_unknown_tick_fallback() {
    assert_eq!(super::format_price(12.345, Some(0.005)), "12.345");
    assert_eq!(super::format_price(1200.0, Some(0.01)), "1200.00");
    assert_eq!(super::format_price(0.0000001234, None), "12.34 sat");
    assert_eq!(super::format_price(0.0000000001, None), "0.0100 sat");
    assert_eq!(super::format_price(0.00001234, None), "0.00001234");
    assert_eq!(super::format_price(1500.0, None), "1500.0");
    assert_eq!(super::format_price(-12.345, Some(0.005)), "-12.345");
    // An axis value just under a power of ten once widened must not gain a digit.
    assert_eq!(super::format_price(0.01_f32, None), "0.01000");
}

/// A wide plot must retain enough round time labels to use its available horizontal space.
///
/// Breakage this pins: removing the width divisor or restoring the former fixed target would
/// leave wide charts under-labeled.
#[test]
fn time_label_target_wide_plot_is_ten() {
    assert_eq!(time_label_target(1900.0), 10.0);
}

/// A medium-width plot must reduce the target without reverting to a fixed label count.
///
/// Breakage this pins: bypassing width scaling would keep narrow detached charts at the former
/// fixed target and crowd their axis labels.
#[test]
fn time_label_target_medium_plot_is_four() {
    assert_eq!(time_label_target(760.0), 4.0);
}

/// Extremely narrow plots must retain a readable lower bound instead of targeting too few labels.
///
/// Breakage this pins: removing the floor would make small chart hosts lose their useful time
/// scale.
#[test]
fn time_label_target_sub_floor_plot_is_three() {
    assert_eq!(time_label_target(1.0), 3.0);
}

/// Removing the one-second step leaves the three-second view without useful round labels.
#[test]
fn super_zoom_has_one_second_time_labels() {
    for width in [180.0, 760.0, 1900.0] {
        assert_eq!(super::nice_time_step(3.0, time_label_target(width)), 1.0);
    }
}

/// A window that falls between two round steps takes the next coarser step.
///
/// One label across fifty seconds must be at least fifty seconds wide. Thirty seconds would
/// crowd the axis, and two minutes would skip the one-minute mark. Ninety seconds likewise
/// lands on two minutes. The three-second super-zoom test stays on the one-second step, so it
/// cannot see a missing one-minute step.
#[test]
fn a_window_between_round_steps_takes_the_next_coarser_step() {
    assert_eq!(super::nice_time_step(50.0, 1.0), 60.0);
    assert_eq!(super::nice_time_step(90.0, 1.0), 2.0 * 60.0);
}

/// A window longer than six hours stays on the six-hour step.
///
/// Time labels stop at six hours. A longer window must not invent a raw interval such as the
/// window length itself. The super-zoom test returns on the first step and never reaches this cap.
#[test]
fn a_window_past_the_coarsest_step_stays_on_six_hours() {
    let six_hours = 6.0 * 60.0 * 60.0;
    assert_eq!(super::nice_time_step(100_000.0, 1.0), six_hours);
}

/// A price of one thousand keeps one decimal, and the next price below it keeps two.
///
/// Chart price labels use this split. One thousand and nine hundred ninety-nine are exact in
/// f32, so the change is the threshold itself.
#[test]
fn price_decimals_changes_at_one_thousand() {
    assert_eq!(super::price_decimals(1000.0), 1);
    assert_eq!(super::price_decimals(999.0), 2);
}

/// A negative price uses the same decimal count as its magnitude.
///
/// The positive one-thousand check never takes the absolute value. Fifteen hundred below zero
/// is still a four-figure price and keeps one decimal.
#[test]
fn price_decimals_uses_magnitude_below_zero() {
    assert_eq!(super::price_decimals(-1500.0), 1);
}

/// A rough spacing just below one and a half stays on one; equality rounds up to two.
/// Existing time-step and decimal tests never exercise price-grid interval selection.
#[test]
fn price_grid_rounds_up_at_one_and_a_half() {
    assert_eq!(super::nice_interval(14.0, 10.0), 1.0);
    assert_eq!(super::nice_interval(15.0, 10.0), 2.0);
}

/// Three units of requested spacing must move the price grid from two to five.
/// Existing axis tests do not reach this price-grid branch.
#[test]
fn price_grid_rounds_up_at_three() {
    assert_eq!(super::nice_interval(29.0, 10.0), 2.0);
    assert_eq!(super::nice_interval(30.0, 10.0), 5.0);
}

/// Seven units of requested spacing starts the next decade instead of retaining five.
/// Existing axis tests do not reach this price-grid branch.
#[test]
fn price_grid_rounds_up_at_seven() {
    assert_eq!(super::nice_interval(69.0, 10.0), 5.0);
    assert_eq!(super::nice_interval(70.0, 10.0), 10.0);
}
