//! Regressions for flattening grouped roster blocks into rail items.

use std::net::{IpAddr, Ipv4Addr};

use gpui::SharedString;
use moon_core::feed::{ConnStatus, CoreStartupStatus, ExchangeId};
use moon_core::venue::CoreVenue;

use super::{RailItem, append_roster_items, core_machine};
use crate::workspace::roster::WorkspaceRosterSection;
use crate::workspace::{
    RosterSectionHeading, RosterServer, WorkspaceCoreStatus, WorkspaceRosterRow,
    WorkspaceRosterServer,
};

/// One ready row for `core`.
fn row(core: u64) -> WorkspaceRosterRow {
    WorkspaceRosterRow {
        core,
        name: format!("Core {core}"),
        group: "G1".to_string(),
        status: WorkspaceCoreStatus::Ready,
        selectable: true,
        selected: false,
        connection: Some(ConnStatus::Ready),
        startup: CoreStartupStatus::default(),
        fault: None,
        mode_suggestion: None,
    }
}

/// An exchange section for venue `code` holding `cores`.
fn exchange(code: u8, cores: &[u64]) -> WorkspaceRosterSection {
    WorkspaceRosterSection {
        heading: RosterSectionHeading::Exchange(Some(CoreVenue {
            id: ExchangeId::new(code),
            dex: String::new(),
            reported: String::new(),
        })),
        rows: cores.iter().copied().map(row).collect(),
    }
}

/// One item reduced to what a test compares: kind, element id, run-cell section, nesting, cores.
#[derive(Debug, PartialEq)]
enum Flat {
    Server(String, usize, Vec<u64>),
    Exchange(String, usize, bool, Vec<u64>),
    Core(u64, bool, bool),
}

/// Flatten `servers` with fixed name and id resolvers, dropping the Overview row.
fn flatten(servers: Vec<WorkspaceRosterServer>) -> Vec<Flat> {
    let mut items = Vec::new();
    append_roster_items(
        &mut items,
        servers,
        |_| (SharedString::from("name"), SharedString::from("n")),
        |server| match server {
            RosterServer::Address(_) => "7".to_string(),
            RosterServer::Unknown => "unknown".to_string(),
        },
        |_| None,
    );
    items
        .into_iter()
        .map(|item| match item {
            RailItem::Server {
                id, section, cores, ..
            } => Flat::Server(id.to_string(), section, cores.to_vec()),
            RailItem::Exchange {
                id,
                section,
                nested,
                cores,
                ..
            } => Flat::Exchange(id.to_string(), section, nested, cores.to_vec()),
            RailItem::Core {
                row,
                is_last_in_section,
                nested,
                ..
            } => Flat::Core(row.core, is_last_in_section, nested),
            RailItem::Overview { .. } => panic!("the flattener must not add an Overview row"),
        })
        .collect()
}

/// Catches the exchange heading's element id changing for users who never turn server grouping on:
/// GPUI would treat it as a new element (hover and tooltip state lost) and the UI atlas would lose
/// the control it already documents.
#[test]
fn exchange_only_rail_keeps_its_historical_ids_and_shape() {
    let actual = flatten(vec![WorkspaceRosterServer {
        server: None,
        sections: vec![exchange(4, &[1, 2])],
    }]);
    assert_eq!(
        actual,
        vec![
            Flat::Exchange("workspace-exchange-4-0".to_string(), 0, false, vec![1, 2]),
            Flat::Core(1, false, false),
            Flat::Core(2, true, false),
        ]
    );
}

/// Catches a server heading commanding only its first exchange's cores, two headings sharing a run
/// cell position (their buttons would trade press state), the same venue under two machines
/// sharing one element id, or an address leaking into an id.
#[test]
fn server_blocks_number_every_heading_and_nest_exchanges() {
    let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5));
    let actual = flatten(vec![
        WorkspaceRosterServer {
            server: Some(RosterServer::Address(ip)),
            sections: vec![exchange(2, &[1]), exchange(4, &[2, 3])],
        },
        WorkspaceRosterServer {
            server: Some(RosterServer::Unknown),
            sections: vec![WorkspaceRosterSection {
                heading: RosterSectionHeading::None,
                rows: vec![row(4)],
            }],
        },
    ]);
    assert_eq!(
        actual,
        vec![
            Flat::Server("workspace-server-7".to_string(), 0, vec![1, 2, 3]),
            Flat::Exchange("workspace-exchange-s7-2-0".to_string(), 1, true, vec![1]),
            Flat::Core(1, true, true),
            Flat::Exchange("workspace-exchange-s7-4-0".to_string(), 2, true, vec![2, 3]),
            Flat::Core(2, false, true),
            Flat::Core(3, true, true),
            Flat::Server("workspace-server-unknown".to_string(), 3, vec![4]),
            Flat::Core(4, true, false),
        ]
    );
}

/// Synthetic TESTKEY export naming the documentation address 198.51.100.42:4321 — the fixture of
/// `moon-core/src/config/key_endpoint/tests.rs`, never a live credential.
const TEST_KEY: &str = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA==";

/// One configured core with `key` and the address override `endpoint_override`.
fn configured(key: &str, endpoint_override: &str) -> moon_core::config::ServerConfig {
    moon_core::config::ServerConfig {
        id: 1,
        uid: 1,
        name: "core".into(),
        active: true,
        feed: moon_core::config::FeedFlags::default(),
        key: moon_core::config::Secret::new(key),
        endpoint_override: endpoint_override.into(),
        endpoint_to_station: false,
        group: "G1".into(),
        market: "BTCUSDT".into(),
        color: [0, 0, 0],
        synthetic: false,
        chart_bundle: String::new(),
        default_alert_strategy: 0,
        own_trade_config: false,
        strat_slots: None,
        manual_strategy: None,
        trade: None,
        transport: None,
        workspace_membership: Default::default(),
        total_mode: Default::default(),
    }
}

/// Catches a reconnecting core leaving its server block: `begin_connection_attempt` clears the
/// reported endpoint on every attempt, and only the configured address keeps the core in place.
/// Also catches the fallback outranking the feed, or a host name / missing key being guessed.
#[test]
fn core_machine_prefers_the_feed_and_falls_back_to_the_configured_ip() {
    let reported = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
    let mut live = moon_core::session::store::CoreData::default();
    live.endpoint = Some(moon_core::feed::CoreEndpoint {
        address: reported,
        port: 5017,
    });
    let keyed = configured(TEST_KEY, "");
    assert_eq!(core_machine(&keyed, Some(&live)), Some(reported));

    let reconnecting = moon_core::session::store::CoreData::default();
    let from_key = Some(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 42)));
    assert_eq!(core_machine(&keyed, Some(&reconnecting)), from_key);
    assert_eq!(core_machine(&keyed, None), from_key);

    let overridden = configured(TEST_KEY, "192.0.2.5:5020");
    assert_eq!(
        core_machine(&overridden, None),
        Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 5)))
    );
    assert_eq!(
        core_machine(&configured(TEST_KEY, "core.example"), None),
        None
    );
    assert_eq!(core_machine(&configured("", ""), None), None);
    // An override never stands in for a key that cannot be read: nothing would dial it.
    assert_eq!(
        core_machine(&configured("not-a-key", "192.0.2.5"), None),
        None
    );
}
