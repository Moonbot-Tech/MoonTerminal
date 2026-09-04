//! Regression coverage for the width-derived chart time-label target.

use super::time_label_target;

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
