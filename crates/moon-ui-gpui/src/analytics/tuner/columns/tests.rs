//! Regression coverage for unit-bearing Tuning metric values.

use super::{COL_AVG_ORDER, COL_PROFIT};
use crate::analytics::set_pnl_pct;
use moon_core::db::analytics::GroupStat;

/// Removing the dollar suffix from `columns.rs:tuner_profit_text` must fail the USDT assertion;
/// otherwise Tuning shows an ambiguous bare money value next to its fixed percentage column.
#[test]
fn profit_value_carries_the_active_lens_unit() {
    let mut group = GroupStat {
        profit: 12.34,
        ..GroupStat::default()
    };

    set_pnl_pct(false);
    assert_eq!((COL_PROFIT.text)(&group), "+12.34$");

    set_pnl_pct(true);
    assert_eq!((COL_PROFIT.text)(&group), "+12.34%");

    set_pnl_pct(false);
    group.profit = f64::NAN;
    assert_eq!((COL_PROFIT.text)(&group), "—");
}

/// Removing the suffix from `columns.rs:tuner_avg_order_text` must fail the finite assertion;
/// otherwise Avg order again renders an ambiguous unitless money value.
#[test]
fn average_order_value_carries_the_dollar_unit() {
    let mut group = GroupStat {
        avg_order: 19_983.48,
        ..GroupStat::default()
    };

    assert_eq!((COL_AVG_ORDER.text)(&group), "19 983.48$");

    group.avg_order = f64::NAN;
    assert_eq!((COL_AVG_ORDER.text)(&group), "—");
}
