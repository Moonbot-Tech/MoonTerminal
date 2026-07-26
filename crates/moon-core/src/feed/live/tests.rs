use super::*;
use crate::feed::{CoreCmdTx, LatestMarketRole};
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

    assert!(cmd_tx
        .send(CoreCmd::SetMarket {
            provider: false,
            markets: Vec::new(),
            orderbook_markets: Vec::new(),
        })
        .is_err());
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

    let (endpoint, transport) = connection_target(Some(&network));

    assert_eq!(
        endpoint,
        CoreEndpoint {
            address: IpAddr::V4(Ipv4Addr::new(198, 51, 100, 42)),
            port: 4321,
        }
    );
    assert_eq!(transport, TransportMode::V2);
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
