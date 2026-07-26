//! Regression tests for Core Status presentation and transient endpoint state.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};

use gpui::SharedString;

use super::{CoreStatusMode, ServerKey, migrate_keyed_server_state};

/// `core_status/mod.rs:CoreStatusMode::default` must remain By IP; changing it to Flat makes every
/// newly opened Core Status panel bypass the server overview requested by the operator.
#[test]
fn new_core_status_panels_default_to_by_ip() {
    assert_eq!(CoreStatusMode::default(), CoreStatusMode::ByIp);
}

/// `core_status/mod.rs:migrate_keyed_server_state` must transfer hidden and expanded membership
/// from `Unknown(core)` to its decoded address; omitting the insertion visibly resets both eye and
/// expansion state when the endpoint arrives.
#[test]
fn endpoint_discovery_preserves_hidden_and_expanded_state() {
    let old = ServerKey::Unknown(73);
    let new = ServerKey::Address(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 73)));
    let changes = [(old, new)];
    let current = HashSet::from([new]);

    let mut hidden = HashSet::from([old]);
    migrate_keyed_server_state(&mut hidden, &changes, &current, |key| key);
    assert_eq!(hidden, HashSet::from([new]));

    let mut expanded = HashSet::from([SharedString::from(old.tree_id())]);
    migrate_keyed_server_state(&mut expanded, &changes, &current, |key| {
        SharedString::from(key.tree_id())
    });
    assert_eq!(expanded, HashSet::from([SharedString::from(new.tree_id())]));
}
