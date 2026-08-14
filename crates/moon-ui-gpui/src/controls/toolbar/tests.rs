//! Adaptive-fit and manual-strategy target regressions for the trading toolbar.

use super::{label_ladder, manual_strategy_core, LabelLadder, LabelWidths};

#[test]
/// Regression target: changing an inclusive ladder boundary to strict comparison either hides a
/// complete launcher label that exactly fits or retains it one pixel after the row starts clipping.
fn adaptive_label_ladder_honors_every_exact_boundary() {
    let widths = LabelWidths {
        icon_only: 100.0,
        size_unit: 10.0,
        size_noun: 20.0,
        settings: 30.0,
        strategies: 40.0,
        analytics: 50.0,
        sell: 60.0,
    };
    let expected = |rungs: usize| LabelLadder {
        size_unit: rungs >= 1,
        size_noun: rungs >= 2,
        settings: rungs >= 3,
        strategies: rungs >= 4,
        analytics: rungs >= 5,
        sell: rungs >= 6,
    };

    assert_eq!(label_ladder(100.0, widths), expected(0));
    for (index, boundary) in [110.0, 130.0, 160.0, 200.0, 250.0, 310.0]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            label_ladder(boundary - 1.0, widths),
            expected(index),
            "one pixel below boundary {boundary}"
        );
        assert_eq!(
            label_ladder(boundary, widths),
            expected(index + 1),
            "exact boundary {boundary}"
        );
    }
    assert_eq!(label_ladder(500.0, widths), expected(6));
}

#[test]
/// Regression target: preferring `active_core` over `hovered_core` makes TP/SL/S applicability
/// describe core A while a click or market hotkey submits through the hovered chart on core B.
fn hovered_chart_core_controls_manual_strategy_applicability() {
    assert_eq!(manual_strategy_core(Some(1), Some(2), true), Some(2));
    assert_eq!(manual_strategy_core(Some(1), Some(2), false), Some(1));
    assert_eq!(manual_strategy_core(None, Some(2), true), Some(2));
}
