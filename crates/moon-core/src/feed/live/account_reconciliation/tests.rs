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
    let mut reconciliation = AccountReconciliation::new(start);
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
    let mut reconciliation = AccountReconciliation::new(start);
    let stamp = spot_stamp(OrderWorkerStatus::BuySet, 0.0, 0.0);
    reconciliation.observe_order(OrderChange::Created, 7, stamp, start);
    reconciliation.mark_balance_attempt(start);
    reconciliation.mark_spot_wallet_attempt(start);

    reconciliation.observe_order(OrderChange::Updated, 7, stamp, start);

    assert_eq!(reconciliation.next_wait(start), None);
}

/// `deadline.rs:CoalescedDeadline::queue` moving an already-pending deadline on every
/// order change would postpone repair forever on a busy core.
#[test]
fn repeated_account_changes_preserve_the_original_deadline() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new(start);
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
    let mut reconciliation = AccountReconciliation::new(start);
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

/// `deadline.rs:CoalescedDeadline::queue` adding the full interval to every idle change
/// would keep the header and Spot assets stale for 3/10 seconds after placing an order.
#[test]
fn first_change_after_idle_queues_balance_and_spot_repairs_immediately() {
    let start = Instant::now();
    let idle_change = start + Duration::from_secs(30);
    let mut reconciliation = AccountReconciliation::new(start);

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

/// `deadline.rs:CoalescedDeadline::mark_attempt` failing to retain the last request time
/// would restore one full balance and Spot-wallet request per genuine order transition.
#[test]
fn requests_inside_the_cooldown_are_coalesced_to_its_end() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new(start);
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
    let mut reconciliation = AccountReconciliation::new(start);
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
    let mut reconciliation = AccountReconciliation::new(start);
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
    let mut reconciliation = AccountReconciliation::new(start);

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
    let mut reconciliation = AccountReconciliation::new(start);
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

/// A due poll that its caller declines — the feed loop's Ready gate — must still MOVE. The loop
/// derives its wait from this deadline, so a deadline that stays due computes a zero-length wait
/// every pass and spins the core's thread on `recv_timeout(0)` for as long as the core is down.
#[test]
fn a_declined_poll_stops_being_due() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new(start);
    assert!(
        reconciliation.api_expiry_due(start),
        "the first poll is due at once, so a connected core answers immediately"
    );

    reconciliation.defer_api_expiry(start);

    assert!(!reconciliation.api_expiry_due(start));
    assert!(
        reconciliation.api_expiry_wait(start) > Duration::from_secs(0),
        "the loop now has a real wait to sleep on"
    );
    assert!(
        reconciliation.api_expiry_due(start + API_EXPIRY_POLL_INTERVAL),
        "and it comes back on its own"
    );
    assert!(
        !reconciliation.api_expiry_due(start + API_EXPIRY_RETRY_INTERVAL),
        "a FULL interval, not the retry delay: waking a down core's thread every 15 minutes to          decline again is the spin this test exists to prevent, one step removed"
    );
}

/// Reaching Ready is the moment a core can finally answer, so the poll becomes due at once — but a
/// core that flaps must not issue an exchange-bound check on every reconnect.
#[test]
fn reaching_ready_asks_at_once_but_not_on_every_flap() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new(start);
    reconciliation.defer_api_expiry(start);

    reconciliation.poll_api_expiry_on_ready(start);
    assert!(reconciliation.api_expiry_due(start), "first Ready asks now");

    reconciliation.mark_api_expiry_attempt(start);
    let flap = start + Duration::from_secs(60);
    reconciliation.poll_api_expiry_on_ready(flap);
    assert!(
        !reconciliation.api_expiry_due(flap),
        "a reconnect a minute later rides the cooldown instead of asking again"
    );
}

/// A check that answered with an error is retried sooner than the full interval — a core that was
/// merely busy must not wait six hours for its first day count.
#[test]
fn a_failed_check_is_retried_before_the_full_interval() {
    let start = Instant::now();
    let mut reconciliation = AccountReconciliation::new(start);
    reconciliation.mark_api_expiry_attempt(start);
    assert!(!reconciliation.api_expiry_due(start + API_EXPIRY_RETRY_INTERVAL));

    reconciliation.retry_api_expiry(start);

    assert!(reconciliation.api_expiry_due(start + API_EXPIRY_RETRY_INTERVAL));
    assert!(!reconciliation.api_expiry_due(start));
}

/// The balance-repair trace is emitted from three sites on a path that runs on every trading core,
/// and at `info` it measured 83.9% of a day's log — enough to evict the Log panel's whole
/// 5000-record history in under ten minutes. What keeps it out is the LEVEL, so the level is the
/// thing to hold: the default filter admits `moon_core=info`, and anything at or below Info is
/// admitted with it.
#[test]
fn the_balance_trace_level_is_below_the_default_filter() {
    assert!(
        BALANCE_TRACE_LEVEL > log::Level::Info,
        "the default filter admits Info and above; {BALANCE_TRACE_LEVEL} would reach the Log panel"
    );
}

/// The constant is only worth anything while the emit sites go through it — reverting one to
/// `log::info!` restores the flood with the test above still green, which is precisely the
/// regression this pairs with.
///
/// A source scan because a macro call site is not reachable any other way, but a NARROW one: each
/// message is located by `find` (so a rename fails the test instead of silently matching the whole
/// file), and only the ~200 characters that precede it are inspected, so the verdict cannot be
/// decided by an unrelated macro elsewhere in a 1500-line file.
#[test]
fn every_balance_trace_site_goes_through_the_level_constant() {
    const EMITS: [(&str, &str); 3] = [
        (
            "feed/live/mod.rs",
            "core {} balance repair requested (account order change)",
        ),
        (
            "feed/live/mod.rs",
            "core {} balance event after refresh: {bev:?}",
        ),
        (
            "feed/live/commands.rs",
            "core {} balance refresh requested (assets refresh)",
        ),
    ];
    let sources = [
        ("feed/live/mod.rs", include_str!("../mod.rs")),
        ("feed/live/commands.rs", include_str!("../commands.rs")),
    ];

    for (file, message) in EMITS {
        let source = sources
            .iter()
            .find(|(name, _)| *name == file)
            .map(|(_, text)| *text)
            .expect("the scanned file must be listed");
        let at = source
            .find(message)
            .unwrap_or_else(|| panic!("{file} no longer emits `{message}`"));
        let window = &source[at.saturating_sub(200)..at];
        assert!(
            window.contains("BALANCE_TRACE_LEVEL"),
            "{file}: `{message}` must be emitted through the level constant, not a fixed macro"
        );
        for fixed in ["log::info!", "log::warn!", "log::error!"] {
            assert!(
                !window.contains(fixed),
                "{file}: `{message}` is emitted with {fixed}, which the default filter admits"
            );
        }
    }
}
