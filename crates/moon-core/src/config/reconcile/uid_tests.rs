//! Durable uid allocation tests.

use super::{ensure_uids, next_free_uid};
use crate::config::schema::{ServersFile, SettingsFile};
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

/// A pre-v15 config: no durable counter, and no surviving server carries a uid.
fn counterless_config() -> (ServersFile, SettingsFile) {
    (ServersFile::default(), SettingsFile::default())
}

/// Protects the `uid_floor` term in [`next_free_uid`]: a uid that survives ONLY in a durable
/// store must never be issued again.
///
/// The plausible edit: folding the floor in as `max(m)` instead of `max(m + 1)`, reading it as
/// "the highest uid to allow" rather than "the highest uid already taken". The counter then
/// lands exactly ON the deleted core's uid and the next core added receives it.
///
/// Consequence: `reports.sqlite`, `strategies.sqlite` and `figures.json` all key on this number
/// and none of them is purged when a server is deleted, so the new core opens showing another
/// server's closed trades, P&L curve and strategy version history as its own — wrong money on
/// a trading terminal, not a cosmetic mix-up.
#[test]
fn a_uid_surviving_only_in_a_store_is_never_handed_out_again() {
    let (sf, meta) = counterless_config();
    // uid 3 was deleted from the config but still keys rows in the stores.
    let seeded = next_free_uid(&sf, &meta, Some(3));
    assert_eq!(
        seeded, 4,
        "the counter must clear the highest uid a store has seen"
    );

    let mut counter = seeded;
    let mut servers = vec![server(1), server(2), server(0)];
    ensure_uids(&mut servers, &mut counter);
    let fresh = servers.last().expect("just pushed").uid;
    assert!(
        fresh > 3,
        "uid {fresh} reuses a deleted core's identity; reports.sqlite still keys rows on it"
    );
}

/// Protects the fail-soft half of the floor: an unreadable store contributes nothing rather
/// than dragging the high-water mark down to the surviving servers.
#[test]
fn an_unreadable_store_leaves_the_counter_alone() {
    let (sf, meta) = counterless_config();
    assert_eq!(next_free_uid(&sf, &meta, None), 1);
}

/// Protects monotonicity: the floor may only ever RAISE the mark. A truncated or partially
/// synced replica reports a lower maximum than the config already issued past, and honouring
/// it would hand out live uids.
#[test]
fn a_store_below_the_counter_never_lowers_it() {
    let sf = ServersFile::default();
    let meta = SettingsFile {
        next_uid: 10,
        ..Default::default()
    };
    assert_eq!(next_free_uid(&sf, &meta, Some(3)), 10);
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
