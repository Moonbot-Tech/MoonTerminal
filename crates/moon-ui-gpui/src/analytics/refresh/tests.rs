//! Deterministic tests for the report-generation refresh policy.

use std::time::{Duration, Instant};

use super::{
    BusyRetryBudget, CatchUpOutcome, RefreshGate, RefreshPlan, RefreshUrgency, VisibleRefresh,
    core_metadata_wait, preserve_on_catch_up, report_result_is_stale, strategy_base_allows_axis,
    visible_refresh,
};
use crate::analytics::Tab;
use moon_core::db::{
    FailKind, ProfitScope, ProfitUnit, QuoteBreakdown, ReadFail, ValuationCoverage,
};
use std::sync::Arc;

/// Construct a classified database failure without depending on display text.
fn failure(kind: FailKind) -> ReadFail {
    ReadFail::Failed {
        kind,
        msg: Arc::from("test failure"),
    }
}

/// `analytics/refresh.rs:preserve_on_catch_up` must preserve only a transient result covered by a
/// retry allowance or a newer generation. Dropping that correction guard, or letting a settled
/// result through, leaves stale figures under a current period label with nothing scheduled to
/// correct them.
#[test]
fn catch_up_preservation_requires_a_transient_outcome_and_scheduled_correction() {
    for (label, outcome, is_transient) in [
        ("Replacement", CatchUpOutcome::Replacement, false),
        ("Transient", CatchUpOutcome::Transient, true),
        ("Settled", CatchUpOutcome::Settled, false),
    ] {
        for after_report in [false, true] {
            for retry_allowance in [false, true] {
                for superseded in [false, true] {
                    assert_eq!(
                        preserve_on_catch_up(after_report, outcome, retry_allowance, superseded,),
                        after_report && is_transient && (retry_allowance || superseded),
                        "{label}: after_report={after_report}, retry_allowance={retry_allowance}, \
                         superseded={superseded}"
                    );
                }
            }
        }
    }
}

/// `analytics/refresh.rs:CatchUpOutcome::{of_scope,of_read}` must distinguish a valuation gap
/// from unknown-quote or settled results. Removing the unknown-quote guard hides money behind a
/// stale comparable scalar in the wrong unit, while moving Empty or Corrupt to transient hides a
/// legitimate purge result or permanent database failure.
#[test]
fn catch_up_outcome_classifies_scope_and_read_results() {
    let incomplete_split = Ok::<ProfitScope<()>, ReadFail>(ProfitScope::Split(QuoteBreakdown {
        unknown_orders: 0,
        valuation: Some(ValuationCoverage {
            eligible_orders: 5,
            valued_orders: 3,
            unavailable_orders: 1,
            ..Default::default()
        }),
        ..Default::default()
    }));
    let unknown_quote_split = Ok::<ProfitScope<()>, ReadFail>(ProfitScope::Split(QuoteBreakdown {
        unknown_orders: 1,
        valuation: Some(ValuationCoverage {
            eligible_orders: 5,
            valued_orders: 3,
            unavailable_orders: 1,
            ..Default::default()
        }),
        ..Default::default()
    }));
    let settled_split = Ok::<ProfitScope<()>, ReadFail>(ProfitScope::Split(Default::default()));
    let comparable = Ok(ProfitScope::Comparable {
        unit: ProfitUnit::Percent,
        data: (),
    });
    let empty = Ok(ProfitScope::Empty(()));

    assert!(matches!(
        CatchUpOutcome::of_scope(&incomplete_split),
        CatchUpOutcome::Transient
    ));
    assert!(matches!(
        CatchUpOutcome::of_scope(&unknown_quote_split),
        CatchUpOutcome::Settled
    ));
    assert!(matches!(
        CatchUpOutcome::of_scope(&settled_split),
        CatchUpOutcome::Settled
    ));
    assert!(matches!(
        CatchUpOutcome::of_scope(&comparable),
        CatchUpOutcome::Replacement
    ));
    assert!(matches!(
        CatchUpOutcome::of_scope(&empty),
        CatchUpOutcome::Replacement
    ));

    let cases: [(&str, Result<(), ReadFail>, bool); 6] = [
        ("Busy", Err(failure(FailKind::Busy)), true),
        ("NotReady", Err(ReadFail::NotReady), false),
        ("Corrupt", Err(failure(FailKind::Corrupt)), false),
        ("Other", Err(failure(FailKind::Other)), false),
        ("IncomparableQuote", Err(ReadFail::IncomparableQuote), false),
        ("PeriodOutOfRange", Err(ReadFail::PeriodOutOfRange), false),
    ];
    for (label, result, expected_transient) in cases {
        assert_eq!(
            matches!(CatchUpOutcome::of_read(&result), CatchUpOutcome::Transient),
            expected_transient,
            "{label} must be transient exactly when a scheduled correction can resolve it"
        );
    }
}

/// `analytics/refresh.rs:BusyRetryBudget::has_allowance` must read without spending a retry.
/// Letting this publication query increment attempts exhausts a transient Busy recovery budget
/// before `claim` schedules the retry, leaving stale data or a false error.
#[test]
fn checking_the_busy_retry_allowance_does_not_consume_an_attempt() {
    let mut budget = BusyRetryBudget::default();

    assert!(budget.has_allowance());
    assert!(budget.has_allowance());
    assert!(budget.claim());
    assert!(budget.has_allowance());
    assert!(budget.claim());
    assert!(budget.has_allowance());
    assert!(budget.claim());
    assert!(!budget.has_allowance());
    assert!(!budget.claim());
}

/// `analytics/refresh.rs:report_result_is_stale` must compare the start and completion
/// generations; removing that comparison lets a scan started before a new trade clear the
/// trailing catch-up, while discarding the completed scan instead starves continuous traffic.
#[test]
fn completed_snapshot_publishes_but_keeps_newer_generation_pending() {
    assert!(!report_result_is_stale(7, 7, false));
    assert!(report_result_is_stale(7, 8, false));
    assert!(report_result_is_stale(7, 7, true));
}

/// `analytics/refresh.rs:strategy_base_allows_axis` must include every compound base component;
/// ignoring a Busy undated/core result starts a child axis whose success replenishes the parent's
/// exhausted retry budget and creates an endless metadata retry loop.
#[test]
fn partial_strategy_base_never_starts_the_child_axis() {
    assert!(strategy_base_allows_axis(false, false, false));
    assert!(!strategy_base_allows_axis(true, false, false));
    assert!(!strategy_base_allows_axis(false, true, false));
    assert!(!strategy_base_allows_axis(false, false, true));
}

/// `analytics/refresh.rs:core_metadata_wait` must subtract elapsed time from the cadence; returning
/// the full minute loses the promised trailing core refresh when commits repeatedly hit the gate.
#[test]
fn core_metadata_throttle_keeps_the_original_deadline() {
    let start = Instant::now();

    assert_eq!(
        core_metadata_wait(Some(start), start + Duration::from_secs(10)),
        Duration::from_secs(50)
    );
    assert_eq!(
        core_metadata_wait(Some(start), start + Duration::from_secs(70)),
        Duration::ZERO
    );
    assert_eq!(core_metadata_wait(None, start), Duration::ZERO);
}

/// `analytics/refresh.rs:RefreshGate::observe_generation` must set `pending = true`; removing that
/// assignment makes a committed trade stay absent from an already-open Analytics window.
#[test]
fn burst_coalesces_to_one_trailing_refresh() {
    let start = Instant::now();
    let mut gate = RefreshGate::new(0, start);

    assert!(gate.observe_generation(1, start + Duration::from_millis(100)));
    assert_eq!(
        gate.plan(start + Duration::from_millis(100), false),
        RefreshPlan::After(Duration::from_secs(1))
    );

    assert!(gate.observe_generation(2, start + Duration::from_millis(600)));
    assert!(gate.observe_generation(3, start + Duration::from_millis(900)));
    gate.timer_fired();
    assert_eq!(
        gate.plan(start + Duration::from_millis(1100), false),
        RefreshPlan::After(Duration::from_millis(800))
    );

    gate.timer_fired();
    assert_eq!(
        gate.plan(start + Duration::from_millis(1900), false),
        RefreshPlan::Now {
            show_overlay: false
        }
    );
    gate.refresh_started(3, start + Duration::from_millis(1900));
    assert_eq!(
        gate.plan(start + Duration::from_secs(2), false),
        RefreshPlan::Idle
    );
}

/// `analytics/refresh.rs:RefreshGate::plan` must keep the maximum deadline; replacing `min` with
/// `max` lets a continuous report stream postpone visible Analytics updates forever.
#[test]
fn continuous_changes_reach_the_maximum_deadline() {
    let start = Instant::now();
    let mut gate = RefreshGate::new(0, start);

    assert!(gate.observe_generation(1, start + Duration::from_millis(100)));
    assert_eq!(
        gate.plan(start + Duration::from_millis(100), false),
        RefreshPlan::After(Duration::from_secs(1))
    );
    gate.timer_fired();
    assert!(gate.observe_generation(2, start + Duration::from_millis(9500)));
    assert_eq!(
        gate.plan(start + Duration::from_millis(10100), false),
        RefreshPlan::Now {
            show_overlay: false
        }
    );
}

/// `analytics/refresh.rs:RefreshGate::plan` must start the maximum deadline with the pending
/// burst, not the previous refresh; anchoring it to the previous refresh scans immediately after
/// an idle window and then scans the same catch-up burst again.
#[test]
fn first_change_after_idle_still_waits_for_quiet() {
    let start = Instant::now();
    let mut gate = RefreshGate::new(0, start);
    let after_idle = start + Duration::from_secs(30);

    assert!(gate.observe_generation(1, after_idle));
    assert_eq!(
        gate.plan(after_idle, false),
        RefreshPlan::After(Duration::from_secs(1))
    );
}

/// `analytics/refresh.rs:RefreshGate::request_refresh` must restore pending work; omitting the
/// assignment leaves a final transient Busy result failed forever when no later commit wakes it.
#[test]
fn transient_failure_rearms_a_quiet_retry() {
    let start = Instant::now();
    let mut gate = RefreshGate::new(1, start);
    gate.request_refresh(
        start + Duration::from_secs(2),
        false,
        RefreshUrgency::Writer,
    );

    assert_eq!(
        gate.plan(start + Duration::from_secs(2), false),
        RefreshPlan::After(Duration::from_secs(1))
    );
    gate.timer_fired();
    assert_eq!(
        gate.plan(start + Duration::from_secs(3), false),
        RefreshPlan::Now {
            show_overlay: false
        }
    );
}

/// `analytics/refresh.rs:RefreshGate::request_refresh` must retain User urgency and let it
/// override an armed Writer timer; clearing `pending_user` or restoring the timer interlock makes
/// a person wait behind report commits and leaves the visible Analytics surface stale after a click.
#[test]
fn user_requests_run_immediately_and_clear_when_the_refresh_starts() {
    let start = Instant::now();
    let mut gate = RefreshGate::new(0, start);

    gate.request_refresh(start, false, RefreshUrgency::User);
    assert_eq!(
        gate.plan(start, false),
        RefreshPlan::Now {
            show_overlay: false
        },
        "a User request with no elapsed time must not enter the quiet period"
    );
    assert_eq!(
        gate.plan(start, true),
        RefreshPlan::Idle,
        "active database work remains an interlock even for a User request"
    );
    gate.refresh_started(0, start);
    assert_eq!(
        gate.plan(start, false),
        RefreshPlan::Idle,
        "starting the User refresh must clear its urgency"
    );

    gate.request_refresh(start, false, RefreshUrgency::Writer);
    assert_eq!(
        gate.plan(start, false),
        RefreshPlan::After(Duration::from_secs(1)),
        "a Writer request must arm the quiet timer"
    );
    gate.request_refresh(
        start + Duration::from_millis(1),
        false,
        RefreshUrgency::User,
    );
    assert_eq!(
        gate.plan(start + Duration::from_millis(1), false),
        RefreshPlan::Now {
            show_overlay: false
        },
        "a User request must override an already armed Writer timer"
    );
}

/// `analytics/refresh.rs:RefreshGate::plan` must debounce Writer requests and enforce their
/// maximum interval; making every due time `now` turns a report commit stream into a full-period
/// scan storm on a live account.
#[test]
fn writer_requests_keep_the_quiet_period_and_maximum_deadline() {
    let start = Instant::now();
    let mut gate = RefreshGate::new(0, start);

    gate.request_refresh(
        start + Duration::from_millis(100),
        false,
        RefreshUrgency::Writer,
    );
    assert_eq!(
        gate.plan(start + Duration::from_millis(100), false),
        RefreshPlan::After(Duration::from_secs(1)),
        "a Writer request must retain the quiet period"
    );
    gate.timer_fired();
    gate.request_refresh(
        start + Duration::from_millis(9500),
        false,
        RefreshUrgency::Writer,
    );
    assert_eq!(
        gate.plan(start + Duration::from_millis(10100), false),
        RefreshPlan::Now {
            show_overlay: false
        },
        "continuous Writer requests must reach the original maximum deadline"
    );
}

/// `analytics/refresh.rs:RefreshGate::plan` must retain the `db_active` guard; deleting it starts a
/// second full-period scan over the first one and makes report bursts amplify database load.
#[test]
fn active_database_work_defers_the_refresh() {
    let start = Instant::now();
    let mut gate = RefreshGate::new(0, start);

    assert!(gate.observe_generation(1, start + Duration::from_millis(100)));
    assert_eq!(
        gate.plan(start + Duration::from_secs(11), true),
        RefreshPlan::Idle
    );
    assert_eq!(
        gate.plan(start + Duration::from_secs(11), false),
        RefreshPlan::Now {
            show_overlay: false
        }
    );
}

/// `analytics/refresh.rs:RefreshGate::observe_generation` must keep writer-only refreshes
/// nonblocking; setting `pending_overlay` for a commit restores the 5-10 second whole-window dim.
#[test]
fn writer_only_refresh_never_requests_the_busy_overlay() {
    let start = Instant::now();
    let mut gate = RefreshGate::new(0, start);

    assert!(gate.observe_generation(1, start));
    assert_eq!(
        gate.plan(start + Duration::from_secs(1), false),
        RefreshPlan::Now {
            show_overlay: false
        }
    );
}

/// `analytics/refresh.rs:RefreshGate::request_refresh` must OR a manual presentation request with
/// later writer generations; overwriting it hides feedback for a queued user tab/mode change.
#[test]
fn manual_overlay_intent_survives_coalesced_writer_generations() {
    let start = Instant::now();
    let mut gate = RefreshGate::new(0, start);

    gate.request_refresh(start, true, RefreshUrgency::Writer);
    gate.request_refresh(
        start + Duration::from_millis(250),
        false,
        RefreshUrgency::Writer,
    );
    assert!(gate.observe_generation(1, start + Duration::from_millis(500)));
    assert_eq!(
        gate.plan(start + Duration::from_millis(1500), false),
        RefreshPlan::Now { show_overlay: true }
    );
}

/// `analytics/refresh.rs:RefreshGate::refresh_started` must clear pending work; omitting it lets an
/// old debounce timer duplicate a manual period-button refresh and pay for two identical scans.
#[test]
fn manual_refresh_cancels_pending_work_logically() {
    let start = Instant::now();
    let mut gate = RefreshGate::new(0, start);

    assert!(gate.observe_generation(1, start + Duration::from_millis(100)));
    assert!(matches!(
        gate.plan(start + Duration::from_millis(100), false),
        RefreshPlan::After(_)
    ));
    gate.refresh_started(1, start + Duration::from_millis(200));
    gate.timer_fired();
    assert_eq!(
        gate.plan(start + Duration::from_secs(2), false),
        RefreshPlan::Idle
    );
}

/// `analytics/refresh.rs:BusyRetryBudget::claim` must reject the first attempt beyond its fixed
/// allowance; removing the cap makes a persistent SQLite lock run full-history scans forever.
#[test]
fn persistent_busy_has_a_bounded_retry_budget() {
    let mut budget = BusyRetryBudget::default();

    assert!(budget.claim());
    assert!(budget.claim());
    assert!(budget.claim());
    assert!(!budget.claim());
    budget.reset();
    assert!(budget.claim());
}

/// `analytics/refresh.rs:BusyRetryBudget::observe_generation` must not reset an active episode;
/// making it unconditional lets continuous report commits replenish every failed full-period
/// scan and bypass the three-attempt Busy limit.
#[test]
fn continuous_generations_do_not_replenish_an_active_busy_episode() {
    let mut budget = BusyRetryBudget::default();

    assert!(budget.claim());
    budget.observe_generation();
    assert!(budget.claim());
    budget.observe_generation();
    assert!(budget.claim());
    budget.observe_generation();
    assert!(!budget.claim());

    budget.resolve();
    budget.observe_generation();
    assert!(budget.claim());
}

/// `analytics/refresh.rs:visible_refresh` must route a stale axis directly when shared base data
/// is current; treating every Strategies entry alike repeats the full Summary scan before each
/// hidden axis under a large history.
#[test]
fn current_strategy_base_skips_the_redundant_summary_scan() {
    assert_eq!(
        visible_refresh(Tab::Strategies, false),
        VisibleRefresh::StrategyAxis
    );
    assert_eq!(
        visible_refresh(Tab::Strategies, true),
        VisibleRefresh::StrategyBaseAndAxis
    );
    assert_eq!(
        visible_refresh(Tab::Summary, false),
        VisibleRefresh::Summary
    );
    assert_eq!(
        visible_refresh(Tab::Calendar, false),
        VisibleRefresh::Calendar
    );
}
