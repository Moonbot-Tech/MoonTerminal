//! Manual-strategy target selection regressions for the trading toolbar.

use super::manual_strategy_core;

#[test]
/// Regression target: preferring `active_core` over `hovered_core` makes TP/SL/S applicability
/// describe core A while a click or market hotkey submits through the hovered chart on core B.
fn hovered_chart_core_controls_manual_strategy_applicability() {
    assert_eq!(manual_strategy_core(Some(1), Some(2), true), Some(2));
    assert_eq!(manual_strategy_core(Some(1), Some(2), false), Some(1));
    assert_eq!(manual_strategy_core(None, Some(2), true), Some(2));
}
