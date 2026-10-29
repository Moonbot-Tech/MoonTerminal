//! Round-trip and filtering tests for the warning-episode SQLite store (in-memory DB).
//!
//! Explicit imports (no `use super::*`) per the crate's test convention.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};

use rusqlite::Connection;

use super::{
    RECENT_CORE_EPISODES_SQL, RECENT_CORE_TIES_SQL, RECENT_SERVER_EPISODES_SQL,
    RECENT_SERVER_TIES_SQL, WarnStore, read_recent_episodes_for_scope,
};
use crate::backend::core_warn::{WarnAxis, WarnEpisode, WarnSnapshot};

/// A store backed by a throwaway in-memory database.
fn store() -> WarnStore {
    WarnStore::from_connection(Connection::open_in_memory().unwrap()).unwrap()
}

/// A store with the pre-optimization indexes, preserving the old query planner's choices.
fn old_schema_store() -> WarnStore {
    let store = store();
    store
        .conn
        .execute_batch("DROP INDEX ix_warn_core_time; DROP INDEX ix_warn_server_null_time;")
        .unwrap();
    store
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

/// `store.rs:WarnStore::recent_episodes_for_scope` must apply SQL scope predicates before LIMIT.
///
/// Mutation: call `recent_episodes(limit)` and filter its result in Rust. The two newer foreign
/// rows then consume the limit and the assertions lose both older warnings owned by the selected
/// core/server.
#[test]
fn scoped_recent_query_filters_before_limit() {
    let store = store();
    let selected_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
    let foreign_ip = [10, 0, 0, 2];
    for start_ms in [4_000, 3_000] {
        store
            .insert_episode(&episode(
                WarnAxis::MemGrowth,
                foreign_ip,
                Some(99),
                start_ms,
                start_ms + 1,
                10,
            ))
            .unwrap();
    }
    store
        .insert_episode(&episode(
            WarnAxis::MemGrowth,
            [10, 0, 0, 1],
            Some(7),
            2_000,
            2_001,
            20,
        ))
        .unwrap();
    store
        .insert_episode(&episode(
            WarnAxis::SysCpu,
            [10, 0, 0, 1],
            None,
            1_000,
            1_001,
            30,
        ))
        .unwrap();

    let rows = store
        .recent_episodes_for_scope(&HashSet::from([7]), &HashSet::from([selected_ip]), 2)
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].core_id, Some(7));
    assert_eq!(rows[1].server_ip, Some(selected_ip));
    assert_eq!(rows[1].core_id, None);
}

/// Frozen copy of the pre-optimization OR query, used as the independent row/order oracle.
fn reference_recent_episodes_for_scope(
    store: &WarnStore,
    core_ids: &HashSet<u64>,
    server_ips: &HashSet<IpAddr>,
    limit: usize,
) -> Vec<WarnEpisode> {
    if limit == 0 || (core_ids.is_empty() && server_ips.is_empty()) {
        return Vec::new();
    }
    let mut clauses = Vec::with_capacity(2);
    let mut values = Vec::with_capacity(core_ids.len() + server_ips.len() + 1);
    if !core_ids.is_empty() {
        clauses.push(format!(
            "core_id IN ({})",
            std::iter::repeat_n("?", core_ids.len())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        values.extend(
            core_ids
                .iter()
                .map(|core| rusqlite::types::Value::Integer(*core as i64)),
        );
    }
    if !server_ips.is_empty() {
        clauses.push(format!(
            "(core_id IS NULL AND server_ip IN ({}))",
            std::iter::repeat_n("?", server_ips.len())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        values.extend(
            server_ips
                .iter()
                .map(|ip| rusqlite::types::Value::Text(ip.to_string())),
        );
    }
    values.push(rusqlite::types::Value::Integer(
        i64::try_from(limit).unwrap_or(i64::MAX),
    ));
    let sql = format!(
        "SELECT id, axis, server_ip, core_id, start_ms, end_ms, peak, sys_cpu, occ_mem, free_mb, \
         used_mb, logical_cpus, rtt_ms, order_ms FROM core_warnings \
         WHERE ({}) ORDER BY start_ms DESC LIMIT ?",
        clauses.join(" OR ")
    );
    let mut stmt = store.conn.prepare(&sql).unwrap();
    let rows = stmt
        .query_map(
            rusqlite::params_from_iter(values.iter()),
            super::row_to_episode,
        )
        .unwrap();
    rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

/// Per-identity reads must return the frozen OR query's exact rows and order, including start-time
/// ties and rows excluded by scope. Mutation: merge with a different tie rule or limit before merging.
#[test]
fn scoped_recent_query_matches_frozen_or_query() {
    let store = store();
    let reference = old_schema_store();
    for i in 0..240_i64 {
        let core_id = match i % 6 {
            0 => None,
            5 => Some(99),
            _ => Some((i % 4 + 1) as u64),
        };
        let ip = if core_id == Some(99) {
            [10, 0, 0, 9]
        } else {
            [10, 0, 0, (i % 4 + 1) as u8]
        };
        let start_ms = 10_000 + i % 29;
        let row = episode(
            if core_id.is_some() {
                WarnAxis::MemGrowth
            } else {
                WarnAxis::SysCpu
            },
            ip,
            core_id,
            start_ms,
            start_ms + 1,
            i as u16,
        );
        store.insert_episode(&row).unwrap();
        reference.insert_episode(&row).unwrap();
    }
    let core_ids = HashSet::from([1, 2, 3, 4]);
    let server_ips =
        HashSet::from_iter((1..=4).map(|last| IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))));
    for limit in [1, 7, 64, 500] {
        let actual = store
            .recent_episodes_for_scope(&core_ids, &server_ips, limit)
            .unwrap();
        let expected =
            reference_recent_episodes_for_scope(&reference, &core_ids, &server_ips, limit);
        assert_eq!(
            actual.iter().map(|row| row.id).collect::<Vec<_>>(),
            expected.iter().map(|row| row.id).collect::<Vec<_>>(),
            "limit {limit}"
        );
    }
}

/// A descending index walk must not discard an older boundary-tied row before the legacy merge.
#[test]
fn scoped_recent_query_preserves_old_boundary_tie_selection() {
    let store = store();
    let reference = old_schema_store();
    for (start_ms, peak) in [(1_000, 1), (1_000, 2)]
        .into_iter()
        .chain((0..499).map(|i| (2_000 + i, i as u16 + 3)))
    {
        let row = episode(
            WarnAxis::MemGrowth,
            [10, 0, 0, 1],
            Some(1),
            start_ms,
            start_ms + 1,
            peak,
        );
        store.insert_episode(&row).unwrap();
        reference.insert_episode(&row).unwrap();
    }
    let core_ids = HashSet::from([1]);
    let server_ips = HashSet::new();
    let actual = store
        .recent_episodes_for_scope(&core_ids, &server_ips, 500)
        .unwrap();
    let expected = reference_recent_episodes_for_scope(&reference, &core_ids, &server_ips, 500);
    assert_eq!(
        actual.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected.iter().map(|row| row.id).collect::<Vec<_>>()
    );
    assert_eq!(actual.last().map(|row| row.id), Some(1));
}

/// All per-identity statements must walk their indexes directly instead of sorting matching history.
#[test]
fn scoped_recent_queries_have_index_ordered_plans() {
    let store = store();
    for (sql, index) in [
        (RECENT_CORE_EPISODES_SQL, "ix_warn_core_time"),
        (RECENT_CORE_TIES_SQL, "ix_warn_core_time"),
        (RECENT_SERVER_EPISODES_SQL, "ix_warn_server_null_time"),
        (RECENT_SERVER_TIES_SQL, "ix_warn_server_null_time"),
    ] {
        let plan = store
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(rusqlite::params![1, 500], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" | ");
        assert!(plan.contains(index), "missing {index}: {plan}");
        assert!(
            !plan.contains("USE TEMP B-TREE FOR ORDER BY"),
            "temporary sort: {plan}"
        );
    }
}

/// Episode inserts and deletions must invalidate a persisted-read cache; series writes must not.
#[test]
fn episode_mutations_advance_the_store_revision() {
    let store = store();
    let initial = store.revision();
    store
        .insert_episode(&episode(
            WarnAxis::MemGrowth,
            [10, 0, 0, 1],
            Some(7),
            1_000,
            2_000,
            5,
        ))
        .unwrap();
    assert_eq!(store.revision(), initial + 1);
    store.insert_series(1, 0, "server", 0, &[(1, 2)]).unwrap();
    assert_eq!(store.revision(), initial + 1);
    assert_eq!(store.forget_core(7).unwrap(), 1);
    assert_eq!(store.revision(), initial + 2);
}

/// The background entry point must read the writer's rows through its own read-only connection.
#[test]
fn read_only_scoped_query_matches_writer_side_rows() {
    static NEXT_PATH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = NEXT_PATH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "moon-warn-read-{}-{unique}.sqlite",
        std::process::id()
    ));
    let store = WarnStore::open(&path).unwrap();
    store
        .insert_episode(&episode(
            WarnAxis::SysCpu,
            [10, 0, 0, 1],
            None,
            1_000,
            2_000,
            88,
        ))
        .unwrap();
    let core_ids = HashSet::new();
    let server_ips = HashSet::from([IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))]);
    let expected = store
        .recent_episodes_for_scope(&core_ids, &server_ips, 10)
        .unwrap();
    let actual = read_recent_episodes_for_scope(&path, &core_ids, &server_ips, 10).unwrap();
    assert_eq!(
        actual.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected.iter().map(|row| row.id).collect::<Vec<_>>()
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
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

/// Forgetting a core drops its own episodes and every slice it owns, and nothing else.
///
/// Breaks on: `store.rs:forget_core` leaving the core's slices under SERVER-wide episodes (badge =
/// core id), which is where its per-core graphs live; or deleting the server episode itself, which
/// describes a machine other cores share.
#[test]
fn forgetting_a_core_keeps_other_cores_and_server_episodes() {
    let store = store();
    let a = [10, 0, 0, 1];
    let own = store
        .insert_episode(&episode(WarnAxis::MemGrowth, a, Some(7), 1_000, 2_000, 5))
        .unwrap();
    let other = store
        .insert_episode(&episode(WarnAxis::MemGrowth, a, Some(8), 1_000, 2_000, 5))
        .unwrap();
    let server = store
        .insert_episode(&episode(WarnAxis::SysCpu, a, None, 1_000, 2_000, 90))
        .unwrap();
    for (episode_id, badge) in [(own, 7), (other, 8), (server, 0), (server, 7), (server, 8)] {
        store
            .conn
            .execute(
                "INSERT INTO core_warning_series (episode_id, badge, subject, blob)                  VALUES (?1, ?2, 'cpu', x'00')",
                rusqlite::params![episode_id, badge],
            )
            .unwrap();
    }

    assert_eq!(store.forget_core(7).unwrap(), 1);

    let episodes: Vec<i64> = store
        .conn
        .prepare("SELECT id FROM core_warnings ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(episodes, vec![other, server]);
    let slices: Vec<(i64, i64)> = store
        .conn
        .prepare("SELECT episode_id, badge FROM core_warning_series ORDER BY episode_id, badge")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut want = vec![(other, 8), (server, 0), (server, 8)];
    want.sort();
    assert_eq!(slices, want);
}

/// Measure one scoped Warnings-list read over 100000 retained episodes on ten cores.
#[test]
#[ignore]
fn bench_scoped_recent_episodes_100000() {
    let store = store();
    let tx = store.conn.unchecked_transaction().unwrap();
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO core_warnings \
                 (axis, server_ip, core_id, start_ms, end_ms, peak) \
                 VALUES ('sys_cpu', ?1, ?2, ?3, ?3, 1)",
            )
            .unwrap();
        for i in 0..100_000_i64 {
            let core = i % 10 + 1;
            let core_id = (i % 10 != 0).then_some(core);
            insert
                .execute(rusqlite::params![format!("10.0.0.{core}"), core_id, i])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    let core_ids = HashSet::from_iter(1..=10);
    let server_ips =
        HashSet::from_iter((1..=10).map(|last| IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))));
    let iterations = 20;
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        let rows = store
            .recent_episodes_for_scope(&core_ids, &server_ips, 500)
            .unwrap();
        std::hint::black_box(rows);
    }
    println!(
        "bench_scoped_recent_episodes_100000 us/read={}",
        start.elapsed().as_micros() / iterations
    );
}
