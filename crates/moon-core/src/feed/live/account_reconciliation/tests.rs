use super::*;

/// Builds an account stamp for a Spot platform used by wallet-repair scenarios.
fn spot_stamp(
    status: OrderWorkerStatus,
    buy_actual_q: f64,
    sell_actual_q: f64,
) -> OrderAccountStamp {
    OrderAccountStamp::new(status, buy_actual_q, sell_actual_q, ExchangeCode::BitGet)
}

/// `account_reconciliation.rs:observe_events` treating catalog/trace/corridor events as account
/// changes would restore the 3-second full-balance polling storm on otherwise idle cores.
#[test]
fn presentation_only_order_events_do_not_queue_account_repairs() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new();
    let events = [
        Event::Order(OrderEvent::Snapshot),
        Event::Order(OrderEvent::TracePoint { uid: 7 }),
        Event::Order(OrderEvent::CorridorChanged(7)),
    ];

    reconciliation.observe_events(&events, start + Duration::from_secs(30));

    assert_eq!(reconciliation.next_wait(start), None);
}

/// `account_reconciliation.rs:observe_order` queuing every Updated event instead of comparing the
/// account stamp would turn price/stop updates into redundant full balance and wallet requests.
#[test]
fn unchanged_order_stamp_does_not_requeue_repairs() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new();
    let stamp = spot_stamp(OrderWorkerStatus::BuySet, 0.0, 0.0);
    reconciliation.observe_order(OrderChange::Created, 7, stamp, start);
    reconciliation.mark_balance_attempt(start);
    reconciliation.mark_spot_wallet_attempt(start);

    reconciliation.observe_order(OrderChange::Updated, 7, stamp, start);

    assert_eq!(reconciliation.next_wait(start), None);
}

/// `account_reconciliation.rs:RepairDeadline::queue` moving an already-pending deadline on every
/// order change would postpone repair forever on a busy core.
#[test]
fn repeated_account_changes_preserve_the_original_deadline() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new();
    reconciliation.mark_balance_attempt(start);
    reconciliation.mark_spot_wallet_attempt(start);
    reconciliation.observe_order(
        OrderChange::Created,
        7,
        spot_stamp(OrderWorkerStatus::BuySet, 0.0, 0.0),
        start + Duration::from_secs(1),
    );
    reconciliation.observe_order(
        OrderChange::Updated,
        7,
        spot_stamp(OrderWorkerStatus::BuyDone, 1.0, 0.0),
        start + Duration::from_secs(2),
    );

    assert_eq!(
        reconciliation.next_wait(start + Duration::from_secs(2)),
        Some(Duration::from_secs(1))
    );
    assert!(reconciliation.balance_due(start + Duration::from_secs(3)));
    assert!(!reconciliation.spot_wallet_due(start + Duration::from_secs(3)));
    assert!(reconciliation.spot_wallet_due(start + Duration::from_secs(10)));
}

/// `account_reconciliation.rs:observe_events` failing to satisfy pending repair from a pushed full
/// balance snapshot would request the same authoritative snapshot again.
#[test]
fn pushed_full_balance_satisfies_pending_repair_only() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new();
    reconciliation.observe_order(
        OrderChange::Created,
        7,
        spot_stamp(OrderWorkerStatus::BuySet, 0.0, 0.0),
        start,
    );
    reconciliation.observe_events(
        &[Event::Balance(
            moonproto::state::BalanceEvent::SnapshotApplied { count: 1 },
        )],
        start + Duration::from_secs(1),
    );

    assert!(!reconciliation.balance_due(start + Duration::from_secs(30)));
    assert!(reconciliation.spot_wallet_due(start + Duration::from_secs(10)));
}

/// `account_reconciliation.rs:RepairDeadline::queue` adding the full interval to every idle change
/// would keep the header and Spot assets stale for 3/10 seconds after placing an order.
#[test]
fn first_change_after_idle_queues_balance_and_spot_repairs_immediately() {
    let start = Instant::now();
    let idle_change = start + Duration::from_secs(30);
    let mut reconciliation = AccountReconciliation::new();

    reconciliation.observe_order(
        OrderChange::Created,
        7,
        spot_stamp(OrderWorkerStatus::BuySet, 0.0, 0.0),
        idle_change,
    );

    assert!(reconciliation.balance_due(idle_change));
    assert!(reconciliation.spot_wallet_due(idle_change));
    assert_eq!(reconciliation.next_wait(idle_change), Some(Duration::ZERO));
}

/// `account_reconciliation.rs:RepairDeadline::mark_attempt` failing to retain the last request time
/// would restore one full balance and Spot-wallet request per genuine order transition.
#[test]
fn requests_inside_the_cooldown_are_coalesced_to_its_end() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new();
    reconciliation.mark_balance_attempt(start);
    reconciliation.mark_spot_wallet_attempt(start);

    reconciliation.observe_order(
        OrderChange::Created,
        7,
        spot_stamp(OrderWorkerStatus::BuySet, 0.0, 0.0),
        start + Duration::from_secs(1),
    );

    assert!(!reconciliation.balance_due(start + Duration::from_secs(1)));
    assert_eq!(
        reconciliation.next_wait(start + Duration::from_secs(1)),
        Some(Duration::from_secs(2))
    );
    assert!(reconciliation.balance_due(start + Duration::from_secs(3)));
    assert!(!reconciliation.spot_wallet_due(start + Duration::from_secs(9)));
    assert!(reconciliation.spot_wallet_due(start + Duration::from_secs(10)));
}

/// `account_reconciliation.rs:observe_events` accepting an incremental balance as authoritative
/// would preserve a sold coin whose missing zero row requires a full snapshot to clear.
#[test]
fn incremental_balance_does_not_cancel_full_repair() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new();
    reconciliation.mark_balance_attempt(start);
    reconciliation.observe_order(
        OrderChange::Created,
        7,
        spot_stamp(OrderWorkerStatus::BuySet, 0.0, 0.0),
        start + Duration::from_secs(1),
    );

    reconciliation.observe_events(
        &[Event::Balance(
            moonproto::state::BalanceEvent::IncrementalApplied {
                count: 1,
                global_changed: true,
            },
        )],
        start + Duration::from_secs(1),
    );

    assert!(reconciliation.balance_due(start + Duration::from_secs(3)));
}

/// `account_reconciliation.rs:observe_events` accepting failed or non-Spot wallet events would
/// cancel the only repair capable of revealing a newly purchased Spot asset.
#[test]
fn only_a_successful_spot_wallet_update_cancels_spot_repair() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new();
    reconciliation.observe_order(
        OrderChange::Created,
        7,
        spot_stamp(OrderWorkerStatus::BuySet, 0.0, 0.0),
        start,
    );
    reconciliation.observe_events(
        &[
            Event::TransferAssets(moonproto::state::TransferAssetsEvent::UpdateFailed {
                kind: moonproto::ExchangeKind::Spot,
                error: "timeout".to_string(),
            }),
            Event::TransferAssets(moonproto::state::TransferAssetsEvent::Updated {
                kind: moonproto::ExchangeKind::Futures,
                count: 1,
                nonzero_count: 1,
                revision: 1,
            }),
        ],
        start + Duration::from_secs(1),
    );
    assert!(reconciliation.spot_wallet_due(start + Duration::from_secs(10)));

    reconciliation.observe_events(
        &[Event::TransferAssets(
            moonproto::state::TransferAssetsEvent::Updated {
                kind: moonproto::ExchangeKind::Spot,
                count: 1,
                nonzero_count: 1,
                revision: 2,
            },
        )],
        start + Duration::from_secs(11),
    );

    assert!(!reconciliation.spot_wallet_due(start + Duration::from_secs(30)));
}

/// `account_reconciliation.rs:observe_order` queuing Spot-wallet work for futures platforms would
/// preserve avoidable exchange requests on every ordinary futures fill.
#[test]
fn futures_order_changes_do_not_queue_spot_wallet_repairs() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new();

    reconciliation.observe_order(
        OrderChange::Created,
        7,
        OrderAccountStamp::new(OrderWorkerStatus::BuySet, 0.0, 0.0, ExchangeCode::FBitGet),
        start,
    );

    assert!(reconciliation.balance_due(start));
    assert!(!reconciliation.spot_wallet_due(start + Duration::from_secs(30)));
}

/// `account_reconciliation.rs:observe_order` treating terminal cleanup as a fresh account change
/// would queue a second full balance and wallet repair after the authoritative pushes arrived.
#[test]
fn terminal_cleanup_removal_does_not_requeue_satisfied_repairs() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new();
    reconciliation.observe_order(
        OrderChange::Created,
        7,
        spot_stamp(OrderWorkerStatus::BuySet, 0.0, 0.0),
        start,
    );
    reconciliation.mark_balance_attempt(start);
    reconciliation.mark_spot_wallet_attempt(start);
    let terminal = spot_stamp(OrderWorkerStatus::SellDone, 1.0, 1.0);
    reconciliation.observe_order(
        OrderChange::Updated,
        7,
        terminal,
        start + Duration::from_secs(1),
    );
    reconciliation.mark_balance_attempt(start + Duration::from_secs(1));
    reconciliation.mark_spot_wallet_attempt(start + Duration::from_secs(1));

    reconciliation.observe_order(
        OrderChange::Removed,
        7,
        terminal,
        start + Duration::from_secs(2),
    );

    assert_eq!(
        reconciliation.next_wait(start + Duration::from_secs(30)),
        None
    );
}
