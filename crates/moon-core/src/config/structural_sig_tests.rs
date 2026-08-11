//! Structural-signature tests for presentation-only and server-order changes.

use super::{AppConfig, CoreGroup, CoreSortMode, GroupConfig, ServerConfig};

/// Build an `AppConfig` with the selected order and servers.
fn config(mode: CoreSortMode, servers: Vec<ServerConfig>) -> AppConfig {
    AppConfig {
        servers,
        core_sort: mode,
        ..AppConfig::blank(None)
    }
}

/// Build a minimal server fixture.
fn server(id: u64, name: &str) -> ServerConfig {
    ServerConfig {
        id,
        uid: id,
        name: name.to_string(),
        ..toml::from_str("id = 0").expect("ServerConfig must deserialize from defaults")
    }
}

/// Protect `AppConfig::structural_sig`: changing presentation order must not reconnect cores
/// or rebuild group windows.
#[test]
fn changing_only_the_sort_mode_is_not_structural() {
    let servers = vec![server(1, "Alpha"), server(2, "Bravo")];
    let by_added = config(CoreSortMode::AddedOldest, servers.clone());
    let by_name = config(CoreSortMode::Name, servers);
    assert_eq!(
        by_added.structural_sig(),
        by_name.structural_sig(),
        "a sort-mode change must not trigger a session reconcile"
    );
}

/// Protect `AppConfig::structural_sig`: reordering the server Vec remains structural because it
/// builds `SessionManager::config_order`, which positions a reactivated session.
#[test]
fn a_server_list_reorder_stays_structural() {
    let forward = config(
        CoreSortMode::AddedOldest,
        vec![server(1, "Alpha"), server(2, "Bravo")],
    );
    let reversed = config(
        CoreSortMode::AddedOldest,
        vec![server(2, "Bravo"), server(1, "Alpha")],
    );
    assert_ne!(
        forward.structural_sig(),
        reversed.structural_sig(),
        "reordering servers must reach the session layer"
    );
}

#[test]
/// Regression target: passing live groups instead of neutralized groups in
/// `AppConfig::structural_sig` reconnects every core when the user scrolls an order-size preset.
fn group_manual_controls_are_not_structural() {
    let mut before = config(CoreSortMode::Name, vec![server(1, "Alpha")]);
    before.servers[0].group = "desk".to_string();
    before.groups.push(GroupConfig::new("desk"));
    let mut after = before.clone();
    after.groups[0].trade.order_sizes_usd[0] = 75.0;
    after.groups[0].trade.order_size_sel = 5;

    assert_eq!(before.structural_sig(), after.structural_sig());
}

/// Named breakage (`AppConfig::structural_sig`): passing `&self.core_groups` instead of the
/// neutralizing `&[]` into the `reconcile::split` call. A saved core group is a selection
/// bookmark, not a structural detail -- renaming, adding or deleting one must never reconnect a
/// core or rebuild a window.
#[test]
fn core_group_changes_are_not_structural() {
    let mut before = config(CoreSortMode::Name, vec![server(1, "Alpha")]);
    before.core_groups = vec![CoreGroup {
        name: "Scalpers".to_string(),
        cores: vec![1],
    }];
    let mut renamed = before.clone();
    renamed.core_groups[0].name = "Renamed".to_string();
    let mut deleted = before.clone();
    deleted.core_groups.clear();
    let mut added = before.clone();
    added.core_groups.push(CoreGroup {
        name: "Swing".to_string(),
        cores: vec![1],
    });

    assert_eq!(
        before.structural_sig(),
        renamed.structural_sig(),
        "renaming a core group must not reconnect anything"
    );
    assert_eq!(
        before.structural_sig(),
        deleted.structural_sig(),
        "deleting a core group must not reconnect anything"
    );
    assert_eq!(
        before.structural_sig(),
        added.structural_sig(),
        "adding a core group must not reconnect anything"
    );
}
