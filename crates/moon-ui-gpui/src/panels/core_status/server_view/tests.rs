//! Regression tests for Core Status tree items.

use std::net::{IpAddr, Ipv4Addr};

use moon_core::feed::{ConnStatus, CoreEndpoint};
use moon_core::session::{CoreStartupStatus, CoreSysStatus};

use super::tree_items;
use crate::panels::core_status::model::{CoreStatusRow, aggregate_servers};

/// Build one ready core snapshot at an address.
fn row(id: u64, address: IpAddr, port: u16) -> CoreStatusRow {
    CoreStatusRow {
        fault: None,
        id,
        name: format!("Core {id}"),
        status: ConnStatus::Ready,
        sys: CoreSysStatus::default(),
        endpoint: Some(CoreEndpoint { address, port }),
        ping_warn: false,
        exch_warn: false,
        ping_sev: crate::backend::core_warn::LatencySeverity::Normal,
        exch_sev: crate::backend::core_warn::LatencySeverity::Normal,
        api_key: crate::panels::core_status::model::ApiKeyState::Unknown,
        api_warn: false,
        startup: CoreStartupStatus::default(),
    }
}

/// `server_view.rs:tree_items` must give each server root a folder of its core children, so the
/// row expands to per-core detail.
#[test]
fn server_root_folds_its_core_children() {
    let address = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 17));
    let groups = aggregate_servers(&[row(51, address, 3000)]);

    let items = tree_items(&groups);

    assert_eq!(items.len(), 1);
    assert!(items[0].is_folder());
    assert_eq!(items[0].children.len(), 1);
    assert_eq!(items[0].children[0].id.as_ref(), "core:51");
}

/// `server_view.rs:tree_items` must follow the aggregate's address order, keeping stable server and
/// core ids so the virtual tree does not reshuffle.
#[test]
fn tree_items_follow_address_order() {
    let low = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10));
    let high = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 20));
    let groups = aggregate_servers(&[row(82, high, 3000), row(81, low, 3000)]);

    let items = tree_items(&groups);

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id.to_string(), format!("server:{low}"));
    assert_eq!(items[0].children[0].id.as_ref(), "core:81");
    assert_eq!(items[1].id.to_string(), format!("server:{high}"));
    assert_eq!(items[1].children[0].id.as_ref(), "core:82");
}
