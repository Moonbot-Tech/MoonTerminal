//! Backend-resident core warning engine.
//!
//! Detection lives here (not in the Core Status panel) so it runs continuously from the backend
//! coordination loop, independent of whether any panel is open. It samples every live core once per
//! second, keeps the rolling CPU/memory history the warnings need, and turns the SUSTAINED/trend
//! signals into WARNING EPISODES with a start and an end time.
//!
//! Four axes:
//! - `SysCpu` — a machine's system CPU held at/above the threshold (per server IP).
//! - `MemGrowth` — a core's used memory rising above its window minimum (per core).
//! - `Unreachable` — a core dropped (Disconnected/Failed) while the server still runs a Ready core
//!   (per server IP).
//! - `Ping` — the client↔core UDP round-trip time held at/above the threshold (per core).
//!
//! `SysCpu` and `MemGrowth` are ported verbatim from the panel so their displayed warnings do not
//! change; `Unreachable` uses the same connectivity rule the panel showed, now sourced here (so it
//! also becomes an episode). All three drive both the panel display and the episode log.
//!
//! The panel reads the current warning state and the smoothed CPU from here; the closed/open episode
//! log is produced now and consumed by the upcoming chart-badge phase.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;

use moon_core::feed::ConnStatus;
use moon_core::session::{CoreId, CoreSysStatus};

/// CPU is averaged over this many recent one-second buckets before display and threshold checks.
const CPU_WINDOW_SECS: i64 = 3;
/// Memory-growth is judged against the minimum used within this many recent seconds.
const MEM_WINDOW_SECS: i64 = 30;
/// A memory rise of at least this many MB above the window minimum flags growth.
const MEM_GROWTH_MB: u16 = 64;
/// ...or a rise of at least this percent above the window minimum.
const MEM_GROWTH_PCT: u32 = 12;
/// Machine CPU at or above this percent (averaged) counts toward the sustained-CPU warning.
const WARN_CPU_PCT: u32 = 70;
/// The CPU warning fires only after the machine has stayed high this many consecutive seconds.
const CPU_SUSTAIN_SECS: u32 = 5;
/// A client↔core UDP round-trip at or above this many ms (as reported by the core) counts toward the
/// ping warning. A healthy transport link stays well under it, so ordinary latency does not trip it.
const PING_WARN_MS: u32 = 500;
/// The ping warning fires only after the RTT has stayed high this many consecutive seconds, so a
/// single slow sample does not flag.
const PING_SUSTAIN_SECS: u32 = 5;
/// Closed episodes kept in memory before the oldest is dropped (the disk store arrives with badges).
const EPISODE_CAP: usize = 2048;

/// Per-axis master switches, mirrored from the persisted layout each tick.
///
/// A `false` axis is inert end to end: the engine opens no episode for it (nothing persisted, no
/// warning state, no tab badge), and the read paths drop its already-recorded history. The counters
/// keep advancing regardless, so re-enabling an axis reacts on the next second rather than needing to
/// re-accumulate its whole sustain window.
#[derive(Clone, Copy)]
pub(crate) struct WarnEnabled {
    /// Sustained system-CPU axis on.
    pub(crate) cpu: bool,
    /// Memory-growth axis on.
    pub(crate) mem: bool,
    /// Connectivity axis on.
    pub(crate) conn: bool,
    /// Ping/RTT axis on.
    pub(crate) ping: bool,
}

impl Default for WarnEnabled {
    /// Every axis on, matching the behaviour before the toggles existed.
    fn default() -> Self {
        Self {
            cpu: true,
            mem: true,
            conn: true,
            ping: true,
        }
    }
}

impl WarnEnabled {
    /// Whether one axis is currently enabled, used to filter persisted episodes on the read paths.
    pub(crate) fn allows(&self, axis: WarnAxis) -> bool {
        match axis {
            WarnAxis::SysCpu => self.cpu,
            WarnAxis::MemGrowth => self.mem,
            WarnAxis::Unreachable => self.conn,
            WarnAxis::Ping => self.ping,
        }
    }
}

/// One core's telemetry handed to the engine each tick.
pub(crate) struct CoreSample {
    /// Stable core identity.
    pub(crate) id: CoreId,
    /// Decoded endpoint address, or `None` until the store learns it.
    pub(crate) ip: Option<IpAddr>,
    /// Latest connection state, for the connectivity (dropped-core) warning.
    pub(crate) status: ConnStatus,
    /// Latest process/machine telemetry.
    pub(crate) sys: CoreSysStatus,
}

/// Which trend a warning episode tracks.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum WarnAxis {
    /// Machine system CPU held high (per server).
    SysCpu,
    /// A core's used memory rising (per core).
    MemGrowth,
    /// A core dropped while the server still runs others (per server).
    Unreachable,
    /// The client↔core UDP round-trip time held high (per core).
    Ping,
}

/// The subject a chart-history ring sample belongs to.
pub(crate) enum RingSubject {
    /// A whole machine, keyed by endpoint IP: `(system CPU %, occupied memory %)`.
    Server(IpAddr),
    /// One core, keyed by id: `(process CPU %, process memory share %)`.
    Core(CoreId),
}

/// One second of raw chart-history data for a subject, to be recorded into the shared rings.
pub(crate) struct RingSample {
    /// Whether this is a whole-machine or a per-core sample.
    pub(crate) subject: RingSubject,
    /// CPU percent (system for a server, process for a core).
    pub(crate) cpu: u8,
    /// Memory percent (occupied for a server, process share for a core).
    pub(crate) mem: u8,
}

/// What one engine tick produces for the backend to act on.
#[derive(Default)]
pub(crate) struct TickResult {
    /// Episodes that closed this tick, to persist. Empty on a within-second no-op.
    pub(crate) closed: Vec<WarnEpisode>,
    /// Raw per-subject chart-history samples for this second. Empty on a within-second no-op.
    pub(crate) rings: Vec<RingSample>,
    /// Per-server round-trip samples (ms) for this second — the worst core round-trip per server —
    /// recorded into the ping history ring like the CPU/memory rings. Empty on a within-second no-op.
    pub(crate) pings: Vec<(IpAddr, u16)>,
}

/// The server's telemetry captured at the moment a warning fired, so the card can show the full
/// state at detection instead of only the peak.
#[allow(dead_code)] // Fields read by the chart card and persistence.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WarnSnapshot {
    /// System CPU percent.
    pub(crate) sys_cpu: u8,
    /// Occupied memory percent (process sum over the reconstructed machine total).
    pub(crate) occ_mem: u8,
    /// Free physical memory, MB.
    pub(crate) free_mb: u16,
    /// Total process memory across the server's cores, MB.
    pub(crate) used_mb: u32,
    /// Logical CPU count.
    pub(crate) logical_cpus: u8,
    /// Worst core round-trip on the server, ms (0 = no ping reading yet).
    pub(crate) round_trip_ms: u16,
}

/// A warning period with a start and, once cleared, an end.
#[allow(dead_code)] // Fields read by the chart-badge/persistence phase and the tests.
#[derive(Clone, Debug)]
pub(crate) struct WarnEpisode {
    /// Monotonic id, stable across this process run.
    pub(crate) id: u64,
    /// Trend this episode tracks.
    pub(crate) axis: WarnAxis,
    /// Server this episode belongs to, when the endpoint is known.
    pub(crate) server_ip: Option<IpAddr>,
    /// The specific core for a per-core axis (`MemGrowth`); `None` for a server-wide axis.
    pub(crate) core_id: Option<CoreId>,
    /// Unix ms the warning became active.
    pub(crate) start_ms: i64,
    /// Unix ms the warning cleared, or `None` while still active.
    pub(crate) end_ms: Option<i64>,
    /// Worst value seen during the episode (CPU % or used MB, by axis).
    pub(crate) peak: u16,
    /// Server telemetry at detection.
    pub(crate) snap: WarnSnapshot,
}

/// One second of CPU samples, tagged with its second so a reused ring slot resets cleanly.
#[derive(Default, Clone, Copy)]
struct CpuBucket {
    /// Unix second this bucket accumulates.
    sec: i64,
    /// Summed process CPU percentages this second.
    proc_sum: u32,
    /// Summed whole-machine CPU percentages this second.
    sys_sum: u32,
    /// Number of samples in the sums.
    samples: u32,
}

/// Per-core rolling telemetry for CPU averaging and memory-growth detection.
#[derive(Default, Clone)]
struct CoreTrack {
    /// Three one-second CPU buckets, indexed by `second % 3`.
    cpu: [CpuBucket; 3],
    /// One `(second, used MB)` sample per second over the memory-growth window.
    mem: VecDeque<(i64, u16)>,
}

impl CoreTrack {
    /// Average `(process, system)` CPU over the last `CPU_WINDOW_SECS` seconds.
    fn averaged(&self, now_sec: i64) -> (Option<u8>, Option<u8>) {
        let (mut proc, mut system, mut n) = (0u32, 0u32, 0u32);
        for bucket in &self.cpu {
            if bucket.samples > 0 && now_sec - bucket.sec < CPU_WINDOW_SECS {
                proc += bucket.proc_sum;
                system += bucket.sys_sum;
                n += bucket.samples;
            }
        }
        if n == 0 {
            (None, None)
        } else {
            (
                Some((proc / n).min(255) as u8),
                Some((system / n).min(255) as u8),
            )
        }
    }
}

/// A currently-open warning, kept out of the closed log until it clears.
#[derive(Clone, Copy)]
struct OpenWarn {
    /// Episode id assigned when it opened.
    id: u64,
    /// Unix ms it became active.
    start_ms: i64,
    /// Server this warning belongs to.
    server_ip: Option<IpAddr>,
    /// Core for a per-core axis.
    core_id: Option<CoreId>,
    /// Worst value seen so far.
    peak: u16,
    /// Server telemetry captured when the warning fired.
    snap: WarnSnapshot,
}

/// Subject a warning is keyed by while open.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum WarnKey {
    /// Server-wide system CPU.
    SysCpu(IpAddr),
    /// One core's memory.
    Mem(CoreId),
    /// Server-wide connectivity (a dropped core with survivors).
    Conn(IpAddr),
    /// One core's client↔core round-trip time.
    Ping(CoreId),
}

/// Backend warning engine: rolling per-core history, current warning state, and the episode log.
#[derive(Default)]
pub(crate) struct CoreWarnEngine {
    /// Per-core rolling CPU/memory history.
    track: HashMap<CoreId, CoreTrack>,
    /// Per-server consecutive high-CPU seconds.
    sys_high: HashMap<IpAddr, u32>,
    /// Per-core consecutive high-ping seconds.
    ping_high: HashMap<CoreId, u32>,
    /// Axis master switches, refreshed from the layout each tick.
    enabled: WarnEnabled,
    /// Currently-open warnings, keyed by subject.
    open: HashMap<WarnKey, OpenWarn>,
    /// Closed episodes, oldest first, capped.
    episodes: VecDeque<WarnEpisode>,
    /// Next episode id.
    next_id: u64,
    /// Bumped whenever an episode opens or closes, so chart mark caches know to rebuild.
    episode_rev: u64,
    /// Last processed Unix second, so a faster caller is throttled to 1 Hz.
    last_sec: i64,
    /// Cached averaged `(process, system)` CPU per core, for the panel's smoothed display.
    avg: HashMap<CoreId, (Option<u8>, Option<u8>)>,
    /// Cores whose memory is currently growing.
    mem_growing: HashSet<CoreId>,
    /// Servers whose system CPU is currently sustained-high.
    sys_warn: HashSet<IpAddr>,
    /// Servers with a dropped core while others stay ready (the connectivity warning).
    conn_warn: HashSet<IpAddr>,
    /// Cores whose client↔core round-trip is currently sustained-high (the ping warning).
    ping_warn: HashSet<CoreId>,
}

impl CoreWarnEngine {
    /// Advance the engine by one second's worth of samples (a no-op within the same second).
    ///
    /// Args:
    ///     samples: One entry per live core this tick.
    ///     now_ms: Current Unix milliseconds.
    ///
    /// Returns:
    ///     Closed episodes to persist and the raw ring samples for this second. Both empty within
    ///     the same second (a no-op).
    pub(crate) fn tick(&mut self, samples: &[CoreSample], now_ms: i64) -> TickResult {
        let now_sec = now_ms / 1000;
        if now_sec <= self.last_sec {
            return TickResult::default();
        }
        self.last_sec = now_sec;

        let present: HashSet<CoreId> = samples.iter().map(|s| s.id).collect();
        self.track.retain(|id, _| present.contains(id));

        for sample in samples {
            if sample.sys.updated_ms == 0 {
                continue;
            }
            self.accumulate(sample, now_sec);
        }

        self.recompute_state(samples, now_sec);
        let closed = self.reconcile_episodes(samples, now_ms);
        TickResult {
            closed,
            rings: build_ring_samples(samples),
            pings: build_ping_samples(samples),
        }
    }

    /// Fold one core's current sample into its rolling CPU and memory windows.
    fn accumulate(&mut self, sample: &CoreSample, now_sec: i64) {
        let track = self.track.entry(sample.id).or_default();
        let slot = &mut track.cpu[now_sec.rem_euclid(3) as usize];
        if slot.sec != now_sec {
            *slot = CpuBucket {
                sec: now_sec,
                ..Default::default()
            };
        }
        if let Some(value) = sample.sys.process_cpu_percent {
            slot.proc_sum += u32::from(value);
        }
        if let Some(value) = sample.sys.system_cpu_percent {
            slot.sys_sum += u32::from(value);
        }
        slot.samples += 1;

        if let Some(used) = sample.sys.used_memory_mb {
            if track.mem.back().map(|(sec, _)| *sec) == Some(now_sec) {
                track.mem.back_mut().unwrap().1 = used;
            } else {
                track.mem.push_back((now_sec, used));
                while track
                    .mem
                    .front()
                    .is_some_and(|(sec, _)| now_sec - sec > MEM_WINDOW_SECS)
                {
                    track.mem.pop_front();
                }
            }
        }
    }

    /// Rebuild the per-core averages, per-server sustained-CPU counters, and the current warning sets.
    fn recompute_state(&mut self, samples: &[CoreSample], now_sec: i64) {
        self.avg.clear();
        self.mem_growing.clear();
        for (id, track) in &self.track {
            self.avg.insert(*id, track.averaged(now_sec));
            if self.enabled.mem && mem_grew(&track.mem) {
                self.mem_growing.insert(*id);
            }
        }

        // Group present cores by server IP for the shared system-CPU signal.
        let mut ip_cores: HashMap<IpAddr, Vec<CoreId>> = HashMap::new();
        for sample in samples {
            if let Some(ip) = sample.ip {
                ip_cores.entry(ip).or_default().push(sample.id);
            }
        }
        self.sys_high.retain(|ip, _| ip_cores.contains_key(ip));
        self.sys_warn.clear();
        for (ip, cores) in &ip_cores {
            // System CPU is identical across a server's cores; take the strongest averaged sample.
            let sys = cores
                .iter()
                .filter_map(|id| self.avg.get(id).and_then(|(_, system)| *system))
                .max();
            let next = next_high_secs(self.sys_high.get(ip).copied().unwrap_or(0), sys);
            self.sys_high.insert(*ip, next);
            if self.enabled.cpu && next >= CPU_SUSTAIN_SECS {
                self.sys_warn.insert(*ip);
            }
        }

        // Connectivity: a server with a dropped core (Disconnected/Failed) while another core is
        // still Ready — "one fell off while the rest works". A fully offline server (no ready core)
        // does not warn, and a still-connecting core is not a drop.
        self.conn_warn.clear();
        let mut ready_down: HashMap<IpAddr, (bool, bool)> = HashMap::new();
        for sample in samples {
            let Some(ip) = sample.ip else {
                continue;
            };
            let entry = ready_down.entry(ip).or_insert((false, false));
            match sample.status {
                ConnStatus::Ready => entry.0 = true,
                ConnStatus::Disconnected | ConnStatus::Failed(_) => entry.1 = true,
                _ => {}
            }
        }
        for (ip, (ready, down)) in ready_down {
            if self.enabled.conn && ready && down {
                self.conn_warn.insert(ip);
            }
        }

        // Ping: a core whose client↔core round-trip stays at/above the threshold for the sustain
        // window. Per core, like memory. The counter advances regardless of the enable switch so
        // re-enabling reacts on the next second, but the warning set only fills while enabled.
        let present: HashSet<CoreId> = samples.iter().map(|sample| sample.id).collect();
        self.ping_high.retain(|id, _| present.contains(id));
        self.ping_warn.clear();
        for sample in samples {
            // Only a Ready core has a live RTT; a Disconnected/Failed core keeps its last `sys`
            // reading, so without this gate its stale round-trip would keep the counter climbing and
            // raise a phantom ping warning for an offline core.
            let next = match sample.sys.round_trip_ms {
                Some(ms) if ms >= PING_WARN_MS && sample.status == ConnStatus::Ready => self
                    .ping_high
                    .get(&sample.id)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(1),
                _ => 0,
            };
            self.ping_high.insert(sample.id, next);
            if self.enabled.ping && next >= PING_SUSTAIN_SECS {
                self.ping_warn.insert(sample.id);
            }
        }
    }

    /// Open new episodes, extend open ones, and close those whose warning cleared or subject left.
    ///
    /// Returns the episodes closed on this tick so the caller can persist them.
    fn reconcile_episodes(&mut self, samples: &[CoreSample], now_ms: i64) -> Vec<WarnEpisode> {
        let ip_of: HashMap<CoreId, IpAddr> = samples
            .iter()
            .filter_map(|s| s.ip.map(|ip| (s.id, ip)))
            .collect();

        // Currently-active warnings as `(key, server_ip, core_id, peak)`.
        let mut active: HashMap<WarnKey, (Option<IpAddr>, Option<CoreId>, u16)> = HashMap::new();
        for ip in &self.sys_warn {
            let peak = self
                .sys_high
                .get(ip)
                .and_then(|_| {
                    // Peak stored as the current averaged system CPU on this server.
                    samples
                        .iter()
                        .filter(|s| s.ip == Some(*ip))
                        .filter_map(|s| self.avg.get(&s.id).and_then(|(_, sys)| *sys))
                        .max()
                })
                .unwrap_or(0);
            active.insert(WarnKey::SysCpu(*ip), (Some(*ip), None, u16::from(peak)));
        }
        for id in &self.mem_growing {
            let peak = self
                .track
                .get(id)
                .and_then(|t| t.mem.back().map(|(_, used)| *used))
                .unwrap_or(0);
            active.insert(WarnKey::Mem(*id), (ip_of.get(id).copied(), Some(*id), peak));
        }
        // Connectivity is a server-wide boolean state; it carries no numeric peak.
        for ip in &self.conn_warn {
            active.insert(WarnKey::Conn(*ip), (Some(*ip), None, 0));
        }
        // Ping peak is the current round-trip in ms (clamped to the peak field's width).
        for id in &self.ping_warn {
            let peak = samples
                .iter()
                .find(|sample| sample.id == *id)
                .and_then(|sample| sample.sys.round_trip_ms)
                .unwrap_or(0)
                .min(u32::from(u16::MAX)) as u16;
            active.insert(
                WarnKey::Ping(*id),
                (ip_of.get(id).copied(), Some(*id), peak),
            );
        }

        // Open or extend.
        for (key, (server_ip, core_id, peak)) in &active {
            match self.open.get_mut(key) {
                Some(open) => open.peak = open.peak.max(*peak),
                None => {
                    let id = self.next_id;
                    self.next_id += 1;
                    self.episode_rev += 1;
                    self.open.insert(
                        *key,
                        OpenWarn {
                            id,
                            start_ms: now_ms,
                            server_ip: *server_ip,
                            core_id: *core_id,
                            peak: *peak,
                            snap: (*server_ip)
                                .map(|ip| server_snapshot(samples, ip))
                                .unwrap_or_default(),
                        },
                    );
                }
            }
        }

        // Close any open warning that is no longer active.
        let to_close: Vec<WarnKey> = self
            .open
            .keys()
            .filter(|key| !active.contains_key(key))
            .copied()
            .collect();
        let mut closed = Vec::new();
        for key in to_close {
            let open = self.open.remove(&key).expect("key came from open");
            let axis = match key {
                WarnKey::SysCpu(_) => WarnAxis::SysCpu,
                WarnKey::Mem(_) => WarnAxis::MemGrowth,
                WarnKey::Conn(_) => WarnAxis::Unreachable,
                WarnKey::Ping(_) => WarnAxis::Ping,
            };
            let episode = WarnEpisode {
                id: open.id,
                axis,
                server_ip: open.server_ip,
                core_id: open.core_id,
                start_ms: open.start_ms,
                end_ms: Some(now_ms),
                peak: open.peak,
                snap: open.snap,
            };
            closed.push(episode.clone());
            self.push_episode(episode);
            self.episode_rev += 1;
        }
        closed
    }

    /// Append a closed episode, dropping the oldest past the cap.
    fn push_episode(&mut self, episode: WarnEpisode) {
        self.episodes.push_back(episode);
        while self.episodes.len() > EPISODE_CAP {
            self.episodes.pop_front();
        }
    }

    /// Averaged `(process, system)` CPU for one core, for the panel's smoothed display.
    pub(crate) fn avg_cpu(&self, id: CoreId) -> (Option<u8>, Option<u8>) {
        self.avg.get(&id).copied().unwrap_or((None, None))
    }

    /// Whether one core's memory is currently growing (the memory warning).
    pub(crate) fn core_mem_warn(&self, id: CoreId) -> bool {
        self.mem_growing.contains(&id)
    }

    /// Whether one server's system CPU is currently sustained-high (the CPU warning).
    pub(crate) fn server_cpu_warn(&self, ip: IpAddr) -> bool {
        self.sys_warn.contains(&ip)
    }

    /// Whether one server currently has a dropped core with survivors (the connectivity warning).
    pub(crate) fn server_conn_warn(&self, ip: IpAddr) -> bool {
        self.conn_warn.contains(&ip)
    }

    /// Whether one core's client↔core round-trip is currently sustained-high (the ping warning).
    pub(crate) fn core_ping_warn(&self, id: CoreId) -> bool {
        self.ping_warn.contains(&id)
    }

    /// Refresh the axis master switches from the layout (called each tick before sampling).
    pub(crate) fn set_enabled(&mut self, enabled: WarnEnabled) {
        self.enabled = enabled;
    }

    /// Revision bumped on every episode open/close, so chart mark caches rebuild only on a change.
    pub(crate) fn episode_rev(&self) -> u64 {
        self.episode_rev
    }

    /// Force the revision forward so chart mark caches rebuild even without an episode change.
    ///
    /// Used when the axis toggles change: the set of episodes a chart should draw shifts, but no
    /// episode opened or closed, so nothing else would invalidate the cached marks.
    pub(crate) fn bump_rev(&mut self) {
        self.episode_rev += 1;
    }

    /// Closed episodes, oldest first. Consumed by the upcoming chart-badge/persistence phase.
    #[allow(dead_code)]
    pub(crate) fn episodes(&self) -> impl Iterator<Item = &WarnEpisode> {
        self.episodes.iter()
    }

    /// Currently-open warnings materialized as episodes with no end yet (for a live badge).
    pub(crate) fn open_episodes(&self) -> Vec<WarnEpisode> {
        self.open
            .iter()
            .map(|(key, open)| WarnEpisode {
                id: open.id,
                axis: match key {
                    WarnKey::SysCpu(_) => WarnAxis::SysCpu,
                    WarnKey::Mem(_) => WarnAxis::MemGrowth,
                    WarnKey::Conn(_) => WarnAxis::Unreachable,
                    WarnKey::Ping(_) => WarnAxis::Ping,
                },
                server_ip: open.server_ip,
                core_id: open.core_id,
                start_ms: open.start_ms,
                end_ms: None,
                peak: open.peak,
                snap: open.snap,
            })
            .collect()
    }
}

/// Capture a server's telemetry snapshot from this tick's samples, for the detection card.
///
/// Freshest system CPU / free memory / logical CPUs across the server's cores, and the summed
/// process memory with its occupied percent — the same reconstruction the rings use.
///
/// Args:
///     samples: This tick's per-core telemetry.
///     ip: Server endpoint address.
///
/// Returns:
///     The server's state at this moment; zeroes for fields no core has reported.
fn server_snapshot(samples: &[CoreSample], ip: IpAddr) -> WarnSnapshot {
    let cores = || samples.iter().filter(|s| s.ip == Some(ip));
    let freshest_u16 = |read: fn(&CoreSysStatus) -> Option<u16>| {
        cores()
            .filter_map(|s| read(&s.sys).map(|value| (s.sys.updated_ms, value)))
            .max_by_key(|(updated_ms, _)| *updated_ms)
            .map(|(_, value)| value)
    };
    let freshest_u8 = |read: fn(&CoreSysStatus) -> Option<u8>| {
        cores()
            .filter_map(|s| read(&s.sys).map(|value| (s.sys.updated_ms, value)))
            .max_by_key(|(updated_ms, _)| *updated_ms)
            .map(|(_, value)| value)
    };
    let free = freshest_u16(|sys| sys.free_physical_memory_mb);
    let used_sum: u64 = cores()
        .filter_map(|s| s.sys.used_memory_mb)
        .map(u64::from)
        .sum();
    let total = free.map(|free| used_sum + u64::from(free)).unwrap_or(0);
    WarnSnapshot {
        sys_cpu: freshest_u8(|sys| sys.system_cpu_percent)
            .unwrap_or(0)
            .min(100),
        occ_mem: if total > 0 {
            (used_sum * 100 / total).min(100) as u8
        } else {
            0
        },
        free_mb: free.unwrap_or(0),
        used_mb: used_sum.min(u64::from(u32::MAX)) as u32,
        logical_cpus: freshest_u8(|sys| sys.logical_cpu_count).unwrap_or(0),
        // Worst READY-core round-trip, matching the per-server ping ring (a dropped core's stale
        // round-trip must not dominate the snapshot).
        round_trip_ms: cores()
            .filter(|s| s.status == ConnStatus::Ready)
            .filter_map(|s| s.sys.round_trip_ms)
            .max()
            .unwrap_or(0)
            .min(u32::from(u16::MAX)) as u16,
    }
}

/// Whether a memory window shows sustained growth: the latest sample sits notably above the
/// window minimum.
///
/// A flat footprint keeps the minimum near the current value (no warning); a leak lifts the
/// current well above the window low; a spike that returns leaves the minimum low, so it does not
/// flag. Needs a few samples before it will fire.
fn mem_grew(mem: &VecDeque<(i64, u16)>) -> bool {
    if mem.len() < 5 {
        return false;
    }
    let current = mem.back().map(|(_, used)| *used).unwrap_or(0);
    let min = mem.iter().map(|(_, used)| *used).min().unwrap_or(current);
    let growth = u32::from(current.saturating_sub(min));
    growth >= u32::from(MEM_GROWTH_MB).max(u32::from(min) * MEM_GROWTH_PCT / 100)
}

/// Advance the consecutive-high-seconds counter for the sustained-CPU warning.
///
/// One more than `prev` while CPU stays at/above the threshold, else zero.
fn next_high_secs(prev: u32, system_cpu: Option<u8>) -> u32 {
    match system_cpu {
        Some(value) if u32::from(value) >= WARN_CPU_PCT => prev.saturating_add(1),
        _ => 0,
    }
}

/// Build this second's raw chart-history samples for every addressed server and its cores.
///
/// Per server: the freshest raw system CPU and occupied memory (process-RAM sum over free+sum). Per
/// core: raw process CPU and this process's share of the reconstructed machine total. All clamped to
/// 0..100. Cores without an endpoint are skipped, as they cannot key a server ring. Ported from the
/// panel's former `record_chart_history` so the chart data is unchanged.
///
/// Args:
///     samples: This tick's per-core telemetry.
///
/// Returns:
///     One server sample plus one sample per core, for every server with a known address.
fn build_ring_samples(samples: &[CoreSample]) -> Vec<RingSample> {
    let mut by_ip: HashMap<IpAddr, Vec<&CoreSample>> = HashMap::new();
    for sample in samples {
        if let Some(ip) = sample.ip {
            by_ip.entry(ip).or_default().push(sample);
        }
    }
    let mut out = Vec::new();
    for (ip, cores) in &by_ip {
        let freshest = |read: &dyn Fn(&CoreSysStatus) -> Option<u16>| -> Option<u16> {
            cores
                .iter()
                .filter_map(|s| read(&s.sys).map(|value| (s.sys.updated_ms, value)))
                .max_by_key(|(updated_ms, _)| *updated_ms)
                .map(|(_, value)| value)
        };
        let sys_cpu = cores
            .iter()
            .filter_map(|s| s.sys.system_cpu_percent.map(|c| (s.sys.updated_ms, c)))
            .max_by_key(|(updated_ms, _)| *updated_ms)
            .map(|(_, c)| c)
            .unwrap_or(0)
            .min(100);
        let used_sum: u64 = cores
            .iter()
            .filter_map(|s| s.sys.used_memory_mb)
            .map(u64::from)
            .sum();
        // Occupied memory needs a free-physical reading; without it the server line reads 0, and the
        // per-core lines stay consistent with it (total unavailable → 0).
        let free = freshest(&|sys| sys.free_physical_memory_mb).map(u64::from);
        let total = free.map(|free| used_sum + free).unwrap_or(0);
        let occ = if total > 0 {
            (used_sum * 100 / total).min(100) as u8
        } else {
            0
        };
        out.push(RingSample {
            subject: RingSubject::Server(*ip),
            cpu: sys_cpu,
            mem: occ,
        });
        for sample in cores {
            let proc_cpu = sample.sys.process_cpu_percent.unwrap_or(0).min(100);
            let proc_mem = match sample.sys.used_memory_mb {
                Some(used) if total > 0 => (u64::from(used) * 100 / total).min(100) as u8,
                _ => 0,
            };
            out.push(RingSample {
                subject: RingSubject::Core(sample.id),
                cpu: proc_cpu,
                mem: proc_mem,
            });
        }
    }
    out
}

/// Build this second's per-SERVER round-trip samples (ms): the worst round-trip among a server's
/// READY cores.
///
/// Recorded into the ping history ring exactly like the CPU/memory rings — telemetry, so it is NOT
/// gated by the ping-warning toggle. A value is emitted for EVERY server with a live core (0 when
/// none of its ready cores has measured a ping yet), so the ping ring advances in lockstep with the
/// CPU/memory ring — a skipped second would mis-time the positional 1 Hz slice. Only Ready cores
/// count, so a dropped core's stale (possibly timed-out) round-trip cannot dominate the server ping
/// the way `max` over all cores would (unlike machine CPU, cores have DIFFERENT round-trips).
///
/// Args:
///     samples: This tick's per-core telemetry.
///
/// Returns:
///     One `(server ip, worst ready round-trip ms)` per server with a known endpoint.
fn build_ping_samples(samples: &[CoreSample]) -> Vec<(IpAddr, u16)> {
    let mut by_ip: HashMap<IpAddr, u16> = HashMap::new();
    for sample in samples {
        let Some(ip) = sample.ip else {
            continue;
        };
        // Seed every server so its ring never skips a second, matching the CPU/memory ring.
        let entry = by_ip.entry(ip).or_insert(0);
        if sample.status == ConnStatus::Ready {
            if let Some(ms) = sample.sys.round_trip_ms {
                *entry = (*entry).max(ms.min(u32::from(u16::MAX)) as u16);
            }
        }
    }
    by_ip.into_iter().collect()
}

pub(crate) mod store;

#[cfg(test)]
mod tests;
