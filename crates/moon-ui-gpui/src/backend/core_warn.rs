//! Backend-resident core warning engine.
//!
//! Detection lives here (not in the Core Status panel) so it runs continuously from the backend
//! coordination loop, independent of whether any panel is open. It samples every live core once per
//! second, keeps the rolling CPU/memory history the warnings need, and turns the SUSTAINED/trend
//! signals into WARNING EPISODES with a start and an end time.
//!
//! Five axes:
//! - `SysCpu` — a machine's system CPU held at/above the threshold (per server IP).
//! - `MemGrowth` — a core's used memory rising above its window minimum (per core).
//! - `Unreachable` — a core that had reached Ready is now Disconnected/Failed — a real drop, on any
//!   server (per server IP), including a single-core server going fully down.
//! - `Ping` — the client↔core UDP round-trip held ABOVE THE CORE'S OWN BASELINE (per core).
//! - `ExchPing` — the core→exchange order-API latency held ABOVE THE CORE'S OWN BASELINE (per core).
//!
//! `SysCpu` and `MemGrowth` are ported verbatim from the panel so their displayed warnings do not
//! change; `Unreachable` uses the same connectivity rule the panel showed, now sourced here (so it
//! also becomes an episode). `Ping`/`ExchPing` are PURELY RELATIVE: each is judged against that
//! core's rolling mean latency, so a link that is *always* slow makes that its own baseline and never
//! warns, while a spike above the usual does — even on a fast link. All drive both the panel display
//! and the episode log.
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
/// A memory rise of at least this percent above the window minimum flags growth. Purely relative —
/// no absolute MB floor — so it scales with each core's footprint.
const MEM_GROWTH_PCT: u32 = 12;
/// Machine CPU at or above this percent (averaged) counts toward the sustained-CPU warning.
const WARN_CPU_PCT: u32 = 70;
/// The CPU warning fires only after the machine has stayed high this many consecutive seconds.
const CPU_SUSTAIN_SECS: u32 = 5;
/// A latency baseline is the mean client↔core (or core→exchange) latency over this many recent
/// seconds. A short spike barely moves it, so "above the usual" is measured against a stable mean.
const LATENCY_BASELINE_SECS: i64 = 60;
/// At least this many baseline samples before a deviation can be judged; below it, always `Normal`
/// (no colour, no warning), so a just-connected core cannot warn on one reading.
const LATENCY_MIN_SAMPLES: usize = 5;
/// CRITICAL — the level the ping/exch WARNING fires at — is a latency ≥ baseline × this / 100.
const LATENCY_WARN_NUM: u32 = 130;
/// WARNING colour (yellow) is a latency ≥ baseline × this / 100.
const LATENCY_YELLOW_NUM: u32 = 110;
/// Percentage denominator for the two ratios above.
const LATENCY_PCT_DEN: u32 = 100;
/// A latency stays CRITICAL this many consecutive seconds before its episode/badge opens, so a
/// single slow sample does not flag.
const LATENCY_SUSTAIN_SECS: u32 = 3;
/// Closed episodes kept in memory before the oldest is dropped (the disk store arrives with badges).
const EPISODE_CAP: usize = 2048;

/// Where a latency sample sits relative to its rolling baseline, higher = worse.
///
/// Ordered so a caller can combine severities with `max` (`Normal < Warning < Critical`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum LatencySeverity {
    /// At or near the usual — no colour, no warning.
    Normal,
    /// Notably above the usual — the yellow colour.
    Warning,
    /// Far above the usual — the red colour and the level the ping/exch warning fires at.
    Critical,
}

/// Classify a latency (ms) against its per-core rolling `baseline` (ms), higher = worse.
///
/// PURELY relative to the baseline (no absolute ms floor): a latency is judged only by how far, in
/// percent, it sits above the core's own mean. A `None` baseline (not enough samples yet) or a zero
/// baseline is always `Normal`. The single source of truth shared by the engine's warning decision
/// and the panel's metric colouring, so red and "warning" always mean the same thing.
///
/// Args:
///     value: The current smoothed latency in ms.
///     baseline: The core's rolling mean latency in ms, or `None` until it is established.
///     yellow_num: Warning-colour ratio × 100 (e.g. 110 = +10 %), from the axis config.
///     red_num: Critical-colour AND warning ratio × 100 (e.g. 130 = +30 %), from the axis config.
///
/// Returns:
///     Where `value` sits relative to `baseline`.
pub(crate) fn latency_severity(
    value: u32,
    baseline: Option<u32>,
    yellow_num: u32,
    red_num: u32,
) -> LatencySeverity {
    let Some(base) = baseline.filter(|b| *b > 0) else {
        return LatencySeverity::Normal;
    };
    // Integer ratio test: value >= base * num / 100, without floating point.
    let over =
        |num: u32| u64::from(value) * u64::from(LATENCY_PCT_DEN) >= u64::from(base) * u64::from(num);
    if over(red_num) {
        LatencySeverity::Critical
    } else if over(yellow_num) {
        LatencySeverity::Warning
    } else {
        LatencySeverity::Normal
    }
}

/// Mean of a latency window in ms over the seconds BEFORE `now_sec`, or `None` until at least the
/// minimum count of prior samples exists.
///
/// The current second's sample is excluded so a spike cannot dampen the very baseline it is judged
/// against — most visible at the minimum-sample floor, where including it would lift the effective
/// threshold. Dedup keeps at most one entry per second, so the filter drops exactly the current one.
fn latency_baseline(window: &VecDeque<(i64, u16)>, now_sec: i64) -> Option<u32> {
    let prior = window.iter().filter(|(sec, _)| *sec != now_sec);
    let n = prior.clone().count();
    if n < LATENCY_MIN_SAMPLES {
        return None;
    }
    let sum: u64 = prior.map(|(_, v)| u64::from(*v)).sum();
    Some((sum / n as u64).min(u64::from(u32::MAX)) as u32)
}

/// Advance one core's per-axis latency sustain counter and fill its warning set, shared by the
/// client↔core-ping and core→exchange-ping axes (identical logic, different fields).
///
/// A latency stays CRITICAL (≥ baseline × threshold) while Ready; the counter counts consecutive
/// critical seconds and resets otherwise, and the warning fires once it holds the sustain window —
/// but only while the axis is enabled (the counter advances regardless, so re-enabling reacts fast).
///
/// Args:
///     high: Per-core consecutive-critical-seconds counter for this axis.
///     warn: Per-core warning set for this axis, filled here.
///     id: The core being judged.
///     critical: Whether this second's latency is at/above the critical threshold (Ready-gated).
///     hold: Consecutive critical seconds required before the warning fires.
///     enabled: Whether this axis is switched on.
fn advance_latency_warn(
    high: &mut HashMap<CoreId, u32>,
    warn: &mut HashSet<CoreId>,
    id: CoreId,
    critical: bool,
    hold: u32,
    enabled: bool,
) {
    let next = if critical {
        high.get(&id).copied().unwrap_or(0).saturating_add(1)
    } else {
        0
    };
    high.insert(id, next);
    if enabled && next >= hold {
        warn.insert(id);
    }
}

/// Fold one Ready-second latency reading into a rolling baseline window, dropping samples older than
/// `window_secs`. A `None` reading is skipped (the window keeps its last values).
fn record_latency(
    window: &mut VecDeque<(i64, u16)>,
    now_sec: i64,
    value: Option<u16>,
    window_secs: i64,
) {
    let Some(v) = value else {
        return;
    };
    if window.back().map(|(sec, _)| *sec) == Some(now_sec) {
        window.back_mut().unwrap().1 = v;
    } else {
        window.push_back((now_sec, v));
        while window
            .front()
            .is_some_and(|(sec, _)| now_sec - sec > window_secs)
        {
            window.pop_front();
        }
    }
}

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
    /// Client↔core ping axis on.
    pub(crate) ping: bool,
    /// Core→exchange ping (order-API latency) axis on.
    pub(crate) exch: bool,
}

impl Default for WarnEnabled {
    /// Every axis on, matching the behaviour before the toggles existed.
    fn default() -> Self {
        Self {
            cpu: true,
            mem: true,
            conn: true,
            ping: true,
            exch: true,
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
            WarnAxis::ExchPing => self.exch,
        }
    }
}

/// Numeric detection thresholds, mirrored from the persisted `WarnParams` each tick. Defaults
/// reproduce the engine's former hard-coded constants, so an un-tuned engine behaves exactly as
/// before. Latency thresholds are stored as the ratio × 100 (`110` = +10 %) that `latency_severity`
/// consumes directly.
#[derive(Clone, Copy)]
pub(crate) struct WarnTuning {
    /// Sustained-CPU percent and its sustain seconds.
    pub(crate) cpu_pct: u32,
    pub(crate) cpu_hold: u32,
    /// Memory-growth percent above the window minimum, and the observation window (seconds).
    pub(crate) mem_pct: u32,
    pub(crate) mem_window: i64,
    /// Client↔core ping: yellow/red colour ratios ×100, baseline window (seconds), sustain seconds.
    pub(crate) ping_yellow_num: u32,
    pub(crate) ping_red_num: u32,
    pub(crate) ping_window: i64,
    pub(crate) ping_hold: u32,
    /// Core→exchange ping: same four knobs.
    pub(crate) exch_yellow_num: u32,
    pub(crate) exch_red_num: u32,
    pub(crate) exch_window: i64,
    pub(crate) exch_hold: u32,
}

impl Default for WarnTuning {
    fn default() -> Self {
        Self {
            cpu_pct: WARN_CPU_PCT,
            cpu_hold: CPU_SUSTAIN_SECS,
            mem_pct: MEM_GROWTH_PCT,
            mem_window: MEM_WINDOW_SECS,
            ping_yellow_num: LATENCY_YELLOW_NUM,
            ping_red_num: LATENCY_WARN_NUM,
            ping_window: LATENCY_BASELINE_SECS,
            ping_hold: LATENCY_SUSTAIN_SECS,
            exch_yellow_num: LATENCY_YELLOW_NUM,
            exch_red_num: LATENCY_WARN_NUM,
            exch_window: LATENCY_BASELINE_SECS,
            exch_hold: LATENCY_SUSTAIN_SECS,
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
    /// A core that was up has dropped (Disconnected/Failed), per server.
    Unreachable,
    /// The client↔core UDP round-trip held above the core's own baseline (per core).
    Ping,
    /// The core→exchange order-API latency held above the core's own baseline (per core).
    ExchPing,
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
    /// Per-server ping samples for this second: `(ip, worst client↔core round-trip ms, worst
    /// core→exchange order-API latency ms)` over the server's Ready cores, recorded into the two ping
    /// history rings like the CPU/memory rings. Empty on a within-second no-op.
    pub(crate) pings: Vec<(IpAddr, u16, u16)>,
    /// Distinct axes whose warning newly OPENED this tick, so the backend can play the axis's alert
    /// sound once. Empty on a within-second no-op or when nothing opened.
    pub(crate) opened: Vec<WarnAxis>,
}

/// The server's telemetry captured at the moment a warning fired, so the card can show the full
/// state at detection instead of only the peak.
// Fields feed the chart card and the persisted episode row; `free_mb`/`logical_cpus` are persisted
// only (the card dropped its free-memory/logical-CPU line), so they read as dead to the linter here.
#[allow(dead_code)]
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
    /// Worst client↔core round-trip on the server, ms (0 = no ping reading yet).
    pub(crate) round_trip_ms: u16,
    /// Worst core→exchange order-API latency on the server, ms (0 = no reading yet).
    pub(crate) order_api_latency_ms: u16,
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
    /// One `(second, client↔core round-trip ms)` per second over the latency-baseline window,
    /// recorded only while the core is Ready (a stale reading must not skew the baseline).
    link: VecDeque<(i64, u16)>,
    /// One `(second, core→exchange order latency ms)` per second over the latency-baseline window,
    /// Ready-seconds only.
    exch: VecDeque<(i64, u16)>,
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
    /// Server-wide connectivity (a core that was up has dropped).
    Conn(IpAddr),
    /// One core's client↔core round-trip time.
    Ping(CoreId),
    /// One core's core→exchange order-API latency.
    ExchPing(CoreId),
}

/// The warning axis a live key belongs to.
fn axis_of(key: &WarnKey) -> WarnAxis {
    match key {
        WarnKey::SysCpu(_) => WarnAxis::SysCpu,
        WarnKey::Mem(_) => WarnAxis::MemGrowth,
        WarnKey::Conn(_) => WarnAxis::Unreachable,
        WarnKey::Ping(_) => WarnAxis::Ping,
        WarnKey::ExchPing(_) => WarnAxis::ExchPing,
    }
}

/// Backend warning engine: rolling per-core history, current warning state, and the episode log.
#[derive(Default)]
pub(crate) struct CoreWarnEngine {
    /// Per-core rolling CPU/memory history.
    track: HashMap<CoreId, CoreTrack>,
    /// Per-server consecutive high-CPU seconds.
    sys_high: HashMap<IpAddr, u32>,
    /// Per-core consecutive above-baseline client↔core-ping seconds.
    ping_high: HashMap<CoreId, u32>,
    /// Per-core consecutive above-baseline core→exchange-ping seconds.
    exch_high: HashMap<CoreId, u32>,
    /// Per-core rolling client↔core-ping baseline (ms), refreshed each tick for the panel colouring.
    ping_base: HashMap<CoreId, u32>,
    /// Per-core rolling core→exchange-ping baseline (ms), refreshed each tick.
    exch_base: HashMap<CoreId, u16>,
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
    /// Servers with a core that was up and has now dropped (the connectivity warning).
    conn_warn: HashSet<IpAddr>,
    /// Cores that have reached `Ready` at least once, so a later drop is a real disconnect rather
    /// than a never-connected core (which must not warn).
    ever_ready: HashSet<CoreId>,
    /// Cores whose client↔core round-trip is sustained above their baseline (the ping warning).
    ping_warn: HashSet<CoreId>,
    /// Cores whose core→exchange latency is sustained above their baseline (the exch-ping warning).
    exch_warn: HashSet<CoreId>,
    /// Numeric detection thresholds, refreshed from the layout each tick.
    tuning: WarnTuning,
    /// Current per-core client↔core-ping colour severity (relative to its baseline and the axis
    /// thresholds), so the panel colours the row without re-deriving the thresholds.
    ping_level: HashMap<CoreId, LatencySeverity>,
    /// Current per-core core→exchange-ping colour severity, for the same reason.
    exch_level: HashMap<CoreId, LatencySeverity>,
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
        let (closed, opened) = self.reconcile_episodes(samples, now_ms);
        TickResult {
            closed,
            rings: build_ring_samples(samples),
            pings: build_ping_samples(samples),
            opened,
        }
    }

    /// Replace the numeric detection thresholds (called each tick before sampling).
    pub(crate) fn set_tuning(&mut self, tuning: WarnTuning) {
        self.tuning = tuning;
    }

    /// Fold one core's current sample into its rolling CPU and memory windows.
    fn accumulate(&mut self, sample: &CoreSample, now_sec: i64) {
        let (mem_window, ping_window, exch_window) = (
            self.tuning.mem_window,
            self.tuning.ping_window,
            self.tuning.exch_window,
        );
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
                    .is_some_and(|(sec, _)| now_sec - sec > mem_window)
                {
                    track.mem.pop_front();
                }
            }
        }

        // Latency baselines: only a Ready core's live reading feeds the rolling mean — a dropped
        // core keeps its last (possibly timed-out) sample, which must not become the "usual".
        if sample.status == ConnStatus::Ready {
            record_latency(
                &mut track.link,
                now_sec,
                sample
                    .sys
                    .round_trip_ms
                    .map(|ms| ms.min(u32::from(u16::MAX)) as u16),
                ping_window,
            );
            record_latency(
                &mut track.exch,
                now_sec,
                sample.sys.order_api_latency_ms,
                exch_window,
            );
        }
    }

    /// Rebuild the per-core averages, per-server sustained-CPU counters, and the current warning sets.
    fn recompute_state(&mut self, samples: &[CoreSample], now_sec: i64) {
        let t = self.tuning;
        self.avg.clear();
        self.mem_growing.clear();
        self.ping_base.clear();
        self.exch_base.clear();
        self.ping_level.clear();
        self.exch_level.clear();
        for (id, track) in &self.track {
            self.avg.insert(*id, track.averaged(now_sec));
            if self.enabled.mem && mem_grew(&track.mem, t.mem_pct) {
                self.mem_growing.insert(*id);
            }
            if let Some(base) = latency_baseline(&track.link, now_sec) {
                self.ping_base.insert(*id, base);
            }
            if let Some(base) = latency_baseline(&track.exch, now_sec) {
                self.exch_base.insert(*id, base.min(u32::from(u16::MAX)) as u16);
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
            let next = next_high_secs(self.sys_high.get(ip).copied().unwrap_or(0), sys, t.cpu_pct);
            self.sys_high.insert(*ip, next);
            if self.enabled.cpu && next >= t.cpu_hold {
                self.sys_warn.insert(*ip);
            }
        }

        // Connectivity: a core that had reached Ready is now Disconnected/Failed — a real drop, on
        // ANY server (a single-core server going fully down warns too). A never-connected core
        // (Disconnected but never Ready, e.g. at startup or intentionally off) is NOT a drop, and a
        // still-connecting core is not a drop.
        let present: HashSet<CoreId> = samples.iter().map(|sample| sample.id).collect();
        for sample in samples {
            if sample.status == ConnStatus::Ready {
                self.ever_ready.insert(sample.id);
            }
        }
        self.ever_ready.retain(|id| present.contains(id));
        self.conn_warn.clear();
        for sample in samples {
            let Some(ip) = sample.ip else {
                continue;
            };
            let dropped = self.ever_ready.contains(&sample.id)
                && matches!(
                    sample.status,
                    ConnStatus::Disconnected | ConnStatus::Failed(_)
                );
            if self.enabled.conn && dropped {
                self.conn_warn.insert(ip);
            }
        }

        // Ping (client↔core) and exch (core→exchange): a Ready core whose smoothed latency stays
        // CRITICAL — at/above its own baseline × 1.30 — for the sustain window. Per core, like memory.
        // Purely relative, so a link that is always slow never warns while a spike above the usual
        // does. The counter advances regardless of the enable switch so re-enabling reacts on the
        // next second, but the warning set only fills while enabled. Only a Ready core is judged: a
        // Disconnected/Failed core keeps its last reading, which must not climb the counter.
        self.ping_high.retain(|id, _| present.contains(id));
        self.exch_high.retain(|id, _| present.contains(id));
        self.ping_warn.clear();
        self.exch_warn.clear();
        for sample in samples {
            let ready = sample.status == ConnStatus::Ready;
            // Ping: severity is computed once (and cached for the panel colour); the warning is the
            // critical severity sustained for the hold window. A non-Ready core has no live reading,
            // so it stays Normal and its counter resets.
            let ping_sev = ready
                .then_some(sample.sys.round_trip_ms)
                .flatten()
                .map(|ms| {
                    latency_severity(
                        ms,
                        self.ping_base.get(&sample.id).copied(),
                        t.ping_yellow_num,
                        t.ping_red_num,
                    )
                })
                .unwrap_or(LatencySeverity::Normal);
            if ready {
                self.ping_level.insert(sample.id, ping_sev);
            }
            advance_latency_warn(
                &mut self.ping_high,
                &mut self.ping_warn,
                sample.id,
                ping_sev == LatencySeverity::Critical,
                t.ping_hold,
                self.enabled.ping,
            );

            let exch_sev = ready
                .then_some(sample.sys.order_api_latency_ms)
                .flatten()
                .map(|ms| {
                    latency_severity(
                        u32::from(ms),
                        self.exch_base.get(&sample.id).copied().map(u32::from),
                        t.exch_yellow_num,
                        t.exch_red_num,
                    )
                })
                .unwrap_or(LatencySeverity::Normal);
            if ready {
                self.exch_level.insert(sample.id, exch_sev);
            }
            advance_latency_warn(
                &mut self.exch_high,
                &mut self.exch_warn,
                sample.id,
                exch_sev == LatencySeverity::Critical,
                t.exch_hold,
                self.enabled.exch,
            );
        }
    }

    /// Open new episodes, extend open ones, and close those whose warning cleared or subject left.
    ///
    /// Returns `(closed episodes to persist, distinct axes that newly opened this tick)` — the latter
    /// so the caller can play each axis's alert sound once.
    fn reconcile_episodes(
        &mut self,
        samples: &[CoreSample],
        now_ms: i64,
    ) -> (Vec<WarnEpisode>, Vec<WarnAxis>) {
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
        // Exch-ping peak is the current core→exchange latency in ms.
        for id in &self.exch_warn {
            let peak = samples
                .iter()
                .find(|sample| sample.id == *id)
                .and_then(|sample| sample.sys.order_api_latency_ms)
                .unwrap_or(0);
            active.insert(
                WarnKey::ExchPing(*id),
                (ip_of.get(id).copied(), Some(*id), peak),
            );
        }

        // Open or extend.
        let mut opened: Vec<WarnAxis> = Vec::new();
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
                    let axis = axis_of(key);
                    if !opened.contains(&axis) {
                        opened.push(axis);
                    }
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
            let episode = WarnEpisode {
                id: open.id,
                axis: axis_of(&key),
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
        (closed, opened)
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

    /// Whether one core's client↔core round-trip is sustained above its baseline (the ping warning).
    pub(crate) fn core_ping_warn(&self, id: CoreId) -> bool {
        self.ping_warn.contains(&id)
    }

    /// Whether one core's core→exchange latency is sustained above its baseline (the exch warning).
    pub(crate) fn core_exch_warn(&self, id: CoreId) -> bool {
        self.exch_warn.contains(&id)
    }

    /// One core's current client↔core-ping colour severity (relative to its baseline and thresholds).
    /// The engine owns the classification, so the panel colour and the warning always agree.
    pub(crate) fn core_ping_level(&self, id: CoreId) -> LatencySeverity {
        self.ping_level
            .get(&id)
            .copied()
            .unwrap_or(LatencySeverity::Normal)
    }

    /// One core's current core→exchange-ping colour severity.
    pub(crate) fn core_exch_level(&self, id: CoreId) -> LatencySeverity {
        self.exch_level
            .get(&id)
            .copied()
            .unwrap_or(LatencySeverity::Normal)
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
                axis: axis_of(key),
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
        // Worst READY-core pings, matching the per-server ping rings (a dropped core's stale reading
        // must not dominate the snapshot).
        round_trip_ms: cores()
            .filter(|s| s.status == ConnStatus::Ready)
            .filter_map(|s| s.sys.round_trip_ms)
            .max()
            .unwrap_or(0)
            .min(u32::from(u16::MAX)) as u16,
        order_api_latency_ms: cores()
            .filter(|s| s.status == ConnStatus::Ready)
            .filter_map(|s| s.sys.order_api_latency_ms)
            .max()
            .unwrap_or(0),
    }
}

/// Whether a memory window shows sustained growth: the latest sample sits notably above the
/// window minimum.
///
/// A flat footprint keeps the minimum near the current value (no warning); a leak lifts the
/// current well above the window low; a spike that returns leaves the minimum low, so it does not
/// flag. Needs a few samples before it will fire.
fn mem_grew(mem: &VecDeque<(i64, u16)>, pct: u32) -> bool {
    if mem.len() < 5 {
        return false;
    }
    let current = mem.back().map(|(_, used)| *used).unwrap_or(0);
    let min = mem.iter().map(|(_, used)| *used).min().unwrap_or(current);
    // Purely relative: a rise of at least `pct` above the window minimum. A zero minimum has no
    // meaningful percentage baseline, so it never flags (avoids "any value > 0" firing).
    if min == 0 {
        return false;
    }
    let growth = u32::from(current.saturating_sub(min));
    growth >= u32::from(min) * pct / 100
}

/// Advance the consecutive-high-seconds counter for the sustained-CPU warning.
///
/// One more than `prev` while CPU stays at/above the threshold, else zero.
fn next_high_secs(prev: u32, system_cpu: Option<u8>, threshold: u32) -> u32 {
    match system_cpu {
        Some(value) if u32::from(value) >= threshold => prev.saturating_add(1),
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

/// Build this second's per-SERVER ping samples: the worst client↔core round-trip AND the worst
/// core→exchange order latency among a server's READY cores.
///
/// Recorded into the two ping history rings exactly like the CPU/memory rings — telemetry, so it is
/// NOT gated by the ping-warning toggle. A value is emitted for EVERY server with a live core (0
/// when none of its ready cores has a reading yet), so the rings advance in lockstep with the
/// CPU/memory ring — a skipped second would mis-time the positional 1 Hz slice. Only Ready cores
/// count, so a dropped core's stale (possibly timed-out) reading cannot dominate the server ping the
/// way `max` over all cores would (unlike machine CPU, cores have DIFFERENT round-trips).
///
/// Args:
///     samples: This tick's per-core telemetry.
///
/// Returns:
///     One `(server ip, worst round-trip ms, worst order-API latency ms)` per server with an endpoint.
fn build_ping_samples(samples: &[CoreSample]) -> Vec<(IpAddr, u16, u16)> {
    let mut by_ip: HashMap<IpAddr, (u16, u16)> = HashMap::new();
    for sample in samples {
        let Some(ip) = sample.ip else {
            continue;
        };
        // Seed every server so its rings never skip a second, matching the CPU/memory ring.
        let entry = by_ip.entry(ip).or_insert((0, 0));
        if sample.status == ConnStatus::Ready {
            if let Some(ms) = sample.sys.round_trip_ms {
                entry.0 = entry.0.max(ms.min(u32::from(u16::MAX)) as u16);
            }
            if let Some(ms) = sample.sys.order_api_latency_ms {
                entry.1 = entry.1.max(ms);
            }
        }
    }
    by_ip
        .into_iter()
        .map(|(ip, (link, exch))| (ip, link, exch))
        .collect()
}

pub(crate) mod store;

#[cfg(test)]
mod tests;
