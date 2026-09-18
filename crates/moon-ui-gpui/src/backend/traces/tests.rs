use std::collections::HashMap;
use std::sync::Arc;

use moon_core::db::order_traces::TraceEntry;
use moon_core::feed::{ArchivedLineKind, ArchivedOrderTrace, FeedMsg, ReportTracesOutcome};
use moon_core::session::CoreStore;
use moon_core::session::store::CoreData;

// Named imports, not `super::*`: the parent glob-imports gpui, whose `test` attribute would
// shadow the built-in one under a glob and recurse.
use super::{SLOT_CAP, TraceResolver, TraceState};

fn line() -> ArchivedOrderTrace {
    ArchivedOrderTrace {
        own: true,
        kind: ArchivedLineKind::Entry,
        stop_price: None,
        stop_time_ms: None,
        points: vec![(1_000.0, 1.0)],
    }
}

fn lines() -> Arc<[ArchivedOrderTrace]> {
    Arc::from(vec![line()])
}

#[test]
fn archive_hits_settle_and_misses_are_asked_up_to_the_cap() {
    let mut r = TraceResolver::default();
    let to_read = r.begin_read(7, &[1, 0, 2, 2, 3, 4]);
    assert_eq!(to_read, vec![1, 2, 3, 4]);
    assert_eq!(r.state(7, 1), TraceState::Loading);
    // Nothing is read twice while loading.
    assert!(r.begin_read(7, &[1, 2]).is_empty());

    let mut read = HashMap::new();
    read.insert(1, TraceEntry::Lines(lines()));
    read.insert(
        2,
        TraceEntry::Empty {
            checked_at_ms: 1_000,
        },
    );
    read.insert(3, TraceEntry::Empty { checked_at_ms: 0 });
    let stale_now = 1_000 + 10 * 24 * 60 * 60 * 1000;
    let ask = r.apply_read(7, &read, &to_read, 1, stale_now, (5, 1));
    // 2's empty answer is stale too at this clock, so the misses are 2, 3, 4 — one gets asked.
    assert_eq!(ask, vec![2]);
    assert!(matches!(r.state(7, 1), TraceState::Lines(_)));
    assert_eq!(r.state(7, 2), TraceState::Pending);
    assert_eq!(r.state(7, 3), TraceState::Unasked);
    assert_eq!(r.state(7, 4), TraceState::Unasked);
    // A later call reaching the unasked rows asks without another read.
    assert!(r.begin_read(7, &[3, 4]).is_empty());
    assert_eq!(r.begin_unasked(7, &[4, 3], 1, (5, 1)), vec![4]);
    assert_eq!(r.state(7, 4), TraceState::Pending);
    assert_eq!(r.state(7, 3), TraceState::Unasked);
}

#[test]
fn a_fresh_empty_answer_from_the_archive_is_final_for_the_session() {
    let mut r = TraceResolver::default();
    let to_read = r.begin_read(1, &[9]);
    let mut read = HashMap::new();
    read.insert(
        9,
        TraceEntry::Empty {
            checked_at_ms: 1_000,
        },
    );
    assert!(
        r.apply_read(1, &read, &to_read, 10, 2_000, (0, 0))
            .is_empty()
    );
    assert_eq!(r.state(1, 9), TraceState::Empty);
}

#[test]
fn adopt_takes_only_answers_filed_after_the_ask_and_fails_on_a_new_epoch() {
    let mut r = TraceResolver::default();
    let mut core = CoreData::new();
    // An answer filed BEFORE the ask: rev 1.
    core.apply(FeedMsg::ReportTraces {
        report_uid: 5,
        outcome: ReportTracesOutcome::Ready(lines()),
    });
    let marks = (core.report_traces_rev, core.report_traces_epoch);
    r.begin_retry(3, 5, marks);
    r.begin_retry(3, 6, marks);
    let mut store = CoreStore::default();
    store.ensure(3);
    *store.core_mut(3).unwrap() = core;
    assert!(
        !r.adopt_all(&store),
        "the stale entry is not this ask's answer"
    );
    assert_eq!(r.state(3, 5), TraceState::Pending);
    store.core_mut(3).unwrap().apply(FeedMsg::ReportTraces {
        report_uid: 5,
        outcome: ReportTracesOutcome::Ready(Arc::from(Vec::new())),
    });
    assert!(r.adopt_all(&store));
    assert_eq!(r.state(3, 5), TraceState::Empty);
    assert_eq!(r.state(3, 6), TraceState::Pending);
    // The core process is replaced: the store clears and bumps its epoch.
    store.core_mut(3).unwrap().apply(FeedMsg::RunStateForgotten);
    assert!(r.adopt_all(&store));
    assert_eq!(r.state(3, 6), TraceState::Failed);
}

#[test]
fn a_core_gone_from_the_store_fails_its_pending_rows() {
    let mut r = TraceResolver::default();
    r.begin_retry(4, 1, (0, 0));
    let store = CoreStore::default();
    assert!(r.adopt_all(&store));
    assert_eq!(r.state(4, 1), TraceState::Failed);
}

#[test]
fn eviction_keeps_what_is_in_flight() {
    let mut r = TraceResolver::default();
    r.begin_retry(1, -1, (0, 0));
    for uid in 1..=(SLOT_CAP as i64 + 10) {
        r.set(1, uid, TraceState::Empty, 0);
    }
    assert_eq!(r.state(1, -1), TraceState::Pending);
    assert_eq!(
        r.state(1, 1),
        TraceState::Unknown,
        "the oldest settled row went"
    );
    assert!(r.slots.len() <= SLOT_CAP);
}
