use super::*;
use crate::feed::{CoreCmdTx, CoreStartupState, LatestMarketRole};
use moonproto::state::BalanceEvent;
use moonproto::{ImportedIpVersion, ImportedNetworkConfig};

/// `feed/mod.rs:CoreCmdTx::send` publishing market roles only through the bounded FIFO would let a
/// stale provider marker resubscribe a replacement client before a queued account-only assignment.
#[test]
fn latest_account_only_role_bypasses_a_long_command_backlog() {
    let (data_tx, data_rx) = std::sync::mpsc::channel();
    let (wake_tx, _wake_rx) = std::sync::mpsc::channel();
    let latest_market_role = LatestMarketRole::default();
    let cmd_tx = CoreCmdTx::new(data_tx, wake_tx, latest_market_role.clone());

    cmd_tx
        .send(CoreCmd::SetMarket {
            provider: true,
            markets: vec!["BTCUSDT".to_owned()],
            orderbook_markets: vec!["BTCUSDT".to_owned()],
        })
        .unwrap();
    for _ in 0..300 {
        cmd_tx.send(CoreCmd::RefreshTransferAssets).unwrap();
    }
    cmd_tx
        .send(CoreCmd::SetMarket {
            provider: false,
            markets: Vec::new(),
            orderbook_markets: Vec::new(),
        })
        .unwrap();

    assert!(matches!(
        data_rx.try_recv(),
        Ok(CoreCmd::SetMarket { provider: true, .. })
    ));
    let mut market_role = MarketRoleState::default();
    assert!(market_role.update(true, vec!["BTCUSDT".to_owned()], vec!["BTCUSDT".to_owned()]));
    let mut force_market_sample = false;
    let latest = commands::lock_and_adopt_latest_market_role(
        &latest_market_role,
        &mut market_role,
        &mut force_market_sample,
    );

    assert!(!market_role.is_provider());
    assert!(market_role.wanted().is_empty());
    assert!(force_market_sample);
    drop(latest);
}

/// `feed/mod.rs:CoreCmdTx::send` updating the role snapshot before a failed channel send would
/// apply an account-only assignment that the coordinator was told had not been accepted.
#[test]
fn failed_market_role_send_does_not_change_the_authoritative_snapshot() {
    let (data_tx, data_rx) = std::sync::mpsc::channel();
    let (wake_tx, _wake_rx) = std::sync::mpsc::channel();
    let latest_market_role = LatestMarketRole::default();
    let cmd_tx = CoreCmdTx::new(data_tx, wake_tx, latest_market_role.clone());
    cmd_tx
        .send(CoreCmd::SetMarket {
            provider: true,
            markets: vec!["ETHUSDT".to_owned()],
            orderbook_markets: Vec::new(),
        })
        .unwrap();
    drop(data_rx);

    assert!(
        cmd_tx
            .send(CoreCmd::SetMarket {
                provider: false,
                markets: Vec::new(),
                orderbook_markets: Vec::new(),
            })
            .is_err()
    );
    let mut market_role = MarketRoleState::default();
    let mut force_market_sample = false;
    let latest = commands::lock_and_adopt_latest_market_role(
        &latest_market_role,
        &mut market_role,
        &mut force_market_sample,
    );

    assert!(market_role.is_provider());
    assert_eq!(market_role.wanted(), ["ETHUSDT"]);
    drop(latest);
}

/// `live/mod.rs:run` treating an exhausted drain budget as an empty queue would consume the only
/// wake signal and block while commands remain queued.
#[test]
fn an_exhausted_command_budget_never_allows_blocking() {
    assert!(!commands::CommandDrain::BudgetExhausted.may_wait());
    assert!(commands::CommandDrain::QueueEmpty.may_wait());
}

/// `live/mod.rs:connection_target` must retain the parsed address and port; replacing either with
/// the legacy fallback connects and groups a remote core under the wrong server.
#[test]
fn parsed_network_selects_the_connection_endpoint() {
    let network = ImportedNetworkConfig {
        ip_version: ImportedIpVersion::V4,
        address: Some(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 42))),
        port: 4321,
        transport_mode: TransportMode::V2,
    };

    let (endpoint, transport) = connection_target(Some(&network), None);

    assert_eq!(
        endpoint,
        CoreEndpoint {
            address: IpAddr::V4(Ipv4Addr::new(198, 51, 100, 42)),
            port: 4321,
        }
    );
    assert_eq!(transport, TransportMode::V2);
}

/// `live/mod.rs:connection_target` must let the configured transport outrank the key's. MoonBot
/// moves its own V0/V1/V2 switch without issuing a new key, and reading only the key is what
/// forced a re-export of every core's key to follow one core's switch.
#[test]
fn the_configured_transport_outranks_the_key() {
    let network = ImportedNetworkConfig {
        ip_version: ImportedIpVersion::V4,
        address: Some(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 42))),
        port: 4321,
        transport_mode: TransportMode::V2,
    };

    let (endpoint, transport) =
        connection_target(Some(&network), Some(crate::config::TransportVersion::V1));

    assert_eq!(transport, TransportMode::V1);
    assert_eq!(
        endpoint.port, 4321,
        "the endpoint still comes from the key; only the mode is overridden"
    );
}

/// A legacy export carries no network block, so the configured mode is the only thing that can
/// answer; without it the V0 fallback would connect a V1/V2 core to nothing.
#[test]
fn a_keyless_network_still_honors_the_configured_transport() {
    let (endpoint, transport) = connection_target(None, Some(crate::config::TransportVersion::V2));

    assert_eq!(transport, TransportMode::V2);
    assert_eq!(endpoint.port, 3000, "legacy exports keep the 3000 fallback");
}

/// `live/mod.rs:should_publish_assets` removing the Balance-event bypass would leave the header's
/// free funds stale until an unrelated event arrives after the five-second background interval.
#[test]
fn balance_events_bypass_the_background_assets_throttle() {
    let balance = Event::Balance(BalanceEvent::IncrementalApplied {
        count: 1,
        global_changed: true,
    });
    let presentation_only = Event::Order(OrderEvent::Snapshot);

    assert!(should_publish_assets(
        &[balance],
        Duration::from_secs(1),
        Duration::from_secs(5)
    ));
    assert!(!should_publish_assets(
        &[presentation_only],
        Duration::from_secs(1),
        Duration::from_secs(5)
    ));
}

/// Build a completion describing a catch-up over `1..=max_rec_id` of `epoch`.
fn alive_completion(epoch: i32, max_rec_id: i64) -> ReportSyncComplete {
    ReportSyncComplete {
        ticket: moonproto::ReportSyncTicket { sync_id: 1 },
        page_count: 1,
        total_rows: 1,
        epoch,
        max_rec_id,
        next_from_rec_id: max_rec_id + 1,
    }
}

/// Only a map answering THIS feed's own pending request may advance the checkpoint.
///
/// Breaks on: `feed/live/mod.rs:alive_map_action` dropping its ticket comparison so any arriving
/// map applies to the newest pending completion — the "it is obviously the answer we asked for"
/// shortcut. A second `SyncComplete` replaces the pending pair while the previous map is still in
/// flight, so the late map covers a SHORTER range than the checkpoint about to be stored: every
/// row above its coverage keeps whatever visibility it had, and the checkpoint then records those
/// rows as reconciled forever.
#[test]
fn a_stale_alive_map_ticket_never_advances_the_checkpoint() {
    let done = alive_completion(91, 400);
    let pending = (
        moonproto::ReportAliveMapTicket { sync_id: 77 },
        done.clone(),
    );

    let stale = alive_map_action(
        Some(&pending),
        moonproto::ReportAliveMapTicket { sync_id: 12 },
        done.epoch,
        done.max_rec_id,
        ReportAliveMapOutcome::Snapshot,
    );
    assert!(matches!(stale, AliveAction::Ignore(_)), "{stale:?}");

    let matching = alive_map_action(
        Some(&pending),
        pending.0,
        done.epoch,
        done.max_rec_id,
        ReportAliveMapOutcome::Snapshot,
    );
    assert_eq!(matching, AliveAction::Apply(done.checkpoint()));
}

/// A map that does not describe the pending catch-up must be refused, and an unrequested one too.
///
/// Breaks on: `feed/live/mod.rs:alive_map_action` losing its `epoch`/`covered_up_to` agreement
/// check, or accepting a map with no pending pair at all. A map is authoritative over
/// `1..=covered_up_to`, so one built from another database — or from a catch-up this feed never
/// ran — would mass-hide rows that are alive, and the checkpoint stored beside it would make that
/// state look reconciled.
#[test]
fn a_map_describing_another_catch_up_is_refused() {
    let done = alive_completion(91, 400);
    let pending = (
        moonproto::ReportAliveMapTicket { sync_id: 77 },
        done.clone(),
    );

    for (epoch, covered) in [(92, done.max_rec_id), (done.epoch, done.max_rec_id - 1)] {
        let action = alive_map_action(
            Some(&pending),
            pending.0,
            epoch,
            covered,
            ReportAliveMapOutcome::Snapshot,
        );
        assert!(
            matches!(action, AliveAction::Ignore(_)),
            "epoch={epoch} covered={covered} -> {action:?}"
        );
    }

    let unrequested = alive_map_action(
        None,
        pending.0,
        done.epoch,
        done.max_rec_id,
        ReportAliveMapOutcome::Snapshot,
    );
    assert!(matches!(unrequested, AliveAction::Ignore(_)));

    // Recreation is decided before the agreement check: the epoch cannot match by definition.
    let recreated = alive_map_action(
        Some(&pending),
        pending.0,
        999,
        0,
        ReportAliveMapOutcome::DatabaseRecreated,
    );
    assert_eq!(recreated, AliveAction::Wipe);
}

/// Advancing the database cursor with the UI cursor would suppress the first complete set when
/// schema defaults arrive later, leaving a fresh scheduled backup empty.
#[test]
fn strategy_database_delivery_waits_for_schema_without_consuming_its_cursor() {
    assert!(!strategy_db_export_due(false, 7, 42, None));
    assert!(strategy_db_export_due(true, 7, 42, None));
    assert!(!strategy_db_export_due(true, 7, 42, Some((7, 42))));
    assert!(strategy_db_export_due(true, 8, 42, Some((7, 42))));
}

/// Advancing on queue acceptance instead of this durable acknowledgement would strand a fresh
/// backup after one transient SQLite write failure.
#[test]
fn a_failed_strategy_commit_keeps_the_same_generation_due() {
    let mut delivered = None;
    let mut retry_due = false;
    let mut initial = true;

    apply_strategy_delivery_ack((7, 42), false, &mut delivered, &mut retry_due, &mut initial);

    assert_eq!(delivered, None);
    assert!(retry_due);
    assert!(initial);
    assert!(strategy_db_export_due(true, 7, 42, delivered));
}

/// `startup_poll_settled` must keep polling while `is_ready` is already true but MoonProto's own
/// status write has not yet caught up — the `Connected{fresh:false}` / status-write reordering
/// described on its docstring. Dropping the snapshot half (`is_ready` alone) reddens this: a stale
/// non-terminal snapshot would incorrectly report settled.
#[test]
fn startup_poll_never_stops_on_is_ready_alone_while_the_snapshot_is_still_mid_transition() {
    let stale = CoreStartupStatus {
        state: CoreStartupState::Reconnecting,
        ..Default::default()
    };
    assert!(!startup_poll_settled(true, Some(stale)));
}

/// `startup_poll_settled` must resume polling once a reconnect drops `is_ready`, even though the
/// last sent snapshot is still the stale terminal `Ready` from the previous episode. Dropping the
/// `is_ready` half (snapshot alone) reddens this: the stale `Ready` snapshot would latch settled
/// forever and this reconnect episode would never be polled again.
#[test]
fn startup_poll_resumes_on_reconnect_even_with_a_stale_terminal_snapshot() {
    let stale_ready = CoreStartupStatus {
        state: CoreStartupState::Ready,
        ..Default::default()
    };
    assert!(!startup_poll_settled(false, Some(stale_ready)));
}

/// The positive case: once both signals agree the core is up, the poll may stop.
#[test]
fn startup_poll_settles_once_both_signals_agree() {
    let ready = CoreStartupStatus {
        state: CoreStartupState::Ready,
        ..Default::default()
    };
    assert!(startup_poll_settled(true, Some(ready)));
}

/// No snapshot has ever been sent yet: the poll must not settle regardless of `is_ready`.
#[test]
fn startup_poll_never_settles_before_any_snapshot_was_sent() {
    assert!(!startup_poll_settled(true, None));
}

/// A re-handshake during the FIRST startup must not be read as a working connection.
///
/// Breakage: MoonProto sets the flag behind `fresh` on the first AuthDone, before the init spine
/// runs, so a link blip mid-startup also arrives as `fresh: false`. Taking that alone is what left
/// cores with no market list reading "connected" for a whole session — the 2026-08-27 incident,
/// where seven of twenty-two cores did exactly this.
#[test]
fn a_reconnect_before_init_finished_is_not_operational() {
    assert!(!reconnect_is_operational(false, false));
}

/// Once initialization has finished, a re-handshake IS the connection resuming.
///
/// Breakage: MoonProto repeats neither init nor `Ready` after a reconnect, so waiting for an event
/// that will never arrive would freeze the status at "reconnected" forever while data flows.
#[test]
fn a_reconnect_after_init_is_operational() {
    assert!(reconnect_is_operational(false, true));
}

/// The first connection of a client always initializes, whatever the latch says.
///
/// Breakage: reducing the rule to the latch alone — plausible once the latch exists — would call
/// the very first `Connected` of a rebuilt client operational before init had run at all.
#[test]
fn a_fresh_connection_is_never_operational_yet() {
    assert!(!reconnect_is_operational(true, true));
}

/// The order table and the chart's order lines carry the SAME rows, and `session/store.rs` feeds the
/// line store from either message — so a turn that published both would put the identical set on the
/// channel twice. The table is the one that also runs the throttle, so it has to be the one that
/// wins; swapping the two arms is the edit this catches.
#[test]
fn a_due_table_wins_over_the_order_lines() {
    assert_eq!(
        OrdersPublish::decide(true, true),
        Some(OrdersPublish::Table)
    );
}

/// The reconnect loop's backoff rule reads a marker attached as an anyhow CONTEXT.
///
/// Breakage: `feed/mod.rs` decides whether an ended run counted as a stable connection by
/// downcasting to `NeverOperational`. If attaching it as context ever stopped being reachable
/// through `downcast_ref` — a different attachment API, an extra wrapper — the downcast would go
/// quietly false and a core that can never finish initialization would reset the backoff on every
/// attempt and rebuild on a fixed cadence for the life of the session, which is the failure this
/// marker exists to prevent.
#[test]
fn a_run_that_never_worked_is_recognisable_through_its_cause() {
    let never = run_failed(false, anyhow::anyhow!("startup stalled"));
    assert!(never.downcast_ref::<NeverOperational>().is_some());
    assert!(
        format!("{never:#}").contains("startup stalled"),
        "the cause must survive the marker: {never:#}"
    );

    let worked = run_failed(true, anyhow::anyhow!("link dropped"));
    assert!(
        worked.downcast_ref::<NeverOperational>().is_none(),
        "a run that reached Ready is not marked"
    );
}
