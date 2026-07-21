//! Durable uid allocation tests.

use super::ensure_uids;
use crate::config::ServerConfig;

/// Build a server fixture with matching runtime and durable ids.
fn server(uid: u64) -> ServerConfig {
    ServerConfig {
        id: uid,
        uid,
        ..deserialize_default()
    }
}

/// Build a `ServerConfig` with every serde default applied.
fn deserialize_default() -> ServerConfig {
    toml::from_str("id = 0").expect("ServerConfig must deserialize from defaults")
}

/// Protects `ensure_uids`: deriving from surviving servers would reuse a deleted uid and
/// attach its `reports.sqlite` history to a new server.
#[test]
fn a_deleted_servers_uid_is_never_handed_out_again() {
    let mut counter = 0u64;
    let mut servers = vec![server(0), server(0), server(0)];
    ensure_uids(&mut servers, &mut counter);
    let issued: Vec<u64> = servers.iter().map(|s| s.uid).collect();
    assert_eq!(issued, [1, 2, 3], "fresh config issues from 1");
    assert_eq!(counter, 4);

    // Delete the highest-uid server, then add a new one.
    servers.pop();
    servers.push(server(0));
    ensure_uids(&mut servers, &mut counter);
    let fresh = servers.last().expect("just pushed").uid;
    assert!(
        fresh > 3,
        "uid {fresh} reuses a deleted server's identity; reports.sqlite keys on it"
    );
}

/// Protects `ensure_uids`: a zero counter must fall back to `max_existing + 1`.
#[test]
fn a_config_without_a_counter_keeps_the_old_issuing_behaviour() {
    let mut counter = 0u64;
    let mut servers = vec![server(7), server(0)];
    ensure_uids(&mut servers, &mut counter);
    assert_eq!(servers[1].uid, 8);
    assert_eq!(counter, 9);
}
