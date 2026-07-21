//! Structural-signature tests for presentation-only and server-order changes.

use super::{AppConfig, CoreSortMode, ServerConfig};

/// Build an `AppConfig` with the selected order and servers.
fn config(mode: CoreSortMode, servers: Vec<ServerConfig>) -> AppConfig {
    AppConfig {
        servers,
        core_sort: mode,
        ..Default::default()
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
