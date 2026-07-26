//! Regression tests for Core Status tree item visibility.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};

use moon_core::feed::{ConnStatus, CoreEndpoint};
use moon_core::session::CoreSysStatus;

use super::tree_items;
use crate::panels::core_status::model::{CoreStatusRow, ServerKey, aggregate_servers};

/// `server_view.rs:tree_items` must keep a hidden server root while removing its children, then
/// restore those children when the eye is toggled back; deleting either branch strands the server
/// invisible or makes the hide action ineffective.
#[test]
fn hidden_server_root_can_restore_its_process_children() {
    let address = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 17));
    let rows = [CoreStatusRow {
        id: 51,
        name: "Core 51".to_string(),
        status: ConnStatus::Ready,
        sys: CoreSysStatus::default(),
        endpoint: Some(CoreEndpoint {
            address,
            port: 3000,
        }),
        exchange: Some("Binance".to_string()),
    }];
    let groups = aggregate_servers(&rows);
    let hidden = HashSet::from([ServerKey::Address(address)]);

    let hidden_items = tree_items(&groups, &hidden, false);
    assert_eq!(hidden_items.len(), 1);
    assert!(hidden_items[0].children.is_empty());
    assert!(!hidden_items[0].is_folder());

    let restored_items = tree_items(&groups, &HashSet::new(), false);
    assert_eq!(restored_items[0].children.len(), 1);
    assert!(restored_items[0].is_folder());
}

/// `model.rs:aggregate_servers` removing the attention-first group sort must fail here; a failed
/// server would remain below the healthy root, or an attempted separator item would change the
/// stable server/core IDs and virtual tree cardinality.
#[test]
fn reordered_servers_keep_stable_tree_ids_without_extra_rows() {
    let ready_address = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 81));
    let failed_address = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 82));
    let rows = [
        CoreStatusRow {
            id: 81,
            name: "Core 81".to_string(),
            status: ConnStatus::Ready,
            sys: CoreSysStatus::default(),
            endpoint: Some(CoreEndpoint {
                address: ready_address,
                port: 3000,
            }),
            exchange: None,
        },
        CoreStatusRow {
            id: 82,
            name: "Core 82".to_string(),
            status: ConnStatus::Failed("offline".to_string()),
            sys: CoreSysStatus::default(),
            endpoint: Some(CoreEndpoint {
                address: failed_address,
                port: 3000,
            }),
            exchange: None,
        },
    ];

    let groups = aggregate_servers(&rows);
    let items = tree_items(&groups, &HashSet::new(), false);

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id.to_string(), format!("server:{failed_address}"));
    assert_eq!(items[0].children.len(), 1);
    assert_eq!(items[0].children[0].id.as_ref(), "core:82");
    assert_eq!(items[1].id.to_string(), format!("server:{ready_address}"));
    assert_eq!(items[1].children.len(), 1);
    assert_eq!(items[1].children[0].id.as_ref(), "core:81");
}
