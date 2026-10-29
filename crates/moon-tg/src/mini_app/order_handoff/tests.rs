//! Fleet identity publication, viewer expiry and reused-UID handoff regressions.

use super::*;
use crate::HostKind;
use crate::mini_app::tests::resting_order;

/// Keep the full ten-core fleet, including an early row after its live row disappears.
#[test]
fn chart_order_snapshots_keep_the_whole_live_fleet() {
    let mut state = TelegramState::new(&Default::default(), HostKind::Station);
    let mut row = resting_order(100.0, 101.0);
    row.entry_fill_time_ms = 100000.0;
    let rows: PreparedSnapshots = (1..=10)
        .flat_map(|core| (0..200).map(move |uid| (core, uid)))
        .map(|(core, uid)| {
            row.uid = uid;
            (core, OrderSnapshot::new(&row))
        })
        .collect();
    remember_order_snapshots(&mut state, &(1..=10).collect::<Vec<_>>(), rows);
    assert_eq!(
        state
            .mini_order_listed
            .values()
            .map(|rows| rows.len())
            .sum::<usize>(),
        2000
    );
    for core in 1..=10 {
        for uid in 0..200 {
            assert!(order_snapshot(&state, core, uid).is_some());
        }
    }
    begin_order_handoff(&mut state, 1, 0);
    remember_order_snapshots(&mut state, &[1], PreparedSnapshots::default());
    assert!(order_snapshot(&state, 1, 0).is_some());
    assert!(order_snapshot(&state, 1, 1).is_none());
    assert!(order_snapshot(&state, 2, 0).is_some());
    finish_order_handoff(&mut state, 1, 0, 99999);
    assert!(order_snapshot(&state, 1, 0).is_some());
    finish_order_handoff(&mut state, 1, 0, 100000);
    assert!(order_snapshot(&state, 1, 0).is_none());
}

/// Repeated turnover keeps current listed rows and a viewed handoff, never all prior fleets.
#[test]
fn chart_order_snapshots_keep_unresolved_handoffs() {
    let mut state = TelegramState::new(&Default::default(), HostKind::Station);
    let mut row = resting_order(100.0, 101.0);
    row.entry_fill_time_ms = 100000.0;
    row.uid = 999;
    remember_order(&mut state, 1, &row);
    begin_order_handoff(&mut state, 1, 999);
    remember_order_snapshots(
        &mut state,
        &[2],
        [(2, OrderSnapshot::new(&row))].into_iter().collect(),
    );
    for batch in 0..10 {
        // This is what the admitted background job drains before preparing the next response.
        drop(std::mem::take(&mut state.mini_order_retired));
        let rows = (0..200)
            .map(|index| {
                row.uid = 2000 + batch * 200 + index;
                (1, OrderSnapshot::new(&row))
            })
            .collect();
        remember_order_snapshots(&mut state, &[1], rows);
        assert_eq!(state.mini_order_listed[&1].len(), 200);
        assert_eq!(state.mini_order_snapshots.len(), 1);
        assert!(order_snapshot(&state, 1, 999).is_some());
        assert!(order_snapshot(&state, 2, 999).is_some());
        assert!(state.mini_order_retired.len() <= 1);
    }
    finish_order_handoff(&mut state, 1, 999, 100000);
    assert!(state.mini_order_handoffs.is_empty());
    assert!(state.mini_order_snapshots.is_empty());
    assert_eq!(state.mini_order_listed[&1].len(), 200);
}

/// Abandoned filled and pending charts must release identities without another request.
#[test]
fn abandoned_order_charts_release_handoffs() {
    let mut state = TelegramState::new(&Default::default(), HostKind::Station);
    let mut row = resting_order(100.0, 101.0);
    for uid in 0..2000 {
        row.uid = uid;
        row.entry_fill_time_ms = if uid % 2 == 0 { 100000.0 } else { 0.0 };
        remember_order(&mut state, 1, &row);
        begin_order_handoff(&mut state, 1, uid);
    }
    remember_order_snapshots(&mut state, &[1], PreparedSnapshots::default());
    assert_eq!(state.mini_order_snapshots.len(), 2000);
    prune_order_handoffs(&mut state, Instant::now() + Duration::from_secs(61));
    assert!(state.mini_order_handoffs.is_empty());
    assert!(state.mini_order_snapshots.is_empty());
}

/// A renewed viewer survives close; stale list results and old UID completions cannot replace it.
#[test]
fn renewed_order_chart_keeps_the_closing_identity() {
    let mut state = TelegramState::new(&Default::default(), HostKind::Station);
    let mut row = resting_order(100.0, 101.0);
    row.uid = 1;
    row.entry_fill_time_ms = 100000.0;
    let base = Instant::now();
    remember_order(&mut state, 1, &row);
    begin_order_handoff(&mut state, 1, 1);
    state
        .mini_order_handoffs
        .get_mut(&(1, 1))
        .unwrap()
        .viewed_at = base;
    remember_order_snapshots(&mut state, &[1], PreparedSnapshots::default());
    prune_order_handoffs(&mut state, base + Duration::from_secs(59));
    assert!(order_snapshot(&state, 1, 1).is_some());
    begin_order_handoff(&mut state, 1, 1);
    state
        .mini_order_handoffs
        .get_mut(&(1, 1))
        .unwrap()
        .viewed_at = base + Duration::from_secs(59);
    prune_order_handoffs(&mut state, base + Duration::from_secs(61));
    // A list captured before this execution was observed finishes after it.
    row.entry_fill_time_ms = 99999.0;
    remember_order_snapshots(
        &mut state,
        &[1],
        [(1, OrderSnapshot::new(&row))].into_iter().collect(),
    );
    assert_eq!(order_snapshot(&state, 1, 1).unwrap().entry_ms, 100000);
    finish_order_handoff(&mut state, 1, 1, 99999);
    assert_eq!(state.mini_order_handoffs[&(1, 1)].entry_ms, 100000);
    remember_order_snapshots(&mut state, &[1], PreparedSnapshots::default());
    finish_order_handoff(&mut state, 1, 1, 100000);
    assert!(order_snapshot(&state, 1, 1).is_none());
    assert!(state.mini_order_handoffs.is_empty());
}
