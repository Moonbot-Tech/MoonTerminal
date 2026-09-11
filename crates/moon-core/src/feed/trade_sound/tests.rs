//! Regression fixtures for execution edges; no protocol diagnostics or live connection required.

use super::{Facts, TradeEdge, TradeSoundState};
use moonproto::OrderWorkerStatus;
use moonproto::{Event, state::OrderEvent};

/// An unchanged protocol snapshot can retain a holding without emitting a row event.
#[test]
fn snapshot_only_holding_then_terminal_removal_closes_once() {
    let mut state = TradeSoundState::default();
    assert!(
        state
            .observe_batch([], true, true, Some([partial(1)]))
            .is_empty()
    );
    // The current read model is already empty; only the captured Removed row proves closure.
    let sounds = state.observe_batch([(closed(1), true)], true, false, Some([]));
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].edge, TradeEdge::Close);
    assert!(
        state
            .observe_batch([(closed(1), true)], true, false, Some([]))
            .is_empty()
    );
}

/// Snapshot reconciliation advances the pending latch even without an accompanying update.
#[test]
fn pending_to_held_snapshot_does_not_replay_on_incremental_update() {
    let mut state = TradeSoundState::default();
    state.observe_batch([(pending(1), false)], true, false, Some([pending(1)]));
    assert!(
        state
            .observe_batch([], true, true, Some([partial(1)]))
            .is_empty()
    );
    assert!(
        state
            .observe_batch([(partial(1), false)], true, false, Some([partial(1)]))
            .is_empty()
    );
    let sounds = state.observe_batch([(closed(1), true)], true, false, Some([]));
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].edge, TradeEdge::Close);
}

/// Read-model membership removes stale holdings without treating disappearance as execution.
#[test]
fn snapshot_forgets_missing_identities_but_retains_present_holdings() {
    let mut state = TradeSoundState::default();
    state.observe_batch([], true, true, Some([partial(1), partial(2)]));
    assert!(
        state
            .observe_batch([], true, true, Some([partial(2)]))
            .is_empty()
    );
    let sounds = state.observe_batch(
        [(closed(1), true), (closed(2), true)],
        true,
        false,
        Some([]),
    );
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].uid, 2);
    assert_eq!(sounds[0].edge, TradeEdge::Close);
}

/// Startup and reconnect can establish a ready baseline without a domain event in the drain.
#[test]
fn startup_and_reconnect_reconcile_retained_rows_without_events() {
    let mut state = TradeSoundState::default();
    for _ in 0..2 {
        assert!(
            state
                .observe_batch([], false, false, Some([pending(1)]))
                .is_empty()
        );
        assert!(
            state
                .observe_batch([], true, false, Some([partial(1)]))
                .is_empty()
        );
        assert!(
            state
                .observe_batch([(partial(1), false)], true, false, Some([partial(1)]))
                .is_empty()
        );
        let sounds = state.observe_batch([(closed(1), true)], true, false, Some([]));
        assert_eq!(sounds.len(), 1);
        assert_eq!(sounds[0].edge, TradeEdge::Close);
        state.reset();
    }
}

/// Ordinary event batches must not seed from a newer snapshot and spend their live entry early.
#[test]
fn incremental_entry_is_not_suppressed_by_latest_held_snapshot() {
    let mut state = TradeSoundState::default();
    state.observe_batch([], true, true, Some([pending(1)]));
    let sounds = state.observe_batch([(partial(1), false)], true, false, Some([partial(1)]));
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].edge, TradeEdge::Open);
}

/// Captured silent terminal facts keep their spent latch even if a retained row is older.
#[test]
fn snapshot_reconciliation_preserves_spent_terminal_latch() {
    let mut state = TradeSoundState::default();
    state.observe_batch([], true, true, Some([partial(1)]));
    assert!(
        state
            .observe_batch([(closed(1), false)], true, true, Some([partial(1)]))
            .is_empty()
    );
    assert!(
        state
            .observe_batch([(closed(1), true)], true, false, Some([]))
            .is_empty()
    );
}

/// An empty canonical snapshot arms the next observed pending-to-fill transition.
#[test]
fn empty_ready_snapshot_arms_the_first_live_batch() {
    let mut state = TradeSoundState::default();
    assert!(
        state
            .observe(
                &[Event::Order(OrderEvent::Snapshot)],
                true,
                Some(&moonproto::state::Orders::new())
            )
            .is_empty()
    );
    let sounds = state.observe_batch(
        [(pending(1), false), (partial(1), false)],
        true,
        false,
        None::<[Facts; 0]>,
    );
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].edge, TradeEdge::Open);
}

/// The final captured row can prove closure even when no update survived before removal.
#[test]
fn removal_only_closure_uses_final_facts_and_deduplicates() {
    let mut state = TradeSoundState::default();
    state.observe_batch([(partial(1), false)], true, true, None::<[Facts; 0]>);
    let sounds = state.observe_batch([(closed(1), true)], true, false, None::<[Facts; 0]>);
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].edge, TradeEdge::Close);
    assert!(
        state
            .observe_batch([(closed(1), true)], true, false, None::<[Facts; 0]>)
            .is_empty()
    );
    state.observe_batch([(partial(2), false)], true, false, None::<[Facts; 0]>);
    let mut canceled = closed(2);
    canceled.status = OrderWorkerStatus::SellCancel;
    assert!(
        state
            .observe_batch([(canceled, true)], true, false, None::<[Facts; 0]>)
            .is_empty()
    );
}

/// Lifecycle reset and silent snapshot segments cannot defer an old edge.
#[test]
fn startup_reconnect_and_snapshot_batches_are_silent() {
    let mut state = TradeSoundState::default();
    assert!(
        state
            .observe_batch(
                [(pending(1), false), (partial(1), false)],
                false,
                false,
                None::<[Facts; 0]>
            )
            .is_empty()
    );
    state.reset();
    assert!(
        state
            .observe_batch(
                [(pending(1), false), (partial(1), false)],
                true,
                true,
                None::<[Facts; 0]>
            )
            .is_empty()
    );
    assert!(
        state
            .observe_batch([(partial(1), false)], true, false, None::<[Facts; 0]>)
            .is_empty()
    );
    state.reset();
    assert!(
        state
            .observe_batch([(closed(1), false)], true, false, None::<[Facts; 0]>)
            .is_empty()
    );
}

/// A resting, unfilled entry of ten units.
fn pending(uid: u64) -> Facts {
    Facts {
        uid,
        market: "BTCUSDT".into(),
        short: false,
        platform: 1,
        created: 100,
        emulator: false,
        quantity: 10.0,
        remaining: 10.0,
        executed: 0.0,
        status: OrderWorkerStatus::BuySet,
        exit_closed: false,
        exit_canceled: false,
        exit_quantity: 0.0,
        exit_remaining: 0.0,
        exit_executed: 0.0,
    }
}

/// First partial fill acquires two units while the entry remains working.
fn partial(uid: u64) -> Facts {
    Facts {
        remaining: 8.0,
        executed: 2.0,
        ..pending(uid)
    }
}

/// Final successful exit, distinct from cancellation and SellAlmostDone.
fn closed(uid: u64) -> Facts {
    Facts {
        remaining: 0.0,
        executed: 10.0,
        status: OrderWorkerStatus::SellDone,
        exit_closed: true,
        exit_quantity: 10.0,
        exit_remaining: 0.0,
        exit_executed: 10.0,
        ..pending(uid)
    }
}

#[test]
/// Repeated and partial executions must not spend more than one edge per lifecycle.
fn partial_entry_announces_once_and_only_complete_exit_closes() {
    let mut state = TradeSoundState::default();
    assert!(state.observe_facts(pending(1), false).is_none());
    assert_eq!(
        state.observe_facts(partial(1), false).unwrap().edge,
        TradeEdge::Open
    );
    assert!(state.observe_facts(partial(1), false).is_none());
    let mut more = partial(1);
    more.remaining = 3.0;
    more.executed = 7.0;
    assert!(state.observe_facts(more, false).is_none());
    let mut almost = closed(1);
    almost.status = OrderWorkerStatus::SellAlmostDone;
    assert!(state.observe_facts(almost, false).is_none());
    assert_eq!(
        state.observe_facts(closed(1), false).unwrap().edge,
        TradeEdge::Close
    );
    assert!(state.observe_facts(closed(1), false).is_none());
}

/// A full first fill is still a single opening, followed by a separate successful exit.
#[test]
fn normal_entry_and_exit_have_separate_edges() {
    let mut state = TradeSoundState::default();
    state.observe_facts(pending(1), false);
    let mut filled = partial(1);
    filled.remaining = 0.0;
    filled.executed = 10.0;
    filled.status = OrderWorkerStatus::BuyDone;
    assert_eq!(
        state.observe_facts(filled, false).unwrap().edge,
        TradeEdge::Open
    );
    assert_eq!(
        state.observe_facts(closed(1), false).unwrap().edge,
        TradeEdge::Close
    );
}

#[test]
/// Rebuilding connection state cannot reinterpret an existing position as a new entry.
fn first_seen_fill_and_reconnect_seed_without_replaying_open() {
    let mut state = TradeSoundState::default();
    assert!(state.observe_facts(partial(1), false).is_none());
    assert_eq!(
        state.observe_facts(closed(1), false).unwrap().edge,
        TradeEdge::Close
    );
    state.reset();
    assert!(state.observe_facts(partial(1), false).is_none());
    assert!(state.observe_facts(partial(1), false).is_none());
    state.reset();
    assert!(state.observe_facts(closed(1), false).is_none());
}

#[test]
/// Silent reconciliation still advances latches instead of deferring the notification.
fn silent_snapshot_spends_both_edges() {
    let mut state = TradeSoundState::default();
    state.observe_facts(pending(1), false);
    assert!(state.observe_facts(partial(1), true).is_none());
    assert!(state.observe_facts(partial(1), false).is_none());
    assert!(state.observe_facts(closed(1), true).is_none());
    assert!(state.observe_facts(closed(1), false).is_none());
}

#[test]
/// Terminal failures and contradictory exit facts cannot masquerade as a closed position.
fn cancellation_failure_and_unproven_exit_never_close() {
    for status in [
        OrderWorkerStatus::BuyCancel,
        OrderWorkerStatus::BuyFail,
        OrderWorkerStatus::SellCancel,
        OrderWorkerStatus::SellFail,
    ] {
        let mut state = TradeSoundState::default();
        state.observe_facts(partial(1), true);
        let mut row = closed(1);
        row.status = status;
        assert!(state.observe_facts(row, false).is_none());
    }
    for change in 0..4 {
        let mut state = TradeSoundState::default();
        state.observe_facts(partial(1), true);
        let mut row = closed(1);
        match change {
            0 => row.exit_canceled = true,
            1 => row.exit_closed = false,
            2 => row.exit_remaining = 1.0,
            _ => row.exit_executed = f64::NAN,
        }
        assert!(state.observe_facts(row, false).is_none());
    }
}

#[test]
/// Each core's reducer treats the canonical buy leg as entry for either direction.
fn shorts_keep_direction_and_core_local_uids_do_not_collide() {
    for short in [false, true] {
        let mut state = TradeSoundState::default();
        let mut initial = pending(7);
        initial.short = short;
        state.observe_facts(initial, false);
        let mut row = partial(7);
        row.short = short;
        let sound = state.observe_facts(row, false).unwrap();
        assert_eq!(sound.edge, TradeEdge::Open);
        assert_eq!(sound.is_short, short);
        assert_eq!(sound.platform, 1);
        assert_eq!(sound.uid, 7);
    }
}

#[test]
/// Reused UIDs cannot join different instruments, routes, instances, or simulated trades.
fn changed_identity_and_emulation_seed_silently() {
    for change in 0..5 {
        let mut state = TradeSoundState::default();
        state.observe_facts(pending(1), false);
        let mut row = partial(1);
        match change {
            0 => row.market = "ETHUSDT".into(),
            1 => row.short = true,
            2 => row.platform = 2,
            3 => row.created = 200,
            _ => row.emulator = true,
        }
        assert!(state.observe_facts(row, false).is_none());
    }
}

#[test]
/// Incomplete execution sections must favor missed notifications over inferred fills.
fn mismatched_or_nonfinite_execution_sections_do_not_open() {
    for change in 0..4 {
        let mut state = TradeSoundState::default();
        state.observe_facts(pending(1), false);
        let mut row = partial(1);
        match change {
            0 => row.executed = 0.0,
            1 => row.remaining = -1.0,
            2 => row.quantity = 20.0,
            _ => row.remaining = f64::NAN,
        }
        assert!(state.observe_facts(row, false).is_none());
    }
}

/// A completed unchanged catalog cannot spend the subsequent first fill or completed exit.
#[test]
fn completed_catalog_preserves_live_open_and_close_suffix() {
    use super::Observation::{Row, Snapshot};
    let mut state = TradeSoundState::default();
    state.observe_batch([], true, true, Some([pending(1), partial(2)]));
    let sounds = state.observe_sequence(
        &[Snapshot, Row(partial(1), false), Row(closed(2), true)],
        true,
        // Deliberately newer than the captured first fill: seeding it first loses the open.
        Some([closed(1)]),
    );
    assert_eq!(
        sounds.iter().map(|s| (s.uid, s.edge)).collect::<Vec<_>>(),
        vec![(1, TradeEdge::Open), (2, TradeEdge::Close)]
    );
    assert!(
        state
            .observe_sequence(
                &[Row(partial(1), false), Row(closed(2), true)],
                true,
                None::<[Facts; 0]>
            )
            .is_empty()
    );
}

/// Snapshot image rows seed silently; only rows after the final completed marker may announce.
#[test]
fn final_marker_partitions_multiple_images_from_live_suffix() {
    use super::Observation::{Row, Snapshot};
    let mut state = TradeSoundState::default();
    state.observe_batch([], true, true, Some([pending(1), pending(2), pending(3)]));
    let sounds = state.observe_sequence(
        &[
            Row(partial(1), false),
            Snapshot,
            Row(partial(2), false),
            Snapshot,
            Row(partial(3), false),
            Row(closed(3), true),
        ],
        true,
        Some([]),
    );
    assert_eq!(
        sounds.iter().map(|s| (s.uid, s.edge)).collect::<Vec<_>>(),
        vec![(3, TradeEdge::Open), (3, TradeEdge::Close)]
    );
    assert!(
        state
            .observe_sequence(
                &[Row(partial(1), false), Row(partial(2), false)],
                true,
                None::<[Facts; 0]>
            )
            .is_empty()
    );
}

/// A ready initial image establishes its captured pending baseline before its live suffix.
#[test]
fn initial_marker_arms_suffix_without_reading_future_snapshot() {
    use super::Observation::{Row, Snapshot};
    let mut state = TradeSoundState::default();
    let observations = [
        Row(pending(7), false),
        Snapshot,
        Row(partial(7), false),
        Row(closed(7), true),
    ];
    let sounds = state.observe_sequence(&observations, true, Some([closed(7)]));
    assert_eq!(
        sounds.iter().map(|s| s.edge).collect::<Vec<_>>(),
        vec![TradeEdge::Open, TradeEdge::Close]
    );
    state.reset();
    assert!(
        state
            .observe_sequence(&observations, false, Some([closed(7)]))
            .is_empty()
    );
}

/// Catalog removals forget identity before suffix rows; disappearance alone never closes.
#[test]
fn catalog_removal_cannot_join_suffix_to_a_removed_holding() {
    use super::Observation::{Row, Snapshot};
    let mut state = TradeSoundState::default();
    state.observe_batch([], true, true, Some([partial(9)]));
    assert!(
        state
            .observe_sequence(
                &[Row(partial(9), true), Snapshot, Row(closed(9), true)],
                true,
                Some([])
            )
            .is_empty()
    );
}
