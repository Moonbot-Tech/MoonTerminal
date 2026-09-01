//! Unit tests for manual-strategy picker visibility and ordering.

use moon_core::feed::StrategyRow;

use super::manual_strategy_options;
use crate::backend::MANUAL_STRATEGY_KIND;

/// Build a strategy row carrying only the fields used by the picker-option filter.
fn strategy(id: u64, name: &str, kind_ordinal: u8) -> StrategyRow {
    StrategyRow {
        id,
        name: name.to_string(),
        kind: "Test".to_string(),
        kind_ordinal,
        folder_path: String::new(),
        checked: false,
        is_short: false,
        fields: Vec::new(),
    }
}

/// Returning `Some(Vec::new())` would render the header control and an empty popover.
#[test]
fn an_empty_snapshot_hides_the_manual_strategy_control() {
    assert!(manual_strategy_options(&[]).is_none());
}

/// Removing the Manual-kind filter would show the control for a core with only other strategies.
#[test]
fn non_manual_strategies_do_not_make_the_control_visible() {
    let strategies = [strategy(7, "Auto", MANUAL_STRATEGY_KIND - 1)];

    assert!(manual_strategy_options(&strategies).is_none());
}

/// Sorting or retaining other kinds would change the existing picker order and contents.
#[test]
fn manual_strategy_options_preserve_snapshot_order() {
    let strategies = [
        strategy(7, "Manual B", MANUAL_STRATEGY_KIND),
        strategy(8, "Auto", MANUAL_STRATEGY_KIND - 1),
        strategy(9, "Manual A", MANUAL_STRATEGY_KIND),
    ];

    assert_eq!(
        manual_strategy_options(&strategies),
        Some(vec![
            (7, "Manual B".to_string()),
            (9, "Manual A".to_string()),
        ])
    );
}
