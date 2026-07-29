//! Round-trip and filtering tests for the warning-episode SQLite store (in-memory DB).
//!
//! Explicit imports (no `use super::*`) per the crate's test convention.

use std::net::{IpAddr, Ipv4Addr};

use rusqlite::Connection;

use super::WarnStore;
use crate::backend::core_warn::{WarnAxis, WarnEpisode};

/// A store backed by a throwaway in-memory database.
fn store() -> WarnStore {
    WarnStore::from_connection(Connection::open_in_memory().unwrap()).unwrap()
}

/// Build one closed episode; `id` is ignored on insert (the DB assigns the row id).
fn episode(
    axis: WarnAxis,
    ip: [u8; 4],
    core_id: Option<u64>,
    start_ms: i64,
    end_ms: i64,
    peak: u16,
) -> WarnEpisode {
    WarnEpisode {
        id: 0,
        axis,
        server_ip: Some(IpAddr::V4(Ipv4Addr::from(ip))),
        core_id,
        start_ms,
        end_ms: Some(end_ms),
        peak,
    }
}

/// Episodes must round-trip through SQLite and filter by both server and the start-time window,
/// oldest first, reconstructing axis / core_id / peak / end_ms.
#[test]
fn roundtrip_filters_by_server_and_time() {
    let store = store();
    let a = [10, 0, 0, 1];
    let b = [10, 0, 0, 2];
    let ip_a = IpAddr::V4(Ipv4Addr::from(a));

    store
        .insert_episode(&episode(WarnAxis::SysCpu, a, None, 1_000, 2_000, 88))
        .unwrap();
    store
        .insert_episode(&episode(WarnAxis::MemGrowth, a, Some(7), 5_000, 6_000, 512))
        .unwrap();
    store
        .insert_episode(&episode(WarnAxis::SysCpu, b, None, 1_500, 1_800, 75))
        .unwrap();
    store
        .insert_episode(&episode(WarnAxis::Unreachable, a, None, 8_000, 9_000, 0))
        .unwrap();

    // Server A, start within [0, 4000] → only the first episode (B is filtered out, the others are
    // outside the window).
    let narrow = store.episodes_for_server(ip_a, 0, 4_000).unwrap();
    assert_eq!(narrow.len(), 1);
    assert_eq!(narrow[0].axis, WarnAxis::SysCpu);
    assert_eq!(narrow[0].server_ip, Some(ip_a));
    assert_eq!(narrow[0].peak, 88);
    assert_eq!(narrow[0].end_ms, Some(2_000));
    assert_eq!(narrow[0].core_id, None);

    // A wide window returns all server-A episodes, oldest first, reconstructing each axis (including
    // the connectivity axis string round-trip).
    let wide = store.episodes_for_server(ip_a, 0, 100_000).unwrap();
    assert_eq!(wide.len(), 3);
    assert_eq!(wide[1].axis, WarnAxis::MemGrowth);
    assert_eq!(wide[1].core_id, Some(7));
    assert_eq!(wide[1].peak, 512);
    assert_eq!(wide[2].axis, WarnAxis::Unreachable);
    assert_eq!(wide[2].end_ms, Some(9_000));
}
