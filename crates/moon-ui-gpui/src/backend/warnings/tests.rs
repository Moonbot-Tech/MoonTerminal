//! Endpoint ownership checks for warning-history eviction.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use super::reconcile_endpoint_owners;

/// An unresolved core keeps its own last-known IP but cannot preserve a removed core's server.
#[test]
fn unresolved_endpoints_do_not_block_removed_server_eviction() {
    let removed_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    let live_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));
    let mut owners = HashMap::from([(1, removed_ip), (2, live_ip)]);

    let live = reconcile_endpoint_owners(&mut owners, [2], []);

    assert_eq!(live, [live_ip].into());
    assert_eq!(owners, HashMap::from([(2, live_ip)]));
}
