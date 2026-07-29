//! Backend-resident core warning engine.
//!
//! Detection lives here (not in the Core Status panel) so it runs continuously from the backend
//! coordination loop, independent of whether any panel is open. It samples every live core once per
//! second, keeps the rolling CPU/memory history the warnings need, and turns the SUSTAINED/trend
//! signals into WARNING EPISODES with a start and an end time.
//!
//! Two axes today, ported verbatim from the panel so the displayed warnings do not change:
//! - `SysCpu` — a machine's system CPU held at/above the threshold (per server IP).
//! - `MemGrowth` — a core's used memory rising above its window minimum (per core).
//!
//! The panel reads the current warning state and the smoothed CPU from here; the closed/open episode
//! log is produced now and consumed by the upcoming chart-badge phase.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;

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
/// Closed episodes kept in memory before the oldest is dropped (the disk store arrives with badges).
const EPISODE_CAP: usize = 2048;

/// One core's telemetry handed to the engine each tick.
pub(crate) struct CoreSample {
    /// Stable core identity.
    pub(crate) id: CoreId,
    /// Decoded endpoint address, or `None` until the store learns it.
    pub(crate) ip: Option<IpAddr>,
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
}

/// A warning period with a start and, once cleared, an end.
#[allow(dead_code)] // Fields read by the upcoming chart-badge/persistence phase and the tests.
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
}

/// Subject a warning is keyed by while open.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum WarnKey {
    /// Server-wide system CPU.
    SysCpu(IpAddr),
    /// One core's memory.
    Mem(CoreId),
}

/// Backend warning engine: rolling per-core history, current warning state, and the episode log.
#[derive(Default)]
pub(crate) struct CoreWarnEngine {
    /// Per-core rolling CPU/memory history.
    track: HashMap<CoreId, CoreTrack>,
    /// Per-server consecutive high-CPU seconds.
    sys_high: HashMap<IpAddr, u32>,
    /// Currently-open warnings, keyed by subject.
    open: HashMap<WarnKey, OpenWarn>,
    /// Closed episodes, oldest first, capped.
    episodes: VecDeque<WarnEpisode>,
    /// Next episode id.
    next_id: u64,
    /// Last processed Unix second, so a faster caller is throttled to 1 Hz.
    last_sec: i64,
    /// Cached averaged `(process, system)` CPU per core, for the panel's smoothed display.
    avg: HashMap<CoreId, (Option<u8>, Option<u8>)>,
    /// Cores whose memory is currently growing.
    mem_growing: HashSet<CoreId>,
    /// Servers whose system CPU is currently sustained-high.
    sys_warn: HashSet<IpAddr>,
}

impl CoreWarnEngine {
    /// Advance the engine by one second's worth of samples (a no-op within the same second).
    ///
    /// Args:
    ///     samples: One entry per live core this tick.
    ///     now_ms: Current Unix milliseconds.
    ///
    /// Returns:
    ///     The episodes that CLOSED this tick, for the caller to persist. Empty within the same
    ///     second or when nothing cleared.
    pub(crate) fn tick(&mut self, samples: &[CoreSample], now_ms: i64) -> Vec<WarnEpisode> {
        let now_sec = now_ms / 1000;
        if now_sec <= self.last_sec {
            return Vec::new();
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
        self.reconcile_episodes(samples, now_ms)
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
            if mem_grew(&track.mem) {
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
            if next >= CPU_SUSTAIN_SECS {
                self.sys_warn.insert(*ip);
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
            active.insert(
                WarnKey::Mem(*id),
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
                    self.open.insert(
                        *key,
                        OpenWarn {
                            id,
                            start_ms: now_ms,
                            server_ip: *server_ip,
                            core_id: *core_id,
                            peak: *peak,
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
            };
            let episode = WarnEpisode {
                id: open.id,
                axis,
                server_ip: open.server_ip,
                core_id: open.core_id,
                start_ms: open.start_ms,
                end_ms: Some(now_ms),
                peak: open.peak,
            };
            closed.push(episode.clone());
            self.push_episode(episode);
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

    /// Closed episodes, oldest first. Consumed by the upcoming chart-badge/persistence phase.
    #[allow(dead_code)]
    pub(crate) fn episodes(&self) -> impl Iterator<Item = &WarnEpisode> {
        self.episodes.iter()
    }

    /// Currently-open warnings materialized as episodes with no end yet. Consumed by the badge phase.
    #[allow(dead_code)]
    pub(crate) fn open_episodes(&self) -> Vec<WarnEpisode> {
        self.open
            .iter()
            .map(|(key, open)| WarnEpisode {
                id: open.id,
                axis: match key {
                    WarnKey::SysCpu(_) => WarnAxis::SysCpu,
                    WarnKey::Mem(_) => WarnAxis::MemGrowth,
                },
                server_ip: open.server_ip,
                core_id: open.core_id,
                start_ms: open.start_ms,
                end_ms: None,
                peak: open.peak,
            })
            .collect()
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

pub(crate) mod store;

#[cfg(test)]
mod tests;
