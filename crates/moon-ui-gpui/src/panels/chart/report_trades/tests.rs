//! Regression tests for durable-history asynchronous request identity.

use super::history_result_is_current;

/// Removing the sequence check lets a slower Report scope A overwrite newer scope B on the same
/// tab; removing the target check lets core A history land after the tab moves to core B.
#[test]
fn stale_history_results_require_both_latest_sequence_and_exact_target() {
    let core_a = (7, "BTCUSDT".to_string());
    let core_b = (8, "BTCUSDT".to_string());

    assert!(!history_result_is_current(1, 2, &core_a, Some(&core_a)));
    assert!(!history_result_is_current(2, 2, &core_a, Some(&core_b)));
    assert!(history_result_is_current(2, 2, &core_b, Some(&core_b)));
}

use super::draws_any_trade_kind;
use moon_core::config::ChartGraphicsCfg;

/// Only "both boxes clear" may skip the durable read; every other combination must fetch the same
/// set and differ at drawing time.
///
/// Narrowing the query by these boxes is the failure this pins: `ChartTradeRecord::emulator` is
/// carried per row precisely so the drawing filter can hide marks, and the row cap is applied AFTER
/// the predicate — so a query narrowed by a checkbox frees slots under that cap and surfaces older
/// real trades that had been truncated away. A checkbox must not change what the history contains.
#[test]
fn only_both_checkboxes_clear_skips_the_durable_read() {
    let kinds = |real, emulator| ChartGraphicsCfg {
        show_real_trades: real,
        show_emulator_trades: emulator,
        ..ChartGraphicsCfg::default()
    };
    assert!(draws_any_trade_kind(&kinds(true, true)));
    assert!(draws_any_trade_kind(&kinds(true, false)));
    assert!(draws_any_trade_kind(&kinds(false, true)));
    assert!(!draws_any_trade_kind(&kinds(false, false)));
    assert!(draws_any_trade_kind(&ChartGraphicsCfg::default()));
}
