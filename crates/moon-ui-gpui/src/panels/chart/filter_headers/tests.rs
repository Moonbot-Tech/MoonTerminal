//! Tooltip key for a chart blacklist line.

use super::filter_line_tooltip_key;
use crate::chartdx::FilterLineTarget;

/// A global line and a strategy line name different sentences. Swapping the keys would open
/// the core tooltip on a strategy line.
#[test]
fn the_tooltip_key_follows_the_blacklist_kind() {
    assert_eq!(
        filter_line_tooltip_key(&FilterLineTarget::GlobalBlacklist {
            coin: "XALPHA".into(),
        }),
        "chart_labels.filter.open_core_bl"
    );
    assert_eq!(
        filter_line_tooltip_key(&FilterLineTarget::StrategyBlacklist {
            strategy: "UNIT_01".into(),
            coin: "WBTC".into(),
        }),
        "chart_labels.filter.open_strategy_bl"
    );
}
