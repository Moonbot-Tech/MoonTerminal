//! Synthetic scale measurements for warning persistence paths.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use super::capture_topology;
use crate::backend::core_warn::store::{WarnStore, read_episode_series, read_episodes_for_server};
use crate::backend::core_warn::{WarnAxis, WarnEpisode, WarnSnapshot};
use crate::backend::server_chart::CoreMetrics;

/// Return a unique file for one synthetic warning database.
fn database_path() -> std::path::PathBuf {
    static NEXT_PATH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = NEXT_PATH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "moon-warn-scale-{}-{unique}.sqlite",
        std::process::id()
    ))
}

/// Build one closed synthetic warning episode.
fn episode() -> WarnEpisode {
    WarnEpisode {
        id: 0,
        axis: WarnAxis::SysCpu,
        server_ip: Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))),
        core_id: None,
        start_ms: 80_000_000,
        end_ms: Some(80_001_000),
        peak: 90,
        snap: WarnSnapshot::default(),
    }
}

/// Return the middle elapsed sample after sorting.
fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// Measure UI submission and background SQLite work over 100000 episodes and ten cores.
#[test]
#[ignore = "manual 100000-episode warning UI timing probe"]
fn bench_warning_sqlite_ui_paths_100000() {
    let path = database_path();
    let store = WarnStore::open(&path).unwrap();
    let seed = rusqlite::Connection::open(&path).unwrap();
    seed.execute_batch(
        "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i <= 100000) \
         INSERT INTO core_warnings(axis,server_ip,core_id,start_ms,end_ms,peak) \
         SELECT 'sys_cpu','192.0.2.1',i%10+1,i*864,i*864+1,90 FROM n;",
    )
    .unwrap();
    drop(store);
    let store = WarnStore::open(&path).unwrap();
    store
        .insert_series(1, 0, "server", 0, &[(10, 20), (30, 40)])
        .unwrap();
    store
        .insert_ping_series(1, 0, "ping", 0, &[10, 20])
        .unwrap();
    store
        .insert_ping_series(1, 0, "exch", 0, &[30, 40])
        .unwrap();
    store.flush_writer().unwrap();

    let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    let mut episodes = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        std::hint::black_box(read_episodes_for_server(&path, ip, 0, 86_400_000).unwrap());
        episodes.push(start.elapsed());
    }

    let mut hover = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        std::hint::black_box(read_episode_series(&path, 1).unwrap());
        hover.push(start.elapsed());
    }

    let server: VecDeque<(u8, u8)> = (0..61).map(|i| (i, 100 - i)).collect();
    let ping: VecDeque<u16> = (0..61).collect();
    let exch: VecDeque<u16> = (0..61).map(|i| i * 2).collect();
    let core_ring: VecDeque<CoreMetrics> = (0..61)
        .map(|i| CoreMetrics {
            cpu: i,
            mem: 100 - i,
            ping: i as u16,
            exch: i as u16 * 2,
        })
        .collect();
    let cores: Vec<_> = (1..=10).map(|id| (id, Some(&core_ring))).collect();
    let mut capture_ui = Vec::new();
    let mut capture_background = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        let slices = capture_topology(
            Some(&server),
            Some(&ping),
            Some(&exch),
            &cores,
            80_000_000,
            80_000_000,
        );
        std::hint::black_box(
            store
                .insert_episode_with_slices(&episode(), slices)
                .unwrap(),
        );
        capture_ui.push(start.elapsed());
        let background = Instant::now();
        store.flush_writer().unwrap();
        capture_background.push(background.elapsed());
    }

    let mut prune_ui = Vec::new();
    let mut prune_background = Vec::new();
    for _ in 0..7 {
        seed.execute_batch(
            "INSERT OR REPLACE INTO core_warning_series \
             SELECT id,0,'server',zeroblob(128) FROM core_warnings;",
        )
        .unwrap();
        let start = Instant::now();
        std::hint::black_box(store.prune_slices(43_200_000).unwrap());
        prune_ui.push(start.elapsed());
        let background = Instant::now();
        store.flush_writer().unwrap();
        prune_background.push(background.elapsed());
    }

    println!(
        "warning_ui_scale episodes=100000 cores=10 episodes_background_read_ms={:.3} hover_background_three_reads_ms={:.3} capture_ui_submit_ms={:.3} capture_background_commit_ms={:.3} prune_ui_submit_ms={:.3} prune_background_indexed_ms={:.3}",
        median(episodes).as_secs_f64() * 1000.0,
        median(hover).as_secs_f64() * 1000.0,
        median(capture_ui).as_secs_f64() * 1000.0,
        median(capture_background).as_secs_f64() * 1000.0,
        median(prune_ui).as_secs_f64() * 1000.0,
        median(prune_background).as_secs_f64() * 1000.0,
    );
    drop((store, seed));
    std::fs::remove_file(path).unwrap();
}
