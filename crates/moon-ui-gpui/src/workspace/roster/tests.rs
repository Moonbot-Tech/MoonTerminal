//! Regressions for the rail roster's server and exchange grouping.

use std::net::{IpAddr, Ipv4Addr};

use moon_core::feed::{CoreStartupStatus, ExchangeId};
use moon_core::venue::CoreVenue;

use super::{RosterGrouping, RosterSectionHeading, RosterServer, WorkspaceRosterServer};
use crate::workspace::{
    WorkspaceCoreAvailability, WorkspaceRosterInput, WorkspaceWindowState, derive_workspace_roster,
};

/// One ready core on `server` at exchange `exchange`; everything else fixed.
fn input(core: u64, server: Option<u8>, exchange: Option<u8>) -> WorkspaceRosterInput {
    WorkspaceRosterInput {
        core,
        name: format!("Core {core}"),
        group: "G1".to_string(),
        venue: exchange.map(|code| CoreVenue {
            id: ExchangeId::new(code),
            dex: String::new(),
            reported: String::new(),
        }),
        availability: WorkspaceCoreAvailability {
            group_active: true,
            core_active: true,
            live_session: true,
            window: WorkspaceWindowState::Live,
        },
        ready: true,
        connection: None,
        startup: CoreStartupStatus::default(),
        fault: None,
        mode_suggestion: None,
        server: server.map(ip),
    }
}

/// The documentation-range address ending in `last`.
fn ip(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(203, 0, 113, last))
}

/// Each block as (server, [(exchange code or None for "no heading"/unknown, cores)]).
type Shape = Vec<(Option<RosterServer>, Vec<(Option<Option<u8>>, Vec<u64>)>)>;

/// Reduce grouped blocks to plain values a test can compare against a literal.
fn shape(servers: &[WorkspaceRosterServer]) -> Shape {
    servers
        .iter()
        .map(|block| {
            let sections = block
                .sections
                .iter()
                .map(|section| {
                    let heading = match &section.heading {
                        RosterSectionHeading::None => None,
                        RosterSectionHeading::Exchange(venue) => {
                            Some(venue.as_ref().map(|venue| venue.id.code))
                        }
                    };
                    (heading, section.rows.iter().map(|row| row.core).collect())
                })
                .collect();
            (block.server, sections)
        })
        .collect()
}

/// Group `inputs` the way the rail does.
fn grouped(inputs: &[WorkspaceRosterInput], by_server: bool, by_exchange: bool) -> Shape {
    let roster = derive_workspace_roster(
        inputs,
        "G1",
        None,
        inputs.len(),
        RosterGrouping {
            by_server,
            by_exchange,
        },
    );
    shape(&roster.servers)
}

/// A fleet on two machines plus one core that never reported an address, in canonical order that
/// interleaves the machines and the exchanges.
fn fleet() -> Vec<WorkspaceRosterInput> {
    vec![
        input(1, Some(20), Some(2)),
        input(2, None, Some(4)),
        input(3, Some(10), Some(4)),
        input(4, Some(20), Some(4)),
        input(5, Some(10), Some(2)),
        input(6, Some(20), Some(2)),
    ]
}

/// Catches flipping the default: the rail must keep the exchange-only shape for every profile that
/// never opened the popup, which is the whole promise of this feature to existing users.
#[test]
fn default_grouping_is_exchange_only() {
    assert_eq!(
        RosterGrouping::default(),
        RosterGrouping {
            by_server: false,
            by_exchange: true,
        }
    );
}

/// Catches server blocks in arrival order instead of address order, the unknown block leading,
/// exchanges inside a block leaving the directory order (`exchange_sections`: code 4 before 2),
/// exchange sections leaking across machines, or canonical order lost inside a section.
#[test]
fn server_blocks_follow_address_order_with_exchanges_inside() {
    let expected: Shape = vec![
        (
            Some(RosterServer::Address(ip(10))),
            vec![(Some(Some(4)), vec![3]), (Some(Some(2)), vec![5])],
        ),
        (
            Some(RosterServer::Address(ip(20))),
            vec![(Some(Some(4)), vec![4]), (Some(Some(2)), vec![1, 6])],
        ),
        (Some(RosterServer::Unknown), vec![(Some(Some(4)), vec![2])]),
    ];
    assert_eq!(grouped(&fleet(), true, true), expected);
}

/// Catches exchange headings drawn when exchange grouping is off inside server blocks.
#[test]
fn server_blocks_without_exchange_grouping_hold_one_headless_section() {
    let expected: Shape = vec![
        (
            Some(RosterServer::Address(ip(10))),
            vec![(None, vec![3, 5])],
        ),
        (
            Some(RosterServer::Address(ip(20))),
            vec![(None, vec![1, 4, 6])],
        ),
        (Some(RosterServer::Unknown), vec![(None, vec![2])]),
    ];
    assert_eq!(grouped(&fleet(), true, false), expected);
}

/// Catches both switches off losing cores or reordering them: the list is then the canonical order,
/// one block, no heading at all.
#[test]
fn no_grouping_is_one_flat_section_in_canonical_order() {
    let expected: Shape = vec![(None, vec![(None, vec![1, 2, 3, 4, 5, 6])])];
    assert_eq!(grouped(&fleet(), false, false), expected);
}

/// Catches an empty rail growing a phantom block (and so a heading with nothing under it).
#[test]
fn empty_roster_has_no_blocks() {
    for (by_server, by_exchange) in [(false, false), (false, true), (true, false), (true, true)] {
        assert!(grouped(&[], by_server, by_exchange).is_empty());
    }
}
