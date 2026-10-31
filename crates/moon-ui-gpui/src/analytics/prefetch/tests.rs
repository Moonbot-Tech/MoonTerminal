//! Speculation ownership, promotion and classified-failure regression checks.

use std::time::Instant;

use moon_core::db::ReadFail;
use moon_core::db::analytics::{AnalyticsRead, Query};

use super::{PrefetchState, analytics_failures};
use crate::analytics::Tab;
use crate::analytics::bg::{LatestReads, ReadLane};
use crate::analytics::cache::{CacheKey, CacheRevisions};
use crate::analytics::refresh::{
    RefreshGate, RefreshPlan, RefreshUrgency, VisibleRefresh, visible_refresh,
};

/// Distinct period and publication keys exercise the real foreground/prefetch identity.
fn key(from: i64, reports: u64) -> CacheKey {
    CacheKey::strategies(
        &Query {
            from,
            to: from + 86_400,
            ..Query::default()
        },
        None,
        CacheRevisions {
            reports,
            ..CacheRevisions::default()
        },
    )
}

/// Calendar identities include their resolution and comparison inputs as well as the period.
fn calendar_key(from: i64, reports: u64) -> CacheKey {
    CacheKey::calendar(
        &Query {
            from,
            to: from + 86_400,
            ..Query::default()
        },
        None,
        true,
        None,
        CacheRevisions {
            reports,
            ..CacheRevisions::default()
        },
    )
}

/// Cancellation cannot release the slot early and let rapid tab clicks flood SQLite workers.
#[test]
fn cancelled_speculation_drains_before_a_replacement_starts() {
    let mut state = PrefetchState::default();
    let first = state
        .start(key(1, 0), Tab::Strategies)
        .expect("first worker");
    state.cancel_unrelated(Tab::Calendar);
    assert!(first.is_cancelled());
    assert!(state.is_active());
    assert!(state.start(calendar_key(2, 0), Tab::Calendar).is_none());
    assert!(
        !state.join(&key(1, 0)),
        "cancelled work cannot own a visible page"
    );
    state.finish(&first).expect("cancellation acknowledged");
    assert!(state.start(calendar_key(2, 0), Tab::Calendar).is_some());
}

/// A selected axis should wait for cancellation acknowledgement, not a full neighbouring scan.
#[test]
fn cancelled_speculation_hands_off_to_the_queued_user_axis() {
    let now = Instant::now();
    let mut state = PrefetchState::default();
    let mut gate = RefreshGate::new(0, now);
    let token = state
        .start(calendar_key(1, 0), Tab::Calendar)
        .expect("neighbour preparation");

    state.cancel_unpromoted();
    gate.request_refresh(now, true, RefreshUrgency::User);
    assert!(token.is_cancelled());
    assert!(state.is_active());
    assert_eq!(gate.plan(now, state.is_active()), RefreshPlan::Idle);
    let completed = state.finish(&token).expect("cancellation acknowledged");
    assert!(completed.cancellation.is_cancelled());
    assert_eq!(
        gate.plan(now, state.is_active()),
        RefreshPlan::Now { show_overlay: true },
    );
    assert_eq!(
        visible_refresh(Tab::Strategies, false),
        VisibleRefresh::StrategyAxis,
    );
}

/// Joining a visible base promotes it above background preparation before its axis can be queued.
#[test]
fn foreground_axis_preserves_its_matching_promoted_base() {
    let now = Instant::now();
    let mut state = PrefetchState::default();
    let mut gate = RefreshGate::new(0, now);
    let request = key(1, 0);
    let token = state
        .start(request.clone(), Tab::Strategies)
        .expect("base preparation");
    assert!(state.join(&request));
    state.cancel_unpromoted();
    gate.request_refresh(now, true, RefreshUrgency::User);
    assert!(!token.is_cancelled());
    assert!(state.is_active());
    assert_eq!(gate.plan(now, state.is_active()), RefreshPlan::Idle);
    assert!(
        state
            .finish(&token)
            .expect("visible base complete")
            .promoted
    );
    assert_eq!(
        gate.plan(now, state.is_active()),
        RefreshPlan::Now { show_overlay: true }
    );
}

/// Foreground requests must cancel speculation before the shared refresh interlock is inspected.
#[test]
fn user_refresh_yields_speculation_before_queuing_foreground_work() {
    let observer = include_str!("../observe.rs");
    let request = observer
        .split_once("fn request_report_refresh(")
        .expect("foreground request")
        .1
        .split_once("fn schedule_report_refresh(")
        .expect("request boundary")
        .0;
    let user = request
        .find("if urgency == RefreshUrgency::User {")
        .expect("user-only priority override");
    let cancel = request
        .find("self.yield_speculative_prefetch();")
        .expect("yield speculation");
    let queue = request.find(".request_refresh(").expect("queue foreground");
    let dispatch = request
        .find("self.schedule_report_refresh(cx);")
        .expect("shared interlock");
    assert!(user < cancel && cancel < queue && queue < dispatch);
}

/// Entering the exact target reuses its worker, while another period cannot adopt its answer.
#[test]
fn exact_join_promotes_without_cancelling_or_duplicating_work() {
    let mut state = PrefetchState::default();
    let token = state
        .start(key(1, 0), Tab::Strategies)
        .expect("first worker");
    state.cancel_unrelated(Tab::Strategies);
    assert!(!token.is_cancelled());
    assert!(!state.join(&key(2, 0)));
    assert!(state.join(&key(1, 0)));
    assert!(state.finish(&token).expect("promoted completion").promoted);
    let next = state
        .start(calendar_key(2, 0), Tab::Calendar)
        .expect("next worker");
    assert!(
        state.finish(&token).is_none(),
        "old completion must preserve new ownership"
    );
    assert!(state.is_active());
    assert!(!next.is_cancelled());
}

/// A joined base owns the queued navigation request, so its axis completion cannot trigger it again.
#[test]
fn promoted_prefetch_acknowledges_navigation_before_axis_completion() {
    let now = Instant::now();
    let mut state = PrefetchState::default();
    let mut gate = RefreshGate::new(0, now);
    let token = state
        .start(key(1, 0), Tab::Strategies)
        .expect("speculative base");
    gate.request_refresh(now, true, RefreshUrgency::User);
    assert_eq!(gate.plan(now, state.is_active()), RefreshPlan::Idle);

    assert!(
        state.join(&key(1, 0)),
        "the visible page reuses the running base"
    );
    gate.refresh_started(0, now);
    assert!(!gate.has_pending());
    assert_eq!(gate.plan(now, state.is_active()), RefreshPlan::Idle);

    let completed = state.finish(&token).expect("joined base published");
    assert!(completed.promoted);
    let mut reads = LatestReads::default();
    let axis = reads.replace(&[ReadLane::FilterKpi, ReadLane::FilterHistogram]);
    assert_eq!(
        gate.plan(
            now,
            state.is_active() || reads.contains(ReadLane::FilterKpi)
        ),
        RefreshPlan::Idle
    );
    reads.finish(&axis);
    assert_eq!(
        gate.plan(
            now,
            state.is_active() || reads.contains(ReadLane::FilterKpi)
        ),
        RefreshPlan::Idle,
        "finishing the chained axis must not schedule a duplicate full axis refresh"
    );
}

/// Selection can cancel a joined base, but its still-visible result must remain owed after the axis.
#[test]
fn cancelled_promoted_base_recovers_through_the_serialized_refresh_gate() {
    let now = Instant::now();
    let mut state = PrefetchState::default();
    let mut gate = RefreshGate::new(0, now);
    let token = state
        .start(key(1, 0), Tab::Strategies)
        .expect("speculative base");
    gate.request_refresh(now, false, RefreshUrgency::User);
    assert!(state.join(&key(1, 0)));
    gate.refresh_started(0, now);
    assert!(!gate.has_pending());

    let mut reads = LatestReads::default();
    let axis = reads.replace(&[ReadLane::FilterKpi, ReadLane::FilterHistogram]);
    state.cancel();
    assert!(token.is_cancelled());
    let completed = state
        .finish(&token)
        .expect("promoted cancellation acknowledged");
    assert!(completed.needs_visible_retry(Tab::Strategies, &key(1, 1)));
    if completed.needs_visible_retry(Tab::Strategies, &key(1, 1)) {
        gate.request_refresh(now, false, RefreshUrgency::User);
    }
    assert_eq!(
        gate.plan(now, reads.contains(ReadLane::FilterKpi)),
        RefreshPlan::Idle,
        "recovery must not overlap the axis that cancelled the promoted base"
    );
    reads.finish(&axis);
    assert_eq!(
        gate.plan(now, true),
        RefreshPlan::Idle,
        "an outstanding write still blocks recovery"
    );
    assert_eq!(
        gate.plan(now, false),
        RefreshPlan::Now {
            show_overlay: false
        }
    );
    assert_eq!(
        visible_refresh(Tab::Strategies, true),
        VisibleRefresh::StrategyBaseAndAxis,
        "the retained dirty base must recover before its axis can be considered complete"
    );
    gate.refresh_started(1, now);
    assert_eq!(gate.plan(now, false), RefreshPlan::Idle);
}

/// A cancelled owner must never resurrect work under another page or a different period/filter.
#[test]
fn promoted_retry_does_not_cross_visible_semantic_scope() {
    let mut state = PrefetchState::default();
    let token = state
        .start(key(1, 0), Tab::Strategies)
        .expect("speculative base");
    assert!(state.join(&key(1, 0)));
    state.cancel();
    let completed = state.finish(&token).expect("cancelled owner");
    assert!(!completed.needs_visible_retry(Tab::Summary, &key(1, 0)));
    assert!(!completed.needs_visible_retry(Tab::Strategies, &key(2, 0)));
    assert!(completed.needs_visible_retry(Tab::Strategies, &key(1, 1)));
    let unpromoted = state
        .start(key(3, 0), Tab::Strategies)
        .expect("unpromoted base");
    state.cancel();
    let unpromoted = state.finish(&unpromoted).expect("unpromoted cancellation");
    assert!(!unpromoted.needs_visible_retry(Tab::Strategies, &key(3, 0)));
}

/// Every lost promoted result, including cancellation, must queue recovery through the shared gate.
#[test]
fn promoted_completion_recovery_is_serialized_and_scope_checked() {
    let source = include_str!("../prefetch.rs");
    let completion = source
        .split_once("let Some(completed) = this.prefetch.finish(&cancellation)")
        .expect("speculative completion ownership")
        .1
        .split_once("this.schedule_report_refresh(cx);")
        .expect("shared refresh dispatch")
        .0;
    let compact: String = completion
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(compact.contains("published=this.publish_prefetched_snapshot(&request,result,cx);"));
    let cancelled = completion
        .find("if !completed.cancellation.is_cancelled() {")
        .expect("cancelled publication guard");
    let retry = completion
        .find("if !published")
        .expect("lost publication recovery");
    assert!(
        cancelled < retry,
        "cancelled reads must reach the recovery branch too"
    );
    assert!(compact.contains("completed.needs_visible_retry("));
    assert!(completion.contains("&this.snapshot_request(this.tab).key"));
    assert!(completion.contains("this.request_report_refresh(RefreshUrgency::User, false, cx);"));
    assert!(!completion.contains("reload_visible_snapshot"));
    let predicate = source
        .split_once("fn needs_visible_retry(")
        .expect("retry ownership predicate")
        .1
        .split_once("\n    }")
        .expect("retry predicate boundary")
        .0;
    assert!(
        predicate.contains("self.promoted && self.target == visible && self.key.same_scope(key)")
    );
}

/// A different period of the same tab cancels its worker and retains the draining interlock.
#[test]
fn same_destination_different_query_cancels_before_join() {
    let mut state = PrefetchState::default();
    let token = state
        .start(calendar_key(1, 0), Tab::Calendar)
        .expect("calendar prefetch");
    state.cancel_unrelated(Tab::Calendar);
    assert!(!token.is_cancelled(), "the destination alone still matches");
    state.cancel_unless_matches(&calendar_key(1, 0));
    assert!(
        !token.is_cancelled(),
        "an exact foreground request retains the read"
    );
    state.cancel_unless_matches(&calendar_key(2, 0));
    assert!(token.is_cancelled());
    assert!(!state.join(&calendar_key(2, 0)));
    assert!(state.is_active());
    assert!(state.start(calendar_key(2, 0), Tab::Calendar).is_none());
    state
        .finish(&token)
        .expect("old scope acknowledged cancellation");
    assert!(state.start(calendar_key(2, 0), Tab::Calendar).is_some());
}

/// Foreground work and publication invalidation must relinquish speculative database ownership.
#[test]
fn production_work_cancels_speculation_before_replacement_or_invalidation() {
    let background = include_str!("../bg.rs");
    for signature in ["fn spawn_db<", "fn spawn_latest_db<"] {
        let body = background
            .split_once(signature)
            .unwrap_or_else(|| panic!("{signature} entrypoint"))
            .1
            .split_once("cx.spawn(")
            .expect("background launch boundary")
            .0;
        let cancel = body
            .find("self.cancel_prefetch();")
            .expect("cancel speculative owner");
        let count = body
            .find("self.db_ops += 1;")
            .expect("foreground work admitted");
        assert!(
            cancel < count,
            "{signature} must cancel speculation before admitting foreground work"
        );
    }
    let observers = include_str!("../observe.rs");
    let invalidation = observers
        .split_once("fn mark_report_data_stale(")
        .expect("publication invalidation")
        .1
        .split_once("fn acknowledge_report_refresh(")
        .expect("invalidation boundary")
        .0;
    let cancel = invalidation
        .find("self.cancel_prefetch();")
        .expect("cancel obsolete input");
    let dirty = invalidation
        .find("self.data_dirty = true;")
        .expect("invalidate report snapshot");
    assert!(cancel < dirty);

    let prefetch = include_str!("../prefetch.rs");
    let join = prefetch
        .split_once("fn join_prefetch(")
        .expect("foreground join")
        .1
        .split_once("fn schedule_prefetch(")
        .expect("join boundary")
        .0;
    let cancel = join
        .find("self.cancel_prefetch_unless_matches(key);")
        .expect("validate full query");
    let promote = join
        .find("self.prefetch.join(key)")
        .expect("promote identical read");
    assert!(cancel < promote);
}

/// Failed or uncacheable work stays attempted until the visible request or publication changes.
#[test]
fn completed_attempt_is_not_retried_in_an_idle_loop() {
    let mut state = PrefetchState::default();
    state.set_visible_scope(key(10, 0));
    let token = state
        .start(key(1, 0), Tab::Strategies)
        .expect("first worker");
    state.finish(&token).expect("completed attempt");
    state.set_visible_scope(key(10, 0));
    assert!(state.start(key(1, 0), Tab::Strategies).is_none());
    state.set_visible_scope(key(10, 1));
    assert!(state.start(key(1, 1), Tab::Strategies).is_some());
}

/// The detached worker must stop when its only destination window has been destroyed.
#[test]
fn closing_window_cancels_owned_speculative_work() {
    let token = {
        let mut state = PrefetchState::default();
        state
            .start(key(1, 0), Tab::Strategies)
            .expect("owned worker")
    };
    assert!(token.is_cancelled());
}

/// Speculation preserves independent failures instead of manufacturing a successful empty page.
#[test]
fn compound_prefetch_failure_keeps_each_classified_error() {
    let failures = analytics_failures(AnalyticsRead::<()> {
        data: Err(ReadFail::NotReady),
        undated: Err(ReadFail::PeriodOutOfRange),
        cores: Some(Err(ReadFail::NotReady)),
    });
    assert_eq!(failures.len(), 3);
    assert!(matches!(failures[0], ReadFail::NotReady));
    assert!(matches!(failures[1], ReadFail::PeriodOutOfRange));
    assert!(matches!(failures[2], ReadFail::NotReady));
}
