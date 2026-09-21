//! Regression tests for durable-history asynchronous request identity.

use std::time::Duration;

use moon_core::db::FailKind;

use super::{
    ReportTradesStatus, busy_read_backoff, draws_any_trade_kind, generation_refresh_interval,
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
        generation_refresh_interval(ReportTradesStatus::Failed(FailKind::Busy), true),
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

/// `report_trades.rs:busy_read_backoff` must wait 250 ms times the attempt number and stop after
/// the third try. A single-shot Busy would keep drawing the badge on a momentary lock; sleeping
/// after the last attempt delays the badge without another read.
#[test]
fn busy_history_read_retries_twice_with_linear_backoff() {
    assert_eq!(busy_read_backoff(1), Some(Duration::from_millis(250)));
    assert_eq!(busy_read_backoff(2), Some(Duration::from_millis(500)));
    assert_eq!(busy_read_backoff(3), None);
    assert_eq!(busy_read_backoff(0), None);
}

/// `report_trades.rs:ReportTradesStatus::offers_retry` must hide Retry for kinds that cannot
/// recover. Showing the button on Corrupt or ReplicaAccessDenied is the defect the user hit.
#[test]
fn trade_history_retry_is_offered_only_when_a_later_read_can_help() {
    assert!(ReportTradesStatus::NotReady.offers_retry());
    assert!(ReportTradesStatus::Failed(FailKind::Busy).offers_retry());
    assert!(ReportTradesStatus::Failed(FailKind::Other).offers_retry());
    assert!(!ReportTradesStatus::Failed(FailKind::Corrupt).offers_retry());
    assert!(!ReportTradesStatus::Failed(FailKind::ReplicaAccessDenied).offers_retry());
    assert!(!ReportTradesStatus::Ready.offers_retry());
    assert!(!ReportTradesStatus::Loading.offers_retry());
    assert!(!ReportTradesStatus::Empty.offers_retry());
    assert!(!ReportTradesStatus::Idle.offers_retry());
}

/// `report_trades.rs:ReportTradesStatus::overlay_label` must name each Failed kind with the
/// shared reports-replica copy. Collapsing them onto `chart.trade_history.failed` is the badge
/// the user reported.
#[test]
fn trade_history_failed_overlay_names_the_cause_in_english_and_russian() {
    for (locale, busy, corrupt, other) in [
        (
            "en",
            "The reports database is busy right now. Retry — the period will recompute.",
            "The database file is damaged — retrying will not help. See the Log tab.",
            "This is a read error, not an absence of trades. See the Log tab for details.",
        ),
        (
            "ru",
            "База отчётов сейчас занята. Повторите — период пересчитается.",
            "Файл базы повреждён — повтор не поможет. Подробности во вкладке «Лог».",
            "Это ошибка чтения, а не отсутствие сделок. Подробности — во вкладке «Лог».",
        ),
    ] {
        let _locale = crate::test_locale::force(locale);
        assert_eq!(
            ReportTradesStatus::Failed(FailKind::Busy)
                .overlay_label()
                .as_deref(),
            Some(busy)
        );
        assert_eq!(
            ReportTradesStatus::Failed(FailKind::Corrupt)
                .overlay_label()
                .as_deref(),
            Some(corrupt)
        );
        assert_eq!(
            ReportTradesStatus::Failed(FailKind::Other)
                .overlay_label()
                .as_deref(),
            Some(other)
        );
        assert!(ReportTradesStatus::Ready.overlay_label().is_none());
        let denial = ReportTradesStatus::Failed(FailKind::ReplicaAccessDenied)
            .overlay_label()
            .expect("lease denial must state a badge");
        assert!(
            denial.contains(if locale == "en" {
                "Access to the reports replica is unavailable."
            } else {
                "Доступ к реплике отчётов закрыт."
            }),
            "{locale}: lease denial overlay must carry the recovery title, got {denial}"
        );
    }
}

/// `report_trades.rs:ReportTradesStatus::auto_retries` must wake only a Failed kind a later
/// read can still clear. Arming a timer for Corrupt keeps hammering a file that never self-heals.
#[test]
fn trade_history_auto_retries_only_retryable_failures() {
    assert!(ReportTradesStatus::Failed(FailKind::Busy).auto_retries());
    assert!(ReportTradesStatus::Failed(FailKind::Other).auto_retries());
    assert!(!ReportTradesStatus::Failed(FailKind::Corrupt).auto_retries());
    assert!(!ReportTradesStatus::Failed(FailKind::ReplicaAccessDenied).auto_retries());
    assert!(!ReportTradesStatus::NotReady.auto_retries());
}
