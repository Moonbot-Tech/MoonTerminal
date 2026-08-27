//! Diagnostic process and system metrics for the status bar: process/system CPU,
//! process RAM, and RAM growth over a time window. On Windows, this also samples
//! GPU Engine utilization for the current process through PDH.
//!
//! Sampled on a THREAD OF ITS OWN, and that is the point of the module's shape. Refreshing
//! sysinfo and querying PDH is not merely expensive, it BLOCKS: measured at 12 to 27
//! milliseconds a call on Windows, where the GPU query enumerates every engine of every
//! process on the machine. Done on the UI thread once a second, as it used to be, that is one
//! to three frames dropped in a burst, every second — invisible in an average and plainly
//! visible in a drag. The UI now only copies the snapshot the worker last published.
//!
//! [`Metrics`] itself is deliberately private and is BUILT INSIDE the worker: its GPU sampler
//! holds a raw PDH handle and is not `Send`, so the object cannot be moved onto a thread —
//! only created on one.

mod cpu_watch;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};

/// Interval the worker sleeps between polls.
///
/// Also the cadence `cpu_watch` and the memory window assume: the detector counts its samples
/// as seconds. The worker is the only caller, so this is the pacing — there is no second
/// throttle inside [`Metrics::refresh`], and there must not be, or a sleep landing a hair early
/// would silently skip a whole second's sample.
const REFRESH_EVERY: Duration = Duration::from_millis(1000);
/// Time window used to calculate memory growth or decline.
const MEM_WINDOW: Duration = Duration::from_secs(5);

/// Copyable metrics snapshot passed cheaply to UI and diagnostic consumers.
#[derive(Clone, Copy, Default)]
pub struct MetricsSnapshot {
    /// Process CPU as a percentage of total machine capacity, matching Task Manager.
    pub cpu_process: f32,
    /// Total system CPU usage as a percentage.
    pub cpu_system: f32,
    /// Resident process memory in MiB.
    pub mem_mb: f32,
    /// Process memory change over `MEM_WINDOW` in MiB; positive values indicate growth.
    pub mem_delta_mb: f32,
    /// Current process GPU usage from Windows GPU Engine counters; zero elsewhere.
    pub gpu_process: f32,
}

struct Metrics {
    sys: System,
    pid: Pid,
    ncpu: f32,
    snap: MetricsSnapshot,
    gpu: GpuProcessSampler,
    /// Timestamped process memory samples used to calculate change over `MEM_WINDOW`.
    mem_hist: VecDeque<(Instant, f32)>,
    /// Reports a CPU spike of this process to the application log; see [`cpu_watch`].
    ///
    /// It lives here because this is the ONE place the process polls itself.
    watch: cpu_watch::SpikeDetector,
}

impl Metrics {
    fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        let logical_cpus = sys.cpus().len().max(1);
        let pid = sysinfo::get_current_pid().unwrap_or(Pid::from(0));
        Self {
            sys,
            pid,
            ncpu: logical_cpus as f32,
            snap: MetricsSnapshot::default(),
            gpu: GpuProcessSampler::new(pid_as_u32(pid)),
            mem_hist: VecDeque::new(),
            watch: cpu_watch::SpikeDetector::new(logical_cpus),
        }
    }

    /// Poll the system and return the fresh snapshot.
    ///
    /// Unconditional: pacing belongs to the worker that owns this, and a second throttle here
    /// would turn a sleep that woke a millisecond early into a skipped second.
    fn refresh(&mut self) -> MetricsSnapshot {
        let now = Instant::now();
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys
            .refresh_processes(ProcessesToUpdate::Some(&[self.pid]), true);

        let cpu_system = self.sys.global_cpu_usage();
        let (cpu_process, mem_mb) = match self.sys.process(self.pid) {
            // `cpu_usage()` uses 100% per core, so divide by the core count to match Task Manager.
            Some(p) => (
                p.cpu_usage() / self.ncpu,
                p.memory() as f32 / (1024.0 * 1024.0),
            ),
            None => (0.0, 0.0),
        };

        self.mem_hist.push_back((now, mem_mb));
        while self
            .mem_hist
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > MEM_WINDOW)
        {
            self.mem_hist.pop_front();
        }
        let mem_delta_mb = self
            .mem_hist
            .front()
            .map(|(_, m0)| mem_mb - *m0)
            .unwrap_or(0.0);
        let gpu_process = self.gpu.sample().unwrap_or(self.snap.gpu_process);

        self.snap = MetricsSnapshot {
            cpu_process,
            cpu_system,
            mem_mb,
            mem_delta_mb,
            gpu_process,
        };
        // The detector counts its samples AS SECONDS, which is exactly why this sits on the
        // refresh path and why the worker's sleep is the only thing pacing it.
        if let Some(event) = self
            .watch
            .observe(crate::util::now_unix_ms_i64(), &self.snap)
        {
            cpu_watch::report(event);
        }
        self.snap
    }
}

/// Handle to the metrics worker: the UI's only contact with it.
///
/// Holds no `Metrics` and does no system work — [`snapshot`](Self::snapshot) is a lock and a copy
/// of five floats, which is the whole reason this type exists.
pub struct MetricsSampler {
    latest: Arc<Mutex<MetricsSnapshot>>,
}

impl MetricsSampler {
    /// The most recent snapshot the worker published.
    ///
    /// Before the first poll completes, and if the worker thread could not be started at all, this
    /// is the default snapshot — all zeros, which is what the status bar showed during that window
    /// anyway. A poisoned lock is recovered rather than propagated: the guarded value is a `Copy`
    /// struct written in one assignment, so there is no half-written state to protect anyone from,
    /// and losing process metrics is not worth taking the UI down for.
    pub fn snapshot(&self) -> MetricsSnapshot {
        match self.latest.lock() {
            Ok(slot) => *slot,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }
}

/// Start the metrics worker and return its handle.
///
/// The thread holds a `Weak` to the published snapshot, so dropping the handle ends it within one
/// [`REFRESH_EVERY`]: nothing has to be signalled and nothing can outlive its owner. It writes no
/// files and touches no shared state beyond that slot, so a process exiting mid-sleep loses
/// nothing.
pub fn spawn_sampler() -> MetricsSampler {
    let latest = Arc::new(Mutex::new(MetricsSnapshot::default()));
    let published = Arc::downgrade(&latest);
    let started = std::thread::Builder::new()
        .name("moon-metrics".to_string())
        .spawn(move || {
            // Built HERE rather than passed in: `GpuProcessSampler` owns a raw PDH handle and is
            // therefore not `Send`. Creating it on this thread is also where it belongs — the
            // query is opened and collected by one thread for the life of the process.
            let mut metrics = Metrics::new();
            loop {
                // Sleep FIRST. `Metrics::new` takes the baseline CPU reading, and sysinfo
                // derives usage from the gap between two refreshes — polling immediately
                // would divide by no elapsed time and publish a zero. It also means the
                // strong reference below never spans a sleep, so dropping the handle is
                // noticed within one interval.
                std::thread::sleep(REFRESH_EVERY);
                let Some(slot) = published.upgrade() else {
                    return;
                };
                let snap = metrics.refresh();
                // The semicolon is load-bearing: as the block's tail expression, the
                // `LockResult` temporary would outlive `slot` and be dropped after it.
                match slot.lock() {
                    Ok(mut held) => *held = snap,
                    Err(poisoned) => *poisoned.into_inner() = snap,
                };
            }
        });
    if let Err(err) = started {
        // Reported, not fatal: the terminal runs fine without a status-bar CPU readout, and the
        // snapshot simply stays at its default.
        log::warn!("metrics sampler thread could not start: {err}");
    }
    MetricsSampler { latest }
}

fn pid_as_u32(pid: Pid) -> u32 {
    let text = pid.to_string();
    text.parse().unwrap_or(0)
}

#[cfg(windows)]
struct GpuProcessSampler {
    pid_pattern: String,
    query: windows_sys::Win32::System::Performance::PDH_HQUERY,
    counter: windows_sys::Win32::System::Performance::PDH_HCOUNTER,
    available: bool,
}

#[cfg(windows)]
impl GpuProcessSampler {
    fn new(pid: u32) -> Self {
        use windows_sys::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhCollectQueryData, PdhOpenQueryW,
        };

        let mut query = std::ptr::null_mut();
        let mut counter = std::ptr::null_mut();
        let mut available = false;
        let path = to_wide("\\GPU Engine(*)\\Utilization Percentage");
        unsafe {
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) == 0
                && PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) == 0
            {
                let _ = PdhCollectQueryData(query);
                available = true;
            }
        }
        if !available && !query.is_null() {
            unsafe {
                windows_sys::Win32::System::Performance::PdhCloseQuery(query);
            }
            query = std::ptr::null_mut();
            counter = std::ptr::null_mut();
        }
        Self {
            pid_pattern: format!("pid_{pid}_"),
            query,
            counter,
            available,
        }
    }

    fn sample(&mut self) -> Option<f32> {
        use windows_sys::Win32::System::Performance::{
            PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA, PdhCollectQueryData,
            PdhGetFormattedCounterArrayW,
        };

        if !self.available {
            return None;
        }
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                self.available = false;
                return None;
            }

            let mut bytes = 0_u32;
            let mut count = 0_u32;
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                std::ptr::null_mut(),
            );
            if status != PDH_MORE_DATA || bytes == 0 {
                return None;
            }

            let item_size = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() as u32;
            let item_count = count.max(bytes.div_ceil(item_size)).max(1);
            let mut items = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); item_count as usize];
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                items.as_mut_ptr(),
            );
            if status != 0 {
                return None;
            }

            let mut total = 0.0_f64;
            let mut matched = false;
            for item in items.iter().take(count as usize) {
                if item.szName.is_null() || item.FmtValue.CStatus != 0 {
                    continue;
                }
                let name = wide_ptr_to_string(item.szName);
                if !name.contains(&self.pid_pattern) {
                    continue;
                }
                let value = item.FmtValue.Anonymous.doubleValue;
                if value.is_finite() && value > 0.0 {
                    total += value;
                    matched = true;
                }
            }
            matched.then_some(total.clamp(0.0, 100.0) as f32)
        }
    }
}

#[cfg(windows)]
impl Drop for GpuProcessSampler {
    fn drop(&mut self) {
        if !self.query.is_null() {
            unsafe {
                windows_sys::Win32::System::Performance::PdhCloseQuery(self.query);
            }
        }
    }
}

#[cfg(windows)]
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
unsafe fn wide_ptr_to_string(ptr: *const u16) -> String {
    let mut len = 0_usize;
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
}

#[cfg(not(windows))]
struct GpuProcessSampler;

#[cfg(not(windows))]
impl GpuProcessSampler {
    fn new(_pid: u32) -> Self {
        Self
    }

    fn sample(&mut self) -> Option<f32> {
        Some(0.0)
    }
}
