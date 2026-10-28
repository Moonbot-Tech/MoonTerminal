//! Regression coverage for the chart-graphics popup's per-trade opacity steps.

use super::{TRADE_VOLUME_ALPHAS, percent_label};

/// Restoring the old step or hardcoding a caption would make the row disagree with the opacity it
/// applies; every label must instead come from its exact selectable value.
#[test]
fn trade_volume_alpha_steps_have_computed_percent_labels() {
    assert_eq!(TRADE_VOLUME_ALPHAS, [0.0, 0.15, 0.35, 0.5, 0.75, 1.0]);
    let labels: Vec<String> = TRADE_VOLUME_ALPHAS
        .iter()
        .map(|alpha| percent_label(*alpha))
        .collect();
    assert_eq!(labels, ["0%", "15%", "35%", "50%", "75%", "100%"]);
}
