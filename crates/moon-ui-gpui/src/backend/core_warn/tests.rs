//! Detection and episode-lifecycle tests for the backend warning engine.
//!
//! Explicit imports (no `use super::*`) per the crate's test convention.

use std::net::{IpAddr, Ipv4Addr};

use moon_core::feed::ConnStatus;
use moon_core::session::{CoreId, CoreSysStatus};

use super::{
    CPU_SUSTAIN_SECS, CoreSample, CoreWarnEngine, PING_SUSTAIN_SECS, RingSubject, WarnAxis,
    WarnEnabled,
};

/// Build one Ready core sample; `process` and `system` CPU are set equal, memory optional.
fn sample(id: CoreId, ip: [u8; 4], cpu: Option<u8>, used: Option<u16>) -> CoreSample {
    CoreSample {
        id,
        ip: Some(IpAddr::V4(Ipv4Addr::from(ip))),
        status: ConnStatus::Ready,
        sys: CoreSysStatus {
            system_cpu_percent: cpu,
            process_cpu_percent: cpu,
            used_memory_mb: used,
            updated_ms: 1,
            ..CoreSysStatus::default()
        },
    }
}

/// Build one Ready sample carrying a specific client↔core round-trip (ms); CPU/memory idle.
fn sample_rtt(id: CoreId, ip: [u8; 4], rtt: Option<u32>) -> CoreSample {
    CoreSample {
        id,
        ip: Some(IpAddr::V4(Ipv4Addr::from(ip))),
        status: ConnStatus::Ready,
        sys: CoreSysStatus {
            round_trip_ms: rtt,
            updated_ms: 1,
            ..CoreSysStatus::default()
        },
    }
}

/// Build one core sample carrying a specific connection status (telemetry irrelevant).
fn sample_conn(id: CoreId, ip: [u8; 4], status: ConnStatus) -> CoreSample {
    CoreSample {
        id,
        ip: Some(IpAddr::V4(Ipv4Addr::from(ip))),
        status,
        sys: CoreSysStatus {
            updated_ms: 1,
            ..CoreSysStatus::default()
        },
    }
}

/// Feed one core's `(cpu, used)` reading at second `sec`.
fn tick_one(
    engine: &mut CoreWarnEngine,
    sec: i64,
    id: CoreId,
    ip: [u8; 4],
    cpu: Option<u8>,
    used: Option<u16>,
) {
    engine.tick(&[sample(id, ip, cpu, used)], sec * 1000);
}

const IP: [u8; 4] = [10, 0, 0, 1];

/// Sustained high system CPU must open exactly one server episode only AFTER the sustain window, and
/// clearing the CPU must close it with a real start<end interval.
#[test]
fn sustained_cpu_opens_then_closes_one_episode() {
    let mut engine = CoreWarnEngine::default();
    let ip = IpAddr::V4(Ipv4Addr::from(IP));

    // One high second is not yet a warning.
    tick_one(&mut engine, 1, 1, IP, Some(85), Some(500));
    assert!(!engine.server_cpu_warn(ip), "one high second must not warn");

    // Stay high well past the sustain threshold.
    for sec in 2..=(CPU_SUSTAIN_SECS as i64 + 3) {
        tick_one(&mut engine, sec, 1, IP, Some(85), Some(500));
    }
    assert!(engine.server_cpu_warn(ip), "sustained high CPU must warn");
    assert_eq!(engine.open_episodes().len(), 1);
    assert!(engine.episodes().next().is_none(), "nothing closed yet");

    // Drop CPU; the averaging window needs a few low seconds to fall under the threshold.
    let mut sec = CPU_SUSTAIN_SECS as i64 + 4;
    for _ in 0..5 {
        tick_one(&mut engine, sec, 1, IP, Some(5), Some(500));
        sec += 1;
    }
    assert!(
        !engine.server_cpu_warn(ip),
        "CPU dropped, warning must clear"
    );
    let closed: Vec<_> = engine.episodes().collect();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].axis, WarnAxis::SysCpu);
    assert_eq!(closed[0].server_ip, Some(ip));
    assert_eq!(closed[0].core_id, None);
    assert!(closed[0].end_ms.unwrap() > closed[0].start_ms);
    assert!(closed[0].peak >= 70);
}

/// A rising memory footprint must open a per-core memory episode, and returning to baseline must
/// close it.
#[test]
fn memory_growth_opens_then_closes_per_core_episode() {
    let mut engine = CoreWarnEngine::default();

    // Flat baseline, then a clear rise above the window minimum.
    let rise = [400u16, 400, 400, 400, 420, 460, 500];
    for (i, used) in rise.iter().enumerate() {
        tick_one(&mut engine, i as i64 + 1, 7, IP, Some(10), Some(*used));
    }
    assert!(engine.core_mem_warn(7), "sustained rise must warn");
    assert_eq!(engine.open_episodes().len(), 1);

    // Fall back to baseline: current returns to the window minimum, so growth is zero.
    let mut sec = rise.len() as i64 + 1;
    for _ in 0..3 {
        tick_one(&mut engine, sec, 7, IP, Some(10), Some(400));
        sec += 1;
    }
    assert!(
        !engine.core_mem_warn(7),
        "footprint returned, warning must clear"
    );
    let closed: Vec<_> = engine.episodes().collect();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].axis, WarnAxis::MemGrowth);
    assert_eq!(closed[0].core_id, Some(7));
    assert!(closed[0].end_ms.is_some());
}

/// The engine smooths CPU for the panel: `avg_cpu` reflects the recent buckets.
#[test]
fn avg_cpu_reports_the_smoothed_value() {
    let mut engine = CoreWarnEngine::default();
    tick_one(&mut engine, 1, 3, IP, Some(40), Some(500));
    tick_one(&mut engine, 2, 3, IP, Some(60), Some(500));
    // Average of 40 and 60 is 50.
    assert_eq!(engine.avg_cpu(3), (Some(50), Some(50)));
}

/// A second call within the same Unix second must be a no-op (1 Hz throttle), so a mid-second
/// reading does not double-count or replace the second's data.
#[test]
fn same_second_tick_is_ignored() {
    let mut engine = CoreWarnEngine::default();
    engine.tick(&[sample(3, IP, Some(40), Some(500))], 1000);
    engine.tick(&[sample(3, IP, Some(90), Some(500))], 1500); // same second — ignored
    assert_eq!(engine.avg_cpu(3), (Some(40), Some(40)));
}

/// An open episode must CLOSE when its subject vanishes from the live set (not leak as forever-open):
/// the trickiest branch of `reconcile_episodes`.
#[test]
fn open_episode_closes_when_its_core_vanishes() {
    let mut engine = CoreWarnEngine::default();
    let rise = [400u16, 400, 400, 400, 420, 460, 500];
    for (i, used) in rise.iter().enumerate() {
        tick_one(&mut engine, i as i64 + 1, 7, IP, Some(10), Some(*used));
    }
    assert_eq!(
        engine.open_episodes().len(),
        1,
        "memory warning must be open first"
    );

    // The core disappears from the live set while its warning is still active.
    engine.tick(&[], (rise.len() as i64 + 1) * 1000);
    assert!(
        engine.open_episodes().is_empty(),
        "a vanished core must close its open episode, not leak it"
    );
    let closed: Vec<_> = engine.episodes().collect();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].core_id, Some(7));
    assert!(closed[0].end_ms.is_some());
}

/// Two distinct servers warning at once must open two independent episodes keyed by their own IP,
/// with no cross-contamination or duplicate.
#[test]
fn distinct_servers_open_independent_cpu_episodes() {
    let mut engine = CoreWarnEngine::default();
    let ip_a = [10, 0, 0, 1];
    let ip_b = [10, 0, 0, 2];
    for sec in 1..=(CPU_SUSTAIN_SECS as i64 + 3) {
        engine.tick(
            &[
                sample(1, ip_a, Some(85), Some(500)),
                sample(2, ip_b, Some(85), Some(500)),
            ],
            sec * 1000,
        );
    }
    assert!(engine.server_cpu_warn(IpAddr::V4(Ipv4Addr::from(ip_a))));
    assert!(engine.server_cpu_warn(IpAddr::V4(Ipv4Addr::from(ip_b))));

    let open = engine.open_episodes();
    assert_eq!(
        open.len(),
        2,
        "two servers must open two independent episodes"
    );
    let mut ips: Vec<_> = open
        .iter()
        .filter_map(|episode| episode.server_ip)
        .collect();
    ips.sort();
    ips.dedup();
    assert_eq!(ips.len(), 2, "each episode must carry its own server IP");
}

/// `tick` must emit one server ring sample plus one per core, with the freshest system CPU, the
/// occupied-memory share, and each core's process-memory share of the reconstructed machine total.
#[test]
fn tick_emits_server_and_core_ring_samples() {
    let mut engine = CoreWarnEngine::default();
    let mk = |id: u64, proc_cpu: u8, used: u16, updated: i64| CoreSample {
        id,
        ip: Some(IpAddr::V4(Ipv4Addr::from(IP))),
        status: ConnStatus::Ready,
        sys: CoreSysStatus {
            system_cpu_percent: Some(40),
            process_cpu_percent: Some(proc_cpu),
            used_memory_mb: Some(used),
            free_physical_memory_mb: Some(500),
            updated_ms: updated,
            ..CoreSysStatus::default()
        },
    };
    // Two cores, 500 MB each → used_sum 1000, free 500, total 1500.
    let result = engine.tick(&[mk(1, 30, 500, 10), mk(2, 20, 500, 20)], 1_000);

    assert_eq!(result.rings.len(), 3, "one server + two cores");
    let server = result
        .rings
        .iter()
        .find(|r| matches!(r.subject, RingSubject::Server(_)))
        .expect("server sample");
    assert_eq!(server.cpu, 40, "freshest system CPU");
    assert_eq!(server.mem, 66, "occupied = 1000/1500");
    let core1 = result
        .rings
        .iter()
        .find(|r| matches!(r.subject, RingSubject::Core(1)))
        .expect("core 1 sample");
    assert_eq!(core1.cpu, 30, "process CPU");
    assert_eq!(core1.mem, 33, "share = 500/1500");
}

/// A core that had come up and then dropped opens a connectivity episode; recovery closes it.
#[test]
fn dropped_core_opens_then_recovery_closes_connectivity_episode() {
    let mut engine = CoreWarnEngine::default();
    let ip = IpAddr::V4(Ipv4Addr::from(IP));

    // Both cores come up first, so a later drop reads as a real disconnect (not never-connected).
    engine.tick(
        &[
            sample_conn(1, IP, ConnStatus::Ready),
            sample_conn(2, IP, ConnStatus::Ready),
        ],
        1_000,
    );
    assert!(!engine.server_conn_warn(ip), "both up must not warn");

    // core 2 drops.
    engine.tick(
        &[
            sample_conn(1, IP, ConnStatus::Ready),
            sample_conn(2, IP, ConnStatus::Disconnected),
        ],
        2_000,
    );
    assert!(
        engine.server_conn_warn(ip),
        "a core that was up and dropped must warn"
    );
    let open_conn = engine
        .open_episodes()
        .into_iter()
        .filter(|e| e.axis == WarnAxis::Unreachable)
        .count();
    assert_eq!(open_conn, 1);

    // The dropped core recovers.
    engine.tick(
        &[
            sample_conn(1, IP, ConnStatus::Ready),
            sample_conn(2, IP, ConnStatus::Ready),
        ],
        3_000,
    );
    assert!(
        !engine.server_conn_warn(ip),
        "recovery must clear the warning"
    );
    let closed: Vec<_> = engine
        .episodes()
        .filter(|e| e.axis == WarnAxis::Unreachable)
        .collect();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].server_ip, Some(ip));
    assert!(closed[0].end_ms.is_some());
}

/// A SINGLE-core server whose only core drops (was Ready, now down) must warn — the full-outage case
/// the old "needs a surviving ready core" rule missed.
#[test]
fn solo_core_drop_warns_connectivity() {
    let mut engine = CoreWarnEngine::default();
    let ip = IpAddr::V4(Ipv4Addr::from(IP));

    engine.tick(&[sample_conn(1, IP, ConnStatus::Ready)], 1_000);
    assert!(
        !engine.server_conn_warn(ip),
        "a lone ready core must not warn"
    );

    engine.tick(
        &[sample_conn(1, IP, ConnStatus::Failed("lost".into()))],
        2_000,
    );
    assert!(
        engine.server_conn_warn(ip),
        "the server's only core dropping must warn"
    );
}

/// Connectivity must NOT warn for a NEVER-connected core (Disconnected but never Ready, e.g. at
/// startup or intentionally off) or a core that is merely connecting.
#[test]
fn never_connected_or_connecting_does_not_warn_connectivity() {
    let mut engine = CoreWarnEngine::default();
    let ip = IpAddr::V4(Ipv4Addr::from(IP));

    engine.tick(
        &[
            sample_conn(1, IP, ConnStatus::Disconnected),
            sample_conn(2, IP, ConnStatus::Failed("down".into())),
        ],
        1_000,
    );
    assert!(
        !engine.server_conn_warn(ip),
        "a core that was never Ready is not a drop"
    );

    engine.tick(
        &[
            sample_conn(1, IP, ConnStatus::Ready),
            sample_conn(2, IP, ConnStatus::Connecting),
        ],
        2_000,
    );
    assert!(!engine.server_conn_warn(ip), "connecting is not a drop");
}

/// Sustained high round-trip must open exactly one per-core ping episode only after the sustain
/// window, and a recovered ping must close it with a real interval and its peak retained.
#[test]
fn sustained_high_ping_opens_then_closes_per_core_episode() {
    let mut engine = CoreWarnEngine::default();

    // One high sample is not yet a warning.
    engine.tick(&[sample_rtt(4, IP, Some(800))], 1_000);
    assert!(!engine.core_ping_warn(4), "one high ping must not warn");

    // Stay high past the sustain threshold.
    for sec in 2..=(PING_SUSTAIN_SECS as i64 + 2) {
        engine.tick(&[sample_rtt(4, IP, Some(800))], sec * 1000);
    }
    assert!(engine.core_ping_warn(4), "sustained high ping must warn");
    let open = engine.open_episodes();
    let ping_open: Vec<_> = open.iter().filter(|e| e.axis == WarnAxis::Ping).collect();
    assert_eq!(ping_open.len(), 1, "one ping episode open");
    assert_eq!(ping_open[0].core_id, Some(4), "keyed by its core");

    // Ping recovers to normal.
    engine.tick(
        &[sample_rtt(4, IP, Some(40))],
        (PING_SUSTAIN_SECS as i64 + 3) * 1000,
    );
    assert!(
        !engine.core_ping_warn(4),
        "recovered ping must clear the warning"
    );
    let closed: Vec<_> = engine
        .episodes()
        .filter(|e| e.axis == WarnAxis::Ping)
        .collect();
    assert_eq!(closed.len(), 1);
    assert!(closed[0].end_ms.unwrap() > closed[0].start_ms);
    assert!(closed[0].peak >= 500, "peak retains the high RTT in ms");
}

/// A core that drops offline but keeps its last (stale) high RTT reading must NOT keep raising a
/// ping warning — the detector only counts a Ready core's live round-trip.
#[test]
fn offline_core_does_not_ping_warn_on_stale_rtt() {
    let mut engine = CoreWarnEngine::default();
    for sec in 1..=(PING_SUSTAIN_SECS as i64 + 2) {
        engine.tick(&[sample_rtt(4, IP, Some(800))], sec * 1000);
    }
    assert!(
        engine.core_ping_warn(4),
        "sustained high ping while Ready must warn first"
    );

    // The core disconnects but its last sample still carries the high RTT.
    let mut stale = sample_rtt(4, IP, Some(800));
    stale.status = ConnStatus::Disconnected;
    engine.tick(&[stale], (PING_SUSTAIN_SECS as i64 + 3) * 1000);
    assert!(
        !engine.core_ping_warn(4),
        "an offline core must not ping-warn on a stale round-trip"
    );
}

/// A disabled axis must record no episode and light no warning state, even under a sustained signal
/// that would otherwise fire — "off" means the engine ignores it entirely.
#[test]
fn disabled_axis_records_no_episode() {
    let mut engine = CoreWarnEngine::default();
    engine.set_enabled(WarnEnabled {
        cpu: false,
        mem: true,
        conn: true,
        ping: true,
    });
    let ip = IpAddr::V4(Ipv4Addr::from(IP));
    for sec in 1..=(CPU_SUSTAIN_SECS as i64 + 4) {
        tick_one(&mut engine, sec, 1, IP, Some(95), Some(500));
    }
    assert!(!engine.server_cpu_warn(ip), "a disabled axis must not warn");
    assert!(
        engine.open_episodes().is_empty(),
        "a disabled axis must open no episode"
    );
}

/// A core absent from a later tick is evicted from the rolling history (no stale accumulation).
#[test]
fn absent_core_is_evicted() {
    let mut engine = CoreWarnEngine::default();
    tick_one(&mut engine, 1, 9, IP, Some(40), Some(500));
    assert_eq!(engine.avg_cpu(9), (Some(40), Some(40)));
    // Next second the core is gone from the sample set.
    engine.tick(&[], 2000);
    assert_eq!(engine.avg_cpu(9), (None, None));
}
