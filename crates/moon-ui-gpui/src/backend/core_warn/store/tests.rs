//! Round-trip and filtering tests for the warning-episode SQLite store (in-memory DB).
//!
//! Explicit imports (no `use super::*`) per the crate's test convention.

use std::net::{IpAddr, Ipv4Addr};

use rusqlite::Connection;

use super::WarnStore;
use crate::backend::core_warn::{WarnAxis, WarnEpisode, WarnSnapshot};

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
        snap: WarnSnapshot {
            sys_cpu: 77,
            occ_mem: 55,
            free_mb: 2048,
            used_mb: 4096,
            logical_cpus: 8,
            round_trip_ms: 180,
            order_api_latency_ms: 95,
        },
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
    // The detection snapshot round-trips.
    assert_eq!(wide[1].snap.sys_cpu, 77);
    assert_eq!(wide[1].snap.free_mb, 2048);
    assert_eq!(wide[1].snap.used_mb, 4096);
    assert_eq!(wide[1].snap.logical_cpus, 8);
    assert_eq!(wide[1].snap.round_trip_ms, 180);
    assert_eq!(wide[1].snap.order_api_latency_ms, 95);
    assert_eq!(wide[2].axis, WarnAxis::Unreachable);
    assert_eq!(wide[2].end_ms, Some(9_000));
}

/// A persisted ±1 min history slice must round-trip by episode id and subject; an absent subject or
/// unknown episode reads `None`.
#[test]
fn series_slice_round_trips_by_episode() {
    let store = store();
    let rowid = store
        .insert_episode(&episode(
            WarnAxis::SysCpu,
            [10, 0, 0, 1],
            None,
            1_000,
            2_000,
            88,
        ))
        .unwrap();

    // Includes the byte-boundary values (0 and 255) to catch an encode/decode off-by-one.
    let samples = [(10u8, 20u8), (30, 40), (255, 0), (0, 255)];
    store
        .insert_series(rowid, 0, "server", 60_000, &samples)
        .unwrap();

    let read = store.series_for_episode(rowid, 0, "server").unwrap();
    assert_eq!(read.as_deref(), Some(&samples[..]));

    // A subject never written and an unknown episode both read as absent, not an error.
    assert_eq!(store.series_for_episode(rowid, 0, "core").unwrap(), None);
    assert_eq!(
        store.series_for_episode(rowid + 999, 0, "server").unwrap(),
        None
    );

    // Re-capturing (partial at close, then the complete window) overwrites, never duplicates: the
    // second write wins and the unique index keeps a single row.
    let complete = [(1u8, 2u8), (3, 4), (5, 6)];
    store
        .insert_series(rowid, 0, "server", 60_000, &complete)
        .unwrap();
    assert_eq!(
        store
            .series_for_episode(rowid, 0, "server")
            .unwrap()
            .as_deref(),
        Some(&complete[..])
    );
    let rows: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM core_warning_series WHERE episode_id = ?1 AND badge = 0 AND subject = 'server'",
            [rowid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        rows, 1,
        "OR REPLACE must keep exactly one row per (episode, badge, subject)"
    );
}

/// Per-core slices (badge = core id) coexist with the server slice (badge 0) on one episode: same
/// subject on different badges is a distinct row, so a server graph and each core's graph all persist.
#[test]
fn per_core_slices_coexist_by_badge() {
    let store = store();
    let rowid = store
        .insert_episode(&episode(
            WarnAxis::SysCpu,
            [10, 0, 0, 1],
            None,
            1_000,
            2_000,
            88,
        ))
        .unwrap();

    // Server graph and two cores' graphs, all under the same subjects but different badges.
    store
        .insert_series(rowid, 0, "server", 60_000, &[(50, 60)])
        .unwrap();
    store
        .insert_series(rowid, 7, "core", 60_000, &[(30, 40)])
        .unwrap();
    store
        .insert_series(rowid, 9, "core", 60_000, &[(10, 20)])
        .unwrap();
    store
        .insert_ping_series(rowid, 7, "ping", 60_000, &[120])
        .unwrap();
    store
        .insert_ping_series(rowid, 9, "ping", 60_000, &[999])
        .unwrap();

    assert_eq!(
        store
            .series_for_episode(rowid, 0, "server")
            .unwrap()
            .as_deref(),
        Some(&[(50u8, 60u8)][..])
    );
    assert_eq!(
        store
            .series_for_episode(rowid, 7, "core")
            .unwrap()
            .as_deref(),
        Some(&[(30u8, 40u8)][..])
    );
    assert_eq!(
        store
            .series_for_episode(rowid, 9, "core")
            .unwrap()
            .as_deref(),
        Some(&[(10u8, 20u8)][..])
    );
    assert_eq!(
        store
            .ping_series_for_episode(rowid, 7, "ping")
            .unwrap()
            .as_deref(),
        Some(&[120u16][..])
    );
    assert_eq!(
        store
            .ping_series_for_episode(rowid, 9, "ping")
            .unwrap()
            .as_deref(),
        Some(&[999u16][..])
    );

    // Pruning below the episode start removes its slices but keeps the episode row.
    let deleted = store.prune_slices(10_000).unwrap();
    assert_eq!(deleted, 5, "all five slices pruned");
    assert_eq!(store.series_for_episode(rowid, 0, "server").unwrap(), None);
    assert_eq!(
        store.recent_episodes(10).unwrap().len(),
        1,
        "episode row survives the prune"
    );
}

/// The per-episode ping slice (a `u16`-per-sample blob under the `ping` subject) must round-trip,
/// coexist with the `server` slice, and read `None` when absent.
#[test]
fn ping_series_round_trips() {
    let store = store();
    let rowid = store
        .insert_episode(&episode(
            WarnAxis::Ping,
            [10, 0, 0, 1],
            Some(3),
            1_000,
            2_000,
            640,
        ))
        .unwrap();

    // Boundary values catch a u16 encode/decode off-by-one.
    let pings = [0u16, 65535, 250, 1000];
    store
        .insert_ping_series(rowid, 0, "ping", 60_000, &pings)
        .unwrap();
    assert_eq!(
        store
            .ping_series_for_episode(rowid, 0, "ping")
            .unwrap()
            .as_deref(),
        Some(&pings[..])
    );

    // The exchange ping is a distinct subject on the same episode and does not collide, nor does the
    // (u8,u8) server slice.
    let exch = [10u16, 200, 65535];
    store
        .insert_ping_series(rowid, 0, "exch", 60_000, &exch)
        .unwrap();
    store
        .insert_series(rowid, 0, "server", 60_000, &[(1, 2), (3, 4)])
        .unwrap();
    assert_eq!(
        store
            .ping_series_for_episode(rowid, 0, "ping")
            .unwrap()
            .as_deref(),
        Some(&pings[..])
    );
    assert_eq!(
        store
            .ping_series_for_episode(rowid, 0, "exch")
            .unwrap()
            .as_deref(),
        Some(&exch[..])
    );

    // An episode with no ping slice reads absent.
    let other = store
        .insert_episode(&episode(
            WarnAxis::SysCpu,
            [10, 0, 0, 2],
            None,
            3_000,
            4_000,
            90,
        ))
        .unwrap();
    assert_eq!(
        store.ping_series_for_episode(other, 0, "ping").unwrap(),
        None
    );
}
