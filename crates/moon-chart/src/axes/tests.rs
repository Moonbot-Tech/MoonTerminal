//! Regression coverage for chart axis prices, time steps and caption placement.

use super::{TimeCaptionWidths, time_step};

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

/// Caption widths of the axis font at its default size: `HH:MM:SS`, `HH:MM`, `DD.MM`.
const WIDTHS: TimeCaptionWidths = TimeCaptionWidths {
    seconds: 55.0,
    minutes: 35.0,
    date: 35.0,
};

/// The step for a window of `window_sec` across `plot_px` with the default widths.
fn step(window_sec: f64, plot_px: f32) -> f64 {
    time_step(window_sec, plot_px, &WIDTHS)
}

/// The densest step whose captions fit wins, not the first step past a fixed caption count.
///
/// The three windows are the ones reported as too sparse: a fixed one-caption-per-190 px target
/// picked one minute, five seconds and fourteen days there, captions 200–425 px apart.
#[test]
fn the_densest_fitting_step_wins() {
    // 7.5 minutes across 1500 px: 30 s marks are 100 px apart, room for `HH:MM:SS` plus a gap.
    assert_eq!(step(450.0, 1500.0), 30.0);
    // 56.6 px per second: two-second marks, 113 px apart.
    assert_eq!(step(26.5, 1500.0), 2.0);
    // 70 days across 1300 px: weekly dates 130 px apart; two days would be 37 px.
    assert_eq!(step(70.0 * super::DAY_SECS, 1300.0), 7.0 * super::DAY_SECS);
}

/// A step fits only with a gap beside its caption, so a narrow plot steps coarser instead of
/// crowding: a one-second mark is 60 px at 180 px for three seconds, under `HH:MM:SS` + gap.
#[test]
fn a_narrow_plot_steps_coarser_rather_than_crowding() {
    assert_eq!(step(3.0, 760.0), 1.0);
    assert_eq!(step(3.0, 180.0), 2.0);
}

/// A dated clock (`DD.MM HH:MM`) is wider than today's `HH:MM`, so the same window steps
/// coarser once the captions carry a date.
#[test]
fn a_wider_caption_takes_a_coarser_step() {
    assert_eq!(step(super::DAY_SECS, 1600.0), 3600.0);
    let dated = TimeCaptionWidths {
        minutes: 76.0,
        ..WIDTHS
    };
    assert_eq!(time_step(super::DAY_SECS, 1600.0, &dated), 4.0 * 3600.0);
    // Four days of dated clocks: six-hour marks are 81 px, too close; twelve hours, not a day.
    assert_eq!(
        time_step(4.0 * super::DAY_SECS, 1300.0, &dated),
        12.0 * 3600.0
    );
}

/// Past six hours the step moves on to whole days instead of staying on six-hour marks.
///
/// With the old six-hour cap a window of weeks kept marks a pixel apart and the spacing walk
/// printed them at arbitrary hours.
#[test]
fn a_window_past_six_hours_takes_a_day_step() {
    assert_eq!(step(14.0 * super::DAY_SECS, 1300.0), super::DAY_SECS);
}

/// Windows of years land on calendar-month steps, and a window too long for any step stays on
/// years rather than inventing a raw interval such as the window length itself.
#[test]
fn long_windows_step_by_calendar_months_then_years() {
    let two_years = step(730.0 * super::DAY_SECS, 1700.0);
    assert_eq!(super::calendar_months(two_years), Some(1));
    let decade = step(3650.0 * super::DAY_SECS, 1700.0);
    assert_eq!(super::calendar_months(decade), Some(6));
    let century = step(36_500.0 * super::DAY_SECS, 1700.0);
    assert_eq!(super::calendar_months(century), Some(12));
}

/// An empty window or plot has no density to fit; the coarsest step keeps the walk finite.
#[test]
fn a_degenerate_window_takes_the_coarsest_step() {
    assert_eq!(super::calendar_months(step(0.0, 1500.0)), Some(12));
    assert_eq!(super::calendar_months(step(450.0, 0.0)), Some(12));
}

/// Clock, day and week steps are not month steps, so their ticks keep the fixed lattice.
#[test]
fn only_month_steps_read_as_calendar_months() {
    assert_eq!(super::calendar_months(21_600.0), None);
    assert_eq!(super::calendar_months(super::DAY_SECS), None);
    assert_eq!(super::calendar_months(14.0 * super::DAY_SECS), None);
    assert_eq!(super::calendar_months(3.0 * super::MONTH_SECS), Some(3));
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

/// Drawn caption indices for 76 px captions centred every 60 px, optionally with one covered.
fn drawn_time_captions(covered: Option<usize>) -> Vec<usize> {
    let mut walk = super::TimeCaptionWalk::new(0.0);
    (0..12)
        .filter(|&i| {
            let centre = 50.0 + 60.0 * i as f32;
            walk.place(centre - 38.0, centre + 38.0, covered == Some(i))
        })
        .collect()
}

/// The crosshair readout hides the caption it covers but must not shift the ones to its right:
/// with the slot left empty, the row to the right of the cursor re-phased on every move (#1117).
#[test]
fn a_covered_time_caption_keeps_its_slot() {
    let open = drawn_time_captions(None);
    assert_eq!(open, vec![0, 2, 4, 6, 8, 10]);
    let expected: Vec<usize> = open.iter().copied().filter(|&i| i != 4).collect();
    assert_eq!(drawn_time_captions(Some(4)), expected);
    // A covered caption that would have been dropped for spacing changes nothing at all.
    assert_eq!(drawn_time_captions(Some(5)), open);
}

/// Without a usable tick the axis prints at the price's magnitude, which on Hyperliquid is the
/// exchange's own five-figure step; the exact `f32` must not leak its binary tail (#1289).
#[test]
fn an_unknown_tick_prints_the_price_at_its_magnitude() {
    assert_eq!(super::format_price(277.533_84_f32, None), "277.53");
    assert_eq!(super::format_price(7652.068_f32, None), "7652.1");
}
