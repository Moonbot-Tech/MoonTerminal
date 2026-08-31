//! Regression tests for durable-history asynchronous request identity.

use std::time::Duration;

use super::{
    ReportTradesStatus, draws_any_trade_kind, generation_refresh_interval,
    history_result_is_current,
};

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

/// Changing `report_trades.rs:HISTORY_LIVE_REFRESH_INTERVAL` from 250 ms to 5 s must redden this
/// assertion; otherwise a closed trade's dashed line and triangle can again take several seconds
/// to appear on the foreground chart.
#[test]
fn generation_refresh_interval_keeps_foreground_closed_trades_near_instant() {
    assert!(
        generation_refresh_interval(ReportTradesStatus::Ready, true) <= Duration::from_millis(250)
    );
}

/// Removing the `report_trades.rs:generation_refresh_interval` NotReady/Failed backoff arm must
/// redden this assertion; otherwise a broken foreground replica retries and log-spams every 250 ms.
#[test]
fn generation_refresh_interval_backs_off_failed_and_not_ready_foreground_reads() {
    assert_eq!(
        generation_refresh_interval(ReportTradesStatus::Failed, true),
        Duration::from_secs(5)
    );
    assert_eq!(
        generation_refresh_interval(ReportTradesStatus::NotReady, true),
        Duration::from_secs(5)
    );
}

/// Changing `report_trades.rs:HISTORY_REFRESH_INTERVAL_BACKGROUND` away from 30 s must redden
/// this assertion; otherwise background chart tiles lose their bounded durable-read cadence.
#[test]
fn generation_refresh_interval_keeps_background_tiles_at_thirty_seconds() {
    assert_eq!(
        generation_refresh_interval(ReportTradesStatus::Ready, false),
        Duration::from_secs(30)
    );
}
