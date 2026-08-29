//! Detection and episode-lifecycle tests for the backend warning engine.
//!
//! Explicit imports (no `use super::*`) per the crate's test convention.

use std::net::{IpAddr, Ipv4Addr};

use moon_core::feed::ConnStatus;
use moon_core::session::{CoreId, CoreSysStatus};

use super::{
    CPU_SUSTAIN_SECS, CoreSample, CoreWarnEngine, LATENCY_SUSTAIN_SECS, LatencySeverity,
    RingSubject, WarnAxis, WarnEnabled, latency_severity,
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
        api_days: None,
        api_quota: None,
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
        api_days: None,
        api_quota: None,
    }
}

/// Build one Ready sample carrying a specific core→exchange order latency (ms); CPU/memory idle.
fn sample_exch(id: CoreId, ip: [u8; 4], exch: Option<u16>) -> CoreSample {
    CoreSample {
        id,
        ip: Some(IpAddr::V4(Ipv4Addr::from(ip))),
        status: ConnStatus::Ready,
        sys: CoreSysStatus {
            order_api_latency_ms: exch,
            updated_ms: 1,
            ..CoreSysStatus::default()
        },
        api_days: None,
        api_quota: None,
    }
}

/// Feed one core a `rtt` for enough seconds (starting at `from_sec`) to FILL its baseline window, so
/// a following spike does not immediately dominate a short mean. Returns the next free second.
fn seed_ping_baseline(engine: &mut CoreWarnEngine, from_sec: i64, id: CoreId, rtt: u32) -> i64 {
    let mut sec = from_sec;
    for _ in 0..62 {
        engine.tick(&[sample_rtt(id, IP, Some(rtt))], sec * 1000);
        sec += 1;
    }
    sec
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
        api_days: None,
        api_quota: None,
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
        api_days: None,
        api_quota: None,
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

/// `latency_severity` is PURELY relative (a multiple of the baseline): no/zero baseline is `Normal`;
/// below the yellow multiple is `Normal`; at/above yellow is `Warning`; at/above red is `Critical` —
/// the multiple is the only test, so it fires the same on small and large pings.
#[test]
fn latency_severity_is_purely_relative() {
    // Default multipliers: yellow ×2 (num 200), red ×10 (num 1000).
    let sev = |v, b| latency_severity(v, b, 200, 1000);
    assert_eq!(sev(5000, None), LatencySeverity::Normal);
    assert_eq!(sev(5000, Some(0)), LatencySeverity::Normal);
    // 100 → 150 is ×1.5: below the yellow ×2.
    assert_eq!(sev(150, Some(100)), LatencySeverity::Normal);
    // 100 → 220 is ×2.2: yellow, under the ×10 critical.
    assert_eq!(sev(220, Some(100)), LatencySeverity::Warning);
    // 100 → 1000 is ×10: critical.
    assert_eq!(sev(1000, Some(100)), LatencySeverity::Critical);
    // A small ping obeys the same multiple: 20 → 50 (×2.5) is yellow.
    assert_eq!(sev(50, Some(20)), LatencySeverity::Warning);
    assert_eq!(sev(200, Some(20)), LatencySeverity::Critical);
}

/// A ping that spikes ABOVE the core's established baseline must open exactly one per-core ping
/// episode after the sustain window, and a return to baseline must close it with its peak retained.
#[test]
fn ping_spike_above_baseline_opens_then_closes_per_core_episode() {
    let mut engine = CoreWarnEngine::default();

    // Establish a low, stable baseline (~40 ms).
    let mut sec = seed_ping_baseline(&mut engine, 1, 4, 40);

    // One high sample is not yet a warning (needs the sustain window). 500 ms is well past ×10 of 40.
    engine.tick(&[sample_rtt(4, IP, Some(2000))], sec * 1000);
    sec += 1;
    assert!(!engine.core_ping_warn(4), "one spike second must not warn");

    // Stay high past the sustain threshold.
    for _ in 0..(LATENCY_SUSTAIN_SECS as i64 + 1) {
        engine.tick(&[sample_rtt(4, IP, Some(2000))], sec * 1000);
        sec += 1;
    }
    assert!(engine.core_ping_warn(4), "sustained spike must warn");
    let open = engine.open_episodes();
    let ping_open: Vec<_> = open.iter().filter(|e| e.axis == WarnAxis::Ping).collect();
    assert_eq!(ping_open.len(), 1, "one ping episode open");
    assert_eq!(ping_open[0].core_id, Some(4), "keyed by its core");

    // Ping returns to its baseline.
    for _ in 0..3 {
        engine.tick(&[sample_rtt(4, IP, Some(40))], sec * 1000);
        sec += 1;
    }
    assert!(
        !engine.core_ping_warn(4),
        "ping back at baseline must clear the warning"
    );
    let closed: Vec<_> = engine
        .episodes()
        .filter(|e| e.axis == WarnAxis::Ping)
        .collect();
    assert_eq!(closed.len(), 1);
    assert!(closed[0].end_ms.unwrap() > closed[0].start_ms);
    assert!(closed[0].peak >= 300, "peak retains the spike RTT in ms");
}

/// A core whose ping is ALWAYS high (its baseline IS high) must never warn — the 20/60/200 ms case:
/// a stably-slow link is its own normal, not a spike.
#[test]
fn stable_high_ping_never_warns() {
    let mut engine = CoreWarnEngine::default();
    for sec in 1..=(LATENCY_SUSTAIN_SECS as i64 + 20) {
        engine.tick(&[sample_rtt(4, IP, Some(200))], sec * 1000);
    }
    assert!(
        !engine.core_ping_warn(4),
        "a constantly-high ping is the baseline, not a spike"
    );
    assert!(
        engine.open_episodes().is_empty(),
        "no ping episode opens for a stable baseline"
    );
}

/// A core→exchange latency spike above the core's baseline must open exactly one per-core exch-ping
/// episode after the sustain window.
#[test]
fn exch_spike_above_baseline_opens_episode() {
    let mut engine = CoreWarnEngine::default();

    // Fill the baseline window with a stable exchange latency (~120 ms).
    let mut sec = 1;
    for _ in 0..62 {
        engine.tick(&[sample_exch(4, IP, Some(120))], sec * 1000);
        sec += 1;
    }
    assert!(!engine.core_exch_warn(4), "at baseline must not warn");

    // Spike well past ×10 of 120 (and staying there as it enters the baseline), sustained.
    for _ in 0..(LATENCY_SUSTAIN_SECS as i64 + 1) {
        engine.tick(&[sample_exch(4, IP, Some(4000))], sec * 1000);
        sec += 1;
    }
    assert!(engine.core_exch_warn(4), "sustained exch spike must warn");
    let open = engine.open_episodes();
    let exch_open: Vec<_> = open
        .iter()
        .filter(|e| e.axis == WarnAxis::ExchPing)
        .collect();
    assert_eq!(exch_open.len(), 1, "one exch-ping episode open");
    assert_eq!(exch_open[0].core_id, Some(4), "keyed by its core");
    assert!(exch_open[0].peak >= 500, "peak retains the spike latency");
}

/// A core that drops offline but keeps its last (stale) high RTT reading must NOT keep raising a
/// ping warning — the detector only counts a Ready core's live round-trip.
#[test]
fn offline_core_does_not_ping_warn_on_stale_rtt() {
    let mut engine = CoreWarnEngine::default();

    // Establish a low baseline, then spike sustained while Ready → warns.
    let mut sec = seed_ping_baseline(&mut engine, 1, 4, 40);
    for _ in 0..(LATENCY_SUSTAIN_SECS as i64 + 1) {
        engine.tick(&[sample_rtt(4, IP, Some(2000))], sec * 1000);
        sec += 1;
    }
    assert!(
        engine.core_ping_warn(4),
        "sustained spike while Ready must warn first"
    );

    // The core disconnects but its last sample still carries the high RTT.
    let mut stale = sample_rtt(4, IP, Some(2000));
    stale.status = ConnStatus::Disconnected;
    engine.tick(&[stale], sec * 1000);
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
        exch: true,
        api: true,
        api_quota: true,
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

/// Build one Ready sample carrying a specific API-key day count (`None` = no key answer to judge).
fn sample_api(id: CoreId, ip: [u8; 4], api_days: Option<i32>) -> CoreSample {
    CoreSample {
        api_days,
        ..sample(id, ip, None, None)
    }
}

/// The API-key axis is a THRESHOLD, not a sustained measurement: it must fire on the very first
/// sample inside the horizon. Requiring a hold window here — the shape every other axis has — would
/// delay a warning whose input only moves once a day.
#[test]
fn an_expiring_key_warns_on_the_first_sample() {
    let mut engine = CoreWarnEngine::default();

    engine.tick(&[sample_api(1, IP, Some(3))], 1_000);

    assert!(
        engine.core_api_warn(1),
        "3 days is inside the 7-day default"
    );
    assert_eq!(engine.open_episodes().len(), 1);
}

/// A key with plenty of time left, and a key with NO expiration at all, must both stay silent. The
/// second is the dangerous one: moonproto reports it as zero days beside `is_known() == false`, so
/// a caller that reads the number alone would warn on every perpetual key forever.
#[test]
fn a_distant_or_perpetual_key_never_warns() {
    let mut engine = CoreWarnEngine::default();

    engine.tick(
        &[sample_api(1, IP, Some(60)), sample_api(2, IP, None)],
        1_000,
    );

    assert!(!engine.core_api_warn(1), "60 days is outside the horizon");
    assert!(!engine.core_api_warn(2), "no expiry is not an expiry today");
    assert!(engine.open_episodes().is_empty());
}

/// The worst moment of an expiring-key episode is its FEWEST days, so `peak` must fall, not climb.
/// Reusing the `max` rule every other axis has would record the day count the episode STARTED at
/// and permanently understate how close the key got.
#[test]
fn the_key_episode_records_its_lowest_day_count() {
    let mut engine = CoreWarnEngine::default();

    engine.tick(&[sample_api(1, IP, Some(5))], 1_000);
    engine.tick(&[sample_api(1, IP, Some(2))], 2_000);
    engine.tick(&[sample_api(1, IP, Some(4))], 3_000);

    let open = engine.open_episodes();
    assert_eq!(open.len(), 1, "one episode spanning all three ticks");
    assert_eq!(open[0].peak, 2, "the closest the key came to expiring");
    assert_eq!(open[0].axis, WarnAxis::ApiExpiry);
    assert_eq!(open[0].core_id, Some(1), "the key belongs to the core");
}

/// An expired key reports a NEGATIVE day count; `peak` is unsigned, so it has to clamp rather than
/// wrap — a wrapped value would read as 65 000 days left on the row that matters most.
#[test]
fn an_expired_key_clamps_its_day_count() {
    let mut engine = CoreWarnEngine::default();

    engine.tick(&[sample_api(1, IP, Some(-12))], 1_000);

    let open = engine.open_episodes();
    assert!(engine.core_api_warn(1), "an expired key is still a warning");
    assert_eq!(open[0].peak, 0, "clamped, not wrapped");
}

/// Replacing the key clears the warning and closes the episode — the "stays on until replaced"
/// contract, seen from its end.
#[test]
fn replacing_the_key_closes_the_episode() {
    let mut engine = CoreWarnEngine::default();
    engine.tick(&[sample_api(1, IP, Some(2))], 1_000);
    assert!(engine.core_api_warn(1));

    let result = engine.tick(&[sample_api(1, IP, Some(365))], 2_000);

    assert!(!engine.core_api_warn(1));
    assert_eq!(result.closed.len(), 1, "the episode closed");
    assert_eq!(result.closed[0].axis, WarnAxis::ApiExpiry);
}

/// Build one Ready sample carrying a specific remaining API request quota (`None` = the core
/// publishes no quota, which every exchange but HyperLiquid does).
fn sample_quota(id: CoreId, ip: [u8; 4], api_quota: Option<u64>) -> CoreSample {
    CoreSample {
        api_quota,
        ..sample(id, ip, None, None)
    }
}

/// The quota axis is a threshold like the key above, so it fires on the first sample at or below
/// the floor. The boundary itself counts: a core sitting exactly ON the configured floor has
/// reached it, and an exclusive comparison would leave that core silent forever at 5000.
#[test]
fn an_exhausted_quota_warns_on_the_first_sample() {
    let mut engine = CoreWarnEngine::default();

    engine.tick(
        &[
            sample_quota(1, IP, Some(4_000)),
            sample_quota(2, IP, Some(5_000)),
        ],
        1_000,
    );

    assert!(
        engine.core_api_quota_warn(1),
        "4000 is under the 5000 floor"
    );
    assert!(engine.core_api_quota_warn(2), "exactly at the floor counts");
    assert_eq!(engine.open_episodes().len(), 2);
}

/// A healthy quota and a core that reports none must both stay silent. The second is the one that
/// matters: every non-HyperLiquid core reports `None`, so a warning derived from the absence would
/// light the whole fleet.
#[test]
fn a_healthy_or_absent_quota_never_warns() {
    let mut engine = CoreWarnEngine::default();

    engine.tick(
        &[
            sample_quota(1, IP, Some(1_065_447)),
            sample_quota(2, IP, None),
        ],
        1_000,
    );

    assert!(
        !engine.core_api_quota_warn(1),
        "a full quota is not a warning"
    );
    assert!(
        !engine.core_api_quota_warn(2),
        "no quota published is not an exhausted quota"
    );
    assert!(engine.open_episodes().is_empty());
}

/// Unlike an expiring key, a quota RECOVERS: a HyperLiquid address earns requests back with volume.
/// The warning must therefore clear on its own once the count climbs back over the floor, and the
/// episode must close — a rule copied from the key axis would leave it warning until a restart.
#[test]
fn a_recovering_quota_closes_its_episode() {
    let mut engine = CoreWarnEngine::default();
    engine.tick(&[sample_quota(1, IP, Some(900))], 1_000);
    assert!(engine.core_api_quota_warn(1));

    let result = engine.tick(&[sample_quota(1, IP, Some(20_000))], 2_000);

    assert!(!engine.core_api_quota_warn(1));
    assert_eq!(result.closed.len(), 1, "the episode closed");
    assert_eq!(result.closed[0].axis, WarnAxis::ApiQuota);
}

/// The worst moment of a quota episode is its SMALLEST count, so `peak` must fall like the key
/// axis and not climb like every measured one.
#[test]
fn the_quota_episode_records_its_lowest_count() {
    let mut engine = CoreWarnEngine::default();

    engine.tick(&[sample_quota(1, IP, Some(4_800))], 1_000);
    engine.tick(&[sample_quota(1, IP, Some(1_200))], 2_000);
    engine.tick(&[sample_quota(1, IP, Some(3_000))], 3_000);

    assert_eq!(engine.open_episodes()[0].peak, 1_200);
}
